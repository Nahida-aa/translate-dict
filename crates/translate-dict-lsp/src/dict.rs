use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
use std::path::Path;

use ahash::AHashMap;
use serde_json::Value;

use crate::dict_dir;
use crate::reverse_query::ReverseResult;

const SEP: char = '/';

// Built-in dictionary embedded at compile time (build.rs writes the literal path to OUT_DIR/embedded_dict.rs).
include!(concat!(env!("OUT_DIR"), "/embedded_dict.rs"));

#[derive(Clone)]
pub struct DictEntry {
    pub phonetic: String,
    pub translation: String,
}

// Data source that can be either compiled into the binary or loaded from the
// filesystem next to the executable.
#[derive(Clone)]
pub enum ShardSource {
    Embedded,
    Fs(PathBuf),
}

// Keeps parsed shards (`{prefix}.json`) around with a simple LRU eviction
// policy, so that both the startup cost and the steady-state memory footprint
// stay small.
struct ShardCache {
    cap: usize,
    stamp: u64,
    map: AHashMap<String, (AHashMap<String, Value>, u64)>,
}

impl ShardCache {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            stamp: 0,
            map: AHashMap::new(),
        }
    }

    fn contains_key(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    fn get(&mut self, key: &str) -> Option<&AHashMap<String, Value>> {
        self.stamp = self.stamp.wrapping_add(1);
        let stamp = self.stamp;
        match self.map.get_mut(key) {
            Some((shard, used)) => {
                *used = stamp;
                Some(&*shard)
            }
            None => None,
        }
    }

    fn put(&mut self, key: String, shard: AHashMap<String, Value>) {
        if !self.map.contains_key(&key) && self.map.len() >= self.cap {
            let victim = self
                .map
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            if let Some(victim) = victim {
                self.map.remove(&victim);
            }
        }
        self.stamp = self.stamp.wrapping_add(1);
        self.map.insert(key, (shard, self.stamp));
    }
}

// A small LRU of previously computed Chinese -> English reverse results, so
// re-hovering the same Chinese word is instant instead of re-scanning every
// shard. Results are bounded (see scan_reverse) to keep memory negligible.
struct ReverseCache {
    cap: usize,
    stamp: u64,
    map: AHashMap<String, (Vec<ReverseResult>, u64)>,
}

impl ReverseCache {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            stamp: 0,
            map: AHashMap::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<&Vec<ReverseResult>> {
        self.stamp = self.stamp.wrapping_add(1);
        let stamp = self.stamp;
        match self.map.get_mut(key) {
            Some((v, used)) => {
                *used = stamp;
                Some(v)
            }
            None => None,
        }
    }

    fn put(&mut self, key: String, value: Vec<ReverseResult>) {
        if !self.map.contains_key(&key) && self.map.len() >= self.cap {
            let victim = self
                .map
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone());
            if let Some(victim) = victim {
                self.map.remove(&victim);
            }
        }
        self.stamp = self.stamp.wrapping_add(1);
        self.map.insert(key, (value, self.stamp));
    }
}

// A compact membership test used by is_chinese_word for FMM segmentation.
// Storing every distinct 2-3 char Chinese fragment as a String ballooned RSS
// by ~80MB; a bloom filter is a few MB with a negligible false-positive rate
// (which merely occasionally picks a slightly-too-long segment). The bit set
// is atomic so shard parsing and insertion can run on multiple threads.
struct BloomFilter {
    bits: Vec<std::sync::atomic::AtomicU64>,
    n_bits: usize,
    k: usize,
}

impl BloomFilter {
    fn new(max_elems: usize, fp_rate: f64) -> Self {
        use std::sync::atomic::AtomicU64;
        let n_bits =
            (-(max_elems as f64) * fp_rate.ln() / std::f64::consts::LN_2.powf(2.0)).ceil()
                as usize;
        let n_bits = n_bits.max(64);
        let k = (((n_bits as f64 / max_elems as f64) * std::f64::consts::LN_2).ceil() as usize).max(1);
        Self {
            bits: (0..(n_bits + 63) / 64).map(|_| AtomicU64::new(0)).collect(),
            n_bits,
            k,
        }
    }

    fn indices(&self, x: u64) -> impl Iterator<Item = usize> {
        let n_bits = self.n_bits;
        let k = self.k;
        let h1 = splitmix64(x);
        let h2 = splitmix64(h1);
        (1..=k).map(move |i| {
            ((h1.wrapping_add((i as u64).wrapping_mul(h2))) % n_bits as u64) as usize
        })
    }

    fn insert(&self, x: u64) {
        use std::sync::atomic::Ordering;
        for i in self.indices(x) {
            self.bits[i >> 6].fetch_or(1 << (i & 63), Ordering::Relaxed);
        }
    }

