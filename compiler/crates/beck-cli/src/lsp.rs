//! `beck lsp` — the Language Server Protocol, over the same front end every other command uses.
//!
//! [`docs/04`](../../../../../docs/04-compiler-architecture.md) §4.6 fixes the design in one sentence:
//! *"One binary serves `beck build`, `beck check`, `beck lsp` and `beck explain`; there is no
//! separate language server implementation to drift."* So nothing here parses, checks or infers
//! anything. It calls [`beck_core::compile_or_library_str`] — the same entry point `beck test`
//! uses, and for the same reason ([`docs/27`](../../../../../docs/27-walls-report.md) §27.4: a module
//! being edited is usually a library) — and translates.
//!
//! # Where the answers come from
//!
//! Not from here. [`beck_core::editor`] holds the indexing, the positions, the word-under-the-caret
//! rule, the token classification and the completion list, because a browser tab wants every one of
//! them too ([`docs/102`](../../../../../docs/102-playground-phase-3-report.md)) and a second copy
//! is a second thing to be wrong. What this file does is translate: JSON-RPC in, LSP shapes out.
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
use beck_core::editor::{byte_offset, utf16_position, Editor, Index, TokenKind};
use serde_json::{json, Value};

/// The protocol version this server implements enough of to be useful.
const LSP_VERSION: &str = "3.17";

/// Every open document, by URI. The client owns the text; this is the server's copy of it.
///
/// `last` is the most recent analysis that produced a *program* — what completion falls back to
/// while a name is half-typed, which is most of the time somebody is asking for one
/// ([`Editor::completing_from`]).
#[derive(Default)]
struct Documents {
    text: BTreeMap<String, String>,
    last: BTreeMap<String, Index>,
}

impl Documents {
    /// Analyse a document, remembering the names if it checked.
    ///
    /// Every request that needs an index goes through here rather than calling the front end
    /// directly, so "the names are from the last text that compiled" is one rule in one place.
    fn analyse(&mut self, uri: &str) -> Option<Editor> {
        let text = self.text.get(uri)?;
        let name = path_of(uri).unwrap_or_else(|| uri.to_string());
        let editor = Editor::of(&name, text);
        if editor.placed().is_some() {
            self.last.insert(uri.to_string(), editor.index());
            return Some(editor);
        }
        Some(match self.last.get(uri) {
            Some(previous) => editor.completing_from(previous),
            None => editor,
        })
    }
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
                    docs.text.insert(uri.clone(), text);
                    publish(&mut writer, &mut docs, &uri)?;
                }
            }
            "textDocument/didChange" => {
                if let Some((uri, text)) = changed(&params) {
                    docs.text.insert(uri.clone(), text);
                    publish(&mut writer, &mut docs, &uri)?;
                }
            }
            "textDocument/didSave" => {
                if let Some(uri) = uri_of(&params) {
                    publish(&mut writer, &mut docs, &uri)?;
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = uri_of(&params) {
                    docs.text.remove(&uri);
                    docs.last.remove(&uri);
                    // An empty list is how a client is told to clear the squiggles it is holding.
                    notify(
                        &mut writer,
                        "textDocument/publishDiagnostics",
                        json!({ "uri": uri, "diagnostics": [] }),
                    )?;
                }
            }
            "textDocument/hover" => {
                respond(&mut writer, id, hover(&mut docs, &params))?;
            }
            "textDocument/definition" => {
                respond(&mut writer, id, definition(&mut docs, &params))?;
            }
            "textDocument/documentSymbol" => {
                respond(&mut writer, id, document_symbols(&mut docs, &params))?;
            }
            "textDocument/completion" => {
                respond(&mut writer, id, completion(&mut docs, &params))?;
            }
            "textDocument/semanticTokens/full" => {
                respond(&mut writer, id, semantic_tokens(&docs, &params))?;
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
            // No trigger characters: Beck has no `.` completion to offer — a field access resolves
            // through a type this server does not track positions in — so completion is asked for
            // explicitly and answers on the word being typed.
            "completionProvider": { "resolveProvider": false },
            // The legend is `beck_core::editor`'s, so the categories an editor colours are the
            // categories the playground colours (docs/102).
            "semanticTokensProvider": {
                "legend": { "tokenTypes": TokenKind::legend(), "tokenModifiers": [] },
                "full": true,
            },
        },
        "serverInfo": { "name": "beck", "version": env!("CARGO_PKG_VERSION") },
        "lspVersion": LSP_VERSION,
    })
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

