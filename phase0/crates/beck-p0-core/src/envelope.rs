//! Envelopes — where time enters.
//!
//! `docs/03-type-and-effect-system.md` §3.7: ingress stamps each occurrence with `seq` (assigned
//! here and nowhere else), `at` (wall-clock captured *as data*, so folds never read a clock) and
//! `actor` (a stable authenticated identity — **never** the live `Session` capability or a token,
//! per `docs/14-review-findings.md` F5).
//!
//! Two envelope types, deliberately: `Envelope<Event>` is durable, `CommandEnvelope` is transient.
//! Only validated events are logged (F3), so rejected traffic never becomes permanent storage;
//! command envelopes are retained briefly for idempotency de-duplication and then dropped.

use serde::{Deserialize, Serialize};

use crate::domain::{ActorId, Command, Event};

/// Position in the total order. One totally-ordered log per application (§3.7 v1 semantics).
pub type Seq = u64;

/// Wall-clock instant, milliseconds since the Unix epoch, captured at ingress **as data**.
///
/// A fold may read `env.at`; it may not call a clock. That distinction is the whole determinism
/// story, so the type is deliberately a plain number with no way to obtain "now" from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Instant(pub i64);

impl Instant {
    pub const EPOCH: Instant = Instant(0);

    pub fn millis(self) -> i64 {
        self.0
    }
}

/// A durably logged occurrence: `seq` is its position in the total order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub seq: Seq,
    pub at: Instant,
    pub actor: ActorId,
    pub body: T,
}

impl<T> Envelope<T> {
    pub fn new(seq: Seq, at: Instant, actor: ActorId, body: T) -> Self {
        Self {
            seq,
            at,
            actor,
            body,
        }
    }
}

/// The durable envelope type of this application.
pub type EventEnvelope = Envelope<Event>;

/// A client's proposal. Transient: it is de-duplicated by `id`, validated, and discarded.
///
/// `session` is a live capability and is *not* part of `Envelope`; only `actor` survives into the
/// log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    /// Client-minted command id — the idempotency key that makes retries safe (§4.3).
    pub id: uuid::Uuid,
    pub at: Instant,
    pub actor: ActorId,
    pub body: Command,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Id;

    #[test]
    fn envelopes_round_trip_through_the_wire_encoding() {
        let env = Envelope::new(
            7,
            Instant(1_700_000_000_000),
            ActorId::new("alice"),
            Event::Toggled { id: Id::nil() },
        );
        let bytes = postcard::to_allocvec(&env).unwrap();
        let back: EventEnvelope = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(env, back);
    }
}
