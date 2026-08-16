//! The playground — the whole stack in a browser tab.
//!
//! [`docs/17-playground.md`](../../../../docs/17-playground.md) describes three rungs. Two of them
//! are this crate:
//!
//! * **Rung A** ([`analysis`]) — the compiler, client-side. Type checking with real diagnostics,
//!   the two surfaces, inferred placement per definition, the dataflow plan, the read model's SQL,
//!   the generated Kubernetes objects, effect signatures, `beck explain`. Zero servers: a static
//!   file host and this module are the whole deployment.
//! * **Rung B** ([`tab`]) — the whole *application*, in the same tab. The worker holds a [`tab::Tab`]
//!   — a log, an accumulator and N subscriptions — and the client iframes speak the same patch
//!   protocol a deployed Beck application speaks, over a `MessageChannel` instead of a websocket.
//!
//! # What makes rung B a demonstration rather than a demo
//!
//! Everything that decides *behaviour* is the deployed implementation, linked into a second host:
//! [`mod@beck_host::sequence`] decides a batch, [`beck_host::Runtime`] runs the program's own
//! `validate`, fold and `view`, [`mod@beck_core::diff`] produces the patch ops, and the browser applies
//! them with `beck-patch.js` — the same file `beck run` serves. What the tab supplies is a `Vec`
//! where a deployment has Postgres and a function call where it has a socket.
//!
//! §17.2 puts it as a guarantee rather than a resemblance: "by the differential harness's own
//! guarantee, rung-B behaviour *is* the deployed behaviour". `playground.rs` is that harness —
//! the same commands through `beck_rt::App` and through a `Tab`, asserting the same pages and the
//! same frames.
//!
//! # The boundary
//!
//! Four exports and a length-prefixed byte buffer, exactly as [`beck_wasm`] does it, and for the
//! same reason: no `wasm-bindgen`, no generated glue, no `unsafe`. A buffer is a `Vec<u8>` this
//! module keeps in a table keyed by the address of its own allocation; the host writes through
//! linear memory, and Rust reads its own `Vec` back out of the table.
//!
//! | Export | What it does |
//! |---|---|
//! | `beck_alloc(len) -> ptr` | reserve `len` bytes for the host to write into |
//! | `beck_free(ptr)` | release a buffer, whichever side allocated it |
//! | `beck_call(ptr, len) -> ptr` | one JSON request, one JSON response |
//!
//! [`beck_wasm`]: ../beck_wasm/index.html

pub mod analysis;
#[cfg(not(target_arch = "wasm32"))]
pub mod serve;
pub mod share;
pub mod tab;

pub use analysis::{analyse, Analysis, Section};
pub use tab::Tab;

use serde_json::{json, Value};

/// The page's whole state: a program, once one has been loaded, and the last analysis that checked.
///
/// Rung A needs no program — an analysis is a pure function of a string — so a visitor who never
/// presses *run* never builds one.
#[derive(Default)]
pub struct Playground {
    tab: Option<Tab>,
    /// The names from the last text that checked, so a half-typed name still completes
    /// ([`beck_core::editor::Editor::completing_from`]). It is the whole of what this holds on
    /// behalf of the editor: the analysis itself is rebuilt per request.
    index: beck_core::editor::Index,
}

impl Playground {
    pub fn new() -> Playground {
        Playground::default()
    }

    /// The loaded application, for a test that wants to ask it something directly.
    pub fn tab(&mut self) -> Option<&mut Tab> {
        self.tab.as_mut()
    }

    /// Analyse the editor's text, remembering the names if it checked.
    ///
    /// The module is called `playground.beck` here and everywhere else the playground compiles
    /// something, because a tier crossing's id is content-derived from the module name (§4.3): two
    /// names would be two programs.
    fn editor(&mut self, source: &str) -> beck_core::editor::Editor {
        let editor = beck_core::editor::Editor::of("playground.beck", source);
        if editor.placed().is_some() {
            self.index = editor.index();
            return editor;
        }
        editor.completing_from(&self.index)
    }
}

