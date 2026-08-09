//! The Mode B client: a local copy of the state, the same fold, and `seq` reconciliation.
//!
//! [`docs/10-decisions.md`](../../../../../docs/10-decisions.md) D5 describes what this is for:
//!
//! > Interactions are instant because the browser applies the expected event to its local copy
//! > *speculatively* — legitimate because it runs the *same pure fold* the server runs; when the
//! > server's authoritative answer arrives (tagged with its position `seq` in the log), the guess
//! > is confirmed or corrected. This is why clients mint ids: "browsers here are replicas, not
//! > terminals."
//!
//! Everything here is a consequence of taking "the same pure fold" literally. The `Core` for
//! `validate`, the fold and the view arrives in a [`Bundle`] — the compiler's own output, not a
//! port of it — and is executed by a [`beck_core::Backend`]. There is no second implementation of
//! anything to keep in step, which is why the differential gate in `beck-cli/tests/mode_b.rs` can
//! assert *equality* between what the server renders and what this renders rather than similarity.
//!
//! # Two states, and why
//!
//! * **confirmed** — the accumulator at `seq`, exactly as the server has it. Only a data patch
//!   moves it ([`beck_core::delta`]).
//! * **optimistic** — confirmed, plus every pending command's events folded on top. It is
//!   *derived*, never stored: a guess that is kept would be a guess that has to be un-kept, and
//!   recomputing it is a fold over the handful of commands in flight.
//!
//! Reconciliation is therefore not an operation. When a data patch moves `seq` past a command's
//! acknowledged position, that command stops being pending and the same derivation produces the
//! corrected page. A guess that was right leaves the page unchanged and costs one empty patch; a
//! guess that was wrong is corrected by the same code path, and neither is a special case.
//!
//! # What the client refuses on its own
//!
//! `validate` is in the bundle, so a command the server would refuse is refused here first, with
//! no round trip and the program's own `Rejection` value as the reason. That is not a duplicated
//! rule: it is the same rule, run early. The server still runs it — the client's copy is advice to
//! the person typing, and authority stays at the chokepoint (§3.5).

use std::sync::Arc;

use beck_core::backend::{Backend, Callable};
use beck_core::bundle::Bundle;
use beck_core::delta;
use beck_core::diff;
use beck_core::edge;
use beck_core::html::Html;
use beck_core::Value;

/// A command this client has sent and the server has not yet reflected in the state it holds.
#[derive(Clone, Debug)]
struct Pending {
    /// The idempotency key the client minted — the same one the socket frame carries, so an ack
    /// can be matched to it after a reconnect (§4.3).
    id: String,
    command: Value,
    /// The client's guess at the envelope's timestamp. It is usually wrong by a few milliseconds
    /// and it does not matter: the authoritative fold happens on the server with the sequencer's
    /// clock, and this state is replaced rather than merged when that arrives.
    at: i64,
    /// The log position the server gave it, once it says so. Until then the command is in flight;
    /// after `seq` reaches this, the confirmed state already includes it.
    acked: Option<u64>,
}

/// What a proposal did.
#[derive(Clone, Debug, PartialEq)]
pub enum Proposed {
    /// The client's own `validate` accepted it. The page has been re-rendered against the guess,
    /// and the command should now be sent.
    Accepted { dom: Vec<diff::Op> },
    /// The client's own `validate` refused it, so there is nothing to send. `why` is the program's
    /// `Rejection`, rendered.
    Refused { why: String },
}

/// One Mode B component, running in one browser tab.
pub struct Client {
    bundle: Bundle,
    validate: Callable,
    fold: Callable,
    view: Callable,
    actor: String,
    confirmed: Value,
    seq: u64,
    pending: Vec<Pending>,
    /// What the DOM shows, so a re-render is a patch rather than a rebuild. `None` before the
    /// first render.
    shown: Option<Html>,
}

