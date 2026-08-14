// Goal: split identifiers like getUserProfile / HTTPService / redblacktree / send_email
// into English words the dictionary can look up, then query each to return translations.
//
// The algorithm has two layers:
//   1) split_by_case — splits on separators (- _ space) and case boundaries, handling abbreviation chains;
//   2) split_compound_word — for each segment does "best compound-word split", preferring complete
//      dictionary words and picking the best split with a scoring function.
//
// All lookups go through the in-memory Dictionary (loaded once at startup): zero IO, zero network.

use crate::dict::Dictionary;

/// Whether a single-char fragment is allowed (a / i count as valid words)
fn is_allowed_single_character(part: &str) -> bool {
    part.eq_ignore_ascii_case("a") || part.eq_ignore_ascii_case("i")
}

/// Technical abbreviation: all-caps and length >= 2
fn is_technical_abbreviation(part: &str) -> bool {
    !part.is_empty() && part.chars().all(|c| c.is_ascii_uppercase()) && part.len() >= 2
}

/// Split an all-caps abbreviation chain, e.g. HTTPService -> HTTP + Service
/// Ported from splitUppercaseAbbreviationChain
fn split_uppercase_abbreviation_chain(word: &str, dict: &Dictionary) -> Vec<String> {
    if dict.contains(word) {
        return vec![word.to_string()];
    }

    let n = word.chars().count();
    // Try cutting at i from the back; the front segment must be in the dictionary
    for i in (2..n.saturating_sub(1)).rev() {
        let (first, second) = word.split_at(i);
        if !dict.contains(first) {
            continue;
        }
        let second_parts = split_compound_word(second, dict);
        if second_parts.len() == 1 && second_parts[0] == second && !dict.contains(second) {
            continue;
        }
        let mut out = vec![first.to_string()];
        out.extend(second_parts);
        return out;
    }

    vec![word.to_string()]
}

/// Best split of a lowercase compound word (DP + memo, ported from splitLowercaseCompoundWord)
fn split_lowercase_compound_word(word: &str, dict: &Dictionary) -> Vec<String> {
    let lower = word.to_lowercase();
    let n = lower.chars().count();
    let mut memo: Vec<Option<(Vec<String>, i64)>> = vec![None; n + 1];
    search_lc(0, &lower, word, dict, &mut memo)
        .map(|(parts, _)| parts)
        .unwrap_or_else(|| vec![word.to_string()])
}

/// Best split starting at `start` (byte offset; the word is all-ASCII) as (parts + score), None if impossible
fn search_lc(
    start: usize,
    lower: &str,
    word: &str,
    dict: &Dictionary,
    memo: &mut Vec<Option<(Vec<String>, i64)>>,
) -> Option<(Vec<String>, i64)> {
    if start == lower.len() {
        return Some((vec![], 0));
    }
    if let Some(cached) = &memo[start] {
        return Some(cached.clone());
    }

    let mut best: Option<(Vec<String>, i64)> = None;
    let byte_len = lower.len();
    for end in (start + 1)..=byte_len {
        let part = &lower[start..end];
        let is_dict_word = part.len() > 1 && dict.contains(part);
        let is_single = part.len() == 1 && is_allowed_single_character(part);
        if !is_dict_word && !is_single {
            continue;
        }
        // If this branch can't cover the tail, try the next split point (can't early-return
        // with ?, which would miss other viable branches and wrongly memoize None)
        let Some(rest) = search_lc(end, lower, word, dict, memo) else {
            continue;
        };

        // Candidate score: longer words are better, fewer segments are better
        let mut score: i64 = 0;
        for p in &rest.0 {
            if p.len() == 1 {
                score -= if is_allowed_single_character(p) {
                    20
                } else {
                    200
                };
                continue;
            }
            score += (p.len() * p.len() * 10) as i64;
        }
        score -= rest.0.len() as i64 * 25;
        if part.len() == 1 {
            score -= if is_allowed_single_character(part) {
                20
            } else {
                200
            };
        } else {
            score += (part.len() * part.len() * 10) as i64;
        }

        let mut parts = vec![word[start..end].to_string()];
        parts.extend(rest.0.clone());

        best = match best {
            None => Some((parts, score)),
            Some((_, bscore)) => {
                if score > bscore {
                    Some((parts, score))
                } else {
                    Some((best.unwrap().0, bscore))
                }
            }
        };
    }
    memo[start] = best.clone();
    best
}

