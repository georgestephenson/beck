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

/// A file with something to *say* about it: a definition the solver has to place, and a row it has
/// to infer. `GOOD` is pure arithmetic, so it is unplaced and performs nothing — which is the right
/// fixture for the questions above and answers none of the two below.
const PLACED: &str = "\
def key() -> secret[Str]:
    return secret_env(\"API_KEY\")

def stamped(n: Int) -> Int:
    return n + now()
";

/// Where a line and column land in a text, so a test can apply the edits a client would.
fn offset_of(text: &str, position: &Value) -> u32 {
    beck_core::editor::byte_offset(
        text,
        position["line"].as_u64().expect("a line") as u32,
        position["character"].as_u64().expect("a character") as u32,
    )
    .expect("a position inside the document")
}

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
    // docs/27 §27.2's point, arriving here: a module being edited usually has no merge point, and
    // a server that only accepted applications would be useless for most files. `GOOD` is a library
    // — every test above depends on this and none of them says so, so one does.
    let (placed, _, _) = beck_core::compile_or_library_str("/tmp/good.beck", GOOD);
    assert!(
        !placed.expect("it compiles").is_application(),
        "the fixture these tests are built on has to be a library, or they prove the easy case"
    );
}

/// The two capabilities `docs/98` added, over the protocol rather than through the module.
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

// ---------------------------------------------------------------------------------------------
// 4. The answers that change the file
// ---------------------------------------------------------------------------------------------

/// A caret on the *use* of `double` inside `label`'s body, as a client would send it.
fn a_use_of_double() -> Value {
    let line = GOOD
        .lines()
        .position(|l| l.contains("return str(double(n))"))
        .expect("it is there");
    let character = GOOD
        .lines()
        .nth(line)
        .expect("a line")
        .find("double")
        .expect("it is there");
    json!({ "line": line, "character": character })
}

#[test]
fn references_are_every_use_and_the_declaration_among_them() {
    let mut server = handshake();
    let uri = "file:///tmp/good.beck";
    server.open(uri, GOOD);
    let _ = server.diagnostics(uri);

    let reply = server.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri },
            "position": a_use_of_double(),
            "context": { "includeDeclaration": true },
        }),
    );
    let all = reply
        .pointer("/result")
        .and_then(Value::as_array)
        .expect("an array")
        .clone();
    assert_eq!(all.len(), 2, "declared once, called once: {all:#?}");
    assert!(all.iter().all(|l| l["uri"] == json!(uri)));
    for location in &all {
        let start = offset_of(GOOD, &location["range"]["start"]);
        let end = offset_of(GOOD, &location["range"]["end"]);
        assert_eq!(
            &GOOD[start as usize..end as usize],
            "double",
            "a reference range has to cover the name and nothing else"
        );
    }

    // And the declaration is droppable, which is what "who calls this" means.
    let reply = server.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri },
            "position": a_use_of_double(),
            "context": { "includeDeclaration": false },
        }),
    );
    let calls = reply
        .pointer("/result")
        .and_then(Value::as_array)
        .expect("an array");
    assert_eq!(calls.len(), 1, "{calls:#?}");
    assert_eq!(
        calls[0]["range"]["start"]["line"],
        json!(4),
        "the one left is the call, not the definition"
    );

    // The same set, as the editor's own highlight: `3` is Write, and the only write a name gets in
    // a language with no assignment is the one that declares it.
    let reply = server.request(
        "textDocument/documentHighlight",
        json!({ "textDocument": { "uri": uri }, "position": a_use_of_double() }),
    );
    let marks = reply
        .pointer("/result")
        .and_then(Value::as_array)
        .expect("an array");
    assert_eq!(marks.len(), 2);
    assert_eq!(marks.iter().filter(|m| m["kind"] == json!(3)).count(), 1);

    server.shutdown();
}

#[test]
fn a_rename_produces_edits_the_server_itself_then_accepts() {
    // The property that matters is not the shape of the `WorkspaceEdit` — it is that a client
    // which applies it has a file that still compiles. So this applies them and hands the result
    // back through the same server, which is exactly what an editor does next.
    let mut server = handshake();
    let uri = "file:///tmp/renaming.beck";
    server.open(uri, GOOD);
    let _ = server.diagnostics(uri);

    let reply = server.request(
        "textDocument/prepareRename",
        json!({ "textDocument": { "uri": uri }, "position": a_use_of_double() }),
    );
    assert_eq!(
        reply.pointer("/result/placeholder"),
        Some(&json!("double")),
        "the box a client opens is filled with the name being changed: {reply}"
    );

    let reply = server.request(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri },
            "position": a_use_of_double(),
            "newName": "twice",
        }),
    );
    // Indexed rather than `pointer`ed: a URI is full of `/`, which a JSON pointer reads as its
    // own separator, so the key has to be looked up as a key.
    let edits = reply["result"]["changes"][uri]
        .as_array()
        .unwrap_or_else(|| panic!("edits for this document: {reply}"))
        .clone();
    assert_eq!(edits.len(), 2, "{edits:#?}");

    // Applied back to front, which is what a client does and what keeps the offsets valid.
    let mut applied: Vec<(u32, u32, String)> = edits
        .iter()
        .map(|e| {
            (
                offset_of(GOOD, &e["range"]["start"]),
                offset_of(GOOD, &e["range"]["end"]),
                e["newText"].as_str().expect("new text").to_string(),
            )
        })
        .collect();
    applied.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut renamed = GOOD.to_string();
    for (start, end, text) in applied {
        renamed.replace_range(start as usize..end as usize, &text);
    }
    assert!(!renamed.contains("double"), "{renamed}");
    assert_eq!(renamed.matches("twice").count(), 2, "{renamed}");

    server.change(uri, &renamed);
    assert!(
        server.diagnostics(uri).is_empty(),
        "the edits a rename returned have to leave a file that compiles:\n{renamed}"
    );

    server.shutdown();
}

