//! Phase 0 log engine — "the log is the database" (§5.3).
//!
//! We do not write a storage engine ([`docs/01-vision-and-premise.md`](../../../../docs/01-vision-and-premise.md)
//! §1.5). This is a small log engine *on top of* proven storage: PostgreSQL for the durable
//! substrate, redb for the embedded rung-0 log, and an in-memory store for tests and for measuring
//! everything else without a disk in the way.
//!
//! The contract every substrate keeps:
//!
//! * `seq` is assigned **here and nowhere else**, densely, from a single writer (§3.7).
//! * A batch of events from one command is appended **atomically at contiguous `seq`s** — no fold
//!   ever observes half a command's consequences (§3.7).
//! * Reads are ordered and replayable from any position; that is what `beck replay`,
//!   `(subscription, seq)` resumption and forked worlds are made of.

mod memory;
mod postgres;
mod redb_store;

pub use memory::MemoryLog;
pub use postgres::{PgLog, DDL};
pub use redb_store::RedbLog;

use anyhow::Result;
use async_trait::async_trait;

use beck_p0_core::domain::{ActorId, Event, TodoState};
use beck_p0_core::envelope::{Envelope, EventEnvelope, Instant, Seq};

/// A validated event on its way to the log, before `seq` exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingEvent {
    pub at: Instant,
    pub actor: ActorId,
    pub body: Event,
}

impl PendingEvent {
    pub fn new(at: Instant, actor: ActorId, body: Event) -> Self {
        Self { at, actor, body }
    }

    fn stamp(&self, seq: Seq) -> EventEnvelope {
        Envelope::new(seq, self.at, self.actor.clone(), self.body.clone())
    }
}

/// A snapshot of the durable fold: the accumulator plus the position it was taken at.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub seq: Seq,
    pub state: TodoState,
}

#[async_trait]
pub trait LogStore: Send + Sync + 'static {
    /// Which substrate this is — reported by `/metrics` and by the benchmark harness, because
    /// every number in the Phase 0 report is meaningless without it.
    fn kind(&self) -> &'static str;

    /// The highest assigned `seq`; 0 when the log is empty.
    async fn head(&self) -> Result<Seq>;

    /// The lowest `seq` still readable. Non-zero once retention has trimmed the head of the log,
    /// which is what makes a resumption unreachable and forces a reset.
    async fn floor(&self) -> Result<Seq>;

    /// Append a batch atomically at contiguous `seq`s. Returns the stamped envelopes.
    async fn append(&self, batch: &[PendingEvent]) -> Result<Vec<EventEnvelope>>;

    /// Read up to `limit` envelopes with `seq > after`, in order.
    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<EventEnvelope>>;

    /// Persist a snapshot of the fold. "`durable` is the entire database administration story."
    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()>;

    /// The newest snapshot at or before `seq`, if any.
    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>>;
}

/// Fold the log from the best available starting point — a snapshot if there is one, genesis
/// otherwise. This is `beck replay`, and the resumption path uses it to reconstruct the view a
/// reconnecting subscriber last saw.
pub async fn replay_to(store: &dyn LogStore, target: Seq) -> Result<(TodoState, Seq)> {
    let mut state;
    let mut at;
    match store.snapshot_at_or_before(target).await? {
        Some(snapshot) => {
            at = snapshot.seq;
            state = snapshot.state;
        }
        None => {
            at = 0;
            state = TodoState::new();
        }
    }

    const CHUNK: usize = 4096;
    while at < target {
        let batch = store.read(at, CHUNK.min((target - at) as usize)).await?;
        if batch.is_empty() {
            break;
        }
        for env in &batch {
            beck_p0_core::domain::apply_event(&mut state, env);
            at = env.seq;
        }
    }
    Ok((state, at))
}

/// Fold the whole log from genesis, ignoring snapshots.
///
/// The genesis-replay discipline of [`docs/10-decisions.md`](../../../../docs/10-decisions.md) D3:
/// snapshots are an optimisation, and a snapshot that disagrees with the log is a bug we want CI to
/// find, not a fact we want to trust.
pub async fn replay_from_genesis(store: &dyn LogStore) -> Result<(TodoState, Seq)> {
    let mut state = TodoState::new();
    let mut at = 0;
    loop {
        let batch = store.read(at, 4096).await?;
        if batch.is_empty() {
            break;
        }
        for env in &batch {
            beck_p0_core::domain::apply_event(&mut state, env);
            at = env.seq;
        }
    }
    Ok((state, at))
}