/// Normalize a leading I prefix: IUserService -> UserService (drops the I)
fn normalize_leading_interface_prefix(parts: &[String]) -> Vec<String> {
    if parts.len() < 2 || parts[0] != "I" || !is_technical_abbreviation(&parts[1]) {
        return parts.to_vec();
    }
    let mut out = vec![parts[1].clone()];
    out.extend_from_slice(&parts[2..]);
    out
}

/// Score a set of split parts (ported from scoreSplitParts)
fn score_split_parts(parts: &[String], dict: &Dictionary) -> i64 {
    let mut score: i64 = 0;
    score -= parts.len() as i64 * 24;

    for part in parts {
        let normalized = part.to_lowercase();
        let dict_result = dict.lookup(part).or_else(|| dict.lookup(&normalized));

        if dict_result.is_some() {
            score += 60 + (normalized.len() as i64 * 8).min(96);
        } else if is_allowed_single_character(part) {
            score -= 20;
        } else {
            score -= 120;
        }

        if is_technical_abbreviation(part) {
            score += 18;
        }
        if part.len() == 1 && !is_allowed_single_character(part) {
            score -= 80;
        }
        if part.len() == 2 {
            score -= 36;
        }
        if part.len() == 3 {
            score -= 12;
        }
    }
    score
}

fn pick_better_candidate(
    current: Option<(Vec<String>, i64)>,
    candidate: (Vec<String>, i64),
) -> (Vec<String>, i64) {
    match current {
        None => candidate,
        Some((cparts, cscore)) => {
            if candidate.1 != cscore {
                return if candidate.1 > cscore {
                    candidate
                } else {
                    (cparts, cscore)
                };
            }
            if candidate.0.len() != cparts.len() {
                return if candidate.0.len() < cparts.len() {
                    candidate
                } else {
                    (cparts, cscore)
                };
            }
            let cand_longest = candidate.0.iter().map(|p| p.len()).max().unwrap_or(0);
            let cur_longest = cparts.iter().map(|p| p.len()).max().unwrap_or(0);
            if cand_longest > cur_longest {
                candidate
            } else {
                (cparts, cscore)
            }
        }
    }
}

/// Best compound-word split (ported from findBestCompoundSplit)
fn find_best_compound_split(word: &str, dict: &Dictionary) -> Vec<String> {
    if word.len() >= 4 && word.chars().all(|c| c.is_ascii_uppercase()) {
        return split_uppercase_abbreviation_chain(word, dict);
    }
    if word.chars().all(|c| c.is_ascii_lowercase()) {
        return split_lowercase_compound_word(word, dict);
    }

    let mut best = (
        vec![word.to_string()],
        score_split_parts(&[word.to_string()], dict),
    );
    let lower = word.to_lowercase();

    for i in 1..=(lower.len().saturating_sub(2)) {
        let first = &lower[..i];
        let second = &word[i..];
        let first_valid = if i == 1 {
            is_allowed_single_character(first)
        } else {
            dict.contains(first)
        };
        if !first_valid {
            continue;
        }
        let second_parts = normalize_leading_interface_prefix(&split_compound_word(second, dict));
        let mut candidate_parts = vec![word[..i].to_string()];
        candidate_parts.extend(second_parts);
        let candidate = (
            candidate_parts.clone(),
            score_split_parts(&candidate_parts, dict),
        );
        best = pick_better_candidate(Some(best), candidate);
    }

    normalize_leading_interface_prefix(&best.0)
}

