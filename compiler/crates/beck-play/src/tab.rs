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
//! Not durable: a reload starts from `init`. §17.2's IndexedDB row is the honest version of
//! "storage" and is not built — [`docs/96`](../../../../../docs/96-playground-report.md) §96.7 says
//! so rather than implying otherwise.

use std::collections::BTreeMap;

use beck_core::diff::{diff, Op};
use beck_core::render::Mode;
use beck_core::{Html, Placed, Value};
use beck_host::protocol::{Resumption, ServerMsg};
use beck_host::sequence::{Proposal, Seen, Untimed};
use beck_host::{Decision, Envelope, Instant, Runtime, Seq};

/// One subscription, as the tab holds it.
///
/// The server's equivalent is a task holding an engine and the last page it sent. This holds the
/// same two facts and no task: a tab advances every subscription when the state moves, in the
/// order they subscribed, because a `postMessage` is the only thing that ever happens.
struct Subscriber {
    actor: String,
    /// The page this client's DOM is showing. The next frame is the difference from it — which is
    /// the whole of Mode A, and the reason an idle subscriber costs no bytes.
    ///
    /// The position it reflects is deliberately *not* here: it is always this tab's head, because a
    /// tab advances every subscription before it answers anything else. A server holds one per
    /// subscription because its subscriptions render concurrently.
    shown: Html,
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

    /// Which rendering mode this program's page is in. The tab serves Mode A only, and says so
    /// rather than rendering the wrong thing (§96.7).
    pub fn mode(&self) -> Mode {
        self.runtime.placed().render.mode
    }

    /// Subscribe, or resume — the `hello` frame, answered.
    ///
    /// The resumption rule is [`beck_host::protocol`]'s, not a second one: absent means "I hold
    /// nothing", `Some(n)` means "I hold the frame as of n", and a position this log cannot reach
    /// is a reset that says so.
    pub fn hello(&mut self, sub: &str, actor: &str, from: Option<Seq>) -> Vec<Outgoing> {
        let head = self.head();
        let how = match from {
            None => Resumption::Fresh,
            Some(n) if n > head => Resumption::Reset { from: n },
            Some(n) => Resumption::Resumed {
                from: n,
                replayed: head - n,
            },
        };

        let now = match self.runtime.view(&self.state, actor) {
            Ok(page) => page,
            Err(why) => return vec![self.error(sub, &why.to_string())],
        };
        let ops = match how {
            Resumption::Fresh | Resumption::Reset { .. } => vec![Op::Replace {
                path: vec![],
                html: now.clone(),
            }],
            Resumption::Resumed { from, .. } => match self.page_at(from, actor) {
                Ok(then) => diff(&then, &now),
                Err(why) => return vec![self.error(sub, &why)],
            },
        };

        self.subs.retain(|(id, _)| id != sub);
        self.subs.push((
            sub.to_string(),
            Subscriber {
                actor: actor.to_string(),
                shown: now,
            },
        ));

        let mut out = vec![Outgoing {
            sub: sub.to_string(),
            msg: ServerMsg::welcome(sub, head, how),
        }];
        if !ops.is_empty() {
            out.push(Outgoing {
                sub: sub.to_string(),
                msg: patch(head, &ops),
            });
        }
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
        let actor = subscriber.actor.clone();

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
                actor: &actor,
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
            let page = match self.runtime.view(&self.state, &s.actor) {
                Ok(page) => page,
                Err(why) => {
                    out.push(self.error(&sub, &why.to_string()));
                    continue;
                }
            };
            let ops = diff(&s.shown, &page);
            let Some((_, s)) = self.subs.iter_mut().find(|(id, _)| *id == sub) else {
                continue;
            };
            s.shown = page;
            if !ops.is_empty() {
                out.push(Outgoing {
                    sub,
                    msg: patch(head, &ops),
                });
            } else if sub == waiting {
                out.push(Outgoing {
                    sub,
                    msg: ServerMsg::up_to_date(head),
                });
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
    pub fn page_at(&self, seq: Seq, actor: &str) -> Result<Html, String> {
        let state = self.state_at(seq)?;
        self.runtime
            .view(&state, actor)
            .map_err(|why| why.to_string())
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

    /// What the tab currently shows one actor, as markup — what a test compares against.
    pub fn rendered(&self, actor: &str) -> Result<String, String> {
        self.runtime
            .view(&self.state, actor)
            .map(|h| h.render())
            .map_err(|e| e.to_string())
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
