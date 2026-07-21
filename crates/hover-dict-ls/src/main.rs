// hover-dict-ls — 翻译语言服务器
//
// 用 tower-lsp 实现最小 LSP，监听 textDocument/hover，
// 从光标处取词 -> 智能拆分 -> 查本地词库 -> 返回 Markdown。
// 词库在 initialize 时一次性加载进内存（dict/ 目录，aa.json~zz.json）。
//
// 模块拆分：
// - config.rs   用户配置（平台 / 候选数 / 自定义 URL）
// - dict.rs     词库加载 + 中文词索引
// - word.rs     取词（标识符边界 + 中文 FMM 分词）
// - markdown.rs 词条 -> Markdown 渲染
// - query.rs / reverse_query.rs  英文拆分查词 / 中文反查
// - utils.rs    辅助
// main.rs 只做编排：全局状态、LSP 生命周期、hover 处理。

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::OnceCell;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod config;
use config::Settings;
mod dict;
use dict::Dictionary;
mod markdown;
mod query;
mod reverse_query;
mod utils;
mod word;

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
        let Some((word, start, end)) = word::word_at(&line_text, offset, dict) else {
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
                .map(|r| markdown::reverse_result_to_markdown(r, &settings))
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
                blocks.push(markdown::entry_to_markdown(entry, &settings));
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
