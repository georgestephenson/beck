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

use serde_json::{json, Value};

mod support;
use support::lsp::handshake;

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

/// The two capabilities `docs/101` added, over the protocol rather than through the module.
///
/// The answers themselves are `beck_core::editor`'s and are gated against the playground's in
/// `playground.rs`; what is asserted here is that a real editor can *get* them — the capability is
/// advertised, the legend is published, and the encodings are the protocol's.
#[test]
fn it_offers_completions_and_semantic_tokens() {
    let mut server = handshake();
    let uri = "file:///tmp/complete.beck";
    server.open(uri, GOOD);

    // The legend has to be published, because a client decodes every token against it: an index
    // into a list nobody sent is a colour chosen at random.
    let reply = server.request("initialize", json!({ "capabilities": {} }));
    let legend = reply
        .pointer("/result/capabilities/semanticTokensProvider/legend/tokenTypes")
        .and_then(Value::as_array)
        .expect("a legend");
    assert!(legend.iter().any(|t| t == "keyword"));
    assert_eq!(
        reply.pointer("/result/capabilities/completionProvider/resolveProvider"),
        Some(&json!(false))
    );

    // `def double` — the caret at the end of `doub`, which is where somebody asking for a
    // completion has their caret.
    let line = GOOD
        .lines()
        .position(|l| l.contains("return str(double"))
        .expect("it is there");
    let column = GOOD
        .lines()
        .nth(line)
        .expect("the line")
        .find("double")
        .expect("it is there")
        + 4;
    let items = server.request(
        "textDocument/completion",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": column },
        }),
    );
    let items = items
        .pointer("/result/items")
        .and_then(Value::as_array)
        .expect("a completion list");
    let offered = items
        .iter()
        .find(|c| c["label"] == "double")
        .expect("the name being typed is offered");
    assert_eq!(offered["detail"], json!("def double(x: Int) -> Int"));
    // 3 is `Function` in the protocol's `CompletionItemKind`.
    assert_eq!(offered["kind"], json!(3));

    let data = server.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": uri } }),
    );
    let data = data
        .pointer("/result/data")
        .and_then(Value::as_array)
        .expect("the token data");
    assert!(!data.is_empty());
    assert_eq!(data.len() % 5, 0, "five integers per token");
    // The first token of the file is `def`, at line 0, column 0, three units long, and a keyword.
    assert_eq!(data[..3], [json!(0), json!(0), json!(3)]);
    assert_eq!(
        legend[data[3].as_u64().expect("an index") as usize],
        json!("keyword")
    );

    server.shutdown();
}
