// 本地词库加载与查询。
// 词库文件放在扩展仓库的 dict/ 目录，按单词前两字母分片
// （aa.json ~ zz.json），每个文件是 { "word": {"w","p","t"} | "translation" }。
// 启动时全部读入内存（约 760k 词，几十 MB，一次性加载、常驻）。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

#[derive(Clone)]
pub struct DictEntry {
    pub word: String,
    pub phonetic: String,
    pub translation: String,
}

pub struct Dictionary {
    map: HashMap<String, DictEntry>,
}

impl Dictionary {
    pub fn load_from_dir(dir: &Path) -> Self {
        let mut map: HashMap<String, DictEntry> = HashMap::new();

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&content) {
                        for (key, val) in obj {
                            let entry = match val {
                                Value::String(t) => DictEntry {
                                    word: key.clone(),
                                    phonetic: String::new(),
                                    translation: t,
                                },
                                Value::Object(o) => DictEntry {
                                    word: o
                                        .get("w")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or(&key)
                                        .to_string(),
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
                        }
                    }
                }
            }
        }

        Self { map }
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

    /// 返回全部词条（用于中译英反向查询的全表扫描）
    pub fn all_entries(&self) -> impl Iterator<Item = &DictEntry> {
        self.map.values()
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
}