impl Client {
    /// Load a bundle and evaluate the fold's initial state.
    ///
    /// The initial state is what the client renders before it has heard anything — an offline cold
    /// start (D7), and the reason `init` is in the bundle at all.
    pub fn load(bytes: &[u8], actor: &str) -> Result<Client, String> {
        let bundle = Bundle::from_bytes(bytes).map_err(|e| e.to_string())?;
        // The kernel picks a backend exactly as the server does, through the same seam: a
        // compiling client backend is a different argument here and no change anywhere else
        // (`beck_core::backend`).
        let backend: Arc<dyn Backend> = Arc::new(beck_eval::Evaluator::for_defs(
            bundle.defs.clone().into_iter().collect(),
        ));
        let role = |code, what: &str| {
            backend
                .function(code)
                .map_err(|e| format!("preparing {what}: {e}"))
        };
        let validate = role(&bundle.validate, "`validate`")?;
        let fold = role(&bundle.fold, "the fold")?;
        let view = role(&bundle.view, "the view")?;
        let confirmed = backend
            .constant(&bundle.init)
            .map_err(|e| format!("evaluating the initial state: {e}"))?;

        Ok(Client {
            bundle,
            validate,
            fold,
            view,
            actor: actor.to_string(),
            confirmed,
            seq: 0,
            pending: Vec::new(),
            shown: None,
        })
    }

    pub fn component(&self) -> &str {
        &self.bundle.component
    }

    /// The program the bundle was cut from, by its command channel's content-derived id (§4.3).
    /// A client whose bundle and server disagree here is a stale tab, and it can say so.
    pub fn wire_id(&self) -> &str {
        &self.bundle.wire_id
    }

