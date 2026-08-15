//! End-to-end tests (e2e): launch the compiled translate-dict-lsp binary and talk to it over
//! stdio with the real LSP JSON-RPC protocol (initialize / initialized / didOpen / hover),
//! asserting that hover returns the expected translation Markdown.
//!
//! Difference from unit tests: nothing is mocked here; the real binary + real dictionary run,
//! verifying the whole "identifier -> split -> lookup -> Markdown" chain is still correct
//! across the process boundary.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

/// JSON-RPC connection framed with Content-Length
struct LspClient {
    child: Child,
    stdin: ChildStdin,
    reader: Mutex<Box<dyn BufRead>>,
    next_id: u64,
}

impl LspClient {
    fn start() -> Self {
        // CARGO_BIN_EXE_<name> is injected by cargo for integration tests, pointing at the built binary
        let exe = env!("CARGO_BIN_EXE_translate-dict-lsp");
        // Repo root (dict/ lives here); MANIFEST_DIR = crates/translate-dict-lsp
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();

        let mut child = Command::new(exe)
            .current_dir(repo_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn translate-dict-lsp");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader: Box<dyn BufRead> = Box::new(BufReader::new(stdout));

        LspClient {
            child,
            stdin,
            reader: Mutex::new(reader),
            next_id: 1,
        }
    }

    fn send(&mut self, msg: &serde_json::Value) {
        let body = serde_json::to_vec(msg).unwrap();
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        self.stdin.write_all(&body).unwrap();
        self.stdin.flush().unwrap();
    }

    /// Read the next JSON-RPC message (either a response or a notification)
    fn read_message(&self) -> serde_json::Value {
        let mut reader = self.reader.lock().unwrap();
        let mut header = String::new();
        let mut content_length: Option<usize> = None;
        loop {
            header.clear();
            let n = reader.read_line(&mut header).unwrap();
            if n == 0 {
                panic!("LSP connection closed unexpectedly");
            }
            let line = header.trim_end();
            if line.is_empty() {
                break; // header end
            }
            if let Some(val) = line.strip_prefix("Content-Length:") {
                content_length = Some(val.trim().parse().unwrap());
            }
        }
        let len = content_length.expect("missing Content-Length");
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    /// Send a request and wait for the response with the matching id
    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        loop {
            let msg = self.read_message();
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return msg;
            }
            // Ignore notifications (e.g. window/logMessage)
        }
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    /// Simulate file content change (Full sync: send the complete latest text)
    fn did_change(&mut self, uri: &str, version: i32, text: &str) {
        self.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }],
            }),
        );
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn initialize(client: &mut LspClient) -> serde_json::Value {
    let resp = client.request("initialize", serde_json::json!({ "capabilities": {} }));
    // Declares the hover capability
    assert_eq!(
        resp["result"]["capabilities"]["hoverProvider"],
        serde_json::json!(true)
    );
    client.notify("initialized", serde_json::json!({}));
    resp
}

/// Open a document and send a hover request, returning the hover Markdown text (None if no result)
fn hover(
    client: &mut LspClient,
    uri: &str,
    text: &str,
    line: u32,
    character: u32,
) -> Option<String> {
    client.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": "rust",
                "version": 1,
                "text": text,
            }
        }),
    );
    let resp = client.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        }),
    );
    resp["result"]
        .get("contents")
        .and_then(|c| c.get("value"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[test]
fn e2e_hover_camel_case() {
    let mut client = LspClient::start();
    initialize(&mut client);
    // Give the dictionary a moment to load after initialize
    std::thread::sleep(std::time::Duration::from_secs(3));

    let md = hover(
        &mut client,
        "file:///x.rs",
        "let p = getUserProfile;",
        0,
        14,
    )
    .expect("hover should return a result");

    // The split result should contain titles for the get / user / profile word blocks
    assert!(md.contains("[get]("), "missing 'get':\n{md}");
    assert!(md.contains("[user]("), "missing 'user':\n{md}");
    assert!(md.contains("[profile]("), "missing 'profile':\n{md}");
    // Word blocks are separated by divider lines
    assert!(md.contains("*****"), "missing separator:\n{md}");
}

#[test]
fn e2e_hover_abbreviation_chain() {
    let mut client = LspClient::start();
    initialize(&mut client);
    std::thread::sleep(std::time::Duration::from_secs(3));

    let md = hover(&mut client, "file:///y.rs", "let s = HTTPService;", 0, 13)
        .expect("hover should return a result");

    assert!(md.contains("[http]("), "missing 'http':\n{md}");
    assert!(md.contains("[service]("), "missing 'service':\n{md}");
}

#[test]
fn e2e_hover_lowercase_compound() {
    let mut client = LspClient::start();
    initialize(&mut client);
    std::thread::sleep(std::time::Duration::from_secs(3));

    let md = hover(&mut client, "file:///z.rs", "let t = redblacktree;", 0, 14)
        .expect("hover should return a result");

    assert!(md.contains("[red]("), "missing 'red':\n{md}");
    assert!(md.contains("[black]("), "missing 'black':\n{md}");
    assert!(md.contains("[tree]("), "missing 'tree':\n{md}");
}

#[test]
fn e2e_hover_chinese_reverse() {
    let mut client = LspClient::start();
    initialize(&mut client);
    std::thread::sleep(std::time::Duration::from_secs(3));

    let md = hover(&mut client, "file:///c.rs", "项目", 0, 1)
        .expect("hover should return a result for Chinese");

    // Chinese-to-English: should list English candidates (e.g. item / project)
    assert!(
        md.contains("[item](") || md.contains("[project]("),
        "unexpected chinese reverse result:\n{md}"
    );
}

/// Regression test: after editing, the cache must refresh, otherwise hover would translate stale text at stale positions.
#[test]
fn e2e_hover_after_edit_reflects_new_text() {
    let mut client = LspClient::start();
    initialize(&mut client);
    std::thread::sleep(std::time::Duration::from_secs(3));

    let uri = "file:///edit.rs";
    // Open first: line 0 is "let a = redblacktree;" (line 0)
    client.notify(
        "textDocument/didOpen",
        serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": "rust",
                "version": 1,
                "text": "let a = redblacktree;\nlet b = userProfile;",
            }
        }),
    );
    // Hover line 1 "userProfile" at (12, 22)
    let before = client.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": 17 },
        }),
    );
    let before_md = before["result"]
        .get("contents")
        .and_then(|c| c.get("value"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    assert!(
        before_md.map(|m| m.contains("[user](")).unwrap_or(false),
        "precondition: expect 'user' in old text"
    );

    // Edit line 1: replace userProfile with getUserProfile (more splits)
    client.did_change(uri, 2, "let a = redblacktree;\nlet b = getUserProfile;");

    // Now hovering the same position should reflect the new text (get / user / profile)
    let after = client.request(
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": 17 },
        }),
    );
    let after_md = after["result"]
        .get("contents")
        .and_then(|c| c.get("value"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .expect("hover after edit should return a result");
    assert!(
        after_md.contains("[get](")
            && after_md.contains("[user](")
            && after_md.contains("[profile]("),
        "after edit, cache should reflect new text; got:\n{after_md}"
    );
}
