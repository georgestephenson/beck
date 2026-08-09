//! Mode B's client kernel — the component's slice, executed where the person is.
//!
//! [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.1 calls for "the
//! component's pure code compiled to WASM … fine-grained signal graph, local speculative fold +
//! `seq`-based reconciliation". This crate is the executable half of that: a
//! `wasm32-unknown-unknown` module that loads a [`beck_core::bundle::Bundle`], holds the
//! accumulator, folds commands optimistically and emits the same DOM patch ops the thin client
//! already applies.
//!
//! # The kernel interprets; it does not compile
//!
//! The bundle carries `Core`, and this executes it with [`beck_eval`] — the same backend the
//! server uses, compiled to a different target. What §5.1 describes is codegen ("GC proposal where
//! available; Perceus-style refcounting fallback"), and that is a backend rather than a client:
//! when one exists, it arrives through [`beck_core::backend::Backend`] and nothing here changes —
//! not the bundle, not the protocol, not the reconciliation.
//! [`docs/adr/0022`](../../../../docs/adr/0022-mode-b-ships-the-backend-it-has.md) is that
//! decision and its cost, which is measured rather than estimated: the kernel is the same size for
//! every program, so it is a fixed download and the *component* is the small one.
//!
//! # The boundary
//!
//! There is no `wasm-bindgen` and no generated glue. The module exports four functions taking and
//! returning `i32`, and everything else crosses as bytes in linear memory:
//!
//! | Export | What it does |
//! |---|---|
//! | `beck_alloc(len) -> ptr` | reserve `len` bytes for the host to write into |
//! | `beck_free(ptr)` | release a buffer, whichever side allocated it |
//! | `beck_load(ptr, len) -> ptr` | load a bundle; the result is a JSON response |
//! | `beck_call(ptr, len) -> ptr` | one JSON request, one JSON response |
//!
//! A returned pointer addresses a little-endian `u32` length followed by that many bytes.
//!
//! **No `unsafe` code.** Nothing here dereferences a pointer, which takes one idea: a buffer is a
//! `Vec<u8>` this module keeps in a table keyed by the address of its own heap allocation. The
//! host writes through linear memory — that is what linear memory is — and Rust reads its own
//! `Vec` back out of the table by the address it handed out. Nothing is reconstructed from an
//! integer, so there is no `unsafe` block, no `unsafe fn` and no raw-pointer read anywhere in the
//! crate.
//!
//! The four `#[allow(unsafe_code)]` below are on `#[no_mangle]`, which rustc classifies as unsafe
//! because two libraries exporting one symbol is undefined at link time. A module that exports
//! nothing cannot be called, so the attribute is the boundary itself; `Cargo.toml` says why the
//! crate denies rather than forbids, and `mode_b.rs` gates the extent of the exception.

pub mod kernel;

pub use kernel::{Client, Proposed};

#[cfg(target_arch = "wasm32")]
mod exports {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use super::kernel::Client;

    thread_local! {
        /// Buffers this module has handed the host, by the address of each one's allocation.
        ///
        /// Moving a `Vec` moves three words and not the bytes, so the address stays the one that
        /// was returned. WebAssembly is single-threaded here, which is what makes a thread-local
        /// the whole of the synchronisation story.
        static BUFFERS: RefCell<BTreeMap<i32, Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
        /// The loaded component. One per module instance, because one module instance is one tab.
        static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) };
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
        // The frame is what the host reads, so the address that identifies it is the frame's own.
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

    /// Load a bundle. The request is the bundle's bytes with the actor's name on the front, as
    /// `<u32 len><actor><bundle>`.
    #[allow(unsafe_code)] // the export attribute, and nothing else — see the crate docs
    #[no_mangle]
    pub extern "C" fn beck_load(ptr: i32, len: i32) -> i32 {
        let Some(bytes) = take(ptr) else {
            return error("no such buffer");
        };
        let bytes = &bytes[..(len.max(0) as usize).min(bytes.len())];
        if bytes.len() < 4 {
            return error("a load needs an actor and a bundle");
        }
        let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if bytes.len() < 4 + n {
            return error("the actor is longer than the request");
        }
        let actor = String::from_utf8_lossy(&bytes[4..4 + n]).to_string();
        match Client::load(&bytes[4 + n..], &actor) {
            Ok(client) => {
                let info = serde_json::json!({
                    "component": client.component(),
                    "wire": client.wire_id(),
                    "optimistic": client.optimistic(),
                });
                CLIENT.with(|c| *c.borrow_mut() = Some(client));
                reply(info)
            }
            Err(why) => error(why),
        }
    }

    /// One request, one response. See [`crate::kernel`] for what each op means.
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
        CLIENT.with(|c| match c.borrow_mut().as_mut() {
            Some(client) => match super::dispatch(client, &request) {
                Ok(v) => reply(v),
                Err(why) => error(why),
            },
            None => error("no bundle is loaded"),
        })
    }
}

