//! F3's per-actor write quota: how much one actor may turn into permanent storage.
//!
//! [`docs/14`](../../../../../docs/14-review-findings.md) F3 splits the "events are forever"
//! problem in two. Channel (a) — rejected garbage — was closed by §3.7's rule that **only validated
//! events are durably logged**, so a refused command leaves nothing behind. Channel (b) is the one
//! this module is for:
//!
//! > *validated* spam from a legitimate but abusive session — permanent by design. Remediation:
//! > per-actor rate/volume quotas … on by default with generous limits.
//!
//! On by default is the load-bearing half. A quota a program has to ask for is a quota most
//! programs do not have, and F3's whole point is that the *default* deployment should not turn an
//! abusive session into a permanent cost.
//!
//! # The table is bounded, which is the part that is easy to get wrong
//!
//! The obvious implementation is a map from actor to a counter. That map is **unbounded memory
//! keyed by a string the client chooses** — the same denial of service the quota exists to prevent,
//! moved one level down and made harder to see. Under `DevIdentity` the actor *is* whatever the
//! client says, so an attacker sending a fresh name per proposal would both evade the quota and
//! grow the table.
//!
//! So the counters are **sharded**: a fixed number of buckets, an actor hashed into one, and no
//! per-actor allocation ever. Memory is [`BUCKETS`] × a few bytes, for the life of the process,
//! whatever arrives. Two consequences, both deliberate:
//!
//! * **Two actors can share a bucket, and therefore a budget.** That is why the limit is generous
//!   rather than tight: a shared bucket must still be ample for both. [`Quota::default`] says what
//!   the numbers are and why.
//! * **The hash is keyed per process** (`RandomState`), so a client cannot compute a name that
//!   lands in a chosen bucket. Without that, sharing a bucket stops being an accident an operator
//!   accepts and becomes a way to spend somebody else's budget on purpose.
//!
//! # What this is not
//!
//! It binds an **actor**, so it is worth exactly what the actor is worth. Under
//! [`crate::identity::DevIdentity`] the actor is the claim the client sent, so an attacker who
//! rotates names spreads across buckets rather than being stopped — the *total* is still bounded by
//! [`BUCKETS`] × the limit, which is a bound rather than the bound anybody wanted.
//! [`docs/48`](../../../../../docs/48-identity-report.md) is the seam that fixes this, and
//! [`docs/82`](../../../../../docs/82-the-edge-report.md) §82.5 is the
//! composition written out rather than left to be inferred.

use std::sync::atomic::{AtomicU64, Ordering};

/// How many counters exist, for all actors, forever.
///
/// A power of two so the index is a mask. 1,024 × 16 bytes is 16 KiB, which is small enough that
/// the table never has to be swept, resized or evicted from — and "never has to be swept" is the
/// property that makes this bounded rather than merely large.
pub const BUCKETS: usize = 1024;

/// The limits, and the window they are counted over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    /// How many events one bucket may add to the log per window. `None` disables the quota.
    pub events_per_window: Option<u32>,
    /// How long a window is, in milliseconds.
    pub window_ms: u64,
}

impl Default for Quota {
    /// "On by default with generous limits" — F3's own words, with numbers attached.
    ///
    /// **600 events a minute**, which is ten a second sustained. The number is chosen from what a
    /// *person* can produce: a fast typist committing a todo per keystroke does not reach it, and a
    /// UI that batches at all is nowhere near. It is deliberately far above any interactive use and
    /// far below what a script can do in a second, which is the gap F3 asks to be closed.
    ///
    /// It is generous for the reason the module doc gives as well: two actors can share a bucket,
    /// so the limit has to be ample for both and still bite a script.
    fn default() -> Self {
        Quota {
            events_per_window: Some(600),
            window_ms: 60_000,
        }
    }
}

impl Quota {
    /// A quota that refuses nothing — for a deployment that enforces elsewhere, and for the tests
    /// that assert what the runtime does when the limit is not the thing under test.
    pub fn unlimited() -> Quota {
        Quota {
            events_per_window: None,
            window_ms: 60_000,
        }
    }
}

/// One bucket: how many, and when the window it belongs to started.
///
/// Two atomics rather than a lock, because this is read and written on the path every proposal
/// takes and a lock there would serialise the merge point on bookkeeping.
#[derive(Default)]
struct Bucket {
    count: AtomicU64,
    window_start_ms: AtomicU64,
}

/// The sharded counters, and the process-random hash that decides which bucket an actor lands in.
pub struct RateLimit {
    quota: Quota,
    buckets: Box<[Bucket]>,
    /// `RandomState` is the standard library's answer to exactly this question — it is what makes a
    /// `HashMap` resistant to a caller choosing colliding keys — and it seeds itself once per
    /// process from the OS. Using it here rather than minting a key by hand keeps the workspace's
    /// `forbid(unsafe)` intact and keeps this module out of the business of finding entropy.
    hash: std::collections::hash_map::RandomState,
}