fn publish(writer: &mut impl Write, docs: &mut Documents, uri: &str) -> Result<()> {
    let Some(editor) = docs.analyse(uri) else {
        return Ok(());
    };
    let Some(text) = docs.text.get(uri) else {
        return Ok(());
    };
    let items: Vec<Value> = editor
        .marks()
        .iter()
        .map(|m| {
            json!({
                "range": range(text, m.start, m.end),
                // 1 = Error, 2 = Warning. Beck has no `Note` severity at the top level of a
                // diagnostic; its notes ride on the diagnostic they belong to.
                "severity": if m.error { 1 } else { 2 },
                "code": m.code,
                "source": "beck",
                "message": m.message,
            })
        })
        .collect();
    notify(
        writer,
        "textDocument/publishDiagnostics",
        json!({ "uri": uri, "diagnostics": items }),
    )
}

fn hover(docs: &mut Documents, params: &Value) -> Value {
    let Some((uri, offset)) = position(docs, params) else {
        return Value::Null;
    };
    let Some(editor) = docs.analyse(&uri) else {
        return Value::Null;
    };
    let Some(symbol) = editor.hover(offset) else {
        return Value::Null;
    };
    // The documentation the declaration carries, under the signature — `##` is metadata rather
    // than a form (`docs/34`), so an editor is exactly where it is supposed to end up.
    let doc = symbol
        .doc
        .as_ref()
        .map(|d| format!("\n\n{d}"))
        .unwrap_or_default();
    json!({
        "contents": {
            "kind": "markdown",
            "value": format!(
                "```beck\n@on({})\n{}\n```{doc}",
                symbol.tier, symbol.signature
            ),
        }
    })
}

fn definition(docs: &mut Documents, params: &Value) -> Value {
    let Some((uri, offset)) = position(docs, params) else {
        return Value::Null;
    };
    let Some(editor) = docs.analyse(&uri) else {
        return Value::Null;
    };
    // An imported name has no declaration in this document, and the front end says so by having no
    // span for it rather than by pointing somewhere plausible.
    let Some(span) = editor.definition(offset) else {
        return Value::Null;
    };
    let Some(text) = docs.text.get(&uri) else {
        return Value::Null;
    };
    json!({ "uri": uri, "range": range(text, span.0, span.1) })
}

fn document_symbols(docs: &mut Documents, params: &Value) -> Value {
    let Some(uri) = uri_of(params) else {
        return json!([]);
    };
    let Some(editor) = docs.analyse(&uri) else {
        return json!([]);
    };
    let Some(text) = docs.text.get(&uri) else {
        return json!([]);
    };
    let out: Vec<Value> = editor
        .symbols()
        .filter_map(|(name, s)| {
            let span = s.span?;
            let r = range(text, span.0, span.1);
            Some(json!({
                "name": name,
                "kind": s.kind.lsp_symbol(),
                "detail": s.signature,
                "range": r,
                "selectionRange": r,
            }))
        })
        .collect();
    json!(out)
}

/// What could finish the word being typed.
///
/// `isIncomplete` is false: the list is everything that matches, so a client may filter it further
/// itself rather than asking again on the next keystroke.
fn completion(docs: &mut Documents, params: &Value) -> Value {
    let Some((uri, offset)) = position(docs, params) else {
        return json!({ "isIncomplete": false, "items": [] });
    };
    let Some(editor) = docs.analyse(&uri) else {
        return json!({ "isIncomplete": false, "items": [] });
    };
    let items: Vec<Value> = editor
        .completions(offset)
        .into_iter()
        .map(|c| {
            json!({
                "label": c.label,
                "kind": c.kind.lsp(),
                "detail": c.detail,
                "documentation": c.doc,
            })
        })
        .collect();
    json!({ "isIncomplete": false, "items": items })
}

