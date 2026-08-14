//! The merge point's rules, with nothing around them.
//!
//! ```text
//!   proposals ──▶ [ de-duplicate ─▶ validate ─▶ check storable ─▶ fold ] ──▶ events to append
//!                       ▲                against the batch's own
//!                       │                speculative state
//!                  the answer to a
//!                  retry is the first
//!                  attempt's position
//! ```
//!
//! Everything §3.7 calls "the single writer" that is not *writing*: which proposals become events,
//! in what order, and what each proposer is told. A host supplies the queue, the durable append and
//! the reply channel; those are the parts that differ between a process with a Postgres log and a
//! browser tab with an array, and they are the parts that are not the semantics.
//!
//! Two rules here were learned the expensive way and are the reason this is one function rather
//! than one per host:
//!
//! * **A retry is acknowledged with the position the first attempt got**, never refused. Answering
//!   "duplicate" to a command that is in the log tells a client replaying an offline queue that its
//!   work was rejected, and it takes that work back off the page one card at a time
//!   ([`docs/94`](../../../../../docs/94-the-client-report.md) §94.10).
//! * **Validation sees the batch it is inside.** `Add(x)` followed by `Toggle(x)` in one batch must
//!   work, so each command is validated against the state the previous ones produced
//!   (`docs/18` §18.5 item 5).

use std::collections::VecDeque;

use beck_core::Value;

use crate::program::{Runtime, Viewer};
use crate::record::{Instant, Pending, Seq};

/// One client's proposal, as the merge point sees it.
///
/// The reply channel is not here: what a host does with a [`Decision`] — a `oneshot`, a returned
/// frame, a `postMessage` — is the host's business, and is the one part of ingress that genuinely
/// differs between a server and a tab.
pub struct Proposal<'a> {
    /// The idempotency key that makes a retry after a reconnect safe (§4.3).
    pub id: String,
    /// When the merge point admitted it — the one place time enters (§3.7), read by the host from
    /// the clock it was configured with and data from here onwards.
    pub at: Instant,
    /// Who is proposing. A `&dyn` rather than a type parameter because the two hosts genuinely
    /// have different ones: a connection supplies a verified actor with claims, and a test or a tab
    /// supplies a name.
    pub actor: &'a dyn Viewer,
    /// The command, already decoded against the program's own `Command` union.
    pub command: Value,
}

/// What one proposal became.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// This id is already in the log. The position is the **first** attempt's, which is what makes
    /// the answer to a retry an acknowledgement rather than a refusal.
    Duplicate(Seq),
    /// Accepted. Its last event lands at `base + offset` once the batch is appended.
    Accepted { offset: usize },
    /// Refused, with the reason the program gave — or the reason the boundary gave, for an event
    /// that cannot be written durably.
    Refused { why: String },
}

/// The host's stopwatch.
///
/// `beck-host` may not read a clock — a `wasm32-unknown-unknown` build has none, and
/// `std::time::Instant::now()` on that target is a panic rather than a number — so a host that
/// wants to know what a fold cost passes one of these in. It is also the reason the fold is the
/// only thing metered here: it is the one step whose cost is the program's rather than the
/// machine's.
pub trait Meter {
    fn fold(&self, f: &mut dyn FnMut() -> anyhow::Result<Value>) -> anyhow::Result<Value>;
}

/// A host that is not measuring anything — a tab, a test, a replay.
pub struct Untimed;

impl Meter for Untimed {
    fn fold(&self, f: &mut dyn FnMut() -> anyhow::Result<Value>) -> anyhow::Result<Value> {
        f()
    }
}

/// The batch, decided.
pub struct Committed {
    /// The accumulator after every accepted event. Speculative: it becomes the application's state
    /// only once the host's append succeeds.
    pub state: Value,
    /// The events to append, in order, at `base + 1 …`.
    pub pending: Vec<Pending>,
    /// One per proposal, in the order they were given.
    pub decisions: Vec<Decision>,
}

/// The ids this application has already sequenced, and where each landed.
///
/// A bounded memory rather than a set: idempotency is a property of a *recent* retry, and a client
/// that reconnects after the window has moved on gets a second copy of its command — which is the
/// trade every at-least-once channel makes and is worth saying out loud.
pub struct Seen {
    capacity: usize,
    entries: VecDeque<(String, Seq)>,
}

