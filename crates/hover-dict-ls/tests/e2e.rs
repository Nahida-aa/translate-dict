//! 端到端测试（e2e）：启动编译好的 hover-dict-ls 二进制，通过 stdio 走真实
//! LSP JSON-RPC 协议（initialize / initialized / didOpen / hover），断言 hover
//! 返回符合预期的翻译 Markdown。
//!
//! 与单元测试的区别：这里不 mock 任何逻辑，直接跑真实二进制 + 真实词库，
//! 验证「标识符 -> 拆分 -> 查词 -> Markdown」整条链路在进程边界外仍然正确。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

/// 带 Content-Length 帧的 JSON-RPC 连接
struct LspClient {
    child: Child,
    stdin: ChildStdin,
    reader: Mutex<Box<dyn BufRead>>,
    next_id: u64,
}

impl LspClient {
    fn start() -> Self {
        // CARGO_BIN_EXE_<name> 由 cargo 在集成测试时注入，指向编译好的二进制
        let exe = env!("CARGO_BIN_EXE_hover-dict-ls");
        // 仓库根（dict/ 在此），MANIFEST_DIR = crates/hover-dict-ls
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
            .expect("failed to spawn hover-dict-ls");

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

    /// 读取下一条 JSON-RPC 消息（可能是响应，也可能是通知）
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
                break; // 头部结束
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

    /// 发送请求并等待对应 id 的响应
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
            // 忽略通知（如 window/logMessage）
        }
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    /// 模拟文件内容变化（Full 同步：传完整最新文本）
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
    // 声明了 hover 能力
    assert_eq!(
        resp["result"]["capabilities"]["hoverProvider"],
        serde_json::json!(true)
    );
    client.notify("initialized", serde_json::json!({}));
    resp
}

/// 打开一个文档并发 hover 请求，返回 hover 的 Markdown 文本（若无结果返回 None）
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
    // 给 initialize 后的词库加载留一点时间
    std::thread::sleep(std::time::Duration::from_secs(3));

    let md = hover(
        &mut client,
        "file:///x.rs",
        "let p = getUserProfile;",
        0,
        14,
    )
    .expect("hover should return a result");

    // 拆分结果应包含 get / user / profile 三个词块的标题
    assert!(md.contains("[get]("), "missing 'get':\n{md}");
    assert!(md.contains("[user]("), "missing 'user':\n{md}");
    assert!(md.contains("[profile]("), "missing 'profile':\n{md}");
    // 词块之间用分隔线隔开
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

    // 中译英：应列出英文候选（如 item / project）
    assert!(
        md.contains("中译英") && (md.contains("[item](") || md.contains("[project](")),
        "unexpected chinese reverse result:\n{md}"
    );
}

/// 回归测试：文件编辑后缓存必须刷新，否则 hover 会翻译旧位置的旧词。
#[test]
fn e2e_hover_after_edit_reflects_new_text() {
    let mut client = LspClient::start();
    initialize(&mut client);
    std::thread::sleep(std::time::Duration::from_secs(3));

    let uri = "file:///edit.rs";
    // 先打开：第 0 行是 "let a = redblacktree;"（line 0）
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
    // hover 第 1 行 "userProfile" 在 (12, 22)
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

    // 编辑第 1 行：把 userProfile 换成 getUserProfile（更多分词）
    client.did_change(uri, 2, "let a = redblacktree;\nlet b = getUserProfile;");

    // 现在 hover 同一位置应反映新文本（含 get / user / profile）
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
