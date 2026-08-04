//! `beck lsp` — the Language Server Protocol, over the same front end every other command uses.
//!
//! [`docs/04`](../../../../../docs/04-compiler-architecture.md) §4.6 fixes the design in one sentence:
//! *"One binary serves `beck build`, `beck check`, `beck lsp` and `beck explain`; there is no
//! separate language server implementation to drift."* So nothing here parses, checks or infers
//! anything. It calls [`beck_core::compile_or_library_str`] — the same entry point `beck test`
//! uses, and for the same reason ([`docs/27`](../../../../../docs/27-walls-report.md) §27.4: a module
//! being edited is usually a library) — and translates.
//!
//! # Why the whole file, every time
//!
//! §4.6 describes a Salsa query graph in which editing a body invalidates that body and nothing
//! upstream. That is not built. This server re-checks the whole file on every change, and
//! [`docs/64`](../../../../../docs/64-compile-speed-report.md) §64.6 is why that is a defensible
//! choice today rather than a shortcut: the *worst* file in the tree costs 4.7 ms through parse,
//! expand and check, and the median costs 0.75 ms. §65.4 says exactly where that stops being true.
//!
//! # Why the protocol is hand-written
//!
//! [`adr/0016`](../../../../../docs/adr/0016-the-language-server-speaks-json-rpc-directly.md). The
//! wire format is a `Content-Length` header, a blank line and a JSON body; the six requests below
//! need no more of LSP than that, and `serde_json` was already a dependency.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use beck_core::iface::{Interface, Kind};
use beck_diag::{Diagnostics, Severity, SourceMap};
use serde_json::{json, Value};

/// The protocol version this server implements enough of to be useful.
const LSP_VERSION: &str = "3.17";

/// Every open document, by URI. The client owns the text; this is the server's copy of it.
#[derive(Default)]
struct Documents(BTreeMap<String, String>);

/// One analysed document: what the front end made of the text the client last sent.
struct Analysis {
    diagnostics: Diagnostics,
    map: SourceMap,
    /// Every top-level name, with the byte span of its declaration and the line to show on hover.
    ///
    /// A `BTreeMap` so that document symbols come out in a stable order whatever order the checker
    /// resolved them in — the same reason [`Interface`] keeps its types in declaration order.
    names: BTreeMap<String, Symbol>,
}

struct Symbol {
    /// The whole declaration, for `documentSymbol` and `definition`.
    span: (u32, u32),
    /// The signature as `beck iface` would publish it — `render_item`, not a second renderer.
    signature: String,
    /// `12` (Function) or `13` (Variable) in LSP's `SymbolKind`.
    lsp_kind: u32,
    tier: String,
}

/// Run the server until the client says `exit`.
///
/// stdin and stdout are the transport, so **nothing else may write to stdout**: a stray `println!`
/// is a protocol violation the client reports as a parse error. Logging goes to stderr, which is
/// where `tracing_subscriber` already sends it.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    let mut docs = Documents::default();
    let mut shutdown_requested = false;

    while let Some(message) = read_message(&mut reader)? {
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method.as_str() {
            "initialize" => {
                respond(&mut writer, id, capabilities())?;
            }
            "initialized" => {}
            "textDocument/didOpen" => {
                if let Some((uri, text)) = opened(&params) {
                    docs.0.insert(uri.clone(), text);
                    publish(&mut writer, &docs, &uri)?;
                }
            }
            "textDocument/didChange" => {
                if let Some((uri, text)) = changed(&params) {
                    docs.0.insert(uri.clone(), text);
                    publish(&mut writer, &docs, &uri)?;
                }
            }
            "textDocument/didSave" => {
                if let Some(uri) = uri_of(&params) {
                    publish(&mut writer, &docs, &uri)?;
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = uri_of(&params) {
                    docs.0.remove(&uri);
                    // An empty list is how a client is told to clear the squiggles it is holding.
                    notify(
                        &mut writer,
                        "textDocument/publishDiagnostics",
                        json!({ "uri": uri, "diagnostics": [] }),
                    )?;
                }
            }
            "textDocument/hover" => {
                respond(&mut writer, id, hover(&docs, &params))?;
            }
            "textDocument/definition" => {
                respond(&mut writer, id, definition(&docs, &params))?;
            }
            "textDocument/documentSymbol" => {
                respond(&mut writer, id, document_symbols(&docs, &params))?;
            }
            "shutdown" => {
                shutdown_requested = true;
                respond(&mut writer, id, Value::Null)?;
            }
            "exit" => {
                // The protocol's own rule: `exit` after `shutdown` is success, and without one is
                // not. Honoured rather than ignored, because a client that never sent `shutdown`
                // has lost track of the server and should hear about it.
                return if shutdown_requested {
                    Ok(())
                } else {
                    std::process::exit(1)
                };
            }
            _ => {
                // A request must be answered even when it is not understood, or the client waits
                // forever. A notification (no id) must not be.
                if id.is_some() {
                    respond_error(&mut writer, id, -32601, &format!("no method `{method}`"))?;
                }
            }
        }
    }
    Ok(())
}

