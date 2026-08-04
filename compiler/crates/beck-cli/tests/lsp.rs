//! `beck lsp`, driven the way an editor drives it: a subprocess, and JSON-RPC over its pipes.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.6 says
//! there is no separate language server implementation to drift, and the way that claim goes wrong
//! is not that somebody writes a second checker — it is that the server answers from a stale copy,
//! or renders a signature its own way, or reports a diagnostic the compiler does not. So the
//! interesting assertions here are the ones that compare the server's answer to
//! **`beck check`'s and `beck iface`'s**, rather than to a string written in this file.
//!
//! Through a subprocess rather than by calling `lsp::serve`, because the transport is half the
//! protocol: framing, flushing and not writing anything else to stdout are properties of the
//! process, and a test that called the function directly would assert none of them.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

/// A running server, and the pipes to talk to it.
struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Server {
    fn start() -> Server {
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

    fn send(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).expect("serialisable");
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("the server is alive");
        self.stdin.write_all(&body).expect("the server is alive");
        self.stdin.flush().expect("the server is alive");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
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

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// The next message the server sends, framed.
    fn read(&mut self) -> Value {
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
    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
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

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "beck", "version": 1, "text": text } }),
        );
    }

    fn change(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": text }],
            }),
        );
    }

    fn shutdown(mut self) {
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

fn handshake() -> Server {
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

const GOOD: &str = "\
def double(x: Int) -> Int:
    return x * 2

def label(n: Int) -> Str:
    return str(double(n))
";

const BAD: &str = "\
def double(x: Int) -> Str:
    return x * 2
";

// ---------------------------------------------------------------------------------------------
// 1. The protocol
// ---------------------------------------------------------------------------------------------

#[test]
fn it_initializes_publishes_and_exits_cleanly() {
    let mut server = handshake();
    let uri = "file:///tmp/good.beck";
    server.open(uri, GOOD);
    assert!(
        server.diagnostics(uri).is_empty(),
        "a program that compiles has no squiggles"
    );
    server.shutdown();
}

#[test]
fn a_request_it_does_not_understand_is_answered_rather_than_dropped() {
    // The failure this rules out is a client hanging forever. `-32601` is JSON-RPC's own
    // "method not found", and answering with it is required whether or not we implement the method.
    let mut server = handshake();
    let reply = server.request("textDocument/codeLens", json!({}));
    assert_eq!(reply.pointer("/error/code"), Some(&json!(-32601)));
    server.shutdown();
}

// ---------------------------------------------------------------------------------------------
// 2. The claim that matters: the server's answers are the compiler's
// ---------------------------------------------------------------------------------------------

#[test]
fn a_squiggle_is_the_same_diagnostic_beck_check_reports() {
    // §4.6's claim, asserted rather than assumed. The code and the message both have to match what
    // the library produces, because "one binary, no drift" is exactly the property that decays
    // silently — a server that formatted its own message would pass a test that only checked a code.
    let mut server = handshake();
    let uri = "file:///tmp/bad.beck";
    server.open(uri, BAD);
    let published = server.diagnostics(uri);
    assert_eq!(published.len(), 1, "{published:#?}");

    let (_, diags, _) = beck_core::compile_or_library_str("/tmp/bad.beck", BAD);
    let expected = diags.iter().next().expect("the library refuses it too");
    assert_eq!(published[0]["code"], json!(expected.code));
    assert_eq!(published[0]["severity"], json!(1));
    assert_eq!(published[0]["source"], json!("beck"));
    assert!(
        published[0]["message"]
            .as_str()
            .expect("a message")
            .starts_with(&expected.message),
        "the editor's message has to be the compiler's:\n{}\nvs\n{}",
        published[0]["message"],
        expected.message
    );

    // And the span points at the same place, converted rather than invented.
    let text_before = &BAD[..expected.primary.start as usize];
    assert_eq!(
        published[0]["range"]["start"]["line"],
        json!(text_before.matches('\n').count() as u32)
    );

    server.shutdown();
}

#[test]
fn hovering_shows_what_beck_iface_publishes() {
    // The other half of §4.6. `render_item` is the renderer `beck iface` writes `.becki` with, and
    // hover calls it — so a change to how a signature is written shows up in both or neither.
    let mut server = handshake();
    let uri = "file:///tmp/good.beck";
    server.open(uri, GOOD);
    let _ = server.diagnostics(uri);

    let line = GOOD
        .lines()
        .position(|l| l.contains("def label"))
        .expect("it is there");
    let character = GOOD
        .lines()
        .nth(line)
        .expect("a line")
        .find("label")
        .expect("it is there");
    let reply = server.request(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        }),
    );
    let shown = reply
        .pointer("/result/contents/value")
        .and_then(Value::as_str)
        .expect("hovering a definition says something");

    let (placed, _, _) = beck_core::compile_or_library_str("/tmp/good.beck", GOOD);
    let placed = placed.expect("it compiles");
    let interface = beck_core::iface::Interface::of(&placed.program);
    let item = interface.item("label").expect("`label` is published");
    let published = beck_core::iface::render_item(item);
    assert!(
        shown.contains(published.trim_end()),
        "hover has to show the published signature:\n{shown}\nvs\n{published}"
    );
    assert!(
        shown.contains(&format!("@on({})", item.tier.name())),
        "and the tier the solver chose, which is half of what a reader wants:\n{shown}"
    );

    server.shutdown();
}