/// Who a request is asking as, and where they are.
///
/// The route matters even for the two answers that are not a subscription — the scrubber's page and
/// the document a client iframe opens with — because a page is a function of `session.path`
/// (`docs/94`), and rendering the root's page into a frame that is about to resume at `/done`
/// would be a first paint the client immediately has to correct.
fn viewer(request: &Value) -> beck_host::At<String> {
    let field = |k: &str| {
        request
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let path = field("path");
    beck_host::At {
        who: field("actor"),
        path: if path.is_empty() {
            beck_core::edge::ROOT.into()
        } else {
            path.as_str().into()
        },
    }
}

/// One request against the playground. See the module docs for the boundary this crosses.
///
/// Split out of the export so it can be driven from a native test: the wasm boundary is bytes and
/// pointers, and none of the behaviour worth gating lives there.
pub fn dispatch(state: &mut Playground, request: &Value) -> Result<Value, String> {
    let op = request
        .get("op")
        .and_then(|o| o.as_str())
        .ok_or("a request needs an `op`")?;
    let text = |k: &str| {
        request
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let number = |k: &str| request.get(k).and_then(|v| v.as_i64()).unwrap_or(0);

    match op {
        // Rung A. Note what it does not touch: `state`. A visitor typing into the editor is
        // compiling, not running, and the running application is not disturbed by an edit until
        // they say so.
        "analyse" => {
            let source = text("source");
            let a = analyse(&source);
            let at = |byte: u32| beck_core::editor::utf16_offset(&source, byte);
            Ok(json!({
                "diagnostics": a.diagnostics,
                "errors": a.errors,
                "warnings": a.warnings,
                "runnable": a.runnable,
                "marks": a.marks.iter().map(|m| json!({
                    "s": at(m.start), "e": at(m.end),
                    "error": m.error, "code": m.code, "message": m.message,
                })).collect::<Vec<_>>(),
                "sections": a.sections.iter().map(|s| json!({
                    "id": s.id, "title": s.title, "text": s.text,
                })).collect::<Vec<_>>(),
            }))
        }
        "examples" => Ok(json!(tab::examples()
            .into_iter()
            .map(|(name, source)| json!({"name": name, "source": source}))
            .collect::<Vec<_>>())),

        // The editor's own answers, which are the language server's
        // ([`beck_core::editor`]). Highlighting is separate from analysis and deliberately so: it
        // is a function of the text rather than of a program, so it costs a lex rather than a
        // check and it still works while the file is broken.
        // Every offset that crosses this boundary is a **UTF-16** offset, because the other side of
        // it is a `<textarea>` and a browser counts its value in UTF-16 code units. The compiler
        // works in bytes; the conversion is `beck_core::editor`'s and happens here, where the text
        // is, rather than in JavaScript where it would be a second implementation of it.
        "tokens" => {
            let source = text("source");
            let at = |byte: u32| beck_core::editor::utf16_offset(&source, byte);
            Ok(json!({
                "tokens": beck_core::editor::tokens(&source)
                    .into_iter()
                    .map(|t| json!({"s": at(t.start), "e": at(t.end), "k": t.kind.name()}))
                    .collect::<Vec<_>>(),
            }))
        }
        "complete" => {
            let source = text("source");
            let editor = state.editor(&source);
            let at = beck_core::editor::byte_of_utf16(&source, number("offset").max(0) as u32);
            Ok(json!({
                "prefix": editor.prefix(at),
                "stale": editor.stale(),
                "items": editor.completions(at).into_iter().map(|c| json!({
                    "label": c.label,
                    "detail": c.detail,
                    "kind": match c.kind {
                        beck_core::editor::CompletionKind::Keyword => "keyword",
                        beck_core::editor::CompletionKind::Signal => "signal",
                        beck_core::editor::CompletionKind::Function => "function",
                    },
                    "doc": c.doc,
                })).collect::<Vec<_>>(),
            }))
        }
        "describe" => {
            let source = text("source");
            let editor = state.editor(&source);
            let at = beck_core::editor::byte_of_utf16(&source, number("offset").max(0) as u32);
            Ok(match editor.hover(at) {
                Some(s) => json!({
                    "signature": s.signature,
                    "tier": s.tier,
                    "doc": s.doc,
                    "own": s.own,
                }),
                None => json!(null),
            })
        }

        // A share link. §17.4's "a playground is a program, and Beck programs are
        // content-addressed artefacts", as far as a tab with no registry behind it can take it:
        // the link *carries* the program and names its digest, so it needs nothing to resolve
        // against and cannot resolve to something else.
        "share" => {
            let source = text("source");
            Ok(json!({
                "digest": beck_core::digest::of(&source),
                "fragment": share::pack(&source),
            }))
        }
        "open" => {
            let (source, digest) = share::unpack(&text("fragment"))?;
            Ok(json!({ "source": source, "digest": digest }))
        }

        // Rung B.
        "load" => {
            let source = text("source");
            let mut diags = beck_diag::Diagnostics::new();
            let mut map = beck_diag::SourceMap::new();
            let id = map.add("playground.beck", &source);
            let Some(placed) = beck_core::compile(id, "playground.beck", &source, &mut diags)
            else {
                return Err(diags.render(&map));
            };
            let mut tab = Tab::load(placed)?;
            tab.set_now(number("now"));
            let answer = json!({
                "wire": tab.wire_id(),
                "mode": match tab.mode() {
                    beck_core::render::Mode::Server => "a",
                    beck_core::render::Mode::Client => "b",
                },
                "head": tab.head(),
            });
            state.tab = Some(tab);
            Ok(answer)
        }
        _ => {
            let tab = state.tab.as_mut().ok_or("no program is running")?;
            tab.set_now(number("now"));
            match op {
                "hello" => Ok(outgoing(tab.hello(
                    &text("sub"),
                    &text("actor"),
                    &text("path"),
                    request.get("seq").and_then(|s| s.as_u64()),
                ))),
                "command" => Ok(outgoing(tab.command(
                    &text("sub"),
                    &text("id"),
                    request.get("command").unwrap_or(&Value::Null),
                ))),
                "nav" => Ok(outgoing(tab.nav(&text("sub"), &text("path")))),
                "history" => Ok(json!({"head": tab.head(), "events": tab.history()})),
                // The records a page keeps, and the bundle a Mode B client renders from. Both are
                // base64 because the boundary this crosses is JSON.
                // The log a page kept from a previous visit, folded back in. Before any client
                // subscribes, which is the only moment it can be: a restore afterwards would be
                // rewriting history under a page that had already rendered it.
                "restore" => {
                    let records: Result<Vec<Vec<u8>>, String> = request
                        .get("records")
                        .and_then(|r| r.as_array())
                        .map(|records| {
                            records
                                .iter()
                                .map(|r| {
                                    beck_core::digest::base64_decode_bytes(
                                        r.as_str().unwrap_or_default(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_else(|| Ok(Vec::new()));
                    Ok(json!({ "head": tab.restore(&records?)? }))
                }
                "records" => Ok(json!({
                    "head": tab.head(),
                    "records": tab.records(number("after").max(0) as u64)?
                        .iter()
                        .map(|r| beck_core::digest::base64_encode_bytes(r))
                        .collect::<Vec<_>>(),
                })),
                "bundle" => Ok(json!({
                    "bundle": beck_core::digest::base64_encode_bytes(&tab.bundle()),
                })),
                "at" => Ok(json!({
                    "seq": number("seq"),
                    "html": tab.page_at(number("seq").max(0) as u64, &viewer(request))?.render(),
                })),
                "rendered" => Ok(json!({"html": tab.rendered(&viewer(request))?})),
                other => Err(format!(
                    "`{other}` is not a request this playground answers"
                )),
            }
        }
    }
}

/// The frames a request produced, each addressed to the subscription that gets it.
///
/// One request, N frames, because one command moves every subscriber's page — which is exactly the
/// shape a server has and is what makes two client iframes a *multiplayer* demonstration rather
/// than two copies of one client.
fn outgoing(out: Vec<tab::Outgoing>) -> Value {
    json!({
        "out": out
            .into_iter()
            .map(|o| json!({"sub": o.sub, "msg": o.msg}))
            .collect::<Vec<_>>(),
    })
}

#[cfg(target_arch = "wasm32")]
mod exports {
    use std::cell::RefCell;

    use super::Playground;
    // The buffer table and the frame, which `beck-wasm`'s module hands its page too.
    use beck_frame::{error, reply, request, reserve, take};

    thread_local! {
        /// One playground per module instance, because one module instance is one tab.
        static STATE: RefCell<Playground> = RefCell::new(Playground::new());
    }

    #[allow(unsafe_code)] // the export attribute, and nothing else — see the crate docs
    #[no_mangle]
    pub extern "C" fn beck_alloc(len: i32) -> i32 {
        reserve(len.max(0) as usize)
    }

    #[allow(unsafe_code)] // the export attribute, and nothing else — see the crate docs
    #[no_mangle]
    pub extern "C" fn beck_free(ptr: i32) {
        take(ptr);
    }

    #[allow(unsafe_code)] // the export attribute, and nothing else — see the crate docs
    #[no_mangle]
    pub extern "C" fn beck_call(ptr: i32, len: i32) -> i32 {
        let Some(bytes) = request(ptr, len) else {
            return error("no such buffer");
        };
        let request: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => return error(e),
        };
        STATE.with(|s| match super::dispatch(&mut s.borrow_mut(), &request) {
            Ok(v) => reply(v),
            Err(why) => error(why),
        })
    }
}