/// What this server can do, which is deliberately a short list.
fn capabilities() -> Value {
    json!({
        "capabilities": {
            // 1 = Full. The whole document arrives on every change, which is what a server that
            // re-checks the whole file wants anyway (docs/65 §65.2).
            "textDocumentSync": { "openClose": true, "change": 1, "save": true },
            "hoverProvider": true,
            "definitionProvider": true,
            "documentSymbolProvider": true,
        },
        "serverInfo": { "name": "beck", "version": env!("CARGO_PKG_VERSION") },
        "lspVersion": LSP_VERSION,
    })
}

// ---------------------------------------------------------------------------------------------
// The front end, called once per change
// ---------------------------------------------------------------------------------------------

/// Parse, expand, check, place and secure one document, and index what a client can ask about.
///
/// `compile_or_library_str` rather than `compile_str`: a file being edited is a module, and most
/// modules are libraries. Refusing to analyse one because it has no merge point would make the
/// server useless for exactly the files people spend their time in.
fn analyse(uri: &str, text: &str) -> Analysis {
    let name = path_of(uri).unwrap_or_else(|| uri.to_string());
    let (placed, diagnostics, map) = beck_core::compile_or_library_str(&name, text);

    // The interface is the signature renderer `beck iface` publishes, and hover shows what it
    // publishes. A second renderer here is the drift §4.6 forbids.
    let mut names = BTreeMap::new();
    if let Some(placed) = placed.as_ref() {
        let interface = Interface::of(&placed.program);
        for item in &interface.items {
            let (span, lsp_kind) = match &item.kind {
                Kind::Function { .. } => (
                    placed
                        .program
                        .defs
                        .get(&item.name)
                        .map(|d| (d.span.start, d.span.end)),
                    12,
                ),
                Kind::Signal { .. } => (
                    placed
                        .program
                        .signals
                        .iter()
                        .find(|s| s.name == item.name)
                        .map(|s| (s.span.start, s.span.end)),
                    13,
                ),
            };
            let Some(span) = span else { continue };
            names.insert(
                item.name.to_string(),
                Symbol {
                    span,
                    signature: beck_core::iface::render_item(item).trim_end().to_string(),
                    lsp_kind,
                    tier: item.tier.name().to_string(),
                },
            );
        }
    }

    Analysis {
        diagnostics,
        map,
        names,
    }
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

fn publish(writer: &mut impl Write, docs: &Documents, uri: &str) -> Result<()> {
    let Some(text) = docs.0.get(uri) else {
        return Ok(());
    };
    let analysis = analyse(uri, text);
    let items: Vec<Value> = analysis
        .diagnostics
        .iter()
        .map(|d| {
            json!({
                "range": range(&analysis.map, text, d.primary.start, d.primary.end),
                // 1 = Error, 2 = Warning. Beck has no `Note` severity at the top level of a
                // diagnostic; its notes ride on the diagnostic they belong to.
                "severity": match d.severity { Severity::Error => 1, _ => 2 },
                "code": d.code,
                "source": "beck",
                "message": message_of(d),
            })
        })
        .collect();
    notify(
        writer,
        "textDocument/publishDiagnostics",
        json!({ "uri": uri, "diagnostics": items }),
    )
}

/// The message a client shows, with the notes the terminal renderer would have printed.
///
/// A `B0350` that says only "cannot find `foo`" is a worse diagnostic in an editor than in a
/// terminal, because the editor drops everything the terminal put underneath it. The notes carry
/// the fix suggestion §3.4 insists on, so they travel.
fn message_of(d: &beck_diag::Diagnostic) -> String {
    let mut out = d.message.clone();
    for note in &d.notes {
        out.push_str("\n\nnote: ");
        out.push_str(note);
    }
    if let Some(fix) = &d.fix {
        out.push_str("\n\nhelp: ");
        out.push_str(fix);
    }
    out
}

fn hover(docs: &Documents, params: &Value) -> Value {
    let Some((uri, offset)) = position(docs, params) else {
        return Value::Null;
    };
    let Some(text) = docs.0.get(&uri) else {
        return Value::Null;
    };
    let Some(word) = word_at(text, offset) else {
        return Value::Null;
    };
    let analysis = analyse(&uri, text);
    let Some(symbol) = analysis.names.get(&word) else {
        return Value::Null;
    };
    json!({
        "contents": {
            "kind": "markdown",
            "value": format!(
                "```beck\n@on({})\n{}\n```",
                symbol.tier, symbol.signature
            ),
        }
    })
}

fn definition(docs: &Documents, params: &Value) -> Value {
    let Some((uri, offset)) = position(docs, params) else {
        return Value::Null;
    };
    let Some(text) = docs.0.get(&uri) else {
        return Value::Null;
    };
    let Some(word) = word_at(text, offset) else {
        return Value::Null;
    };
    let analysis = analyse(&uri, text);
    let Some(symbol) = analysis.names.get(&word) else {
        return Value::Null;
    };
    json!({
        "uri": uri,
        "range": range(&analysis.map, text, symbol.span.0, symbol.span.1),
    })
}

fn document_symbols(docs: &Documents, params: &Value) -> Value {
    let Some(uri) = uri_of(params) else {
        return json!([]);
    };
    let Some(text) = docs.0.get(&uri) else {
        return json!([]);
    };
    let analysis = analyse(&uri, text);
    let out: Vec<Value> = analysis
        .names
        .iter()
        .map(|(name, s)| {
            let r = range(&analysis.map, text, s.span.0, s.span.1);
            json!({
                "name": name,
                "kind": s.lsp_kind,
                "detail": s.signature,
                "range": r,
                "selectionRange": r,
            })
        })
        .collect();
    json!(out)
}

// ---------------------------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------------------------

/// A byte span, as LSP's zero-based line and **UTF-16** character offsets.
///
/// UTF-16 because that is what the protocol specifies by default, and getting it wrong is invisible
/// until somebody writes an emoji in a string literal — which `beck-syntax`'s own security tests
/// say they will. `SourceMap::line_col` counts *characters* and is one-based, so it is the wrong
/// unit twice over and is deliberately not used here.
fn range(_map: &SourceMap, text: &str, start: u32, end: u32) -> Value {
    json!({ "start": utf16_position(text, start), "end": utf16_position(text, end) })
}

fn utf16_position(text: &str, offset: u32) -> Value {
    let offset = (offset as usize).min(text.len());
    let mut line = 0u32;
    let mut character = 0u32;
    for (i, c) in text.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += c.len_utf16() as u32;
        }
    }
    json!({ "line": line, "character": character })
}