impl RateLimit {
    pub fn new(quota: Quota) -> RateLimit {
        let mut buckets = Vec::with_capacity(BUCKETS);
        buckets.resize_with(BUCKETS, Bucket::default);
        RateLimit {
            quota,
            buckets: buckets.into_boxed_slice(),
            hash: std::collections::hash_map::RandomState::new(),
        }
    }

    pub fn quota(&self) -> Quota {
        self.quota
    }

    /// Charge one event to this actor, and say whether it is allowed.
    ///
    /// `now_ms` is passed in rather than read, for the reason `Proposal::at` is: the merge point is
    /// the one place time enters, and it enters from the clock the process was configured with
    /// (§3.7, F11). A quota that read the wall clock itself would be a second, ambient one.
    pub fn admit(&self, actor: &str, now_ms: i64) -> bool {
        let Some(limit) = self.quota.events_per_window else {
            return true;
        };
        let now = now_ms.max(0) as u64;
        let window = now - (now % self.quota.window_ms.max(1));
        let bucket = &self.buckets[self.index(actor)];

        // Roll the window if this bucket is still in an older one. A racing pair may both roll;
        // both write the same `window`, and the loser's `count` reset is the same reset, so the
        // worst case is one event's worth of slack rather than a wrong window.
        if bucket.window_start_ms.swap(window, Ordering::Relaxed) != window {
            bucket.count.store(0, Ordering::Relaxed);
        }
        bucket.count.fetch_add(1, Ordering::Relaxed) < u64::from(limit)
    }

    fn index(&self, actor: &str) -> usize {
        // Keyed, so a client cannot pick a name that shares a bucket with somebody else's. What is
        // required is not that the hash be cryptographic but that an attacker cannot *search* for a
        // collision — and there is no oracle to search against, because which bucket a name landed
        // in is never observable from outside.
        use std::hash::BuildHasher;
        (self.hash.hash_one(actor) as usize) & (BUCKETS - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_actor_is_admitted_up_to_the_limit_and_refused_after_it() {
        let rl = RateLimit::new(Quota {
            events_per_window: Some(3),
            window_ms: 1000,
        });
        assert!(rl.admit("ana", 0));
        assert!(rl.admit("ana", 10));
        assert!(rl.admit("ana", 20));
        assert!(!rl.admit("ana", 30), "the fourth is over the limit");
    }

    #[test]
    fn the_next_window_starts_the_count_again() {
        let rl = RateLimit::new(Quota {
            events_per_window: Some(2),
            window_ms: 1000,
        });
        assert!(rl.admit("ana", 0));
        assert!(rl.admit("ana", 1));
        assert!(!rl.admit("ana", 2));
        assert!(rl.admit("ana", 1000), "a new window is a new budget");
        assert!(rl.admit("ana", 1999));
        assert!(!rl.admit("ana", 1999));
    }

    /// The disabled setting really does disable it, which the runtime's own tests depend on.
    #[test]
    fn an_unlimited_quota_admits_everything() {
        let rl = RateLimit::new(Quota::unlimited());
        for i in 0..10_000 {
            assert!(rl.admit("ana", i));
        }
    }

    /// The property the whole design exists for: the table does not grow.
    ///
    /// Ten thousand distinct actors — which is what a client rotating names produces — and the
    /// memory afterwards is the same [`BUCKETS`] counters it was before. There is nothing to
    /// measure because there is nothing to allocate; the assertion is that the structure has no
    /// per-actor storage to have grown.
    #[test]
    fn ten_thousand_actors_allocate_nothing() {
        let rl = RateLimit::new(Quota::default());
        for i in 0..10_000 {
            rl.admit(&format!("actor-{i}"), 0);
        }
        assert_eq!(rl.buckets.len(), BUCKETS);
    }

    /// Rotating names is not free, even though it is not stopped.
    ///
    /// `docs/82` §82.5: under `DevIdentity` an attacker chooses the actor, so a fresh name per
    /// proposal spreads across buckets rather than exhausting one. The total is still bounded —
    /// `BUCKETS × limit` per window — and this test is what says that bound is real rather than
    /// asserted.
    #[test]
    fn rotating_actor_names_is_bounded_by_the_table_rather_than_by_the_limit() {
        let limit = 4;
        let rl = RateLimit::new(Quota {
            events_per_window: Some(limit),
            window_ms: 60_000,
        });
        let admitted = (0..200_000)
            .filter(|i| rl.admit(&format!("actor-{i}"), 0))
            .count();
        let ceiling = BUCKETS * limit as usize;
        assert!(
            admitted <= ceiling,
            "{admitted} admitted, and the table can only ever allow {ceiling}"
        );
        assert!(
            admitted > ceiling / 2,
            "{admitted} — the buckets should fill roughly evenly, or the hash is the problem"
        );
    }
}
