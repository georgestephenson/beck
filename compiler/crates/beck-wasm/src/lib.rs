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

pub use kernel::{Client, Proposed, Viewer};

#[cfg(target_arch = "wasm32")]
mod exports {
    use std::cell::RefCell;

    use super::kernel::{Client, Viewer};
    // The buffer table and the frame, which `beck-play`'s module hands its page too.
    use beck_frame::{error, reply, request, reserve, take};

    thread_local! {
        /// The loaded component. One per module instance, because one module instance is one tab.
        static CLIENT: RefCell<Option<Client>> = const { RefCell::new(None) };
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

    /// Load a bundle. The request is the bundle's bytes with the viewer on the front, as
    /// `<u32 len><viewer json><bundle>`.
    ///
    /// The viewer is JSON rather than a bare name because it carries the claims as well — the
    /// `Session` the view is rendered against is the actor *and* what the provider said about them
    /// ([`crate::kernel::Viewer`]).
    #[allow(unsafe_code)] // the export attribute, and nothing else — see the crate docs
    #[no_mangle]
    pub extern "C" fn beck_load(ptr: i32, len: i32) -> i32 {
        let Some(bytes) = request(ptr, len) else {
            return error("no such buffer");
        };
        if bytes.len() < 4 {
            return error("a load needs a viewer and a bundle");
        }
        let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if bytes.len() < 4 + n {
            return error("the viewer is longer than the request");
        }
        let viewer = match serde_json::from_slice::<Viewer>(&bytes[4..4 + n]) {
            Ok(v) => v,
            Err(e) => return error(format!("the viewer is not readable: {e}")),
        };
        match Client::load(&bytes[4 + n..], viewer) {
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
        let Some(bytes) = request(ptr, len) else {
            return error("no such buffer");
        };
        let request: serde_json::Value = match serde_json::from_slice(&bytes) {
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
            "path": client.path(),
        })),
        "hydrate" => {
            client.hydrate()?;
            Ok(serde_json::json!({ "dom": [], "seq": client.seq() }))
        }
        "render" => client.repaint().map(|ops| dom(ops, client.seq())),
        // The route the browser is on now. Not carried in a snapshot: after a reload the URL is
        // the browser's own answer to "where am I", and a restored one could disagree with it.
        "nav" => {
            let path = request
                .get("path")
                .and_then(|p| p.as_str())
                .ok_or("a nav needs a `path`")?
                .to_string();
            client.navigate(&path).map(|ops| dom(ops, client.seq()))
        }
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
                // D30: folded here, and the browser is told so rather than left to infer it. The
                // flag is what stops `beck-mode-b.js` queueing and posting something the server
                // has no decoder for and no business seeing.
                Proposed::Folded { dom: ops } => {
                    let mut out = dom(ops, seq);
                    out["accepted"] = serde_json::Value::Bool(true);
                    out["local"] = serde_json::Value::Bool(true);
                    out
                }
            })
        }
        // What a browser stores so that a reload is not a fresh start (D7 rung 2).
        "snapshot" => Ok(serde_json::json!({
            "snapshot": client.snapshot(),
            "queued": client
                .queued()
                .into_iter()
                .map(|(id, command)| serde_json::json!({"id": id, "command": command}))
                .collect::<Vec<_>>(),
        })),
        "restore" => {
            let snapshot: crate::kernel::Snapshot =
                serde_json::from_value(request.get("snapshot").cloned().unwrap_or_default())
                    .map_err(|e| format!("a snapshot that will not decode: {e}"))?;
            let ops = client.restore(snapshot)?;
            let mut out = dom(ops, client.seq());
            out["queued"] = serde_json::json!(client
                .queued()
                .into_iter()
                .map(|(id, command)| serde_json::json!({"id": id, "command": command}))
                .collect::<Vec<_>>());
            Ok(out)
        }
        "settle" => {
            client.settle(&id(), seq());
            Ok(serde_json::json!({ "dom": [], "seq": client.seq() }))
        }
        "refused" => client.refused(&id()).map(|ops| dom(ops, client.seq())),
        other => Err(format!("`{other}` is not a request this kernel answers")),
    }
}
