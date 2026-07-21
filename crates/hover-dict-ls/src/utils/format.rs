// 智能拆分标识符（移植自 translate-dict 的 src/utils/format.ts）。
//
// 目标：把 getUserProfile / HTTPService / redblacktree / send_email 这类
// 标识符拆成词典里能查到的英文单词，再分别查词返回翻译。
//
// 算法分两层：
//   1) split_by_case  —— 按分隔符(- _ 空格)与大小写边界切，处理缩写链；
//   2) split_compound_word —— 对每段再做"组合词最佳拆分"，优先保留词典中
//      存在的完整词，用打分函数挑选最优切分。
//
// 全部查询走内存里的 Dictionary（启动时一次性加载），零 IO、零网络。

use crate::dict::Dictionary;

/// 是否允许出现的单字符片段（a / i 当作合法单词）
fn is_allowed_single_character(part: &str) -> bool {
    part.eq_ignore_ascii_case("a") || part.eq_ignore_ascii_case("i")
}

/// 技术缩写：全大写且长度 >= 2
fn is_technical_abbreviation(part: &str) -> bool {
    !part.is_empty() && part.chars().all(|c| c.is_ascii_uppercase()) && part.len() >= 2
}

/// 拆分全大写缩写链，如 HTTPService -> HTTP + Service
/// 移植自 splitUppercaseAbbreviationChain
fn split_uppercase_abbreviation_chain(word: &str, dict: &Dictionary) -> Vec<String> {
    if dict.contains(word) {
        return vec![word.to_string()];
    }

    let n = word.chars().count();
    // 从后往前尝试在 i 处切一刀，前半段必须在词典里
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

/// 小写组合词最佳拆分（DP + memo，移植自 splitLowercaseCompoundWord）
fn split_lowercase_compound_word(word: &str, dict: &Dictionary) -> Vec<String> {
    let lower = word.to_lowercase();
    let n = lower.chars().count();
    let mut memo: Vec<Option<(Vec<String>, i64)>> = vec![None; n + 1];
    search_lc(0, &lower, word, dict, &mut memo)
        .map(|(parts, _)| parts)
        .unwrap_or_else(|| vec![word.to_string()])
}

/// 从 start（字节偏移，单词全为 ASCII）开始的最佳拆分（parts + score），None 表示无解
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
        // 若该分支无法覆盖到末尾，尝试下一个切分点（不能用 ? 提前返回，
        // 否则会漏掉其它可行分支并错误 memo 化 None）
        let Some(rest) = search_lc(end, lower, word, dict, memo) else {
            continue;
        };

        // 候选分数：词越长越优，段数越少越优
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

/// 归一化前导 I 缩写：IUserService -> UserService（丢弃 I 前缀）
fn normalize_leading_interface_prefix(parts: &[String]) -> Vec<String> {
    if parts.len() < 2 || parts[0] != "I" || !is_technical_abbreviation(&parts[1]) {
        return parts.to_vec();
    }
    let mut out = vec![parts[1].clone()];
    out.extend_from_slice(&parts[2..]);
    out
}

/// 给一组拆分页打分（移植自 scoreSplitParts）
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

/// 组合词最佳拆分（移植自 findBestCompoundSplit）
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

/// 单段组合词拆分入口（移植自 splitCompoundWord）
fn split_compound_word(word: &str, dict: &Dictionary) -> Vec<String> {
    if dict.contains(&word.to_lowercase()) {
        return vec![word.to_string()];
    }
    find_best_compound_split(word, dict)
}

// 手写匹配 translate-dict 的正则：
//   [A-Z]+(?=[A-Z][a-z]|$) | [A-Z][a-z]* | [a-z]+
// 等价于经典 camelCase 切分：连续大写后若是"大写+小写"则把最后一个大写留给小写段。
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
                        // [A-Z][a-z]*  -> 单个大写后接小写，整体作为一个 token（User）
                        while j < n && self.chars[j].is_ascii_lowercase() {
                            j += 1;
                        }
                        let seg = &self.s[self.pos..j];
                        self.pos = j;
                        return Some(Match { s: seg });
                    }
                    // 全大写段（可能后续被缩写链再拆）
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

/// ^I[A-Z]{2,}$  ->  I 后跟至少两个大写
fn regex_is_i_aa(m: &str) -> bool {
    m.len() >= 3 && m.starts_with('I') && m[1..].chars().all(|c| c.is_ascii_uppercase())
}

/// 顶层按大小写 / 分隔符拆分（移植自 splitByCase）
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

/// 拆分并查询（移植自 parseAndQuery）
/// 返回去重后的最终单词列表（已过滤长度 <=1）。
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

    /// 在临时目录写一个迷你词库，覆盖常用拆词验证所需的词条
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
        // 按前两字母分片写入
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
        // 拆分保留原始大小写（与 translate-dict 的 parseAndQuery 行为一致）；
        // 最终 hover 展示时再用词库里的规范词形（小写）渲染。
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
        // HTTPService -> HTTP + Service（拆分词保留原始大小写）
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
        // User/user 去重（忽略大小写），保留原始大小写片段
        let parts = parse_and_query("Useruser", &dict);
        assert_eq!(parts, vec!["User"]);
    }

    #[test]
    fn test_short_word_ignored() {
        let dict = Dictionary::load_from_dir(temp_dict().path());
        // 长度 <=1 的片段被过滤
        assert!(parse_and_query("a", &dict).is_empty());
    }
}
