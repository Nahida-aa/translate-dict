// 本地词库加载与查询。
// 词库文件放在扩展仓库的 dict/ 目录，按单词前两字母分片
// （aa.json ~ zz.json），每个文件是 { "word": {"w","p","t"} | "translation" }。
// 启动时全部读入内存（约 760k 词，一次性加载、常驻）。
//
// 内存优化：DictEntry 不再存 word 字段——map 的键（小写词）即单词本身，
// 重复存一份 String 是纯浪费（约 76 万条 × 一条 String 堆分配）。展示时用
// 查询键即可（translate-dict 同样显示小写）。HashMap/HashSet 使用 ahash，
// 比默认 SipHash 更快、桶更紧凑。

use ahash::{AHashMap, AHashSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

// 编译期嵌入的内置词库（build.rs 生成字面量路径到 OUT_DIR/embedded_dict.rs）。
// 仅发布二进制需要它；开发期有文件系统 dict/，不会用到这份嵌入副本。
include!(concat!(env!("OUT_DIR"), "/embedded_dict.rs"));

#[derive(Clone)]
pub struct DictEntry {
    pub phonetic: String,
    pub translation: String,
}

pub struct Dictionary {
    /// 键为小写词，值即词条（不含 word 字段）
    map: AHashMap<String, DictEntry>,
    /// 中文词索引：从词条翻译文本里提取的 2~3 字全中文片段。
    /// 用于中文正向最大匹配（FMM）分词，O(1) 判定子串是否为有效中文词。
    chinese_words: AHashSet<String>,
}

impl Dictionary {
    pub fn load_from_dir(dir: &Path) -> Self {
        let mut map: AHashMap<String, DictEntry> = AHashMap::new();
        let mut chinese_words: AHashSet<String> = AHashSet::new();

        // 从翻译文本提取中文词片段时使用的分隔符（与 reverse_query 一致）
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

                            // 从翻译文本提取 2~3 字全中文片段，建中文词索引。
                            // 仅取 2~3 字：4 字中文多为短语/句子碎片，作 FMM
                            // 词典词价值低且占近半索引内存；FMM 退化为 2+2 切分。
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

    /// 加载词库：优先读文件系统的 dict/（开发期，便于改词库不重编二进制），
    /// 找不到时回退到编译期嵌入的 dict/（发布二进制自带，无需外部文件）。
    /// 这样发布的 LS 二进制是自包含的——cargo-dist 只打包二进制，
    /// 不会带 dict/ 目录，必须靠嵌入才能给最终用户正常翻译。
    pub fn load() -> Self {
        let fs_dir = crate::dict_dir();
        let fs_dict = Self::load_from_dir(&fs_dir);
        if !fs_dict.map.is_empty() {
            return fs_dict;
        }
        Self::load_embedded()
    }

    /// 从编译期嵌入的 dict/ 加载（include_dir! 在编译时把整个目录打进二进制）。
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

    /// 按原始变体字符串查词（变体已含大小写，内部统一转小写键）
    pub(crate) fn lookup_variant(&self, variant: &str) -> Option<&DictEntry> {
        self.map.get(&variant.to_lowercase())
    }

    /// 查询单词，返回匹配的词条（委托给 query.rs::query_dict）
    pub fn lookup(&self, word: &str) -> Option<&DictEntry> {
        crate::query::query_dict(word, self)
    }

    pub fn contains(&self, word: &str) -> bool {
        crate::query::is_word_in_dict(word, self)
    }

    /// 返回全部 (小写词, 词条)（用于中译英反向查询的全表扫描）
    pub fn entries(&self) -> impl Iterator<Item = (&str, &DictEntry)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// 中文正向最大匹配：判断 `text` 是否是一个已知中文词（在中文词索引里）
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

    /// 在临时目录写一个迷你词库，返回目录路径
    fn make_temp_dict() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("us.json")).unwrap();
        // 字符串形式词条
        writeln!(f, "{{\"user\": \"n. 使用者\", \"use\": \"vt. 使用\"}}").unwrap();
        drop(f);

        let mut f2 = std::fs::File::create(dir.path().join("pr.json")).unwrap();
        // 对象形式词条（含音标/翻译）
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
        // 迷你词库含 user / use / profile 三条
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
        // 小写查询应命中
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

    /// 验证编译期嵌入词库可用（发布二进制无外部 dict/ 时的回退路径）。
    /// 嵌入的是仓库根完整词库，应含常见词如 "user"。
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
        // 中文词索引应已建立（用户 是 2 字中文词）
        assert!(dict.is_chinese_word("用户"), "embedded dict missing 用户");
    }
}