impl Seen {
    pub fn new(capacity: usize) -> Seen {
        Seen {
            capacity,
            entries: VecDeque::with_capacity(capacity.min(1024)),
        }
    }

    /// Where this id's command landed, if it is still remembered.
    pub fn position(&self, id: &str) -> Option<Seq> {
        self.entries
            .iter()
            .find(|(seen, _)| seen == id)
            .map(|(_, at)| *at)
    }

    fn remember(&mut self, id: String, at: Seq) {
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((id, at));
    }
}

/// Decide a batch: what becomes an event, what each proposer is told, and the state that follows.
///
/// Pure with respect to the machine — it reads no clock and touches no store — but not with respect
/// to `seen`, which is the application's idempotency memory and moves as commands are accepted.
pub fn sequence(
    runtime: &Runtime,
    state: &Value,
    base: Seq,
    seen: &mut Seen,
    batch: Vec<Proposal<'_>>,
    meter: &dyn Meter,
) -> Committed {
    let mut speculative = state.clone();
    let mut pending: Vec<Pending> = Vec::new();
    let mut decisions = Vec::with_capacity(batch.len());

    for p in batch {
        if let Some(at) = seen.position(&p.id) {
            decisions.push(Decision::Duplicate(at));
            continue;
        }
        let proposal = runtime.proposal(p.actor, p.command);
        let events = match runtime.validate(&speculative, &proposal) {
            Ok(events) => events,
            Err(why) => {
                decisions.push(Decision::Refused { why });
                continue;
            }
        };
        if events.is_empty() {
            decisions.push(Decision::Refused {
                why: "no events".into(),
            });
            continue;
        }
        // The state and the events this command would add, held apart until every one of them has
        // been folded: a command that fails half way through must add nothing at all, and the next
        // command in the batch must not see the half.
        let mut failure: Option<String> = None;
        let mut folded = speculative.clone();
        let mut added: Vec<Pending> = Vec::with_capacity(events.len());
        for e in events {
            let seq = base + pending.len() as u64 + added.len() as u64 + 1;
            // Checked storable *before* the fold advances, so an event that cannot be written
            // durably is refused rather than folded into a state the log cannot reproduce. A
            // rejection here is a program that should not have compiled — `secure::storable` proves
            // it cannot — but the boundary refuses rather than writing something lossy.
            if let Err(why) = beck_core::repr::Repr::of(&e) {
                failure = Some(why.to_string());
                break;
            }
            let env = crate::record::Envelope {
                seq,
                at: p.at,
                actor: p.actor.actor().to_string(),
                body: e.clone(),
            };
            match meter.fold(&mut || runtime.fold(&folded, &env, e.clone())) {
                Ok(next) => folded = next,
                Err(err) => {
                    failure = Some(err.to_string());
                    break;
                }
            }
            added.push(Pending {
                at: p.at,
                actor: p.actor.actor().to_string(),
                body: e,
            });
        }
        match failure {
            Some(why) => decisions.push(Decision::Refused { why }),
            None => {
                speculative = folded;
                pending.extend(added);
                // The position is `base + pending.len()`: the last event this command produced,
                // which is the seq its reply carries and the seq a retry will be answered with.
                let offset = pending.len();
                seen.remember(p.id, base + offset as u64);
                decisions.push(Decision::Accepted { offset });
            }
        }
    }

    Committed {
        state: speculative,
        pending,
        decisions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retry_is_answered_with_the_position_the_first_attempt_got() {
        let mut seen = Seen::new(4);
        seen.remember("k1".into(), 7);
        assert_eq!(seen.position("k1"), Some(7));
        assert_eq!(seen.position("k2"), None);
    }

    /// The memory is bounded, and the oldest id is the one that goes.
    #[test]
    fn the_idempotency_memory_forgets_the_oldest_first() {
        let mut seen = Seen::new(2);
        seen.remember("a".into(), 1);
        seen.remember("b".into(), 2);
        seen.remember("c".into(), 3);
        assert_eq!(seen.position("a"), None);
        assert_eq!(seen.position("b"), Some(2));
        assert_eq!(seen.position("c"), Some(3));
    }
}
