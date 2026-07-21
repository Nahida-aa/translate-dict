// 取词层：从一行文本里按光标偏移取出"单词"（标识符 / 中文词）。
//
// - 英文标识符的拆分（camelCase / snake_case / 缩写链 / 组合词）在
//   utils::format::parse_and_query 里处理，本模块只负责"光标处是哪一段"。
// - 中文段用正向最大匹配（FMM）分词，只返回光标所在的那一个中文词，
//   使 hover 的 range 高亮与内容一致，且中文段内移动鼠标可自动刷新。

use crate::dict::Dictionary;

/// 判断字符是否属于"单词"边界（英文/数字/下划线 + 中日韩汉字）
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c.is_alphanumeric() && !c.is_ascii()
}

/// 判断字符是否为中文（CJK 统一表意文字），用于区分中英边界
fn is_chinese_char(c: char) -> bool {
    c.is_alphanumeric() && !c.is_ascii()
}

/// 中文正向最大匹配（FMM）分词：把一段连续中文切成已知中文词。
/// 返回每个词 (词, 起始偏移, 结束偏移)，偏移相对整行 text。
/// 词典里没有的词按单字切分。
fn segment_chinese(s: &str, start: usize, dict: &Dictionary) -> Vec<(String, usize, usize)> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        // 从最长(3字)向最短(2字)尝试匹配已知中文词
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

/// 从一行文本里，根据字符偏移取光标处的"单词"（按标识符边界）。
/// 返回 (单词, 起始字符偏移, 结束字符偏移)。
/// offset / start / end 均以字符计（LSP 对 ASCII 标识符 position.character 即字符序）。
/// 返回的 start/end 用于在 hover 响应里带上 Range，使 Zed 能在鼠标移到
/// 另一个词时自动判定旧 hover 失效并刷新（否则 range 为 None 时 Zed 不更新）。
///
/// 关键：中英文混排时各自为政（不跨语言边界捞词）；中文段用 FMM 分词后
/// 只返回"光标所在的那一个中文词"，使 hover range 高亮与内容一致，且
/// 鼠标在中文段内移动到另一个词时 range 变化、自动刷新。
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

    // 中文：FMM 分词后返回光标所在的那一个词（而非整段）
    if cursor_is_chinese {
        let segments = segment_chinese(&raw, start, dict);
        // 仅当分词切出了至少一个多字词时，才采用分词结果；
        // 否则（词典里没有任何中文词）退回整段，保持原行为。
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

    /// 构造一个空词典（英文测试不需要中文分词）
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
        // 空词典下没有中文词，光标在中文段返回整段（兜底）
        let text = "项目";
        let dict = empty_dict();
        assert_eq!(word_at(text, 1, &dict), Some(("项目".to_string(), 0, 2)));
    }

    #[test]
    fn test_word_at_cjk_segment() {
        // 词典含"必须""逐一""列举"，光标在"必"上应只返回"必须"
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
        // cursor 在 "必"(offset 0) → 返回第一个词"必须" [0,2)
        assert_eq!(word_at(text, 0, &dict), Some(("必须".to_string(), 0, 2)));
        // cursor 在 "逐"(offset 2) → 返回"逐一" [2,4)
        assert_eq!(word_at(text, 2, &dict), Some(("逐一".to_string(), 2, 4)));
    }
}
