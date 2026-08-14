// Local dictionary loading and lookup.
// Dictionary files live in the extension repo's dict/ dir, sharded by the
// word's first two letters (aa.json ~ zz.json); each file is { "word": {"w","p","t"} | "translation" }.
// All of it is read into memory at startup (~760k words, loaded once, kept resident).
//
// Memory optimization: DictEntry no longer stores a word field — the map key
// (lowercased word) is the word itself, so a duplicated String is pure waste
// (~760k entries x one String heap alloc). The query key is used for display
// (translate-dict also shows lowercase). HashMap/HashSet use ahash, faster and
// more compact than the default SipHash.

use ahash::{AHashMap, AHashSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

// Built-in dictionary embedded at compile time (build.rs writes the literal path to OUT_DIR/embedded_dict.rs).
// Only release binaries need it; in dev the filesystem dict/ is used instead.
include!(concat!(env!("OUT_DIR"), "/embedded_dict.rs"));

#[derive(Clone)]
pub struct DictEntry {
    pub phonetic: String,
    pub translation: String,
}

pub struct Dictionary {
    /// Key is the lowercased word; value is the entry (no word field)
    map: AHashMap<String, DictEntry>,
    /// Chinese word index: 2~3-char pure-Chinese fragments extracted from entry translations.
    /// Used for Chinese forward-maximum-matching (FMM) segmentation, O(1) check of valid Chinese words.
    chinese_words: AHashSet<String>,
}

impl Dictionary {
    pub fn load_from_dir(dir: &Path) -> Self {
        let mut map: AHashMap<String, DictEntry> = AHashMap::new();
        let mut chinese_words: AHashSet<String> = AHashSet::new();

        // Separators used when extracting Chinese word fragments from translations (consistent with reverse_query)
        let sep: &[char] = &[
            '；', ';', '、', '，', ',', ' ', '\n', '.', '：', ':', '（', '(', '）', ')', '《', '<',
            '》', '>', '“', '"', '”', '【', '[', '】', ']', '！', '!', '？', '?', '—', '~', '·',
            '/', '\\', '-',
        ];

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&content) {
                        for (key, val) in obj {
                            let translation = match &val {
                                Value::String(t) => t.clone(),
                                Value::Object(o) => o
                                    .get("t")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                _ => continue,
                            };
                            let entry = match val {
                                Value::String(t) => DictEntry {
                                    phonetic: String::new(),
                                    translation: t,
                                },
                                Value::Object(o) => DictEntry {
                                    phonetic: o
                                        .get("p")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    translation: o
                                        .get("t")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                },
                                _ => continue,
                            };
                            map.insert(key.to_lowercase(), entry);

                            // Extract 2~3-char pure-Chinese fragments from the translation to build the Chinese word index.
                            // Only 2~3 chars: 4-char Chinese is mostly phrases/sentence fragments, low value
                            // as FMM dictionary words and would consume nearly half the index memory;
                            // FMM then degrades to a 2+2 split.
                            for frag in translation.split(sep) {
                                let chars: Vec<char> = frag
                                    .chars()
                                    .filter(|c| c.is_alphanumeric() && !c.is_ascii())
                                    .collect();
                                if chars.len() >= 2 && chars.len() <= 3 {
                                    chinese_words.insert(chars.iter().collect());
                                }
                            }
                        }
                    }
                }
            }
        }

        Self { map, chinese_words }
    }

    /// Load the dictionary: prefer the filesystem dict/ (dev, so the dictionary can be
    /// changed without recompiling), falling back to the compile-time embedded dict/
    /// (release binaries carry it, needing no external files).
    /// This keeps the released LS binary self-contained — cargo-dist ships only the
    /// binary, not the dict/ dir, so embedding is required for end users to translate.
    pub fn load() -> Self {
        let fs_dir = crate::dict_dir();
        let fs_dict = Self::load_from_dir(&fs_dir);
        if !fs_dict.map.is_empty() {
            return fs_dict;
        }
        Self::load_embedded()
    }

    // Load from the compile-time embedded dict/ (include_dir! bakes the whole dir into the binary at build time).
    fn load_embedded() -> Self {
        let mut map: AHashMap<String, DictEntry> = AHashMap::new();
        let mut chinese_words: AHashSet<String> = AHashSet::new();

        let sep: &[char] = &[
            '；', ';', '、', '，', ',', ' ', '\n', '.', '：', ':', '（', '(', '）', ')', '《', '<',
            '》', '>', '“', '"', '”', '【', '[', '】', ']', '！', '!', '？', '?', '—', '~', '·',
            '/', '\\', '-',
        ];

        for file in EMBEDDED.files() {
            if file.path().extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(text) = std::str::from_utf8(file.contents()) {
                if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text) {
                    for (key, val) in obj {
                        let translation = match &val {
                            Value::String(t) => t.clone(),
                            Value::Object(o) => o
                                .get("t")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            _ => continue,
                        };
                        let entry = match val {
                            Value::String(t) => DictEntry {
                                phonetic: String::new(),
                                translation: t,
                            },
                            Value::Object(o) => DictEntry {
                                phonetic: o
                                    .get("p")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                translation: o
                                    .get("t")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            },
                            _ => continue,
                        };
                        map.insert(key.to_lowercase(), entry);

                        for frag in translation.split(sep) {
                            let chars: Vec<char> = frag
                                .chars()
                                .filter(|c| c.is_alphanumeric() && !c.is_ascii())
                                .collect();
                            if chars.len() >= 2 && chars.len() <= 3 {
                                chinese_words.insert(chars.iter().collect());
                            }
                        }
                    }
                }
            }
        }

        Self { map, chinese_words }
    }

    /// Look up by the raw variant string (variant already has case; internally normalized to a lowercase key)
    pub(crate) fn lookup_variant(&self, variant: &str) -> Option<&DictEntry> {
        self.map.get(&variant.to_lowercase())
    }

    /// Query a word, returning the matching entry (delegates to query.rs::query_dict)
    pub fn lookup(&self, word: &str) -> Option<&DictEntry> {
        crate::query::query_dict(word, self)
    }

    pub fn contains(&self, word: &str) -> bool {
        crate::query::is_word_in_dict(word, self)
    }

    /// Return all (lowercased word, entry) pairs (used for the full scan of Chinese-to-English reverse queries)
    pub fn entries(&self) -> impl Iterator<Item = (&str, &DictEntry)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Chinese FMM: check whether `text` is a known Chinese word (present in the Chinese word index)
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_chinese_word(&self, text: &str) -> bool {
        let chars: Vec<char> = text
            .chars()
            .filter(|c| c.is_alphanumeric() && !c.is_ascii())
            .collect();
        if chars.len() < 2 || chars.len() > 3 {
            return false;
        }
        self.chinese_words
            .contains(&chars.iter().collect::<String>())
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
        assert!(dict.contains("nonexistent") == false);
    }

    /// Verify the compile-time embedded dictionary works (fallback path for released binaries without an external dict/).
    /// It embeds the repo-root full dictionary, so it should contain common words like "user".
    #[test]
    fn test_load_embedded_fallback() {
        let dict = Dictionary::load_embedded();
        assert!(
            !dict.map.is_empty(),
            "embedded dict should not be empty; check build.rs path"
        );
        assert!(
            dict.lookup("user").is_some(),
            "embedded dict missing 'user'"
        );
        // Chinese word index should be built (2-char Chinese words are indexed)
        assert!(dict.is_chinese_word("用户"), "embedded dict missing 用户");
    }
}
