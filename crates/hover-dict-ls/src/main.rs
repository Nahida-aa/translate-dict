// hover-dict-ls — 翻译语言服务器
//
// 用 tower-lsp 实现最小 LSP，监听 textDocument/hover，
// 从光标处取词 -> 智能拆分 -> 查本地词库 -> 返回 Markdown。
// 词库在 initialize 时一次性加载进内存（dict/ 目录，aa.json~zz.json）。

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::Deserialize;
use tokio::sync::OnceCell;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod dict;
use dict::Dictionary;
mod query;
mod reverse_query;
mod utils;

/// 翻译平台 URL 模板：{word} 为占位符
const PLATFORM_URLS: &[(&str, &str)] = &[
    ("google", "https://translate.google.com/?text={word}"),
    ("baidu", "https://fanyi.baidu.com/#en/zh/{word}"),
    ("deepl", "https://www.deepl.com/translator#en/zh/{word}"),
    ("bing", "https://www.bing.com/translator/?text={word}"),
    ("yandex", "https://translate.yandex.net/?text={word}"),
];

/// 用户可配置项（来自 Zed settings.json 的 lsp.hover-dict.initialization_options）
/// 注意：语言级启用/禁用由 Zed 原生的 `languages.<Lang>.language_servers`
/// 控制，本扩展不再重复实现黑白名单。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct Settings {
    /// 中译英最多返回候选数
    #[serde(rename = "hover_dict.chinese_to_english_max_results")]
    chinese_to_english_max_results: usize,
    /// 单词/结果跳转的默认平台：google/baidu/deepl/bing/yandex/custom
    #[serde(rename = "hover_dict.default_translate_platform")]
    default_translate_platform: String,
    /// default_translate_platform=custom 时的 URL 模板，{word} 占位符
    #[serde(rename = "hover_dict.custom_translate_url")]
    custom_translate_url: String,
}

impl Settings {
    fn max_results(&self) -> usize {
        if self.chinese_to_english_max_results == 0 {
            10
        } else {
            self.chinese_to_english_max_results.min(50)
        }
    }

    fn platform_url(&self, word: &str) -> String {
        let encoded = urlencode(word);
        let template: &str = if self.default_translate_platform == "custom"
            && !self.custom_translate_url.is_empty()
        {
            &self.custom_translate_url
        } else {
            PLATFORM_URLS
                .iter()
                .find(|(name, _)| *name == self.default_translate_platform)
                .map(|(_, t)| *t)
                .unwrap_or(PLATFORM_URLS[0].1)
        };
        template.replace("{word}", &encoded)
    }
}

/// 配置全局单例（initialize 时加载，did_change_configuration 时热更新）
static SETTINGS: OnceCell<ArcSwap<Settings>> = OnceCell::const_new();

/// 词库全局单例（initialize 时加载）
static DICT: OnceCell<Dictionary> = OnceCell::const_new();

