//! Rung B: the whole application, in the tab.
//!
//! [`docs/17-playground.md`](../../../../../docs/17-playground.md) §17.2 is a table of three
//! implementations of the same runtime interfaces, and the third one is a browser:
//!
//! | Runtime interface | Production | DST | **Playground** |
//! |---|---|---|---|
//! | Clock | OS | simulated | supplied by the page |
//! | Network | Tokio/websocket | simulated | `MessageChannel` |
//! | Log storage | Postgres/redb | simulated | an array in the worker |
//!
//! What is *not* a third implementation is the application: [`mod@beck_host::sequence`] decides a batch
//! here exactly as it does in `beck run`, `beck_host::Runtime` runs the program's own `validate`,
//! fold and `view`, [`mod@beck_core::diff`] produces the patch, and the client applies it with the same
//! `beck-patch.js` a deployed Beck application serves. The tab holds the parts that are genuinely
//! about *this* host — an array for a log, a call for a socket, no threads — and nothing else.
//!
//! # What this is not
//!
//! Not a second sequencer: the batch is always one command, because a `postMessage` is answered
//! before the next one is read, so there is nothing to group-commit. Group commit is a *latency*
//! optimisation for a queue, and a tab has no queue.
//!
//! Not a store: [`Tab::records`] hands the log out as the bytes a store writes and [`Tab::restore`]
//! reads them back, and where those bytes are kept is the page's business — IndexedDB, in the
//! playground ([`docs/103`](../../../../../docs/103-playground-phase-3-report.md)). The log is still
//! a `Vec`; what changed is that it can be handed over.
//!
//! # Both modes
//!
//! A subscription carries DOM patches or data patches, and which one is the program's rendering
//! mode — the same single branch `beck_rt::session` makes. Mode A diffs two *pages* and sends
//! [`mod@beck_core::diff`] ops; Mode B diffs two *states* and sends [`beck_core::delta`] ops, and the
//! rendering happens in the iframe, in Mode B's kernel, from the bundle [`Tab::bundle`] hands over.

use std::collections::BTreeMap;

use beck_core::delta;
use beck_core::diff::{diff, Op};
use beck_core::render::Mode;
use beck_core::repr::Repr;
use beck_core::{Html, Placed, Value};
use beck_host::protocol::{Resumption, ServerMsg};
use beck_host::sequence::{Proposal, Seen, Untimed};
use beck_host::{At, Decision, Envelope, Instant, Runtime, Seq};

/// One subscription, as the tab holds it.
///
/// The server's equivalent is a task holding an engine and the last page it sent. This holds the
/// same two facts and no task: a tab advances every subscription when the state moves, in the
/// order they subscribed, because a `postMessage` is the only thing that ever happens.
struct Subscriber {
    actor: String,
    /// Where this client is. A route is not evidence of anything and nothing verifies it — it is a
    /// string the browser sent — but it is part of the `Session` the program's own `validate` and
    /// `view` see, so a tab that ignored it would be running the program against a session no
    /// deployment would build ([`docs/100`](../../../../../docs/100-client-polish-report.md)).
    path: String,
    /// What this client is holding, and therefore what the next frame is the difference from —
    /// which is the whole of a subscription, and the reason an idle subscriber costs no bytes.
    ///
    /// The position it reflects is deliberately *not* here: it is always this tab's head, because a
    /// tab advances every subscription before it answers anything else. A server holds one per
    /// subscription because its subscriptions render concurrently.
    shown: Shown,
}

impl Subscriber {
    /// Who is asking and where they are, in the shape every render path takes.
    fn at(&self) -> At<String> {
        At {
            who: self.actor.clone(),
            path: self.path.as_str().into(),
        }
    }
}

/// The last thing sent to a subscriber, in whichever currency its mode trades.
///
/// The variants are deliberately different sizes, exactly as `beck_rt::session`'s `Feed` is: a Mode
/// A subscription holds a page and a Mode B one holds the accumulator. Evening them out would hide
/// the asymmetry that is the mode's whole point.
#[allow(clippy::large_enum_variant)]
enum Shown {
    Dom(Html),
    Data(Value),
}

/// A frame on its way out of the tab, addressed the way the worker will route it.
pub struct Outgoing {
    pub sub: String,
    pub msg: serde_json::Value,
}

