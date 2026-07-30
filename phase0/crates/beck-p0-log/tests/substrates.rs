//! The log-store contract, asserted identically against every substrate.
//!
//! This is the differential discipline of §4.8 applied one level down: the semantics are "an
//! append-only, totally-ordered log", and a substrate is an implementation detail. If redb and
//! Postgres disagree about anything observable, the abstraction is not real.
//!
//! The Postgres cases are skipped (loudly) when no database is reachable, so the suite still runs
//! on a laptop with nothing installed — rung 0 of §6.6 is a promise about the test suite too.

use std::sync::Arc;

use beck_p0_core::domain::{fold, ActorId, Event, Id};
use beck_p0_core::envelope::Instant;
use beck_p0_log::{
    replay_from_genesis, replay_to, LogStore, MemoryLog, PendingEvent, PgLog, RedbLog, Snapshot,
};

fn events(range: std::ops::Range<u128>) -> Vec<PendingEvent> {
    range
        .map(|i| {
            PendingEvent::new(
                Instant(1_700_000_000_000 + i as i64),
                ActorId::new(format!("actor{}", i % 4)),
                Event::Added {
                    id: Id::from_u128(i),
                    text: format!("todo {i}"),
                },
            )
        })
        .collect()
}

/// The Postgres cases share one database, and each starts by truncating it — so they take turns.
/// Held for the whole of each test, not just the setup.
async fn exclusive() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

async fn pg() -> Option<Arc<dyn LogStore>> {
    let url = std::env::var("BECK_PG")
        .unwrap_or_else(|_| "postgres://postgres@localhost/beck_p0".to_string());
    match PgLog::connect(&url).await {
        Ok(store) => {
            store.truncate().await.expect("truncate");
            Some(Arc::new(store))
        }
        Err(e) => {
            eprintln!("skipping Postgres: {e}");
            None
        }
    }
}

async fn substrates(dir: &tempfile::TempDir) -> Vec<Arc<dyn LogStore>> {
    let mut stores: Vec<Arc<dyn LogStore>> = vec![
        Arc::new(MemoryLog::new()),
        Arc::new(RedbLog::open(dir.path().join("log.redb")).expect("open redb")),
    ];
    if let Some(store) = pg().await {
        stores.push(store);
    }
    stores
}

#[tokio::test]
async fn every_substrate_assigns_dense_contiguous_seqs() {
    let _exclusive = exclusive().await;
    let dir = tempfile::tempdir().unwrap();
    for store in substrates(&dir).await {
        let first = store.append(&events(0..3)).await.unwrap();
        let second = store.append(&events(3..5)).await.unwrap();

        let seqs: Vec<_> = first.iter().chain(second.iter()).map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5], "substrate {}", store.kind());
        assert_eq!(store.head().await.unwrap(), 5, "substrate {}", store.kind());
        assert_eq!(
            store.floor().await.unwrap(),
            1,
            "substrate {}",
            store.kind()
        );
    }
}

#[tokio::test]
async fn every_substrate_reads_back_exactly_what_was_appended() {
    let _exclusive = exclusive().await;
    let dir = tempfile::tempdir().unwrap();
    for store in substrates(&dir).await {
        let written = store.append(&events(0..64)).await.unwrap();
        let read = store.read(0, 1000).await.unwrap();
        assert_eq!(written, read, "substrate {}", store.kind());

        let tail = store.read(60, 1000).await.unwrap();
        assert_eq!(tail.len(), 4, "substrate {}", store.kind());
        assert_eq!(tail[0].seq, 61, "substrate {}", store.kind());

        let limited = store.read(0, 10).await.unwrap();
        assert_eq!(limited.len(), 10, "substrate {}", store.kind());
    }
}

#[tokio::test]
async fn every_substrate_replays_to_the_same_state() {
    let _exclusive = exclusive().await;
    let dir = tempfile::tempdir().unwrap();
    let batch = events(0..500);
    let mut digests = Vec::new();

    for store in substrates(&dir).await {
        store.append(&batch).await.unwrap();
        let (state, at) = replay_from_genesis(store.as_ref()).await.unwrap();
        assert_eq!(at, 500, "substrate {}", store.kind());
        digests.push((store.kind(), state.digest()));
    }

    // And the same state the pure fold produces from the same events — the oracle.
    let oracle = {
        let store = MemoryLog::new();
        let stamped = store.append(&batch).await.unwrap();
        fold(&stamped).digest()
    };

    for (kind, digest) in &digests {
        assert_eq!(digest, &oracle, "substrate {kind} disagrees with the fold");
    }
}

#[tokio::test]
async fn snapshots_are_an_optimisation_never_a_source_of_truth() {
    let _exclusive = exclusive().await;
    let dir = tempfile::tempdir().unwrap();
    for store in substrates(&dir).await {
        store.append(&events(0..300)).await.unwrap();

        let (at_150, _) = replay_to(store.as_ref(), 150).await.unwrap();
        store
            .put_snapshot(&Snapshot {
                seq: 150,
                state: at_150,
            })
            .await
            .unwrap();

        let (from_snapshot, at) = replay_to(store.as_ref(), 300).await.unwrap();
        let (from_genesis, _) = replay_from_genesis(store.as_ref()).await.unwrap();
        assert_eq!(at, 300, "substrate {}", store.kind());
        assert_eq!(
            from_snapshot.digest(),
            from_genesis.digest(),
            "substrate {}: the snapshot path must agree with genesis replay",
            store.kind()
        );

        let recovered = store.snapshot_at_or_before(299).await.unwrap();
        assert_eq!(
            recovered.map(|s| s.seq),
            Some(150),
            "substrate {}",
            store.kind()
        );
        assert!(
            store.snapshot_at_or_before(149).await.unwrap().is_none(),
            "substrate {}: must not return a snapshot from the future",
            store.kind()
        );
    }
}

#[tokio::test]
async fn a_reopened_redb_log_keeps_the_total_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reopen.redb");
    {
        let store = RedbLog::open(&path).unwrap();
        store.append(&events(0..10)).await.unwrap();
    }
    let store = RedbLog::open(&path).unwrap();
    assert_eq!(store.head().await.unwrap(), 10);
    let more = store.append(&events(10..12)).await.unwrap();
    assert_eq!(more[0].seq, 11);
    let (state, at) = replay_from_genesis(&store).await.unwrap();
    assert_eq!(at, 12);
    assert_eq!(state.len(), 12);
}