#[test]
fn a_rename_it_will_not_do_says_why_rather_than_doing_nothing() {
    // Three refusals over the wire. `-32803` is 3.17's `RequestFailed`, and the message is the one
    // `beck_core::editor::Refusal` writes — a client shows it, so it is the whole of what somebody
    // sees when a rename does not happen.
    let mut server = handshake();
    let uri = "file:///tmp/refused.beck";
    server.open(uri, GOOD);
    let _ = server.diagnostics(uri);

    let refuse = |server: &mut support::lsp::Server, position: Value, to: &str| -> String {
        let reply = server.request(
            "textDocument/rename",
            json!({
                "textDocument": { "uri": uri },
                "position": position,
                "newName": to,
            }),
        );
        assert_eq!(
            reply.pointer("/error/code"),
            Some(&json!(-32803)),
            "a refused rename is an error response, not an empty edit: {reply}"
        );
        reply
            .pointer("/error/message")
            .and_then(Value::as_str)
            .expect("a reason")
            .to_string()
    };

    assert!(
        refuse(&mut server, a_use_of_double(), "label").contains("already used"),
        "renaming onto a name this file already has is refused"
    );
    assert!(
        refuse(&mut server, a_use_of_double(), "2fast").contains("not a name"),
        "so is a new name the lexer would not read"
    );

    // And an imported name, whose declaration is in a file this server is not showing.
    let importing = "import text\n\ndef size(s: Str) -> Int:\n    return word_count(s)\n";
    let uri = "file:///tmp/importing.beck";
    server.open(uri, importing);
    let _ = server.diagnostics(uri);
    let line = importing
        .lines()
        .position(|l| l.contains("word_count"))
        .expect("it is there");
    let character = importing
        .lines()
        .nth(line)
        .expect("a line")
        .find("word_count")
        .expect("it is there");
    let reply = server.request(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "newName": "words",
        }),
    );
    assert!(
        reply
            .pointer("/error/message")
            .and_then(Value::as_str)
            .expect("a reason")
            .contains("another module"),
        "{reply}"
    );

    server.shutdown();
}

#[test]
fn an_inlay_hint_is_the_placement_the_solver_chose_and_the_row_it_inferred() {
    let mut server = handshake();
    let uri = "file:///tmp/placed.beck";
    server.open(uri, PLACED);
    assert!(server.diagnostics(uri).is_empty());

    let reply = server.request(
        "textDocument/inlayHint",
        json!({ "textDocument": { "uri": uri } }),
    );
    let hints = reply
        .pointer("/result")
        .and_then(Value::as_array)
        .expect("an array")
        .clone();
    assert!(!hints.is_empty(), "{reply}");

    // Against the compiler's own answers rather than against literals: the tier is the one the
    // solver recorded and the row is the one `beck iface` publishes, so a hint that drifted from
    // either fails here.
    let (placed, _, _) = beck_core::compile_or_library_str("/tmp/placed.beck", PLACED);
    let interface = beck_core::iface::Interface::of(&placed.expect("it compiles").program);
    let key = interface.item("key").expect("`key` is published");
    let stamped = interface.item("stamped").expect("`stamped` is published");

    let labels: Vec<&str> = hints.iter().filter_map(|h| h["label"].as_str()).collect();
    assert!(
        labels.contains(&format!("@on({})", key.tier.name()).as_str()),
        "the tier hint is the solver's answer: {labels:?}"
    );
    assert!(
        labels.contains(&beck_core::iface::render_uses(&stamped.effects).as_str()),
        "the row hint is the published row: {labels:?}"
    );

    // Each one is offered where it could be written, which is what makes it worth showing: pasted
    // in at its own position, the file still compiles.
    let mut written: Vec<(u32, String)> = hints
        .iter()
        .map(|h| {
            (
                offset_of(PLACED, &h["position"]),
                h["label"].as_str().expect("a label").to_string(),
            )
        })
        .collect();
    written.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let mut source = PLACED.to_string();
    for (at, label) in written {
        // A tier is a decorator on its own line; a row belongs against the signature it ends.
        let text = if label.starts_with("@on(") {
            format!("{label}\n")
        } else {
            label
        };
        source.insert_str(at as usize, &text);
    }
    server.change(uri, &source);
    assert!(
        server.diagnostics(uri).is_empty(),
        "a hint has to be the text it says it is:\n{source}"
    );

    server.shutdown();
}