/// One request against a loaded client.
///
/// Split out of the export so it can be driven from a native test: the wasm boundary is bytes and
/// pointers, and none of the behaviour worth gating lives there.
pub fn dispatch(
    client: &mut Client,
    request: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let op = request
        .get("op")
        .and_then(|o| o.as_str())
        .ok_or("a request needs an `op`")?;
    let seq = || request.get("seq").and_then(|s| s.as_u64()).unwrap_or(0);
    let id = || {
        request
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string()
    };
    fn dom(ops: Vec<beck_core::diff::Op>, seq: u64) -> serde_json::Value {
        serde_json::json!({
            "dom": ops.iter().map(beck_core::diff::Op::to_wire).collect::<Vec<_>>(),
            "seq": seq,
        })
    }
    match op {
        "info" => Ok(serde_json::json!({
            "component": client.component(),
            "wire": client.wire_id(),
            "optimistic": client.optimistic(),
            "seq": client.seq(),
            "pending": client.in_flight(),
        })),
        "hydrate" => {
            client.hydrate()?;
            Ok(serde_json::json!({ "dom": [], "seq": client.seq() }))
        }
        "render" => client.repaint().map(|ops| dom(ops, client.seq())),
        "data" => {
            let ops: Vec<beck_core::delta::Op> =
                serde_json::from_value(request.get("ops").cloned().unwrap_or_default())
                    .map_err(|e| format!("a data patch that will not decode: {e}"))?;
            client.data(seq(), &ops).map(|ops| dom(ops, client.seq()))
        }
        "reset" => {
            let repr: beck_core::repr::Repr =
                serde_json::from_value(request.get("state").cloned().unwrap_or_default())
                    .map_err(|e| format!("a state that will not decode: {e}"))?;
            // `adopt` is the browser saying "the document already shows this state's page", which
            // it knows from `data-b-seq` and the server cannot.
            if request.get("adopt").and_then(|a| a.as_bool()) == Some(true) {
                client.adopt(seq(), repr.to_value())?;
                return Ok(serde_json::json!({ "dom": [], "seq": client.seq() }));
            }
            client
                .reset(seq(), repr.to_value())
                .map(|ops| dom(ops, client.seq()))
        }
        "propose" => {
            let command = request.get("command").cloned().unwrap_or_default();
            let at = request.get("at").and_then(|a| a.as_i64()).unwrap_or(0);
            let outcome = client.propose(&id(), &command, at);
            let seq = client.seq();
            Ok(match outcome {
                Proposed::Accepted { dom: ops } => {
                    let mut out = dom(ops, seq);
                    out["accepted"] = serde_json::Value::Bool(true);
                    out
                }
                Proposed::Refused { why } => {
                    serde_json::json!({ "accepted": false, "why": why })
                }
            })
        }
        "settle" => {
            client.settle(&id(), seq());
            Ok(serde_json::json!({ "dom": [], "seq": client.seq() }))
        }
        "refused" => client.refused(&id()).map(|ops| dom(ops, client.seq())),
        other => Err(format!("`{other}` is not a request this kernel answers")),
    }
}