/// Entry point for splitting a single segment compound word (ported from splitCompoundWord)
fn split_compound_word(word: &str, dict: &Dictionary) -> Vec<String> {
    if dict.contains(&word.to_lowercase()) {
        return vec![word.to_string()];
    }
    find_best_compound_split(word, dict)
}

// Hand-written match for translate-dict's regex:
//   [A-Z]+(?=[A-Z][a-z]|$) | [A-Z][a-z]* | [a-z]+
// Equivalent to the classic camelCase split: after consecutive caps, if followed by
// "cap+lowercase", the last cap stays with the lowercase segment.
mod regex_match {
    pub struct Match<'a> {
        pub s: &'a str,
    }

    struct Iter<'a> {
        s: &'a str,
        chars: Vec<char>,
        pos: usize,
    }

    impl<'a> Iterator for Iter<'a> {
        type Item = Match<'a>;
        fn next(&mut self) -> Option<Match<'a>> {
            let n = self.chars.len();
            while self.pos < n {
                let c = self.chars[self.pos];
                if c.is_ascii_uppercase() {
                    let mut j = self.pos;
                    while j < n && self.chars[j].is_ascii_uppercase() {
                        j += 1;
                    }
                    if j < n && self.chars[j].is_ascii_lowercase() && (j - self.pos) >= 2 {
                        // HTTPService -> "HTTP" + "Service"  ([A-Z]+(?=[A-Z][a-z]))
                        let upper_end = j - 1;
                        let seg = &self.s[self.pos..upper_end];
                        self.pos = upper_end;
                        return Some(Match { s: seg });
                    }
                    if j < n && self.chars[j].is_ascii_lowercase() {
                        // [A-Z][a-z]*  -> single cap followed by lowercase, whole thing as one token (User)
                        while j < n && self.chars[j].is_ascii_lowercase() {
                            j += 1;
                        }
                        let seg = &self.s[self.pos..j];
                        self.pos = j;
                        return Some(Match { s: seg });
                    }
                    // All-caps segment (may be further split later as an abbreviation chain)
                    let seg = &self.s[self.pos..j];
                    self.pos = j;
                    return Some(Match { s: seg });
                } else if c.is_ascii_lowercase() {
                    let mut j = self.pos;
                    while j < n && self.chars[j].is_ascii_lowercase() {
                        j += 1;
                    }
                    let seg = &self.s[self.pos..j];
                    self.pos = j;
                    return Some(Match { s: seg });
                } else {
                    self.pos += 1;
                }
            }
            None
        }
    }

    pub fn find_all<'a>(s: &'a str) -> impl Iterator<Item = Match<'a>> {
        Iter {
            s,
            chars: s.chars().collect(),
            pos: 0,
        }
    }
}

/// ^I[A-Z]{2,}$  -> I followed by at least two caps
fn regex_is_i_aa(m: &str) -> bool {
    m.len() >= 3 && m.starts_with('I') && m[1..].chars().all(|c| c.is_ascii_uppercase())
}