/// The application: one log, one accumulator, N subscriptions. No I/O of any kind.
pub struct Tab {
    runtime: Runtime,
    /// The log. An array, because that is what a browser tab has — and because the *contract*
    /// (dense `seq`, one writer, ordered replay) is what the folds depend on, not the substrate.
    log: Vec<Envelope>,
    state: Value,
    seen: Seen,
    subs: Vec<(String, Subscriber)>,
    /// Milliseconds since the epoch, supplied by the page. §3.7's "the one place time enters" is
    /// the merge point, and in a tab the merge point is handed the number rather than reading it:
    /// `std::time::SystemTime::now()` on `wasm32-unknown-unknown` is a panic, and a clock that is
    /// supplied is what F11 asked for anyway.
    now: i64,
}

impl Tab {
    /// Prepare a program to run in the tab.
    pub fn load(placed: Placed) -> Result<Tab, String> {
        let backend = beck_eval::backend(&placed);
        let runtime = Runtime::new(placed, backend).map_err(|e| e.to_string())?;
        let state = runtime.initial_state().map_err(|e| e.to_string())?;
        Ok(Tab {
            runtime,
            log: Vec::new(),
            state,
            // The server's default is 16,384 recent ids; a tab has one person in it, and the
            // memory is what makes a repeated command idempotent rather than a second card.
            seen: Seen::new(1024),
            subs: Vec::new(),
            now: 0,
        })
    }

    /// The page's clock reading, for the next command's envelope.
    pub fn set_now(&mut self, millis: i64) {
        self.now = millis;
    }

    pub fn head(&self) -> Seq {
        self.log.last().map(|e| e.seq).unwrap_or(0)
    }

    pub fn wire_id(&self) -> &str {
        self.runtime.wire_id()
    }

    /// Which rendering mode this program's page is in — and therefore which residue the client
    /// iframe loads and which currency its subscription trades in.
    pub fn mode(&self) -> Mode {
        self.runtime.placed().render.mode
    }

    /// Subscribe, or resume — the `hello` frame, answered.
    ///
    /// The resumption rule is [`beck_host::protocol`]'s, not a second one: absent means "I hold
    /// nothing", `Some(n)` means "I hold the frame as of n", and a position this log cannot reach
    /// is a reset that says so.
    pub fn hello(
        &mut self,
        sub: &str,
        actor: &str,
        path: &str,
        from: Option<Seq>,
    ) -> Vec<Outgoing> {
        let head = self.head();
        let how = match from {
            None => Resumption::Fresh,
            Some(n) if n > head => Resumption::Reset { from: n },
            Some(n) => Resumption::Resumed {
                from: n,
                replayed: head - n,
            },
        };

        // In the roster *before* the first render, so a page that reads `presence()` counts the
        // client it is being rendered for. `beck_rt::session` joins on the same line, for the same
        // reason.
        self.subs.retain(|(id, _)| id != sub);
        let path = if path.is_empty() {
            beck_core::edge::ROOT.to_string()
        } else {
            path.to_string()
        };
        self.subs.push((
            sub.to_string(),
            Subscriber {
                actor: actor.to_string(),
                path,
                shown: match self.mode() {
                    Mode::Server => Shown::Dom(Html::text("")),
                    Mode::Client => Shown::Data(Value::Unit),
                },
            },
        ));

        let first = match self.first_frame(sub, how) {
            Ok(frame) => frame,
            Err(why) => return vec![self.error(sub, &why)],
        };

        let mut out = vec![Outgoing {
            sub: sub.to_string(),
            msg: ServerMsg::welcome(sub, head, how),
        }];
        if let Some(msg) = first {
            out.push(Outgoing {
                sub: sub.to_string(),
                msg,
            });
        }
        // Somebody arriving moves every page that reads `presence()`, and nobody else's. The
        // server learns this from a watch its subscriptions select on; a tab learns it because it
        // is the thing that just changed the roster.
        out.extend(
            self.advance("")
                .into_iter()
                .filter(|frame| frame.sub != sub),
        );
        out
    }