// ---------------------------------------------------------------------------------------------
// 5. The same two answers, over programs nobody wrote for this test
// ---------------------------------------------------------------------------------------------

/// Where the corpus lives, relative to this crate.
fn corpus() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .canonicalize()
        .expect("the corpus is checked in")
}

/// Renaming every name in every corpus program either works or is refused — never neither.
///
/// The fixtures above are four-line files written by somebody who knew what the rename would do.
/// [`corpus/`](../../../../corpus) is 30-odd programs written for a different purpose entirely,
/// with folds, signals, views, tests and `expect place(…)` clauses in them, and it is where the
/// interesting shapes are: a name mentioned in a static expectation, a signal a view reads, a
/// helper called from four places.
///
/// Two things are asserted of every name, and the second is the one worth the runtime: the edited
/// text **compiles**, and it publishes exactly the interface the original did with that one name
/// substituted. The first says the rename did not break the program; the second says it did not
/// quietly change it — a rename that dropped an occurrence would still compile whenever the name
/// it left behind resolved to something else.
///
/// The refusal count is asserted too, from both ends. A rename that refused everything would pass
/// an "either works or is refused" test without doing anything at all.
#[test]
fn renaming_every_name_in_the_corpus_either_works_or_says_why() {
    let mut renamed = 0usize;
    let mut refused: Vec<String> = Vec::new();

    for path in std::fs::read_dir(corpus())
        .expect("the corpus is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
    {
        let name = path.display().to_string();
        let source = std::fs::read_to_string(&path).expect("a corpus program");
        let editor = beck_core::editor::Editor::of(&name, &source);
        if editor.diagnostics().has_errors() {
            panic!("the corpus compiles; {name} does not");
        }
        let before: Vec<String> = editor.symbols().map(|(n, _)| n.to_string()).collect();

        for symbol in &before {
            let (start, end) = editor
                .symbol(symbol)
                .and_then(|s| s.span)
                .expect("an own name has a span");
            // The caret on the declaration's own name, which is where somebody presses F2.
            let caret = start
                + source[start as usize..end as usize]
                    .find(symbol.as_str())
                    .expect("a declaration writes its own name") as u32;
            let to = format!("renamed_{symbol}");

            let edits = match editor.rename(caret, &to) {
                Ok(edits) => edits,
                Err(refusal) => {
                    refused.push(format!(
                        "{}: {symbol} — {}",
                        path.display(),
                        refusal.message()
                    ));
                    continue;
                }
            };
            let mut after = source.clone();
            for edit in edits.iter().rev() {
                after.replace_range(edit.start as usize..edit.end as usize, &to);
            }

            let checked = beck_core::editor::Editor::of(&name, &after);
            assert!(
                !checked.diagnostics().has_errors(),
                "renaming `{symbol}` in {} broke it:\n{}",
                path.display(),
                checked.diagnostics().render(checked.source_map())
            );
            let expected: Vec<String> = before
                .iter()
                .map(|n| if n == symbol { to.clone() } else { n.clone() })
                .collect();
            let mut published: Vec<String> =
                checked.symbols().map(|(n, _)| n.to_string()).collect();
            let mut expected = expected;
            published.sort();
            expected.sort();
            assert_eq!(
                published,
                expected,
                "renaming `{symbol}` in {} changed what the module publishes",
                path.display()
            );
            renamed += 1;
        }
    }

    // Both ends, because each catches a different regression: a floor stops a change that refuses
    // its way to green, and a ceiling on the refusals stops one that quietly narrows what an editor
    // can do. The corpus renames 316 of its 325 names today and declines nine.
    assert!(
        renamed >= 250,
        "the corpus should rename hundreds of names; it renamed {renamed}, refusing:\n{}",
        refused.join("\n")
    );
    assert!(
        refused.len() <= 15,
        "{} refusals is more than this corpus has ever needed:\n{}",
        refused.len(),
        refused.join("\n")
    );
    // Every refusal is one of the two stated shapes rather than an accident, and the corpus
    // contains one of each (`docs/65` §65.6):
    //
    // * a name the analysis cannot account for — `page` in a program whose view is a local of the
    //   same name, which is the shadow this declines rather than captures;
    // * a rename that was made and did not compile — a *signal* that is one of several folds, whose
    //   name is also the field it occupies in the fused accumulator, so `expect state.tally.joins`
    //   reads it as a field and the edited file no longer type-checks. That refusal is the
    //   verification step firing, which is the one this suite most wants to see happen.
    for reason in &refused {
        assert!(
            reason.contains("cannot account for") || reason.contains("would not compile"),
            "an unexpected refusal: {reason}"
        );
    }
    assert!(
        refused.iter().any(|r| r.contains("would not compile")),
        "the verification step is load-bearing here, and a corpus that never triggers it is a \
         corpus that stopped testing it:\n{}",
        refused.join("\n")
    );
}
