//! The replay-determinism harness.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.8: "Fold the
//! same recorded log twice (and across dev/release backends); assert bit-identical states **and
//! patch streams**." The second is the stronger claim, and the one that makes time-travel debugging
//! and log-backed property tests fall out of the semantics rather than out of a framework.
//!
//! §3.7 is what makes it true: `fold`'s function must be replay-pure, so the checker rejects
//! `now()`, `rand()` and `uuid()` inside a fold — time is data on the envelope. This file is that
//! rule, cashed in.

use std::sync::Arc;

use beck_core::{digest, Value};
use beck_rt::{diff, replay_from_genesis, replay_to, App, AppConfig, LogStore, MemoryLog};

mod support;
use support::{command, todo_runtime, ACTORS};

/// Drive a log into existence, then hand back the store.
async fn recorded_log(events: usize) -> (Arc<MemoryLog>, Vec<u8>) {
    let store = Arc::new(MemoryLog::new());
    let app = App::start(todo_runtime(), store.clone(), AppConfig::default())
        .await
        .expect("app starts");

    for i in 0..events {
        let actor = ACTORS[i % ACTORS.len()];
        let id = format!("t{i}");
        let _ = app
            .propose(
                format!("add-{i}"),
                actor.to_string(),
                command("Add", &[("id", &id), ("text", &format!("todo {i}"))]),
            )
            .await;
        if i % 3 == 0 {
            let _ = app
                .propose(
                    format!("toggle-{i}"),
                    actor.to_string(),
                    command("Toggle", &[("id", &id)]),
                )
                .await;
        }
        if i % 7 == 0 {
            let _ = app
                .propose(
                    format!("delete-{i}"),
                    actor.to_string(),
                    command("Delete", &[("id", &id)]),
                )
                .await;
        }
    }
    let state = app.state().await;
    (store, digest(&state).to_vec())
}

#[tokio::test]
async fn folding_the_same_log_twice_produces_bit_identical_state() {
    let (store, live_digest) = recorded_log(60).await;
    let runtime = todo_runtime();
    let head = store.head().await.expect("head");
    assert!(head > 0, "the harness recorded nothing");

    let (first, at1) = replay_to(&runtime, store.as_ref(), head)
        .await
        .expect("replay");
    let (second, at2) = replay_to(&runtime, store.as_ref(), head)
        .await
        .expect("replay");
    assert_eq!(at1, head);
    assert_eq!(at2, head);
    assert_eq!(digest(&first), digest(&second));

    // …and it agrees with the state the live process held, which is the property that makes a
    // SIGKILL survivable: the process that folds the log arrives where the dead one was.
    assert_eq!(digest(&first).to_vec(), live_digest);
}

#[tokio::test]
async fn the_snapshot_path_agrees_with_a_fold_from_genesis() {
    // D3's genesis-replay discipline: snapshots are pure optimisation, and one that disagrees with
    // the log is a bug CI should find rather than a fact to trust.
    let (store, _) = recorded_log(40).await;
    let runtime = todo_runtime();
    let head = store.head().await.expect("head");

    let mid = head / 2;
    let (mid_state, _) = replay_to(&runtime, store.as_ref(), mid)
        .await
        .expect("replay");
    store
        .put_snapshot(&beck_rt::Snapshot {
            seq: mid,
            state: mid_state,
        })
        .await
        .expect("snapshot");

    let (via_snapshot, _) = replay_to(&runtime, store.as_ref(), head)
        .await
        .expect("replay");
    let (from_genesis, _) = replay_from_genesis(&runtime, store.as_ref())
        .await
        .expect("genesis");
    assert_eq!(digest(&via_snapshot), digest(&from_genesis));
}

#[tokio::test]
async fn the_patch_stream_is_bit_identical_on_replay() {
    // The stronger claim. Re-deriving the patch stream over a log has to produce the same bytes,
    // or time-travel debugging and patch-level property tests are built on sand.
    let (store, _) = recorded_log(30).await;
    let runtime = todo_runtime();
    let head = store.head().await.expect("head");

    async fn stream(store: &MemoryLog, head: u64, actor: &str) -> Vec<String> {
        let runtime = todo_runtime();
        let mut state: Value = runtime.initial_state().expect("initial");
        let mut last = runtime.view(&state, actor).expect("view");
        let mut frames: Vec<String> = Vec::new();
        for seq in 1..=head {
            let envs = store.read(seq - 1, 1).await.expect("read");
            let Some(env) = envs.first() else { break };
            let event = env.event().expect("event");
            state = runtime.fold(&state, env, event).expect("fold");
            let now = runtime.view(&state, actor).expect("view");
            let ops = diff(&last, &now);
            last = now;
            if !ops.is_empty() {
                frames.push(beck_rt::PatchFrame::new(env.seq, ops).to_json().to_string());
            }
        }
        frames
    }

    let first = stream(&store, head, "alice").await;
    let second = stream(&store, head, "alice").await;
    assert!(!first.is_empty(), "the harness produced no patches");
    assert_eq!(first, second, "the patch stream is not reproducible");

    // A different subscriber sees a different — but equally reproducible — stream, which is what
    // a per-session view means.
    let bob = stream(&store, head, "bob").await;
    assert_ne!(first, bob);
    assert_eq!(bob, stream(&store, head, "bob").await);
    let _ = runtime;
}

#[tokio::test]
async fn a_new_process_folding_the_log_lands_where_the_old_one_was() {
    // The operational property: everything acknowledged survives, with no drain, no snapshot and
    // no destructors. Phase 0 asserted this by SIGKILLing a process; here the *second* `App::start`
    // is the new process, and it recovers by folding.
    let (store, live_digest) = recorded_log(25).await;
    let reborn = App::start(todo_runtime(), store.clone(), AppConfig::default())
        .await
        .expect("recovers");
    assert_eq!(digest(&reborn.state().await).to_vec(), live_digest);
    assert_eq!(reborn.head(), store.head().await.expect("head"));

    // And it keeps going from there.
    reborn
        .propose(
            "after".into(),
            "alice".into(),
            command("Add", &[("id", "after"), ("text", "after recovery")]),
        )
        .await
        .expect("accepted");
    assert!(reborn
        .render("alice")
        .await
        .expect("render")
        .render()
        .contains("after recovery"));
}
