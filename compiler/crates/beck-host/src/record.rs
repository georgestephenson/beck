//! What a logged occurrence is, and what one weighs on the wire.
//!
//! The types a log engine stores, without the engine. `beck-rt`'s [`log`] holds the substrates —
//! redb, SQLite, Postgres, memory — and the contract they keep; what they keep it *about* is here,
//! because a browser tab holds a log too ([`docs/17-playground.md`](../../../../../docs/17-playground.md)
//! §17.2) and an envelope must mean the same thing in both.
//!
//! [`log`]: ../../beck_rt/log/index.html

use anyhow::{Context, Result};
use beck_core::Value;
use serde::{Deserialize, Serialize};

/// Position in the total order. One totally-ordered log per application (§3.7 v1 semantics).
pub type Seq = u64;

/// Wall-clock instant, milliseconds since the Unix epoch, captured at ingress **as data**.
///
/// A fold may read `env.at`; it may not call a clock. The type is deliberately a plain number with
/// no way to obtain "now" from it, so the determinism rule is hard to break by accident.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Instant(pub i64);

/// A durably logged occurrence. The fields are §3.7's, and `actor` is a stable identity — never
/// the live `Session` capability or a token (F5).
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub seq: Seq,
    pub at: Instant,
    pub actor: String,
    /// The event itself.
    ///
    /// A `Value`, not a JSON tree. It was the latter through Phase 2, and the cost was paid on
    /// every append *and* every read: build a `serde_json::Value`, serialise it to text, parse the
    /// text, walk the tree back into a `Value` — four traversals and two allocations per event, at
    /// the one point §3.7 makes serial. [`beck_core::repr`] is the encoding now; the JSON repr
    /// stays for things a person reads.
    pub body: Value,
}

impl Envelope {
    /// The envelope as the `Envelope[Event]` record a fold sees.
    pub fn to_value(&self, event: Value) -> Value {
        beck_core::edge::envelope(self.seq, self.at.0, &self.actor, event)
    }

    /// The event. Kept as a method because every caller had one and the type changed underneath
    /// them; there is nothing to decode any more.
    pub fn event(&self) -> Result<Value> {
        Ok(self.body.clone())
    }

    /// The bytes a store writes.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let wire = Wire {
            seq: self.seq,
            at: self.at,
            actor: self.actor.clone(),
            body: beck_core::repr::Repr::of(&self.body)?,
        };
        Ok(postcard::to_allocvec(&wire)?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Envelope> {
        let wire: Wire = postcard::from_bytes(bytes).context("decoding a logged event")?;
        Ok(Envelope {
            seq: wire.seq,
            at: wire.at,
            actor: wire.actor,
            body: wire.body.to_value(),
        })
    }
}

/// The on-disk shape of an [`Envelope`] — a concrete type, so a non-self-describing codec can
/// encode it. See [`beck_core::repr`] for why that matters.
#[derive(Serialize, Deserialize)]
struct Wire {
    seq: Seq,
    at: Instant,
    actor: String,
    body: beck_core::repr::Repr,
}

/// A validated event on its way to the log, before `seq` exists.
#[derive(Clone, Debug)]
pub struct Pending {
    pub at: Instant,
    pub actor: String,
    pub body: Value,
}

/// A snapshot of the durable fold: the accumulator plus the position it was taken at.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub seq: Seq,
    pub state: Value,
}
