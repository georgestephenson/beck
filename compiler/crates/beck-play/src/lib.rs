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
pub mod tab;

pub use analysis::{analyse, Analysis, Section};
pub use tab::Tab;

use serde_json::{json, Value};

/// The page's whole state: a program, once one has been loaded.
///
/// Rung A needs none of this — an analysis is a pure function of a string — so a visitor who never
/// presses *run* never builds one.
#[derive(Default)]
pub struct Playground {
    tab: Option<Tab>,
}

impl Playground {
    pub fn new() -> Playground {
        Playground::default()
    }

    /// The loaded application, for a test that wants to ask it something directly.
    pub fn tab(&mut self) -> Option<&mut Tab> {
        self.tab.as_mut()
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
            let a = analyse(&text("source"));
            Ok(json!({
                "diagnostics": a.diagnostics,
                "errors": a.errors,
                "warnings": a.warnings,
                "runnable": a.runnable,
                "sections": a.sections.iter().map(|s| json!({
                    "id": s.id, "title": s.title, "text": s.text,
                })).collect::<Vec<_>>(),
            }))
        }
        "examples" => Ok(json!(tab::examples()
            .into_iter()
            .map(|(name, source)| json!({"name": name, "source": source}))
            .collect::<Vec<_>>())),

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
                    request.get("seq").and_then(|s| s.as_u64()),
                ))),
                "command" => Ok(outgoing(tab.command(
                    &text("sub"),
                    &text("id"),
                    request.get("command").unwrap_or(&Value::Null),
                ))),
                "history" => Ok(json!({"head": tab.head(), "events": tab.history()})),
                "at" => Ok(json!({
                    "seq": number("seq"),
                    "html": tab.page_at(number("seq").max(0) as u64, &text("actor"))?.render(),
                })),
                "rendered" => Ok(json!({"html": tab.rendered(&text("actor"))?})),
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
    use std::collections::BTreeMap;

    use super::Playground;

    thread_local! {
        /// Buffers this module has handed the host, by the address of each one's allocation.
        static BUFFERS: RefCell<BTreeMap<i32, Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
        /// One playground per module instance, because one module instance is one tab.
        static STATE: RefCell<Playground> = const { RefCell::new(Playground { tab: None }) };
    }

    fn reserve(len: usize) -> i32 {
        let mut buffer = vec![0u8; len];
        let ptr = buffer.as_mut_ptr() as i32;
        BUFFERS.with(|b| b.borrow_mut().insert(ptr, buffer));
        ptr
    }

    fn take(ptr: i32) -> Option<Vec<u8>> {
        BUFFERS.with(|b| b.borrow_mut().remove(&ptr))
    }

    /// Hand the host a length-prefixed response and keep it alive until `beck_free`.
    fn respond(body: Vec<u8>) -> i32 {
        let mut framed = Vec::with_capacity(4 + body.len());
        framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
        framed.extend_from_slice(&body);
        let ptr = framed.as_mut_ptr() as i32;
        BUFFERS.with(|b| b.borrow_mut().insert(ptr, framed));
        ptr
    }

    fn reply(value: serde_json::Value) -> i32 {
        respond(
            serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"error\":\"unencodable\"}".to_vec()),
        )
    }

    fn error(why: impl std::fmt::Display) -> i32 {
        reply(serde_json::json!({ "error": why.to_string() }))
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
        let Some(bytes) = take(ptr) else {
            return error("no such buffer");
        };
        let bytes = &bytes[..(len.max(0) as usize).min(bytes.len())];
        let request: serde_json::Value = match serde_json::from_slice(bytes) {
            Ok(v) => v,
            Err(e) => return error(e),
        };
        STATE.with(|s| match super::dispatch(&mut s.borrow_mut(), &request) {
            Ok(v) => reply(v),
            Err(why) => error(why),
        })
    }
}