/// Top-level split by case / separators (ported from splitByCase)
fn split_by_case(s: &str, dict: &Dictionary) -> Vec<String> {
    if dict.contains(s) {
        return vec![s.to_string()];
    }

    let parts: Vec<&str> = s
        .split(|c: char| c == '-' || c == '_' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    let mut result: Vec<String> = Vec::new();

    for part in parts {
        let matches: Vec<&str> = regex_match::find_all(part).map(|m| m.s).collect();
        for m in matches {
            if regex_is_i_aa(m) {
                result.push("I".to_string());
                result.push(m[1..].to_string());
            } else if m.len() >= 4 && m.chars().all(|c| c.is_ascii_uppercase()) {
                result.extend(split_uppercase_abbreviation_chain(m, dict));
            } else {
                result.push(m.to_string());
            }
        }
    }
    result
}

/// Split and query (ported from parseAndQuery)
/// Returns the deduplicated final word list (fragments of length <= 1 already filtered).
pub fn parse_and_query(word: &str, dict: &Dictionary) -> Vec<String> {
    let cleaned: String = word
        .replace('"', "")
        .chars()
        .filter(|c| !c.is_ascii_digit())
        .collect();
    if cleaned.is_empty() {
        return vec![];
    }

    let words = split_by_case(&cleaned, dict);

    let mut seen = std::collections::HashSet::new();
    let filtered: Vec<String> = words
        .into_iter()
        .filter(|w| {
            if w.len() <= 1 {
                return false;
            }
            seen.insert(w.to_lowercase())
        })
        .collect();

    let mut expanded: Vec<String> = Vec::new();
    for w in &filtered {
        expanded.extend(split_compound_word(w, dict));
    }

    let mut seen2 = std::collections::HashSet::new();
    expanded
        .into_iter()
        .filter(|w| {
            if w.len() <= 1 {
                return false;
            }
            seen2.insert(w.to_lowercase())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::Dictionary;
    use std::io::Write;

    /// Write a mini dictionary into a temp dir covering the words needed for common split checks
    fn temp_dict() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let words = [
            ("get", "vt. 得到"),
            ("user", "n. 使用者"),
            ("profile", "n. 侧面"),
            ("name", "n. 名字"),
            ("send", "vt. 发送"),
            ("email", "n. 电子邮件"),
            ("info", "n. 信息"),
            ("service", "n. 服务"),
            ("red", "a. 红的"),
            ("black", "a. 黑的"),
            ("tree", "n. 树"),
            ("use", "vt. 使用"),
            ("http", "n. 超文本"),
            ("xml", "n. 可扩展标记语言"),
            ("parser", "n. 解析器"),
            ("user", "n. 使用者"),
        ];
        // Write, sharded by the first two letters
        let mut buckets: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for (w, t) in words {
            let prefix = w[..2].to_string();
            buckets
                .entry(prefix)
                .or_default()
                .push_str(&format!("\"{}\":\"{}\",", w, t));
        }
        for (prefix, body) in buckets {
            let mut f = std::fs::File::create(dir.path().join(format!("{prefix}.json"))).unwrap();
            let _ = writeln!(f, "{{{}}}", body.trim_end_matches(','));
        }
        dir
    }

    #[test]
    fn test_camel_case() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        // Splits preserve original casing (consistent with translate-dict's parseAndQuery);
        // the hover display later uses the canonical (lowercase) form from the dictionary.
        assert_eq!(
            parse_and_query("getUserProfile", &dict),
            vec!["get", "User", "Profile"]
        );
        assert_eq!(
            parse_and_query("getUserInfo", &dict),
            vec!["get", "User", "Info"]
        );
    }

    #[test]
    fn test_pascal_case() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        assert_eq!(parse_and_query("UserName", &dict), vec!["User", "Name"]);
    }

    #[test]
    fn test_snake_case() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        assert_eq!(parse_and_query("user_name", &dict), vec!["user", "name"]);
    }

    #[test]
    fn test_kebab_case() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        assert_eq!(parse_and_query("user-name", &dict), vec!["user", "name"]);
    }

    #[test]
    fn test_abbreviation_chain() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        // HTTPService -> HTTP + Service (split parts keep original casing)
        let parts = parse_and_query("HTTPService", &dict);
        assert!(parts.contains(&"Service".to_string()));
        assert!(parts.contains(&"http".to_string()) || parts.contains(&"HTTP".to_string()));
    }

    #[test]
    fn test_lowercase_compound() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        assert_eq!(
            parse_and_query("redblacktree", &dict),
            vec!["red", "black", "tree"]
        );
    }

    #[test]
    fn test_digits_filtered() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        assert_eq!(parse_and_query("user123", &dict), vec!["user"]);
    }

    #[test]
    fn test_dedup_case_insensitive() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        // User/user deduplicated (case-insensitive), original-casing fragments kept
        let parts = parse_and_query("Useruser", &dict);
        assert_eq!(parts, vec!["User"]);
    }

    #[test]
    fn test_short_word_ignored() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        // Fragments of length <= 1 are filtered out
        assert!(parse_and_query("a", &dict).is_empty());
    }
}
