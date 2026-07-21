// hover-dict-ls — 翻译语言服务器
//
// 用 tower-lsp 实现最小 LSP，监听 textDocument/hover，
// 从光标处取词 -> 智能拆分 -> 查本地词库 -> 返回 Markdown。
// 词库在 initialize 时一次性加载进内存（dict/ 目录，aa.json~zz.json）。

use std::path::PathBuf;

use tokio::sync::OnceCell;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod dict;
use dict::Dictionary;
mod query;
mod reverse_query;
mod utils;

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

/// 从一行文本里，根据字符偏移取光标处的"单词"（按标识符边界）。
/// 返回 (单词, 起始字符偏移, 结束字符偏移)。
/// offset / start / end 均以字符计（LSP 对 ASCII 标识符 position.character 即字符序）。
/// 返回的 start/end 用于在 hover 响应里带上 Range，使 Zed 能在鼠标移到
/// 另一个词时自动判定旧 hover 失效并刷新（否则 range 为 None 时 Zed 不更新）。
fn word_at(text: &str, offset: usize) -> Option<(String, usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if offset > chars.len() {
        return None;
    }
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
    Some((chars[start..end].iter().collect(), start, end))
}

/// 生成一条词条的 Markdown（对齐 translate-dict 的 convert.ts::genMarkdown）
fn entry_to_markdown(entry: &dict::DictEntry) -> String {
    let url = format!(
        "https://translate.google.com?text={}",
        urlencode(&entry.word)
    );
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
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        // 加载词库（只加载一次）
        DICT.get_or_init(|| async {
            let dir = dict_dir();
            Dictionary::load_from_dir(&dir)
        })
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
        let Some((word, start, end)) = word_at(&line_text, offset) else {
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
            let results = reverse_query::reverse_query(&word, dict, 10);
            if results.is_empty() {
                return Ok(None);
            }
            let blocks: Vec<String> = results
                .iter()
                .map(|r| {
                    let url = format!("https://translate.google.com?text={}", urlencode(&r.word));
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
                blocks.push(entry_to_markdown(entry));
            }
        }
        blocks.dedup();

        if blocks.is_empty() {
            return Ok(None);
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

/// 简易文档存储（按 URI 存各文档的整行文本）
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

    #[test]
    fn test_word_at_simple() {
        let text = "let x = getUserProfile;";
        // "let x = " 占 8 个字符，getUserProfile 在 [8,22)
        assert_eq!(
            word_at(text, 11),
            Some(("getUserProfile".to_string(), 8, 22))
        );
    }

    #[test]
    fn test_word_at_with_underscore() {
        let text = "fn user_name() {}";
        // "fn " 占 3 个字符，user_name 在 [3,12)
        assert_eq!(word_at(text, 6), Some(("user_name".to_string(), 3, 12)));
    }

    #[test]
    fn test_word_at_with_cjk() {
        // 中文按字符取词（中译英场景）
        let text = "项目";
        assert_eq!(word_at(text, 1), Some(("项目".to_string(), 0, 2)));
    }

    #[test]
    fn test_word_at_empty() {
        let text = "   ";
        assert_eq!(word_at(text, 1), None);
    }
}
