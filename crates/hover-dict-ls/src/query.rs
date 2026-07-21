// 单词查询（移植自 translate-dict 的 src/query.ts）。
//
// 参考项目里 query.ts 负责「单个词的变体生成 + 在词典里查找」，分为：
//   - getWordVariants：生成原文/小写/首字母大写/缩写加点/全大写 等变体
//   - loadDict：按前两字母懒加载分片（我们改为启动全量预加载，故省略）
//   - findInDict / queryDict / isWordInDict：查词
//
// 这里保留 变体生成 + 查词 的核心，词典数据来自 dict.rs 的 Dictionary。

use crate::dict::{DictEntry, Dictionary};

/// 生成单词的各种大小写变体，按优先级排序：
/// 原文 → 小写 → 首字母大写 → 首字母大写加点(缩写形式) → 全大写
/// 移植自 query.ts::getWordVariants
pub fn get_word_variants(word: &str) -> Vec<String> {
    let mut variants: Vec<String> = vec![word.to_string()];
    let lower_word = word.to_lowercase();
    let upper_word = word.to_uppercase();
    let capitalized_word = format!("{}{}", lower_word[..1].to_uppercase(), &lower_word[1..]);
    // 首字母大写加点（缩写形式），如 Ht -> Ht.
    let capitalized_with_dot = format!("{capitalized_word}.");

    if lower_word != word {
        variants.push(lower_word.clone());
    }
    if capitalized_word != word && capitalized_word != lower_word {
        variants.push(capitalized_word.clone());
    }
    variants.push(capitalized_with_dot);
    if upper_word != word {
        variants.push(upper_word);
    }

    variants
}

/// 查询单词的词典结果（移植自 query.ts::queryDict）
pub fn query_dict<'a>(word: &str, dict: &'a Dictionary) -> Option<&'a DictEntry> {
    if word.len() < 2 {
        return None;
    }
    let variants = get_word_variants(word);
    for variant in variants {
        if let Some(entry) = dict.lookup_variant(&variant) {
            return Some(entry);
        }
    }
    None
}

/// 判断单词是否在词典中存在（移植自 query.ts::isWordInDict）
pub fn is_word_in_dict(word: &str, dict: &Dictionary) -> bool {
    query_dict(word, dict).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::Dictionary;
    use std::io::Write;

    fn temp_dict() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("us.json")).unwrap();
        writeln!(f, "{{\"user\": \"n. 使用者\", \"use\": \"vt. 使用\"}}").unwrap();
        drop(f);
        let mut f2 = std::fs::File::create(dir.path().join("pr.json")).unwrap();
        writeln!(
            f2,
            "{{\"profile\": {{\"w\": \"profile\", \"p\": \"'prәufail\", \"t\": \"n. 侧面\"}}}}"
        )
        .unwrap();
        drop(f2);
        dir
    }

    #[test]
    fn test_get_word_variants() {
        let v = get_word_variants("User");
        // 原文、小写、首字母大写、首字母大写加点、全大写
        assert!(v.contains(&"User".to_string()));
        assert!(v.contains(&"user".to_string()));
        assert!(v.contains(&"User.".to_string()));
        assert!(v.contains(&"USER".to_string()));
    }

    #[test]
    fn test_query_dict_string_entry() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        let e = query_dict("user", &dict).expect("user exists");
        assert_eq!(e.translation, "n. 使用者");
    }

    #[test]
    fn test_query_dict_object_entry() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        let e = query_dict("Profile", &dict).expect("profile exists");
        assert_eq!(e.phonetic, "'prәufail");
    }

    #[test]
    fn test_query_dict_case_insensitive() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        assert!(is_word_in_dict("USER", &dict));
        assert!(is_word_in_dict("User", &dict));
        assert!(!is_word_in_dict("nope", &dict));
    }
}
