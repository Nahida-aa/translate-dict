// Word extraction: take the "word" (identifier / Chinese word) under the cursor from a text line.
//
// - Splitting English identifiers (camelCase / snake_case / abbreviation chains / compound
//   words) is handled in utils::format::parse_and_query; this module only finds which
//   segment the cursor is on.
// - Chinese segments use forward-maximum-matching (FMM) segmentation, returning only the
//   one word under the cursor so the hover range highlight matches the content, and moving
//   the mouse within the Chinese segment refresh automatically.

use crate::dict::Dictionary;

/// Whether a char is a "word" boundary char (ASCII alphanumeric/underscore + CJK hanzi)
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c.is_alphanumeric() && !c.is_ascii()
}

/// Whether a char is Chinese (CJK unified ideograph), used to separate Chinese/English boundaries
fn is_chinese_char(c: char) -> bool {
    c.is_alphanumeric() && !c.is_ascii()
}

/// Chinese forward-maximum-matching (FMM) segmentation: slice a run of Chinese into known Chinese words.
/// Returns each word (word, start offset, end offset); offsets are relative to the whole line.
/// Unknown words are split into single chars.
fn segment_chinese(s: &str, start: usize, dict: &Dictionary) -> Vec<(String, usize, usize)> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        // Try to match a known Chinese word from longest (3 chars) down to shortest (2 chars)
        let mut matched = 1;
        for len in (2..=3.min(n - i)).rev() {
            let sub: String = chars[i..i + len].iter().collect();
            if dict.is_chinese_word(&sub) {
                matched = len;
                break;
            }
        }
        let word: String = chars[i..i + matched].iter().collect();
        out.push((word, start + i, start + i + matched));
        i += matched;
    }
    out
}

/// Take the "word" under the cursor (by identifier boundaries) from a text line.
/// Returns (word, start char offset, end char offset).
/// offset / start / end are in chars (for ASCII identifiers, LSP position.character equals the char index).
/// The returned start/end are attached to the hover response as Range so Zed can detect the old
/// hover is stale and refresh when the mouse moves to another word (without a range, Zed won't update).
///
/// Key point: Chinese and English each operate independently (no cross-language boundary picking);
/// Chinese segments go through FMM, returning only the one word under the cursor so the hover
/// range highlight matches the content, and moving within a Chinese segment changes the range and
/// triggers an automatic refresh.
pub fn word_at(text: &str, offset: usize, dict: &Dictionary) -> Option<(String, usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if offset > chars.len() {
        return None;
    }
    let cursor_is_chinese = is_chinese_char(chars[offset.min(chars.len() - 1)]);

    let mut start = offset;
    let mut end = offset;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }

    let raw: String = chars[start..end].iter().collect();

    // Chinese: FMM, then return the single word under the cursor (not the whole segment)
    if cursor_is_chinese {
        let segments = segment_chinese(&raw, start, dict);
        // Only adopt the segmented result if it produced at least one multi-char word;
        // otherwise (no known Chinese words) fall back to the whole segment, preserving prior behavior.
        let has_multi = segments.iter().any(|(w, _, _)| w.chars().count() >= 2);
        if has_multi {
            for (word, s, e) in segments {
                if offset >= s && offset < e {
                    return Some((word, s, e));
                }
            }
        }
        return Some((raw, start, end));
    }

    Some((raw, start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build an empty dictionary (English tests don't need Chinese segmentation)
    fn empty_dict() -> Dictionary {
        let dir = tempfile::tempdir().unwrap();
        Dictionary::load_from_dir(dir.path())
    }

    #[test]
    fn test_word_at_simple() {
        let text = "let x = getUserProfile;";
        let dict = empty_dict();
        assert_eq!(
            word_at(text, 11, &dict),
            Some(("getUserProfile".to_string(), 8, 22))
        );
    }

    #[test]
    fn test_word_at_with_underscore() {
        let text = "fn user_name() {}";
        let dict = empty_dict();
        assert_eq!(
            word_at(text, 6, &dict),
            Some(("user_name".to_string(), 3, 12))
        );
    }

    #[test]
    fn test_word_at_with_cjk_fallback() {
        // Without Chinese words in the empty dict, the cursor returns the whole Chinese segment (fallback)
        let text = "项目";
        let dict = empty_dict();
        assert_eq!(word_at(text, 1, &dict), Some(("项目".to_string(), 0, 2)));
    }

    #[test]
    fn test_word_at_cjk_segment() {
        // The dict contains all three words in the test text below; a cursor on the
        // first char should return only that first word
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("bi.json")).unwrap();
        writeln!(
            f,
            "{{\"must\": \"v. 必须\", \"one\": \"逐一\", \"enumerate\": \"列举\"}}"
        )
        .unwrap();
        drop(f);
        let dict = Dictionary::load_from_dir(dir.path());
        let text = "必须逐一列举";
        // cursor on the first char (offset 0) -> returns the first word [0,2)
        assert_eq!(word_at(text, 0, &dict), Some(("必须".to_string(), 0, 2)));
        // cursor on the third char (offset 2) -> returns the second word [2,4)
        assert_eq!(word_at(text, 2, &dict), Some(("逐一".to_string(), 2, 4)));
    }
}
