// 中译英反向查询（移植自 translate-dict 的 src/reverseQuery.ts）。
//
// 中文 hover 时，扫描全部词库条目，找出翻译文本里包含该中文词条的英文单词，
// 按匹配度打分排序，返回前 N 个。离线、全表扫描，但词库常驻内存所以很快。

use crate::dict::Dictionary;

pub struct ReverseResult {
    pub word: String,
    pub translation: String,
    pub phonetic: String,
}

/// 是否为"纯中文"（含中文且不含英文字母）
pub fn contains_chinese(text: &str) -> bool {
    let has_chinese = text.chars().any(|c| ('\u{4e00}'..='\u{9fa5}').contains(&c));
    let has_english = text.chars().any(|c| c.is_ascii_alphabetic());
    has_chinese && !has_english
}

/// 计算匹配度分数（对齐 reverseQuery.ts::calculateMatchScore）
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

/// 反向查询：根据中文返回匹配的英文单词列表（按分数降序）
pub fn reverse_query(chinese: &str, dict: &Dictionary, max_results: usize) -> Vec<ReverseResult> {
    let cleaned = chinese.trim();
    if cleaned.is_empty() || !contains_chinese(cleaned) {
        return vec![];
    }

    let mut matches: Vec<(i64, String, ReverseResult)> = Vec::new();

    for entry in dict.all_entries() {
        if entry.translation.contains(cleaned) {
            let score = calculate_match_score(&entry.translation, cleaned);
            matches.push((
                score,
                entry.word.clone(),
                ReverseResult {
                    word: entry.word.clone(),
                    translation: entry.translation.clone(),
                    phonetic: entry.phonetic.clone(),
                },
            ));
        }
    }

    matches.sort_by(|a, b| {
        b.0.cmp(&a.0) // 分数降序
            .then_with(|| a.1.cmp(&b.1)) // 同分时按单词字母序
    });

    matches
        .into_iter()
        .take(max_results)
        .map(|(_, _, r)| r)
        .collect()
}