    /// A command from one client: the merge point, and then every subscription.
    ///
    /// The ack and the frame are different facts and both are sent — §18.5 item 1, learned the hard
    /// way in Phase 0 and re-learned by anybody who waits for "the patch for my command" on a
    /// command whose net effect their own view does not show.
    pub fn command(&mut self, sub: &str, id: &str, command: &serde_json::Value) -> Vec<Outgoing> {
        let Some((_, subscriber)) = self.subs.iter().find(|(s, _)| s == sub) else {
            return vec![Outgoing {
                sub: sub.to_string(),
                msg: ServerMsg::nack(id, "no such subscription"),
            }];
        };
        let at = subscriber.at();

        let decoded = match self.runtime.decode_command(command) {
            Ok(v) => v,
            Err(e) => {
                return vec![Outgoing {
                    sub: sub.to_string(),
                    msg: ServerMsg::nack(id, &e.to_string()),
                }]
            }
        };

        let base = self.head();
        let decided = beck_host::sequence(
            &self.runtime,
            &self.state,
            base,
            &mut self.seen,
            vec![Proposal {
                id: id.to_string(),
                at: Instant(self.now),
                actor: &at,
                command: decoded,
            }],
            &Untimed,
        );

        let mut out = Vec::new();
        match decided.decisions.first() {
            Some(Decision::Refused { why }) => {
                return vec![Outgoing {
                    sub: sub.to_string(),
                    msg: ServerMsg::nack(id, why),
                }]
            }
            // A retry is acknowledged with the position the first attempt got. The log did not
            // move, so there is nothing to send anybody.
            Some(Decision::Duplicate(at)) => {
                return vec![Outgoing {
                    sub: sub.to_string(),
                    msg: ServerMsg::ack(id, *at),
                }]
            }
            Some(Decision::Accepted { offset }) => {
                let at = base + *offset as u64;
                out.push(Outgoing {
                    sub: sub.to_string(),
                    msg: ServerMsg::ack(id, at),
                });
            }
            None => return out,
        }

        // The append. A `Vec` rather than a transaction, and the same contract: contiguous `seq`s,
        // assigned here and nowhere else.
        for pending in decided.pending {
            let seq = self.head() + 1;
            self.log.push(Envelope {
                seq,
                at: pending.at,
                actor: pending.actor,
                body: pending.body,
            });
        }
        self.state = decided.state;

        out.extend(self.advance(sub));
        out
    }

    /// A client that navigated: `{"t":"g","path":"/done"}`, answered.
    ///
    /// In Mode A the page is a function of the route, so this is a re-render and a patch — the same
    /// diff any other change produces, which is what makes a link in the playground behave the way
    /// a link in a deployment does. In Mode B the kernel renders the new route locally and this is
    /// told anyway, so that the `Session` the tab hands `validate` is the one the client's own
    /// `validate` saw.
    pub fn nav(&mut self, sub: &str, path: &str) -> Vec<Outgoing> {
        let Some((_, s)) = self.subs.iter_mut().find(|(id, _)| id == sub) else {
            return Vec::new();
        };
        s.path = if path.is_empty() {
            beck_core::edge::ROOT.to_string()
        } else {
            path.to_string()
        };
        match self.mode() {
            Mode::Server => self.advance(sub),
            // Nothing to send: the client is rendering, and the state did not move.
            Mode::Client => Vec::new(),
        }
    }

    /// The frame a subscription starts with — the whole thing, or the gap it asked for.
    fn first_frame(
        &mut self,
        sub: &str,
        how: Resumption,
    ) -> Result<Option<serde_json::Value>, String> {
        let head = self.head();
        let Some((_, s)) = self.subs.iter().find(|(id, _)| id == sub) else {
            return Ok(None);
        };
        let at = s.at();
        let (shown, frame) = match self.mode() {
            Mode::Server => {
                let now = self.view(&self.state, &at)?;
                let ops = match how {
                    Resumption::Fresh | Resumption::Reset { .. } => vec![Op::Replace {
                        path: vec![],
                        html: now.clone(),
                    }],
                    Resumption::Resumed { from, .. } => diff(&self.page_at(from, &at)?, &now),
                };
                (
                    Shown::Dom(now),
                    (!ops.is_empty()).then(|| patch(head, &ops)),
                )
            }
            Mode::Client => {
                let state = self.state.clone();
                let frame = match how {
                    Resumption::Fresh | Resumption::Reset { .. } => Some(whole(head, &state)?),
                    Resumption::Resumed { from, .. } => {
                        let ops = delta::diff(&self.state_at(from)?, &state);
                        (!ops.is_empty()).then(|| data(head, &ops))
                    }
                };
                (Shown::Data(state), frame)
            }
        };
        if let Some((_, s)) = self.subs.iter_mut().find(|(id, _)| id == sub) {
            s.shown = shown;
        }
        Ok(frame)
    }