/// The inverse: LSP's line and UTF-16 character back to a byte offset.
fn byte_offset(text: &str, line: u32, character: u32) -> Option<u32> {
    let mut at_line = 0u32;
    let mut utf16 = 0u32;
    for (i, c) in text.char_indices() {
        if at_line == line && utf16 == character {
            return Some(i as u32);
        }
        if c == '\n' {
            if at_line == line {
                // The position is past the end of its line, which a client may legitimately send
                // when the cursor sits after the last character.
                return Some(i as u32);
            }
            at_line += 1;
            utf16 = 0;
        } else if at_line == line {
            utf16 += c.len_utf16() as u32;
        }
    }
    (at_line == line).then_some(text.len() as u32)
}

fn position(docs: &Documents, params: &Value) -> Option<(String, u32)> {
    let uri = uri_of(params)?;
    let text = docs.0.get(&uri)?;
    let p = params.get("position")?;
    let line = p.get("line")?.as_u64()? as u32;
    let character = p.get("character")?.as_u64()? as u32;
    Some((uri.clone(), byte_offset(text, line, character)?))
}

/// The identifier the cursor is inside or immediately after.
///
/// "Immediately after" matters: an editor sends the position of the caret, and a caret at the end
/// of `total` is one byte past the `l`. A server that only looked at the byte under the caret would
/// answer nothing for the most common way of asking.
fn word_at(text: &str, offset: u32) -> Option<String> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let bytes = text.as_bytes();
    let mut at = (offset as usize).min(bytes.len());
    if at > 0 && (at == bytes.len() || !is_word(text[at..].chars().next()?)) {
        at -= 1;
    }
    if !text.is_char_boundary(at) || !is_word(text[at..].chars().next()?) {
        return None;
    }
    let mut start = at;
    while start > 0 {
        let prev = text[..start].char_indices().next_back()?;
        if !is_word(prev.1) {
            break;
        }
        start = prev.0;
    }
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map(|(i, _)| at + i)
        .unwrap_or(text.len());
    Some(text[start..end].to_string())
}

// ---------------------------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------------------------