#[test]
fn go_to_definition_lands_on_the_definition() {
    let mut server = handshake();
    let uri = "file:///tmp/good.beck";
    server.open(uri, GOOD);
    let _ = server.diagnostics(uri);

    // The cursor on the *use* of `double`, inside `label`'s body.
    let line = GOOD
        .lines()
        .position(|l| l.contains("return str(double(n))"))
        .expect("it is there");
    let character = GOOD
        .lines()
        .nth(line)
        .expect("a line")
        .find("double")
        .expect("it is there")
        + 2;
    let reply = server.request(
        "textDocument/definition",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        }),
    );
    assert_eq!(reply.pointer("/result/uri"), Some(&json!(uri)));
    assert_eq!(
        reply.pointer("/result/range/start/line"),
        Some(&json!(0)),
        "`double` is declared on the first line, and that is where this has to land: {reply}"
    );

    server.shutdown();
}

#[test]
fn document_symbols_are_every_published_name_and_nothing_else() {
    let mut server = handshake();
    let uri = "file:///tmp/good.beck";
    server.open(uri, GOOD);
    let _ = server.diagnostics(uri);

    let reply = server.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    let symbols = reply
        .pointer("/result")
        .and_then(Value::as_array)
        .expect("an array");
    let names: Vec<&str> = symbols.iter().filter_map(|s| s["name"].as_str()).collect();
    assert_eq!(names, vec!["double", "label"]);

    // Against the interface again rather than against a list written here.
    let (placed, _, _) = beck_core::compile_or_library_str("/tmp/good.beck", GOOD);
    let interface = beck_core::iface::Interface::of(&placed.expect("it compiles").program);
    let mut published: Vec<String> = interface.items.iter().map(|i| i.name.to_string()).collect();
    published.sort();
    assert_eq!(names, published);

    server.shutdown();
}

// ---------------------------------------------------------------------------------------------
// 3. The state a server gets wrong
// ---------------------------------------------------------------------------------------------

#[test]
fn an_edit_replaces_the_previous_answer_rather_than_adding_to_it() {
    // The classic language-server bug: diagnostics accumulate, or a fixed error stays on screen.
    // Both directions, in one session, on one document.
    let mut server = handshake();
    let uri = "file:///tmp/editing.beck";

    server.open(uri, GOOD);
    assert!(server.diagnostics(uri).is_empty());

    server.change(uri, BAD);
    assert_eq!(
        server.diagnostics(uri).len(),
        1,
        "breaking the file has to produce a squiggle"
    );

    server.change(uri, GOOD);
    assert!(
        server.diagnostics(uri).is_empty(),
        "and fixing it has to take the squiggle away — an editor is told the whole list every time"
    );

    // Closing clears what the client is holding.
    server.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri } }),
    );
    assert!(server.diagnostics(uri).is_empty());

    server.shutdown();
}

#[test]
fn it_answers_about_the_text_it_was_last_sent_and_not_about_a_file_on_disk() {
    // A server that read the path in the URI would answer from the saved file, so every answer
    // would be one save behind. The URI here names a file that does not exist.
    let mut server = handshake();
    let uri = "file:///tmp/never-written-to-disk.beck";
    server.open(uri, GOOD);
    assert!(server.diagnostics(uri).is_empty());

    let reply = server.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    assert_eq!(
        reply
            .pointer("/result")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2),
        "the answer came from the buffer, or it came from nowhere: {reply}"
    );
    assert!(
        !std::path::Path::new("/tmp/never-written-to-disk.beck").exists(),
        "this test means nothing if the file exists"
    );

    server.shutdown();
}

#[test]
fn a_library_is_analysed_rather_than_refused() {
    // docs/27 §27.4's point, arriving here: a module being edited usually has no merge point, and
    // a server that only accepted applications would be useless for most files. `GOOD` is a library
    // — every test above depends on this and none of them says so, so one does.
    let (placed, _, _) = beck_core::compile_or_library_str("/tmp/good.beck", GOOD);
    assert!(
        !placed.expect("it compiles").is_application(),
        "the fixture these tests are built on has to be a library, or they prove the easy case"
    );
}
