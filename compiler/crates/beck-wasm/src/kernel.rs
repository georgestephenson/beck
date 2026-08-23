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

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_core::backend::{Backend, Callable};
use beck_core::bundle::Bundle;
use beck_core::delta;
use beck_core::diff;
use beck_core::edge;
use beck_core::html::Html;
use beck_core::repr::Repr;
use beck_core::Value;
use serde::{Deserialize, Serialize};

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

/// A client's whole local copy, in a form a browser can put somewhere and read back.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// The program this is a copy *of*. A deployment that changes the command channel's types
    /// changes this, and a snapshot of the old one is refused rather than folded into the new
    /// program (§4.3).
    pub wire: String,
    /// Whose copy it is. The state is the same for everybody in Mode B (§94.2), but the queue is
    /// not: it is this actor's unsent commands.
    pub actor: String,
    pub seq: u64,
    pub state: Repr,
    pub pending: Vec<Queued>,
}

/// A command proposed and not yet confirmed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Queued {
    pub id: String,
    pub command: Repr,
    pub at: i64,
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
    /// It was a **gesture**, not a command: folded into this client's interface state, the page
    /// re-rendered, and **nothing to send** — D30.
    ///
    /// A third outcome rather than an `Accepted` with a flag, because the caller's next step is
    /// different in kind: an accepted command is queued and posted and may still be refused by the
    /// server, and a folded gesture is finished. A boolean on `Accepted` would put the two on one
    /// path and leave it to every caller to remember the difference.
    Folded { dom: Vec<diff::Op> },
}

/// Who this tab is, as the edge would build it.
///
/// The claims travel with the actor rather than being looked up, because this side has no
/// provider to ask: the server verified an ID token and put what it found in the document, and a
/// client that filled in a blank map would render a *different page* than the server did for a
/// program whose view reads them. Mode B's claim is that the client runs the same fold; the same
/// fold over a different `Session` is not the same render.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Viewer {
    pub actor: String,
    #[serde(default)]
    pub claims: BTreeMap<String, String>,
    /// Where this tab is. The one field of the three that changes while the tab is open, because a
    /// route is the browser's own — which is what makes Mode B navigation a local render rather
    /// than a round trip.
    #[serde(default = "root")]
    pub path: String,
}

fn root() -> String {
    beck_core::edge::ROOT.to_string()
}

impl Viewer {
    /// A viewer with a name and nothing else — a program whose view does not read `session.claims`
    /// renders the same page either way, and one that does gets an empty map rather than a wrong
    /// one.
    pub fn named(actor: &str) -> Viewer {
        Viewer {
            actor: actor.to_string(),
            claims: BTreeMap::new(),
            path: root(),
        }
    }

    pub fn claiming<'a>(
        actor: &str,
        claims: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Viewer {
        Viewer {
            actor: actor.to_string(),
            claims: claims
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            path: root(),
        }
    }

    /// The same viewer, somewhere else.
    pub fn at(mut self, path: &str) -> Viewer {
        self.path = path.to_string();
        self
    }
}

/// One Mode B component, running in one browser tab.
pub struct Client {
    bundle: Bundle,
    validate: Callable,
    fold: Callable,
    view: Callable,
    viewer: Viewer,
    confirmed: Value,
    seq: u64,
    pending: Vec<Pending>,
    /// What the DOM shows, so a re-render is a patch rather than a rebuild. `None` before the
    /// first render.
    shown: Option<Shown>,
    /// How many times `view` has been evaluated. A gate asserts on this rather than on a clock,
    /// because "it did not re-render" is a property and "it was fast" is a measurement
    /// (`docs/13` §13.7).
    renders: u64,
    /// The backend's step counter, taken once at load: what this client has executed, for a gate
    /// whose claim is about cost. Held rather than the backend itself, because nothing else here
    /// needs one and a counter cannot be used to run anything.
    steps: Option<Arc<dyn beck_core::backend::Steps>>,
    /// `gestures(step, init)`'s step, prepared — `None` when the page keeps no interface state.
    gestures: Option<Callable>,
    /// D30's client-local accumulator: what this tab's own gestures have folded to.
    ///
    /// **It lives here and nowhere else.** It is not sent, not appended, not snapshotted and not
    /// replayed, so it does not survive this `Client` — which is the construct's whole definition
    /// rather than a limitation of it. A reload starts again from `init`, and a second tab has its
    /// own.
    ///
    /// `Value::Unit` for a page that keeps none: the view's sixth parameter exists whichever, and
    /// one arity per role is what makes that free.
    interface: Value,
}

