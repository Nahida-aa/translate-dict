// Chinese-to-English reverse query.
//
// On Chinese hover, scan every dictionary entry and find English words whose
// translations contain the Chinese word; sort by match score and return the top N.
// Offline, full scan, but the resident in-memory dictionary keeps it fast.

use crate::dict::Dictionary;

pub struct ReverseResult {
    pub word: String,
    pub translation: String,
    pub phonetic: String,
}

/// Whether the text is "pure Chinese" (contains Chinese and no English letters)
pub fn contains_chinese(text: &str) -> bool {
    let has_chinese = text.chars().any(|c| ('\u{4e00}'..='\u{9fa5}').contains(&c));
    let has_english = text.chars().any(|c| c.is_ascii_alphabetic());
    has_chinese && !has_english
}

fn calculate_match_score(translation: &str, search: &str) -> i64 {
    if translation == search {
        return 1000;
    }
    let separators: &[char] = &['；', ';', '、', '，', ',', ' ', '\n', '.'];
    let parts: Vec<&str> = translation
        .split(separators)
        .filter(|p| !p.is_empty())
        .collect();
    if let Some(idx) = parts.iter().position(|p| *p == search) {
        return 900 - (idx as i64) * 5;
    }
    if translation.starts_with(search) {
        let ratio = search.len() as f64 / translation.len() as f64;
        return 700 + (ratio * 100.0) as i64;
    }
    if let Some(idx) = translation.find(search) {
        let length_ratio = search.len() as f64 / translation.len() as f64;
        let position_penalty = (idx * 2).min(100);
        return 500 + (length_ratio * 100.0) as i64 - position_penalty as i64;
    }
    0
}

/// Reverse query: given Chinese, return matching English words (sorted by score descending)
pub fn reverse_query(chinese: &str, dict: &Dictionary, max_results: usize) -> Vec<ReverseResult> {
    let cleaned = chinese.trim();
    if cleaned.is_empty() || !contains_chinese(cleaned) {
        return vec![];
    }

    let mut matches: Vec<(i64, String, ReverseResult)> = Vec::new();

    for (word, entry) in dict.entries() {
        if entry.translation.contains(cleaned) {
            let score = calculate_match_score(&entry.translation, cleaned);
            matches.push((
                score,
                word.to_string(),
                ReverseResult {
                    word: word.to_string(),
                    translation: entry.translation.clone(),
                    phonetic: entry.phonetic.clone(),
                },
            ));
        }
    }

    matches.sort_by(|a, b| {
        b.0.cmp(&a.0) // score descending
            .then_with(|| a.1.cmp(&b.1)) // ties broken by alphabetical word order
    });

    matches
        .into_iter()
        .take(max_results)
        .map(|(_, _, r)| r)
        .collect()
}