    /// Every subscription's frame after the state moved.
    ///
    /// `waiting` is the client that just proposed: it is the only one told "up to date" when its
    /// own command changed nothing it can see. Idle subscribers get silence, which is the fanout
    /// property the whole design exists for.
    fn advance(&mut self, waiting: &str) -> Vec<Outgoing> {
        let head = self.head();
        let mut out = Vec::new();
        let subs: Vec<String> = self.subs.iter().map(|(s, _)| s.clone()).collect();
        for sub in subs {
            let Some((_, s)) = self.subs.iter().find(|(id, _)| *id == sub) else {
                continue;
            };
            // What this subscriber is owed, in its mode's currency. A Mode A page is rendered per
            // subscriber because it is a function of *their* session; a Mode B state is the one
            // accumulator, and what differs between subscribers is only where each one is up to.
            let next = match &s.shown {
                Shown::Dom(last) => match self.view(&self.state, &s.at()) {
                    Ok(page) => {
                        let ops = diff(last, &page);
                        Ok((
                            Shown::Dom(page),
                            (!ops.is_empty()).then(|| patch(head, &ops)),
                        ))
                    }
                    Err(why) => Err(why),
                },
                Shown::Data(last) => {
                    let ops = delta::diff(last, &self.state);
                    Ok((
                        Shown::Data(self.state.clone()),
                        (!ops.is_empty()).then(|| data(head, &ops)),
                    ))
                }
            };
            let (shown, frame) = match next {
                Ok(next) => next,
                Err(why) => {
                    out.push(self.error(&sub, &why));
                    continue;
                }
            };
            let Some((_, s)) = self.subs.iter_mut().find(|(id, _)| *id == sub) else {
                continue;
            };
            s.shown = shown;
            match frame {
                Some(msg) => out.push(Outgoing { sub, msg }),
                None if sub == waiting => out.push(Outgoing {
                    sub,
                    msg: ServerMsg::up_to_date(head),
                }),
                None => {}
            }
        }
        out
    }

    /// The log, as a person reads it — the left half of the time-travel demo.
    pub fn history(&self) -> Vec<serde_json::Value> {
        self.log
            .iter()
            .map(|e| {
                serde_json::json!({
                    "seq": e.seq,
                    "at": e.at.0,
                    "actor": e.actor,
                    "event": e.body.display(),
                })
            })
            .collect()
    }

    /// The page as of a position, for an actor — `beck replay`, as something a visitor drags.
    ///
    /// A real fold over a real log rather than a recording of what the page looked like: the state
    /// at `seq` is computed from genesis every time, which is what makes the scrubber a
    /// demonstration of determinism rather than an undo stack.
    pub fn page_at(
        &self,
        seq: Seq,
        viewer: &(impl beck_host::Viewer + ?Sized),
    ) -> Result<Html, String> {
        let state = self.state_at(seq)?;
        self.view(&state, viewer)
    }

    /// The log, as the bytes a store writes — everything after `after`.
    ///
    /// [`beck_host::Envelope::encode`] is what redb, SQLite and Postgres write, so a tab keeping
    /// its log in IndexedDB is keeping *records*, not a rendering of them. The `after` argument is
    /// what makes persisting a command cost the command rather than the history: a page that has
    /// stored up to `n` asks for the rest.
    pub fn records(&self, after: Seq) -> Result<Vec<Vec<u8>>, String> {
        self.log
            .iter()
            .filter(|e| e.seq > after)
            .map(|e| e.encode().map_err(|why| why.to_string()))
            .collect()
    }

    /// Read a stored log back, and fold it.
    ///
    /// Only into a tab that has not been used: a restore is what happens *instead* of starting from
    /// `init`, and one that arrived after a subscription had rendered would be rewriting history
    /// under a client that had already seen it.
    ///
    /// The contract the fold depends on is checked rather than assumed — dense `seq`s from 1, in
    /// order. Records belonging to another program are refused by the decoder or by the fold, and
    /// the page keeps them under the wire id anyway (§4.3): what a stored log is *for* is a program
    /// whose event types have not changed.
    pub fn restore(&mut self, records: &[Vec<u8>]) -> Result<Seq, String> {
        if !self.log.is_empty() || !self.subs.is_empty() {
            return Err("a log can only be restored into a tab that has not run yet".into());
        }
        let mut state = self.runtime.initial_state().map_err(|e| e.to_string())?;
        let mut log = Vec::with_capacity(records.len());
        for (i, bytes) in records.iter().enumerate() {
            let env = Envelope::decode(bytes).map_err(|why| why.to_string())?;
            let expected = i as Seq + 1;
            if env.seq != expected {
                return Err(format!(
                    "the stored log is not contiguous: expected seq {expected}, found {}",
                    env.seq
                ));
            }
            let event = env.body.clone();
            state = self
                .runtime
                .fold(&state, &env, event)
                .map_err(|e| e.to_string())?;
            log.push(env);
        }
        self.log = log;
        self.state = state;
        Ok(self.head())
    }