/// Read one `Content-Length`-framed JSON message, or `None` at end of input.
///
/// Headers are case-insensitive per RFC 7230, which LSP inherits, and `Content-Type` is ignored:
/// the protocol permits only one and it is the one we would assume anyway.
fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).context("reading a header")? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let Some(length) = length else {
        // A body of unknown length cannot be skipped safely, so the stream is no longer parseable.
        anyhow::bail!("a message arrived with no `Content-Length`");
    };
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).context("reading a body")?;
    Ok(Some(
        serde_json::from_slice(&body).context("the body is not JSON")?,
    ))
}

fn write_message(writer: &mut impl Write, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

fn respond(writer: &mut impl Write, id: Option<Value>, result: Value) -> Result<()> {
    // A notification has no id and takes no response. Answering one is a protocol violation that
    // some clients report and others silently mishandle.
    let Some(id) = id else { return Ok(()) };
    write_message(
        writer,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )
}

fn respond_error(
    writer: &mut impl Write,
    id: Option<Value>,
    code: i32,
    message: &str,
) -> Result<()> {
    let Some(id) = id else { return Ok(()) };
    write_message(
        writer,
        &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    )
}

fn notify(writer: &mut impl Write, method: &str, params: Value) -> Result<()> {
    write_message(
        writer,
        &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    )
}

// ---------------------------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------------------------

fn uri_of(params: &Value) -> Option<String> {
    params
        .get("textDocument")?
        .get("uri")?
        .as_str()
        .map(str::to_string)
}

fn opened(params: &Value) -> Option<(String, String)> {
    let doc = params.get("textDocument")?;
    Some((
        doc.get("uri")?.as_str()?.to_string(),
        doc.get("text")?.as_str()?.to_string(),
    ))
}

/// The full text of a `didChange`, which is what `textDocumentSync: Full` guarantees.
fn changed(params: &Value) -> Option<(String, String)> {
    let uri = uri_of(params)?;
    let text = params
        .get("contentChanges")?
        .as_array()?
        .last()?
        .get("text")?
        .as_str()?
        .to_string();
    Some((uri, text))
}

/// `file:///a/b.beck` → `/a/b.beck`, for diagnostics that name the file.
///
/// Percent-decoding is deliberately minimal — `%20` and nothing else — because the name is used
/// for display and never to open anything. A URI this does not understand falls back to itself,
/// which is worse-looking and not wrong.
fn path_of(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    Some(rest.replace("%20", " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_utf16_and_round_trip() {
        // The byte, character and UTF-16 counts all differ on this line, which is the only way to
        // tell a correct implementation from one that happens to agree on ASCII.
        let text = "def f() -> Str:\n    return \"🎈 x\"\n";
        let balloon = text.find('🎈').expect("the emoji is there") as u32;
        let p = utf16_position(text, balloon);
        assert_eq!(p["line"], 1);
        // `    return "` is 12 UTF-16 units, and the emoji has not been counted yet.
        assert_eq!(p["character"], 12);
        // And back again.
        assert_eq!(byte_offset(text, 1, 12), Some(balloon));
        // One position past the emoji is *two* UTF-16 units later, not one.
        assert_eq!(byte_offset(text, 1, 14), Some(balloon + 4));
    }

    #[test]
    fn a_caret_at_either_end_of_a_name_finds_it() {
        let text = "def total(x: Int) -> Int:\n    return x\n";
        let at = text.find("total").expect("it is there") as u32;
        assert_eq!(word_at(text, at).as_deref(), Some("total"));
        assert_eq!(word_at(text, at + 2).as_deref(), Some("total"));
        // The caret sits *after* the last character, which is where an editor puts it when you
        // finish typing a name.
        assert_eq!(word_at(text, at + 5).as_deref(), Some("total"));
        // The same rule read from the other side: a caret in the space before `total` is a caret
        // just past `def`, and answering `def` is what "immediately after" means. It is asserted
        // rather than left implicit because it is the one case where the rule looks like a bug.
        assert_eq!(word_at(text, at - 1).as_deref(), Some("def"));
        // Somewhere no identifier touches on either side finds nothing rather than guessing.
        let arrow = text.find("-> Int").expect("it is there") as u32;
        assert_eq!(word_at(text, arrow + 1), None);
    }

    #[test]
    fn a_message_carries_the_notes_the_terminal_would_have_printed() {
        let (_, diags, _) =
            beck_core::compile_or_library_str("x.beck", "def f(x: Int) -> Str:\n    return x\n");
        let d = diags.iter().next().expect("it does not compile");
        let message = message_of(d);
        assert!(message.contains("expected"), "{message}");
    }
}