/// A page, the state it is the page *of*, and the freshness it was rendered at.
///
/// One struct rather than three fields, because the whole of [`Client::repaint`]'s shortcut is
/// that they agree: the same state at the same freshness cannot produce a page different from
/// `html`. Kept apart they could be updated apart, and the failure would be a stale page rather
/// than a compile error.
///
/// `from` holds one extra version of the state alive, which costs the nodes that version does not
/// share with the current one rather than a copy of it — [`Value`] is a pointer and a discriminant,
/// and a fold shares every subtree it did not pass through. The server's `Feed::Data` keeps exactly
/// the same thing for the same reason.
struct Shown {
    html: Html,
    from: Value,
    /// The `Freshness` the page was rendered against. Compared only when the component reads it —
    /// see [`Client::paint`], where skipping the comparison is what keeps `docs/94` §94.12's
    /// shortcut for every program that does not.
    fresh: Value,
    /// D30's interface state the page was rendered against, compared on the same terms: only when
    /// the component keeps any. A gesture moves *this* and neither of the other two, so a guard
    /// that did not hold it would make every gesture render nothing — the panel would open in the
    /// accumulator and stay shut on the screen.
    interface: Value,
}

impl Client {
    /// Load a bundle and evaluate the fold's initial state.
    ///
    /// The initial state is what the client renders before it has heard anything — an offline cold
    /// start (D7), and the reason `init` is in the bundle at all.
    pub fn load(bytes: &[u8], viewer: Viewer) -> Result<Client, String> {
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
        let (gestures, interface) = match &bundle.gestures {
            Some(g) => (
                Some(role(&g.step, "the gesture step")?),
                backend
                    .constant(&g.init)
                    .map_err(|e| format!("evaluating the initial interface state: {e}"))?,
            ),
            None => (None, Value::Unit),
        };

        Ok(Client {
            bundle,
            validate,
            fold,
            view,
            viewer,
            confirmed,
            seq: 0,
            pending: Vec::new(),
            shown: None,
            renders: 0,
            steps: backend.steps(),
            gestures,
            interface,
        })
    }

    pub fn component(&self) -> &str {
        &self.bundle.component
    }

    /// This tab's claims, in the shape [`edge::session`] takes them.
    fn claims(&self) -> impl Iterator<Item = (&str, &str)> {
        self.viewer
            .claims
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
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
        self.shown.as_ref().map(|s| &s.html)
    }

    /// How many times this client has evaluated `view`.
    pub fn renders(&self) -> u64 {
        self.renders
    }

    /// What this client's backend has executed, in the backend's own steps.
    ///
    /// [`renders`](Client::renders)' companion, and the counter a gate reads when the claim is
    /// about *cost* rather than about how many pages were built. A gesture is supposed to skip
    /// `validate` and the derivation the fold performs, and that difference is countable — where
    /// the same claim measured with a clock is a number that depends on what else the machine is
    /// doing ([`docs/13`](../../../../../docs/13-testing.md) §13.7).
    ///
    /// Zero for a backend that does not count, which is not the same as no work.
    pub fn steps(&self) -> u64 {
        self.steps.as_ref().map_or(0, |s| s.taken())
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
        let from = self.state()?;
        let fresh = self.freshness();
        let html = self.render(&from)?;
        self.renders += 1;
        // The server rendered this document against `init` (`B0522`), and so did the render just
        // above: a client that has made no gesture yet holds exactly what the server assumed. That
        // is what keeps hydration free for a page with interface state too.
        self.shown = Some(Shown {
            html,
            from,
            fresh,
            interface: self.interface.clone(),
        });
        Ok(())
    }

    /// Where this client is now.
    ///
    /// The whole of Mode B navigation: the route is a field of the `Session` the view is rendered
    /// against, so moving it and re-rendering *is* the page change — no round trip, no fetch, and
    /// no second rendering path. Navigating to where this client already is renders nothing.
    pub fn navigate(&mut self, path: &str) -> Result<Vec<diff::Op>, String> {
        if self.viewer.path == path {
            return Ok(Vec::new());
        }
        self.viewer.path = path.to_string();
        // Forced, because the short-circuit [`Client::repaint`] takes is about the *state* and the
        // state has not changed here — the session has. It is still a diff against what is on
        // screen, so a route whose page differs in one attribute costs one attribute.
        self.paint(true)
    }