    fn contains(&self, x: u64) -> bool {
        use std::sync::atomic::Ordering;
        self.indices(x)
            .all(|i| (self.bits[i >> 6].load(Ordering::Relaxed) >> (i & 63)) & 1 == 1)
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

/// Pack up to 3 BMP chars (2-3 char Chinese fragments, each <= 0xFFFF) into a
/// single u64, collision-free, for the bloom filter.
fn frag_key(chars: &[char]) -> u64 {
    let mut k = chars.len() as u64;
    for (i, &c) in chars.iter().enumerate().take(3) {
        k |= (c as u64) << (8 + 16 * i);
    }
    k
}

/// A lazily loaded view over the shard files (`aa.json`, `ab.json`, ...).
///
/// Shards are parsed on first use and cached in a small LRU, so startup and
/// idle memory usage stay flat regardless of the total dictionary size. The
/// Chinese membership index is built lazily as a compact bloom filter, and
/// reverse-query results are memoized so repeated hovers stay fast.
pub struct Dictionary {
    source: ShardSource,
    cache: Mutex<ShardCache>,
    reverse_cache: Mutex<ReverseCache>,
    prefixes: OnceLock<Vec<String>>,
    chinese_words: OnceLock<BloomFilter>,
}

impl Dictionary {
    pub fn load() -> Dictionary {
        let source = ShardSource::Fs(dict_dir());
        let mut dict = Dictionary {
            source,
            cache: Mutex::new(ShardCache::new(32)),
            reverse_cache: Mutex::new(ReverseCache::new(128)),
            prefixes: OnceLock::new(),
            chinese_words: OnceLock::new(),
        };
        if dict.shard_prefixes().is_empty() {
            dict.source = ShardSource::Embedded;
            dict.prefixes = OnceLock::new();
        }
        dict
    }

    #[cfg(test)]
    pub fn load_from_dir(dir: &Path) -> Dictionary {
        Dictionary {
            source: ShardSource::Fs(dir.to_path_buf()),
            cache: Mutex::new(ShardCache::new(32)),
            reverse_cache: Mutex::new(ReverseCache::new(128)),
            prefixes: OnceLock::new(),
            chinese_words: OnceLock::new(),
        }
    }

    pub fn shard_count(&self) -> usize {
        self.shard_prefixes().len()
    }

    pub fn shard_prefixes(&self) -> &Vec<String> {
        self.prefixes.get_or_init(|| match &self.source {
            ShardSource::Embedded => EMBEDDED
                .files()
                .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
                .filter_map(|f| f.path().file_stem())
                .filter_map(|s| s.to_str())
                .map(|s| s.to_string())
                .collect(),
            ShardSource::Fs(dir) => fs::read_dir(dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .filter_map(|p| {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
                .collect(),
        })
    }

    fn load_shard_raw(&self, prefix: &str) -> Option<Cow<'_, [u8]>> {
        match &self.source {
            ShardSource::Embedded => {
                let name = format!("{prefix}.json");
                Some(Cow::Borrowed(EMBEDDED.get_file(name)?.contents()))
            }
            ShardSource::Fs(dir) => Some(Cow::Owned(fs::read(dir.join(format!("{prefix}.json"))).ok()?)),
        }
    }

    fn load_shard_parsed(&self, prefix: &str) -> Option<AHashMap<String, Value>> {
        parse_shard_lower(&self.load_shard_raw(prefix)?)
    }

    pub(crate) fn lookup_variant(&self, variant: &str) -> Option<DictEntry> {
        let lower = variant.to_lowercase();
        let prefix = prefix_of(&lower)?;
        let mut cache = self.cache.lock().unwrap();
        if !cache.contains_key(&prefix) {
            let shard = self.load_shard_parsed(&prefix)?;
            cache.put(prefix.clone(), shard);
        }
        let shard = cache.get(&prefix)?;
        val_to_entry(shard.get(&lower)?)
    }

    pub fn lookup(&self, word: &str) -> Option<DictEntry> {
        crate::query::query_dict(word, self)
    }

    pub fn contains(&self, word: &str) -> bool {
        self.lookup_variant(word).is_some()
    }

    pub fn is_chinese_word(&self, text: &str) -> bool {
        let chars: Vec<char> = text
            .chars()
            .filter(|c| c.is_alphanumeric() && !c.is_ascii())
            .collect();
        if chars.len() < 2 || chars.len() > 3 {
            return false;
        }
        self.chinese_words().contains(frag_key(&chars))
    }

    fn chinese_words(&self) -> &BloomFilter {
        self.chinese_words
            .get_or_init(|| self.build_chinese_words())
    }

    fn build_chinese_words(&self) -> BloomFilter {
        use std::thread;

        let bf = BloomFilter::new(5_000_000, 0.01);
        let inner = &bf;
        let prefixes = self.shard_prefixes();
        let workers = thread::available_parallelism()
            .map_or(8, |n| n.get())
            .min(16)
            .max(1);
        let per = (prefixes.len() + workers - 1) / workers;
        thread::scope(|scope| {
            for chunk in prefixes.chunks(per.max(1)) {
                scope.spawn(move || {
                    for prefix in chunk {
                        let Some(shard) = self.load_shard_parsed(prefix) else {
                            continue;
                        };
                        for val in shard.values() {
                            let Some(translation) = val_to_translation(val) else {
                                continue;
                            };
                            for fragment in translation.split(SEP) {
                                let chars: Vec<char> = fragment
                                    .chars()
                                    .filter(|c| c.is_alphanumeric() && !c.is_ascii())
                                    .collect();
                                for width in 2..=3 {
                                    if chars.len() >= width {
                                        for i in 0..=(chars.len() - width) {
                                            inner.insert(frag_key(&chars[i..i + width]));
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            }
        });
        bf
    }

    pub fn reverse_query(&self, chinese: &str, max_results: usize) -> Vec<ReverseResult> {
        use crate::reverse_query::contains_chinese;

        let cleaned = chinese.trim();
        if cleaned.is_empty() || !contains_chinese(cleaned) {
            return vec![];
        }
        let cap = max_results.clamp(1, REVERSE_RESULT_CAP);

        {
            let mut rc = self.reverse_cache.lock().unwrap();
            if let Some(hit) = rc.get(&cleaned) {
                return hit.iter().take(cap).cloned().collect();
            }
        }

        let results = self.scan_reverse(&cleaned);
        {
            let mut rc = self.reverse_cache.lock().unwrap();
            rc.put(cleaned.to_string(), results.clone());
        }
        results.into_iter().take(cap).collect()
    }

    fn scan_reverse(&self, cleaned: &str) -> Vec<ReverseResult> {
        use crate::reverse_query::calculate_match_score;
        use std::thread;

        let prefixes = self.shard_prefixes();
        let workers = thread::available_parallelism()
            .map_or(8, |n| n.get())
            .min(16)
            .max(1);
        let per = (prefixes.len() + workers - 1) / workers;
        let mut matches: Vec<(i64, String, ReverseResult)> = thread::scope(|scope| {
            let handles: Vec<_> = prefixes
                .chunks(per.max(1))
                .map(|chunk| {
                    scope.spawn(move || {
                        let mut local: Vec<(i64, String, ReverseResult)> = Vec::new();
                        for prefix in chunk {
                            let Some(raw) = self.load_shard_raw(prefix) else {
                                continue;
                            };
                            if !bytes_contain(&raw, cleaned.as_bytes()) {
                                continue;
                            }
                            let Some(shard) = self.load_shard_parsed(prefix) else {
                                continue;
                            };
                            for (word, val) in shard.iter() {
                                let Some(translation) = val_to_translation(val) else {
                                    continue;
                                };
                                if translation.contains(cleaned) {
                                    let score = calculate_match_score(translation, cleaned);
                                    local.push((
                                        score,
                                        word.clone(),
                                        ReverseResult {
                                            word: word.clone(),
                                            translation: translation.to_string(),
                                            phonetic: val_to_phonetic(val).unwrap_or_default(),
                                        },
                                    ));
                                }
                            }
                        }
                        local
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .flatten()
                .collect()
        });
        matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        trim_malloc();
        matches
            .into_iter()
            .take(REVERSE_RESULT_CAP)
            .map(|(_, _, result)| result)
            .collect()
    }
}

// Max reverse results retained (settings clamp to 1..=50).
const REVERSE_RESULT_CAP: usize = 50;

// Exact substring test on raw (unparsed) bytes. Valid UTF-8 is
// self-synchronizing, so a byte match of a whole CJK query is equivalent to a
// character substring match; this lets us skip parsing shards that can't
// contain the query.
fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    let first = needle[0];
    let last = needle.len() - 1;
    let mut i = 0;
    while i <= haystack.len() - needle.len() {
        if haystack[i] == first && haystack[i + last] == needle[last]
            && haystack[i..i + needle.len()] == *needle
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Return freed bytes from glibc's per-thread malloc arenas to the OS after a
/// full reverse scan, so transient parse allocations don't inflate RSS in a
/// long-lived LSP process.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim_malloc() {
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim_malloc() {}

fn parse_shard_lower(bytes: &[u8]) -> Option<AHashMap<String, Value>> {
    let object = serde_json::from_slice::<serde_json::Map<String, Value>>(bytes).ok()?;
    let mut out = AHashMap::with_capacity(object.len());
    for (key, value) in object {
        out.insert(key.to_lowercase(), value);
    }
    Some(out)
}

fn prefix_of(lower: &str) -> Option<String> {
    let mut chars = lower.chars();
    let c0 = chars.next()?;
    let c1 = chars.next()?;
    if !c0.is_ascii_alphabetic() || !c1.is_ascii_alphabetic() {
        return None;
    }
    Some(format!("{c0}{c1}"))
}

fn val_to_translation(val: &Value) -> Option<&str> {
    match val {
        Value::String(t) => Some(t),
        Value::Object(o) => Some(o.get("t").and_then(|v| v.as_str()).unwrap_or("")),
        _ => None,
    }
}

fn val_to_phonetic(val: &Value) -> Option<String> {
    match val {
        Value::String(_) => None,
        Value::Object(o) => o.get("p").and_then(|v| v.as_str()).map(|s| s.to_string()),
        _ => None,
    }
}

fn val_to_entry(val: &Value) -> Option<DictEntry> {
    match val {
        Value::String(t) => Some(DictEntry {
            phonetic: String::new(),
            translation: t.clone(),
        }),
        Value::Object(o) => Some(DictEntry {
            phonetic: o.get("p").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            translation: o.get("t").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a mini dictionary into a temp dir, returning the dir path
    fn make_temp_dict() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("us.json")).unwrap();
        // String-form entry
        writeln!(f, "{{\"user\": \"n. 使用者\", \"use\": \"vt. 使用\"}}").unwrap();
        drop(f);

        let mut f2 = std::fs::File::create(dir.path().join("pr.json")).unwrap();
        // Object-form entry (with phonetic / translation)
        writeln!(
            f2,
            "{{\"profile\": {{\"w\": \"profile\", \"p\": \"'prәufail\", \"t\": \"n. 侧面\"}}}}"
        )
        .unwrap();
        drop(f2);

        dir
    }

    #[test]
    fn test_load_and_lookup_string_entry() {
        let dir = make_temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        // The mini dictionary contains user / use / profile
        assert!(dict.lookup("user").is_some());
        assert!(dict.lookup("use").is_some());
        assert!(dict.lookup("profile").is_some());
        let e = dict.lookup("user").expect("user should exist");
        assert_eq!(e.translation, "n. 使用者");
        assert!(e.phonetic.is_empty());
    }

    #[test]
    fn test_lookup_object_entry() {
        let dir = make_temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        let e = dict.lookup("profile").expect("profile should exist");
        assert_eq!(e.phonetic, "'prәufail");
        assert_eq!(e.translation, "n. 侧面");
    }

    #[test]
    fn test_lookup_case_insensitive() {
        let dir = make_temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        // Lowercase queries should match
        assert!(dict.lookup("USER").is_some());
        assert!(dict.lookup("User").is_some());
    }

    #[test]
    fn test_lookup_missing() {
        let dir = make_temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        assert!(dict.lookup("nonexistent").is_none());
        assert!(!dict.contains("nonexistent"));
    }

    #[test]
    fn test_reverse_query() {
        let dir = make_temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        let results = dict.reverse_query("使用者", 10);
        assert!(results.iter().any(|r| r.word == "user"));
    }

    #[test]
    fn test_reverse_query_memoized() {
        let dir = make_temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        let a = dict.reverse_query("使用者", 10);
        let b = dict.reverse_query("使用者", 5);
        assert_eq!(b.len(), a.len().min(5));
        assert!(b.iter().any(|r| r.word == "user"));
    }

    #[test]
    fn test_is_chinese_word() {
        let dir = make_temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        assert!(dict.is_chinese_word("使用"));
        assert!(dict.is_chinese_word("使用者"));
        assert!(!dict.is_chinese_word("使用目录")); // 4 characters
        assert!(!dict.is_chinese_word("user"));
    }

    /// Verify the compile-time embedded dictionary works (fallback path for released binaries without an external dict/).
    /// It embeds the repo-root full dictionary, so it should contain common words like "user".
    #[test]
    fn test_load_embedded_fallback() {
        let dict = Dictionary::load();
        assert!(
            dict.shard_count() > 0,
            "embedded dict should not be empty; check build.rs path"
        );
        assert!(
            dict.lookup("user").is_some(),
            "embedded dict missing 'user'"
        );
        assert!(dict.is_chinese_word("用户"), "embedded dict missing 用户");
    }
}