    /// The component's slice, for a browser that renders it — Mode B's bundle.
    ///
    /// Derived from the running program exactly as `beck run` derives it, so a tab can never hand a
    /// client a bundle it is not itself executing.
    pub fn bundle(&self) -> Vec<u8> {
        beck_core::Bundle::of(self.runtime.placed()).to_bytes()
    }

    /// The accumulator as of a position. D3's genesis replay: snapshots are an optimisation and a
    /// tab has none, so this is the whole story.
    fn state_at(&self, seq: Seq) -> Result<Value, String> {
        let mut state = self.runtime.initial_state().map_err(|e| e.to_string())?;
        for env in self.log.iter().take_while(|e| e.seq <= seq) {
            let event = env.body.clone();
            state = self
                .runtime
                .fold(&state, env, event)
                .map_err(|e| e.to_string())?;
        }
        Ok(state)
    }

    /// What the tab currently shows one viewer, as markup — what a test compares against, and the
    /// document a client iframe is opened with.
    pub fn rendered(&self, viewer: &(impl beck_host::Viewer + ?Sized)) -> Result<String, String> {
        self.view(&self.state, viewer).map(|page| page.render())
    }

    /// The view, against a state and whoever is connected to this tab.
    ///
    /// The roster is `presence()`'s value (`beck_core::edge::presence`) and the tab builds it from
    /// the thing it already knows: its own subscriptions. A server keeps a bounded registry because
    /// the actor is a name the client chose and an unbounded table is a way to kill the process
    /// (`beck_rt::presence`); a tab has as many connections as the page opened, so the bound is the
    /// page.
    fn view(
        &self,
        state: &Value,
        viewer: &(impl beck_host::Viewer + ?Sized),
    ) -> Result<Html, String> {
        let mut here: BTreeMap<&str, i64> = BTreeMap::new();
        for (_, s) in &self.subs {
            *here.entry(s.actor.as_str()).or_default() += 1;
        }
        // A caller who is not connected — `beck rendered` before anyone has said hello, or the
        // scrubber — is still looking at the page, so they are in the roster they are shown.
        // `beck_core::edge::presence_of` is the same decision for the same reason.
        here.entry(viewer.actor()).or_insert(1);
        let here = beck_core::edge::presence(here);
        self.runtime
            .view_with(state, viewer, &here)
            .map_err(|why| why.to_string())
    }

    fn error(&self, sub: &str, why: &str) -> Outgoing {
        Outgoing {
            sub: sub.to_string(),
            msg: ServerMsg::error(why),
        }
    }
}

/// A patch frame, in the shape `beck-patch.js` already applies.
///
/// Not a second wire format: `beck_rt::patch::PatchFrame` produces this same JSON for a deployed
/// application, and the reason it is rebuilt here rather than imported is that `beck-rt` does not
/// cross to `wasm32-unknown-unknown`. `playground.rs::the_tab_and_the_server_send_the_same_frame`
/// is what stops the two drifting.
fn patch(seq: Seq, ops: &[Op]) -> serde_json::Value {
    serde_json::json!({
        "t": "p",
        "q": seq,
        "o": ops.iter().map(Op::to_wire).collect::<Vec<_>>(),
    })
}

/// Mode B's first frame: the whole accumulator, in the shape `beck-mode-b.js` already applies.
///
/// `beck_rt::patch::DataFrame::Whole` produces this same JSON for a deployed application, and is
/// rebuilt here for the reason [`patch`] is. A state that cannot be represented is what
/// `secure::storable` proves cannot exist (`B0411`), so this refuses rather than fabricates.
fn whole(seq: Seq, state: &Value) -> Result<serde_json::Value, String> {
    let repr = Repr::of(state).map_err(|why| why.to_string())?;
    Ok(serde_json::json!({ "t": "s", "q": seq, "v": repr }))
}

/// Mode B's every other frame: the difference between two accumulators.
fn data(seq: Seq, ops: &[delta::Op]) -> serde_json::Value {
    serde_json::json!({ "t": "d", "q": seq, "o": ops })
}

/// The programs the page opens with, so the first thing a visitor sees is a running application.
///
/// Compiled into the module rather than fetched, because rung A "costs a CDN" (§17.1) and a sample
/// that arrives over a second request is a sample that can 404.
pub fn examples() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("todo", include_str!("../../../examples/todo.beck")),
        ("board", include_str!("../../../examples/board.beck")),
        ("counter", include_str!("../examples/counter.beck")),
    ])
}
