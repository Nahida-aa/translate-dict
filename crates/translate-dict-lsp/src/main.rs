// translate-dict-lsp — translation language server
//
// Uses tower-lsp to implement a minimal LSP that serves textDocument/hover:
// takes the word at the cursor -> smart splitting -> local dictionary lookup -> Markdown.
// The dictionary loads fully into memory once at initialize (dict/ dir, aa.json~zz.json).
//
// Module layout:
// - config.rs        user config (platform / candidate count / custom URL)
// - dict.rs          dictionary loading + Chinese word index
// - word.rs          word extraction (identifier boundaries + Chinese FMM segmentation)
// - markdown.rs      entry -> Markdown rendering
// - query.rs / reverse_query.rs  English split lookup / Chinese reverse query
// - utils.rs         helpers
// main.rs only orchestrates: global state, LSP lifecycle, hover handling.

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

/// Global config singleton (loaded at initialize, hot-reloaded on did_change_configuration)
static SETTINGS: OnceCell<ArcSwap<Settings>> = OnceCell::const_new();

/// Global dictionary singleton (loaded at initialize)
static DICT: OnceCell<Dictionary> = OnceCell::const_new();

/// Dictionary dir: look for dict/ next to the LS binary
fn dict_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("dict");
        }
    }
    PathBuf::from("dict")
}

struct TranslateDictServer {
    client: Client,
}

#[tower_lsp::async_trait]
impl LanguageServer for TranslateDictServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Load the dictionary (only once): prefer the filesystem dict/, else the compile-time embedded one
        DICT.get_or_init(|| async { Dictionary::load() }).await;
        let dict = DICT.get().unwrap();
        self.client
            .log_message(
                MessageType::INFO,
                &format!(
                    "[translate-dict-lsp] dict loaded: {} shards from {}",
                    dict.shard_count(),
                    crate::dict_dir().display(),
                ),
            )
            .await;

        // Read user config (from Zed settings.json's lsp.translate-dict-lsp.initialization_options)
        let raw_opts = params.initialization_options.clone();
        self.client
            .log_message(
                MessageType::INFO,
                &format!("[translate-dict-lsp] initialization_options = {:?}", raw_opts),
            )
            .await;
        let settings: Settings = raw_opts
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        self.client
            .log_message(
                MessageType::INFO,
                &format!(
                    "[translate-dict-lsp] parsed platform = {}, max_results = {}",
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
                // Full sync: Zed sends the whole document text after every edit,
                // so the DOCUMENTS cache always holds the latest content and hover can translate new code.
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "translate-dict-lsp initialized")
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

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let docs = DOCUMENTS.get_or_init(|| async { DocStore::new() }).await;
        // Under Full sync, content_changes[0].text is the full latest document text
        if let Some(change) = params.content_changes.into_iter().next() {
            docs.update(&params.text_document.uri, &change.text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        // Drop the cache when a file closes to avoid holding stale text for inactive documents
        if let Some(docs) = DOCUMENTS.get() {
            docs.remove(&params.text_document.uri).await;
        }
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

        // Get the current line's text from the document maintained by did_open / did_change
        let docs = DOCUMENTS.get_or_init(|| async { DocStore::new() }).await;
        let line_text = docs
            .get_line(&text_document.uri, position.line as usize)
            .await;

        let Some(line_text) = line_text else {
            return Ok(None);
        };

        // Character offset (LSP uses UTF-16 columns, which equal character indexes for ASCII identifiers)
        let offset = position.character as usize;
        let Some((word, start, end)) = word::word_at(&line_text, offset, dict) else {
            return Ok(None);
        };
        if word.is_empty() {
            return Ok(None);
        }

        // Hovered word's character range: Zed uses it to invalidate the old hover
        // and refresh when the mouse moves to another word
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

        // Chinese selection -> Chinese-to-English (reverse query)
        if reverse_query::contains_chinese(&word) {
            let results = dict.reverse_query(&word, settings.max_results());
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
            let markdown = format!("中译英 `{}` :  \n{}", word, blocks.join("\n\n*****\n\n"));
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: Some(hover_range),
            }));
        }

        // English identifier -> smart split + dictionary lookup
        let parts = utils::format::parse_and_query(&word, dict);
        let mut blocks: Vec<String> = Vec::new();
        for part in &parts {
            if let Some(entry) = dict.lookup(part) {
                // Display word: lowercased (map keys are lowercase words, no extra word field stored)
                blocks.push(markdown::entry_to_markdown(
                    &part.to_lowercase(),
                    &entry,
                    &settings,
                ));
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

        let markdown = blocks.join("\n\n*****\n\n");
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: Some(hover_range),
        }))
    }
}

/// Minimal document store (per-URI whole-line texts + language name) for the hover request path
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

        /// First open of a file: cache the whole-line texts.
        pub async fn open(&self, doc: &TextDocumentItem) {
            let lines: Vec<String> = doc.text.split('\n').map(|s| s.to_string()).collect();
            self.inner.write().await.insert(doc.uri.clone(), lines);
        }

        /// Content changed: refresh the cache with the full latest text.
        /// Required, otherwise hover would translate stale text at stale positions.
        pub async fn update(&self, uri: &Url, text: &str) {
            let lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
            self.inner.write().await.insert(uri.clone(), lines);
        }

        pub async fn get_line(&self, uri: &Url, line: usize) -> Option<String> {
            self.inner
                .read()
                .await
                .get(uri)
                .and_then(|l| l.get(line).cloned())
        }

        /// File closed: remove the cache so it doesn't keep holding stale text in memory
        pub async fn remove(&self, uri: &Url) {
            self.inner.write().await.remove(uri);
        }
    }
}

use documents::DocStore;
static DOCUMENTS: OnceCell<DocStore> = OnceCell::const_new();

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| TranslateDictServer { client });
    Server::new(stdin, stdout, socket).serve(service).await;
}