    /// Where this client says it is, which is what its `Session` carries.
    pub fn path(&self) -> &str {
        &self.viewer.path
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
        self.take(seq, state);
        self.repaint()
    }

    /// The same, for a state the DOM is already showing the page of.
    ///
    /// The caller is a browser whose document was server-rendered at this `seq`. Rendering and
    /// *not* patching is the whole of hydration here, and it is sound for [`Client::hydrate`]'s
    /// reason: same view, same state, same page. Returning no ops rather than ops the caller is
    /// expected to drop keeps that decision in one place.
    pub fn adopt(&mut self, seq: u64, state: Value) -> Result<(), String> {
        self.take(seq, state);
        self.hydrate()
    }

    fn take(&mut self, seq: u64, state: Value) {
        self.confirmed = state;
        self.seq = seq;
        self.pending.retain(|p| p.acked.is_none_or(|s| s > seq));
    }

    /// Everything this client would need to be itself again after a reload.
    ///
    /// [`docs/10-decisions.md`](../../../../../docs/10-decisions.md) D7's rung 2 — "a Mode B
    /// component holds a local copy of its state" — is this plus somewhere to put it. What comes
    /// back is the *confirmed* state and the commands still in flight, never the optimistic state:
    /// a guess is derived, and a guess that were restored as a fact could not be corrected.
    ///
    /// `None` when the state holds something unstorable, which the checker makes unreachable
    /// (`B0411`) and which is refused here rather than half-written.
    pub fn snapshot(&self) -> Option<Snapshot> {
        Some(Snapshot {
            wire: self.bundle.wire_id.clone(),
            actor: self.viewer.actor.clone(),
            seq: self.seq,
            state: Repr::of(&self.confirmed).ok()?,
            pending: self
                .pending
                .iter()
                .map(|p| {
                    Some(Queued {
                        id: p.id.clone(),
                        command: Repr::of(&p.command).ok()?,
                        at: p.at,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        })
    }

    /// Come back as that client.
    ///
    /// The queue is what makes this more than a cache: a command proposed with no network is still
    /// pending here, and sending it again after a reconnect is safe because it carries the same
    /// idempotency key the server de-duplicates by (§4.3). Offline tolerance is the fold plus that
    /// key; it needed no new agreement between the two sides.
    pub fn restore(&mut self, snapshot: Snapshot) -> Result<Vec<diff::Op>, String> {
        if snapshot.wire != self.bundle.wire_id {
            return Err(format!(
                "this snapshot is of another program ({} rather than {})",
                snapshot.wire, self.bundle.wire_id
            ));
        }
        if snapshot.actor != self.viewer.actor {
            return Err(format!(
                "this snapshot is another actor's ({} rather than {})",
                snapshot.actor, self.viewer.actor
            ));
        }
        self.confirmed = snapshot.state.to_value();
        self.seq = snapshot.seq;
        self.pending = snapshot
            .pending
            .into_iter()
            .map(|q| Pending {
                id: q.id,
                command: q.command.to_value(),
                at: q.at,
                // Nothing this client was told survives a reload, so every restored command is in
                // flight again. An ack it already had would be re-sent and de-duplicated, which is
                // the cheap end of the trade.
                acked: None,
            })
            .collect();
        self.repaint()
    }

    /// The commands a restored client still owes the server, in order.
    pub fn queued(&self) -> Vec<(String, Repr)> {
        self.pending
            .iter()
            .filter(|p| p.acked.is_none())
            .filter_map(|p| Some((p.id.clone(), Repr::of(&p.command).ok()?)))
            .collect()
    }

    /// Apply a command speculatively.
    ///
    /// `at` is the browser's clock, supplied rather than read: this crate has no clock, which is
    /// [`docs/44-wave-0-report.md`](../../../../../docs/44-wave-0-report.md)'s rule and also the
    /// only way the kernel stays a pure function of its inputs.
    pub fn propose(&mut self, id: &str, command: &serde_json::Value, at: i64) -> Proposed {
        // D30's routing, and it is here rather than in the browser on purpose: the page's handlers
        // carry a constructor and the client decides what kind of thing it is, so `beck-patch.js`
        // goes on naming events and never commands. `B0524` is what makes the decision total —
        // the two unions may not share a variant, so at most one decoder can claim a tag.
        if self.is_gesture(command) {
            return self.gesture(command);
        }
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

    /// Apply one gesture to this client's interface state — D30's non-durable fold.
    ///
    /// **Compare [`Client::propose`] line for line, because the differences are the decision.**
    /// A command is decoded against the wire schema, put to `validate`, recorded as pending, sent
    /// up the socket, and settled or refused by the server. A gesture is decoded, folded, and
    /// painted. Nothing is validated, because nothing is being asked for; nothing is pending,
    /// because there is nobody to hear back from; nothing is sent, because there is nowhere for it
    /// to go. The log is not merely left alone — it is not on this path at all.
    ///
    /// It returns the DOM patch the same way, so a gesture and a command render identically; what
    /// a gesture skips is the state derivation and `validate`, which is about a fifth of an
    /// interaction and stays about a fifth as the board grows — the render dominates and both
    /// paths pay it (`measure_mode_b.rs::what_a_gesture_costs_against_a_command`, 1.21× at 100
    /// cards and 1.18× at 1000). The saving is not the point of the construct; what leaves the tab
    /// is, and for a gesture nothing does.
    /// Whether a payload from the page names a variant of the gesture union.
    ///
    /// The tag alone decides. `B0524` refuses a program whose two unions share a variant name, so
    /// a tag belongs to at most one of them and this cannot be a guess.
    fn is_gesture(&self, payload: &serde_json::Value) -> bool {
        let Some(g) = &self.bundle.gestures else {
            return false;
        };
        payload
            .get("c")
            .and_then(|c| c.as_str())
            .is_some_and(|tag| g.schema.variants.iter().any(|v| v.name == tag))
    }

    pub fn gesture(&mut self, gesture: &serde_json::Value) -> Proposed {
        let Some(step) = &self.gestures else {
            return Proposed::Refused {
                why: "this page keeps no interface state".to_string(),
            };
        };
        // The gesture union's own decoder, never the command one. Two schemas is what keeps
        // §3.5's write surface closed: nothing a client sends can be decoded as both.
        let schema = &self
            .bundle
            .gestures
            .as_ref()
            .expect("a prepared step implies a schema")
            .schema;
        let value = match schema.decode(gesture) {
            Ok(v) => v,
            Err(why) => return Proposed::Refused { why },
        };
        match step(vec![self.interface.clone(), value]) {
            Ok(next) => self.interface = next,
            Err(e) => {
                return Proposed::Refused {
                    why: format!("applying a gesture: {e}"),
                }
            }
        }
        match self.repaint() {
            Ok(dom) => Proposed::Folded { dom },
            Err(why) => Proposed::Refused { why },
        }
    }

    /// This client's interface state — what its own gestures have folded to.
    ///
    /// `Value::Unit` for a page that keeps none. Public so a gate can assert on it directly:
    /// "the page moved" and "the accumulator moved" are two claims and a test should be able to
    /// make each.
    pub fn interface(&self) -> &Value {
        &self.interface
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
                let env = edge::envelope(seq, p.at, &self.viewer.actor, event);
                state = (self.fold)(vec![state, env]).map_err(|e| format!("folding: {e}"))?;
            }
        }
        Ok(state)
    }

    /// Render the current state and return the DOM ops that get from what is shown to it.
    ///
    /// A state that has not moved is not rendered again. `view` is a pure function of the state
    /// and the session, and a client's session is fixed for its lifetime — so the same state is
    /// the same page, and rendering it to diff it against itself is work with a known answer.
    ///
    /// That is the *common* case rather than a corner, and it is what optimism costs when it is
    /// right: a client proposes, renders its guess, and the server's data patch then confirms
    /// exactly that guess. The state derived after the confirmation equals the state the guess was
    /// derived from — that equality is what makes the optimism correct — so without this an
    /// interaction pays for two renders and the second is guaranteed to produce no patch (§94.12).
    pub fn repaint(&mut self) -> Result<Vec<diff::Op>, String> {
        self.paint(false)
    }

    /// The render behind [`Client::repaint`], with the short-circuit made a decision of the caller.
    ///
    /// `force` exists for exactly one caller — [`Client::navigate`] — and the reason is that the
    /// guard below asks whether the *state* moved. Everything else that repaints moves the state;
    /// a navigation moves the session instead, and a guard that cannot tell the two apart would
    /// make a route change the one interaction Mode B renders nothing for.
    fn paint(&mut self, force: bool) -> Result<Vec<diff::Op>, String> {
        let from = self.state()?;
        let fresh = self.freshness();
        // Two comparisons, and the second is asked only of a component that reads the answer.
        //
        // A confirmation is exactly the case where the state does not move and the freshness does:
        // the guess was right, so the derived state before and after are equal, and `Pending(1)`
        // becomes `Confirmed`. A page that renders "saving…" has to be repainted for that; a page
        // that does not read `freshness()` renders the same bytes either way, and repainting it
        // would hand back the second render `docs/94` §94.12 removed — 150× the cost of a
        // confirmation, for every program in the tree, to show nobody anything.
        //
        // The third comparison is D30's, and it is the case the other two cannot see: a gesture
        // moves the interface state while the derived state and the freshness both stand still. It
        // is asked only of a page that keeps interface state, for the reason freshness is asked
        // only of a page that reads it — a program with no `gestures` compares `Unit` to `Unit`
        // forever, so the shortcut costs it nothing.
        let same = self.shown.as_ref().is_some_and(|s| {
            s.from == from
                && (!self.bundle.reads_freshness || s.fresh == fresh)
                && (self.gestures.is_none() || s.interface == self.interface)
        });
        if !force && same {
            return Ok(Vec::new());
        }
        let html = self.render(&from)?;
        self.renders += 1;
        let ops = match &self.shown {
            Some(shown) => diff::diff(&shown.html, &html),
            // Nothing is shown yet, so the whole frame is the patch. `Path` is empty because the
            // subscription's root *is* the frame (`beck_core::diff`).
            None => vec![diff::Op::Replace {
                path: Vec::new(),
                html: html.clone(),
            }],
        };
        self.shown = Some(Shown {
            html,
            from,
            fresh,
            interface: self.interface.clone(),
        });
        Ok(ops)
    }

    /// §3.7's freshness dimension, for the state this client is about to render.
    ///
    /// `Pending(n)` counts the commands **in flight**: proposed here and not yet reflected in the
    /// state the server has confirmed. That is deliberately the same set [`Client::in_flight`]
    /// reports and not the narrower "guesses that survived re-validation" — a command whose events
    /// [`Client::state`] now skips is one the server is about to refuse, and a person watching a
    /// spinner is waiting on the answer either way. A client that may not guess (`optimistic`
    /// false) shows the server's state and is therefore `Confirmed`, whatever it has sent.
    ///
    /// This is the only implementation in the project that can return anything but `Confirmed`.
    /// Every other renderer — the server, `beck test`, a read model — goes through
    /// [`beck_core::edge::confirmed`], because what they hold *is* the log.
    fn freshness(&self) -> Value {
        if !self.bundle.optimistic {
            return edge::confirmed();
        }
        edge::freshness(
            self.pending
                .iter()
                .filter(|p| p.acked.is_none_or(|s| s > self.seq))
                .count(),
        )
    }

    /// The component's `view`, over a state — the page this client would show for it.
    ///
    /// Public because it is half of what an interaction costs and the half that grows with the
    /// state, so a measurement that cannot call it separately cannot say which half is which
    /// (`measure_mode_b.rs`).
    pub fn render(&self, state: &Value) -> Result<Html, String> {
        match (self.view)(vec![
            state.clone(),
            edge::session(&self.viewer.actor, self.claims(), &self.viewer.path),
            // Empty, and the compiler is what makes that unobservable: a component that reads
            // `presence` is refused Mode B (`B0516`), because who is connected is a fact the
            // server holds about its own sockets and is in neither the accumulator nor the log.
            edge::presence([]),
            // Empty for the same reason and by the same rule: a component that reads `awareness`
            // is refused Mode B (`B0521`), so this argument is unobservable here too.
            edge::no_awareness(),
            // The one edge value this side answers and the server cannot (`B0518`).
            self.freshness(),
            // D30's interface state, and the second thing only this side has (`B0522`). The server
            // renders `init` here because it has received no gestures; this client renders what
            // its own have folded to.
            self.interface.clone(),
        ]) {
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
        let proposal = edge::proposal(
            &self.viewer.actor,
            self.claims(),
            &self.viewer.path,
            command.clone(),
        );
        let out = (self.validate)(vec![state.clone(), proposal]).map_err(|e| e.to_string())?;
        match out.variant() {
            Some("Ok") => out
                .field("value")
                .and_then(|v| v.as_list().map(|xs| xs.to_vec()))
                .ok_or_else(|| "validate returned Ok without a list of events".to_string()),
            Some("Err") => Err(out
                .field("error")
                .map(|e| e.display())
                .unwrap_or_else(|| "rejected".into())),
            _ => Err(format!("validate returned {}", out.display())),
        }
    }
}
