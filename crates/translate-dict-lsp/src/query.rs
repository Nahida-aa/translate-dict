// This keeps the variant-generation + lookup core; dictionary data comes from dict.rs's Dictionary.

use crate::dict::{DictEntry, Dictionary};

/// Generate case variants of a word, ordered by priority:
/// original -> lowercase -> capitalized -> capitalized-with-dot (abbreviation) -> all-caps
pub fn get_word_variants(word: &str) -> Vec<String> {
    let mut variants: Vec<String> = vec![word.to_string()];
    let lower_word = word.to_lowercase();
    let upper_word = word.to_uppercase();
    let capitalized_word = format!("{}{}", lower_word[..1].to_uppercase(), &lower_word[1..]);
    // Capitalized with a dot (abbreviated form), e.g. Ht -> Ht.
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

pub fn query_dict(word: &str, dict: &Dictionary) -> Option<DictEntry> {
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
        // original, lowercase, capitalized, capitalized-with-dot, all-caps
        assert!(v.contains(&"User".to_string()));
        assert!(v.contains(&"user".to_string()));
        assert!(v.contains(&"User.".to_string()));
        assert!(v.contains(&"USER".to_string()));
    }

    #[test]
    fn test_query_dict_string_entry() {
        let dir = temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        let e = query_dict("user", &dict).expect("user exists");
        assert_eq!(e.translation, "n. 使用者");
    }

    #[test]
    fn test_query_dict_object_entry() {
        let dir = temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        let e = query_dict("Profile", &dict).expect("profile exists");
        assert_eq!(e.phonetic, "'prәufail");
    }

    #[test]
    fn test_query_dict_case_insensitive() {
        let dir = temp_dict();
        let dict = Dictionary::load_from_dir(dir.path());
        assert!(dict.contains("USER"));
        assert!(dict.contains("User"));
        assert!(!dict.contains("nope"));
    }
}