/// Highlighting, in the protocol's five-integers-per-token encoding.
///
/// Each token is `deltaLine, deltaStart, length, type, modifiers`, relative to the previous one —
/// and the lengths and columns are UTF-16 units, like every other position in this protocol.
///
/// It does not go through [`Documents::analyse`]: highlighting is a function of the *text*, and a
/// file being typed into is a file that does not check. A highlighter that waited for a clean parse
/// would go out exactly when it was most wanted.
fn semantic_tokens(docs: &Documents, params: &Value) -> Value {
    let Some(uri) = uri_of(params) else {
        return json!({ "data": [] });
    };
    let Some(text) = docs.text.get(&uri) else {
        return json!({ "data": [] });
    };
    let mut data: Vec<u32> = Vec::new();
    let (mut last_line, mut last_start) = (0u32, 0u32);
    for token in beck_core::editor::tokens(text) {
        let (line, start) = utf16_position(text, token.start);
        let (end_line, end) = utf16_position(text, token.end);
        // A token that spans a line has no length in this encoding. Beck has none — a string
        // literal cannot contain a newline and a comment ends at one — and a client shown a
        // negative length paints the rest of the file, so this refuses rather than guesses.
        if end_line != line {
            continue;
        }
        data.push(line - last_line);
        data.push(if line == last_line {
            start - last_start
        } else {
            start
        });
        data.push(end - start);
        data.push(token.kind.lsp_index());
        data.push(0);
        last_line = line;
        last_start = start;
    }
    json!({ "data": data })
}

// ---------------------------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------------------------

/// A byte span, as LSP's zero-based line and **UTF-16** character offsets.
///
/// The conversion is [`beck_core::editor`]'s, because the playground needs the same one and a
/// second implementation of UTF-16 counting is a second place to be wrong about an emoji.
fn range(text: &str, start: u32, end: u32) -> Value {
    let at = |offset: u32| {
        let (line, character) = utf16_position(text, offset);
        json!({ "line": line, "character": character })
    };
    json!({ "start": at(start), "end": at(end) })
}

fn position(docs: &Documents, params: &Value) -> Option<(String, u32)> {
    let uri = uri_of(params)?;
    let text = docs.text.get(&uri)?;
    let p = params.get("position")?;
    let line = p.get("line")?.as_u64()? as u32;
    let character = p.get("character")?.as_u64()? as u32;
    Some((uri.clone(), byte_offset(text, line, character)?))
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

    /// The positions, the word rule and the message text are `beck_core::editor`'s and are tested
    /// there. What is left here is the translation, and this is the part of it that has an
    /// arithmetic bug waiting in it: the deltas.
    #[test]
    fn semantic_tokens_are_deltas_from_the_previous_token() {
        let text = "def f() -> Int:\n    return 1\n";
        let mut docs = Documents::default();
        docs.text.insert("file:///t.beck".into(), text.to_string());
        let out = semantic_tokens(
            &docs,
            &json!({ "textDocument": { "uri": "file:///t.beck" } }),
        );
        let data: Vec<u64> = out["data"]
            .as_array()
            .expect("five integers per token")
            .iter()
            .map(|v| v.as_u64().expect("an integer"))
            .collect();
        assert_eq!(data.len() % 5, 0);
        // `def` is the first token: line 0, column 0, three units long, and a keyword.
        assert_eq!(&data[..3], &[0, 0, 3]);
        assert_eq!(
            TokenKind::legend()[data[3] as usize],
            TokenKind::Keyword.lsp_type()
        );
        // `f` follows on the same line, so its start is a *delta*: one column past `def `.
        assert_eq!(&data[5..8], &[0, 4, 1]);
        // The first token of the second line is a delta of one line, and its column is absolute
        // again — `    return` starts at 4.
        let second_line = data
            .chunks(5)
            .position(|t| t[0] == 1)
            .expect("a token on the next line");
        assert_eq!(data.chunks(5).nth(second_line).expect("it is there")[1], 4);
    }
}