    pub fn optimistic(&self) -> bool {
        self.bundle.optimistic
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// How many commands are in flight — what a devtools panel calls "pending".
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// What the DOM is showing, as the `Html` value it was patched to. `None` before the first
    /// render. The gate that compares the two modes reads this.
    pub fn showing(&self) -> Option<&Html> {
        self.shown.as_ref()
    }

    /// Adopt the current render as what the DOM already shows, without emitting a patch.
    ///
    /// This is hydration, and it is free for a reason worth stating: the server-rendered document
    /// is `view(state, session)` at some `seq`, and this client holds the same `view` and — once
    /// its first data patch arrives at that same `seq` — the same state. Same function, same
    /// input, same page. Nothing has to be reconciled because nothing can differ.
    ///
    /// The caller is the browser, which knows whether the document it is running in was rendered
    /// by this server at this `seq` (`data-b-seq` on the body). If it was not, it renders normally
    /// and patches.
    pub fn hydrate(&mut self) -> Result<(), String> {
        self.shown = Some(self.render(&self.state()?)?);
        Ok(())
    }

    /// A data patch: the server's account of what changed in the accumulator, up to `seq`.
    pub fn data(&mut self, seq: u64, ops: &[delta::Op]) -> Result<Vec<diff::Op>, String> {
        self.confirmed = delta::apply(&self.confirmed, ops).map_err(|e| e.to_string())?;
        self.seq = seq;
        // A command the confirmed state now includes is not a guess any more. This is the whole of
        // reconciliation: dropping it makes the derived state the server's answer instead of ours.
        self.pending.retain(|p| p.acked.is_none_or(|s| s > seq));
        self.repaint()
    }

    /// Replace the whole state — a fresh subscription, or a reset because the gap was unreachable.
    pub fn reset(&mut self, seq: u64, state: Value) -> Result<Vec<diff::Op>, String> {
        self.confirmed = state;
        self.seq = seq;
        self.pending.retain(|p| p.acked.is_none_or(|s| s > seq));
        self.repaint()
    }

    /// Apply a command speculatively.
    ///
    /// `at` is the browser's clock, supplied rather than read: this crate has no clock, which is
    /// [`docs/44-wave-0-report.md`](../../../../../docs/44-wave-0-report.md)'s rule and also the
    /// only way the kernel stays a pure function of its inputs.
    pub fn propose(&mut self, id: &str, command: &serde_json::Value, at: i64) -> Proposed {
        let command = match self.bundle.command.decode(command) {
            Ok(v) => v,
            Err(why) => return Proposed::Refused { why },
        };
        // Ask the program before pretending: an optimistic UI that shows a card the server is
        // about to refuse is worse than one that waits.
        let state = match self.state() {
            Ok(s) => s,
            Err(why) => return Proposed::Refused { why },
        };
        if let Err(why) = self.decide(&state, &command) {
            return Proposed::Refused { why };
        }
        if !self.bundle.optimistic {
            // The component renders locally but may not guess. The command is still sent; the page
            // moves when the server's data patch arrives.
            return Proposed::Accepted { dom: Vec::new() };
        }
        self.pending.push(Pending {
            id: id.to_string(),
            command,
            at,
            acked: None,
        });
        match self.repaint() {
            Ok(dom) => Proposed::Accepted { dom },
            Err(why) => Proposed::Refused { why },
        }
    }

    /// The server accepted a command and gave it a position.
    ///
    /// This does not move the page: the guess is already on it. What it does is record where the
    /// guess will stop being one, so the next data patch can retire it.
    pub fn settle(&mut self, id: &str, seq: u64) {
        if let Some(p) = self.pending.iter_mut().find(|p| p.id == id) {
            p.acked = Some(seq);
        }
    }

    /// The server refused a command this client accepted.
    ///
    /// The two `validate` calls agreeing is the normal case — same function, same state — and they
    /// disagree exactly when the client's state was behind the server's, which is a race rather
    /// than a bug. Dropping the guess and re-rendering is the correction.
    pub fn refused(&mut self, id: &str) -> Result<Vec<diff::Op>, String> {
        self.pending.retain(|p| p.id != id);
        self.repaint()
    }

    /// The state to render: confirmed, plus every guess still in flight.
    pub fn state(&self) -> Result<Value, String> {
        let mut state = self.confirmed.clone();
        if !self.bundle.optimistic {
            return Ok(state);
        }
        // The server has not assigned these positions yet, so the client numbers them after the
        // last one it knows about. A fold that reads `env.seq` therefore sees a plausible number
        // rather than a wrong one, and the authoritative fold overwrites it either way.
        let mut seq = self.seq;
        for p in &self.pending {
            if p.acked.is_some_and(|s| s <= self.seq) {
                continue;
            }
            let events = match self.decide(&state, &p.command) {
                Ok(events) => events,
                // A command that no longer validates against the state this client now holds is
                // one the server will refuse too. Skipping it here is what makes the page agree
                // with the answer that is coming.
                Err(_) => continue,
            };
            for event in events {
                seq += 1;
                let env = edge::envelope(seq, p.at, &self.actor, event);
                state = (self.fold)(vec![state, env]).map_err(|e| format!("folding: {e}"))?;
            }
        }
        Ok(state)
    }

    /// Render the current state and return the DOM ops that get from what is shown to it.
    pub fn repaint(&mut self) -> Result<Vec<diff::Op>, String> {
        let html = self.render(&self.state()?)?;
        let ops = match &self.shown {
            Some(shown) => diff::diff(shown, &html),
            // Nothing is shown yet, so the whole frame is the patch. `Path` is empty because the
            // subscription's root *is* the frame (`beck_core::diff`).
            None => vec![diff::Op::Replace {
                path: Vec::new(),
                html: html.clone(),
            }],
        };
        self.shown = Some(html);
        Ok(ops)
    }

    fn render(&self, state: &Value) -> Result<Html, String> {
        match (self.view)(vec![state.clone(), edge::session(&self.actor)]) {
            Ok(Value::Html(h)) => Ok((*h).clone()),
            Ok(other) => Err(format!(
                "the view produced {} rather than Html",
                other.display()
            )),
            Err(e) => Err(format!("rendering the view: {e}")),
        }
    }

    /// The program's own chokepoint, run locally: the events a command becomes, or why not.
    fn decide(&self, state: &Value, command: &Value) -> Result<Vec<Value>, String> {
        let proposal = edge::proposal(&self.actor, command.clone());
        let out = (self.validate)(vec![state.clone(), proposal]).map_err(|e| e.to_string())?;
        match out.variant() {
            Some("Ok") => out
                .field("value")
                .and_then(|v| v.as_list().cloned())
                .ok_or_else(|| "validate returned Ok without a list of events".to_string()),
            Some("Err") => Err(out
                .field("error")
                .map(|e| e.display())
                .unwrap_or_else(|| "rejected".into())),
            _ => Err(format!("validate returned {}", out.display())),
        }
    }
}
