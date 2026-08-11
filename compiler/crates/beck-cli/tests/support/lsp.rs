//! `beck lsp`, driven the way an editor drives it: a subprocess, and JSON-RPC over its pipes.
//!
//! Shared, because two harnesses drive it. `lsp.rs` holds the server to its own protocol;
//! `playground.rs` holds the *playground* to the server's answers, since both ask
//! `beck_core::editor` and a playground that answered differently would have grown a second editor
//! (`docs/102`).
//!
//! Through a subprocess rather than by calling `lsp::serve`, because the transport is half the
//! protocol: framing, flushing and not writing anything else to stdout are properties of the
//! process, and a test that called the function directly would assert none of them.

#![allow(dead_code)] // each test binary drives a different half of it

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

/// A running server, and the pipes to talk to it.
pub struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Server {
    pub fn start() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_beck"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the compiler is built");
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Server {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    pub fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialisable");
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("the server is alive");
        self.stdin.write_all(&body).expect("the server is alive");
        self.stdin.flush().expect("the server is alive");
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        // Notifications may arrive before the response — `publishDiagnostics` routinely does — so
        // read until the id comes back rather than assuming the next message is the answer.
        loop {
            let message = self.read();
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                return message;
            }
        }
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// The next message the server sends, framed.
    pub fn read(&mut self) -> Value {
        let mut length = None;
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .expect("the server is alive");
            assert!(read > 0, "the server closed its output mid-message");
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                assert!(
                    name.trim().eq_ignore_ascii_case("content-length")
                        || name.trim().eq_ignore_ascii_case("content-type"),
                    "the server sent a header the protocol does not define: {trimmed}"
                );
                if name.trim().eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse::<usize>().ok();
                }
            }
        }
        let length = length.expect("every message is framed with a Content-Length");
        let mut body = vec![0u8; length];
        self.stdout
            .read_exact(&mut body)
            .expect("the body is as long as the header said");
        serde_json::from_slice(&body).expect("the body is JSON")
    }

    /// The next `publishDiagnostics` for this URI.
    pub fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        loop {
            let message = self.read();
            if message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics")
            {
                let params = message.get("params").expect("a notification has params");
                if params.get("uri").and_then(Value::as_str) == Some(uri) {
                    return params
                        .get("diagnostics")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                }
            }
        }
    }

    pub fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "beck", "version": 1, "text": text } }),
        );
    }

    pub fn change(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": text }],
            }),
        );
    }

    pub fn shutdown(mut self) {
        let reply = self.request("shutdown", Value::Null);
        assert!(reply.get("result").is_some(), "shutdown is answered");
        self.notify("exit", Value::Null);
        let status = self.child.wait().expect("the child is ours");
        assert!(
            status.success(),
            "`exit` after `shutdown` is a clean exit: {status}"
        );
    }
}

pub fn handshake() -> Server {
    let mut server = Server::start();
    let reply = server.request(
        "initialize",
        json!({ "processId": Value::Null, "rootUri": Value::Null, "capabilities": {} }),
    );
    let caps = reply
        .pointer("/result/capabilities")
        .expect("initialize answers with capabilities");
    assert_eq!(caps["hoverProvider"], json!(true));
    assert_eq!(caps["definitionProvider"], json!(true));
    assert_eq!(caps["documentSymbolProvider"], json!(true));
    server.notify("initialized", json!({}));
    server
}