/// 取词库目录：优先 LS 二进制同级的 dict/，否则当前目录 dict/
fn dict_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("dict");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("dict")
}

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
        // 从最长(4字)向最短(2字)尝试匹配已知中文词
        let mut matched = 1;
        for len in (2..=4.min(n - i)).rev() {
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
fn word_at(text: &str, offset: usize, dict: &Dictionary) -> Option<(String, usize, usize)> {
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

/// 生成一条词条的 Markdown（对齐 translate-dict 的 convert.ts::genMarkdown）
/// 单词主链接跳转到默认平台。
fn entry_to_markdown(entry: &dict::DictEntry, settings: &Settings) -> String {
    let url = settings.platform_url(&entry.word);
    let phonetic = if entry.phonetic.is_empty() {
        String::new()
    } else {
        format!(" _/{}/_", entry.phonetic)
    };
    let translation = entry.translation.replace("\\n", "  \n");
    format!("- [{}]({}) {}:\n{}", entry.word, url, phonetic, translation)
}

/// 极简 URL encode（仅编码空格，英文单词场景足够）
fn urlencode(s: &str) -> String {
    s.replace(' ', "%20")
}

struct HoverDictServer {
    client: Client,
}

#[tower_lsp::async_trait]
impl LanguageServer for HoverDictServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // 加载词库（只加载一次）
        DICT.get_or_init(|| async {
            let dir = dict_dir();
            Dictionary::load_from_dir(&dir)
        })
        .await;

        // 读取用户配置（来自 Zed settings.json 的 lsp.hover-dict.initialization_options）
        let raw_opts = params.initialization_options.clone();
        self.client
            .log_message(
                MessageType::INFO,
                &format!("[hover-dict] initialization_options = {:?}", raw_opts),
            )
            .await;
        let settings: Settings = raw_opts
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        self.client
            .log_message(
                MessageType::INFO,
                &format!(
                    "[hover-dict] parsed platform = {}, max_results = {}",
                    settings.default_translate_platform,
                    settings.max_results()
                ),
            )
            .await;
        SETTINGS
            .get_or_init(|| async { ArcSwap::from_pointee(settings) })
            .await;

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "hover-dict-ls initialized")
            .await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        if let Ok(settings) = serde_json::from_value::<Settings>(params.settings) {
            if let Some(cell) = SETTINGS.get() {
                cell.store(Arc::new(settings));
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        DOCUMENTS
            .get_or_init(|| async { DocStore::new() })
            .await
            .open(&params.text_document)
            .await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let dict = match DICT.get() {
            Some(d) => d,
            None => return Ok(None),
        };
        let settings = SETTINGS
            .get()
            .map(|c| c.load_full())
            .unwrap_or_else(|| Arc::new(Settings::default()));

        let text_document = params.text_document_position_params.text_document;
        let position = params.text_document_position_params.position;

        // 从 did_open / did_change 维护的文档里取当前行文本
        let docs = DOCUMENTS.get_or_init(|| async { DocStore::new() }).await;
        let line_text = docs
            .get_line(&text_document.uri, position.line as usize)
            .await;

        let Some(line_text) = line_text else {
            return Ok(None);
        };

        // 计算字符偏移（LSP 用 UTF-16 列，英文标识符场景下等于字符序）
        let offset = position.character as usize;
        let Some((word, start, end)) = word_at(&line_text, offset, dict) else {
            return Ok(None);
        };
        if word.is_empty() {
            return Ok(None);
        }

        // 被悬停词的字符范围：Zed 靠它判断"鼠标移到另一个词时旧 hover 失效并刷新"
        let hover_range = Range {
            start: Position {
                line: position.line,
                character: start as u32,
            },
            end: Position {
                line: position.line,
                character: end as u32,
            },
        };

        // 中文选中 → 中译英（reverse query）
        if reverse_query::contains_chinese(&word) {
            let results = reverse_query::reverse_query(&word, dict, settings.max_results());
            if results.is_empty() {
                let markdown = format!("中译英 `{}` :  \n本地词库暂无匹配的英文单词。", word);
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: Some(hover_range),
                }));
            }
            let blocks: Vec<String> = results
                .iter()
                .map(|r| {
                    let url = settings.platform_url(&r.word);
                    let phonetic = if r.phonetic.is_empty() {
                        String::new()
                    } else {
                        format!(" _/{}/_", r.phonetic)
                    };
                    let translation = r.translation.replace("\\n", "  \n");
                    format!("- [{}]({}) {}:\n{}", r.word, url, phonetic, translation)
                })
                .collect();
            let markdown = format!("中译英 `{}` :  \n{}", word, blocks.join("\n*****\n"));
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: Some(hover_range),
            }));
        }

        // 英文标识符 → 智能拆分 + 查词
        let parts = utils::format::parse_and_query(&word, dict);
        let mut blocks: Vec<String> = Vec::new();
        for part in &parts {
            if let Some(entry) = dict.lookup(part) {
                blocks.push(entry_to_markdown(entry, &settings));
            }
        }
        blocks.dedup();

        if blocks.is_empty() {
            let markdown = format!("翻译 `{}` :  \n本地词库暂无结果。", word);
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: Some(hover_range),
            }));
        }

        let markdown = blocks.join("\n*****\n");
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: Some(hover_range),
        }))
    }
}

/// 简易文档存储（按 URI 存各文档的整行文本 + 语言名）
mod documents {
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower_lsp::lsp_types::TextDocumentItem;
    use tower_lsp::lsp_types::Url;

    #[derive(Clone, Default)]
    pub struct DocStore {
        inner: Arc<RwLock<HashMap<Url, Vec<String>>>>,
    }

    impl DocStore {
        pub fn new() -> Self {
            Self::default()
        }

        pub async fn open(&self, doc: &TextDocumentItem) {
            let lines: Vec<String> = doc.text.split('\n').map(|s| s.to_string()).collect();
            self.inner.write().await.insert(doc.uri.clone(), lines);
        }

        pub async fn get_line(&self, uri: &Url, line: usize) -> Option<String> {
            self.inner
                .read()
                .await
                .get(uri)
                .and_then(|l| l.get(line).cloned())
        }
    }
}

use documents::DocStore;
static DOCUMENTS: OnceCell<DocStore> = OnceCell::const_new();

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| HoverDictServer { client });
    Server::new(stdin, stdout, socket).serve(service).await;
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
        // "let x = " 占 8 个字符，getUserProfile 在 [8,22)
        assert_eq!(
            word_at(text, 11, &dict),
            Some(("getUserProfile".to_string(), 8, 22))
        );
    }

    #[test]
    fn test_word_at_with_underscore() {
        let text = "fn user_name() {}";
        let dict = empty_dict();
        // "fn " 占 3 个字符，user_name 在 [3,12)
        assert_eq!(
            word_at(text, 6, &dict),
            Some(("user_name".to_string(), 3, 12))
        );
    }

    #[test]
    fn test_word_at_with_cjk() {
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

    #[test]
    fn test_word_at_empty() {
        let text = "   ";
        let dict = empty_dict();
        assert_eq!(word_at(text, 1, &dict), None);
    }
}
