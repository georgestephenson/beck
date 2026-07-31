//! The application: one merge point, one sequencer, one durable fold, N per-session views.
//!
//! ```text
//!  sockets ──▶ ingress channel ──▶ [ validate ─▶ append ─▶ fold ] ──▶ version watch ──▶ diff per subscriber
//!             (merge_clients:            the single writer;              coalescing:
//!              the one place time         seq assigned once              a slow client gets
//!              enters, §3.7)              (§3.7)                         fewer, bigger patches)
//! ```
//!
//! Phase 0 wrote this by hand to find out what it costs; Phase 1 keeps the shape — [`docs/18`]
//! §18.7 item 1: "Keep the sequencer shape. One merge point, one writer, group commit, fold under
//! the same lock as the append. It is simple, it is fast enough, and every property in §18.3.6
//! depends on it" — and drives it from a *compiled program* instead of from hand-written Rust.
//!
//! What changed: `validate`, `apply_event` and `view` are now `Core` the splitter handed over,
//! prepared by whichever [`beck_core::backend::Backend`] the process chose. Everything else — the
//! batching, the ordering, the ack-versus-frame protocol rule Phase 0 learned the hard way — is
//! unchanged, because it was never domain-specific.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use beck_core::Value;
use tokio::sync::{mpsc, oneshot, watch, RwLock};

use crate::log::{Instant, LogStore, Pending, Seq, Snapshot};
use crate::program::Runtime;
use crate::telemetry::{telemetry, timed};

#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Snapshot the fold every N events. "`durable` is the entire database administration story".
    pub snapshot_every: u64,
    /// Upper bound on group commit: the ingress task drains everything queued up to this many
    /// commands and appends their events in one statement. Phase 0 measured 11× for this.
    pub max_batch: usize,
    /// How many recent command ids to remember for idempotency (§4.3).
    pub dedup_capacity: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            snapshot_every: 1000,
            max_batch: 256,
            dedup_capacity: 16_384,
        }
    }
}

/// A client's proposal. Transient: de-duplicated by `id`, validated, and discarded (F3 — only
/// validated events are durably logged, so rejected traffic never becomes permanent storage).
struct Proposal {
    id: String,
    at: Instant,
    actor: String,
    command: Value,
    reply: oneshot::Sender<Result<Seq, String>>,
}

pub struct App {
    runtime: Arc<Runtime>,
    store: Arc<dyn LogStore>,
    state: RwLock<Value>,
    /// Bumped after every committed batch; subscribers wake on it. `watch` coalesces by design,
    /// which is exactly the backpressure behaviour a slow connection wants.
    version: watch::Sender<Seq>,
    ingress: mpsc::Sender<Proposal>,
    head: AtomicU64,
}

impl App {
    /// Recover from the log, then open ingress.
    ///
    /// Recovery is not a special mode: it is the same fold the runtime always runs, started from
    /// the newest snapshot. A process that has just been SIGKILLed and one that has just been
    /// deployed take exactly this path.
    ///
    /// Takes a prepared [`Runtime`] rather than a `Placed`, because building one requires choosing
    /// a backend, and that choice belongs to whoever assembles the process — not to the sequencer.
    pub async fn start(
        runtime: Runtime,
        store: Arc<dyn LogStore>,
        config: AppConfig,
    ) -> Result<Arc<App>> {
        let runtime = Arc::new(runtime);
        let head = store.head().await?;
        let (state, at) = replay_to(&runtime, store.as_ref(), head).await?;
        if at != head {
            bail!("recovery stopped at seq {at} but the log head is {head}");
        }

        telemetry().head.set(head);
        // The one line that matters at startup: a pod that was just killed and a pod that was just
        // deployed take exactly this path, and `seq` says which state it came back to.
        tracing::info!(seq = head, store = store.kind(), "recovered from the log");

        let (version, _) = watch::channel(head);
        let (tx, rx) = mpsc::channel::<Proposal>(1024);
        let app = Arc::new(App {
            runtime,
            store,
            state: RwLock::new(state),
            version,
            ingress: tx,
            head: AtomicU64::new(head),
        });
        tokio::spawn(sequencer(app.clone(), rx, config));
        Ok(app)
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn store_kind(&self) -> &'static str {
        self.store.kind()
    }

    pub fn head(&self) -> Seq {
        self.head.load(Ordering::Relaxed)
    }

    pub fn subscribe(&self) -> watch::Receiver<Seq> {
        self.version.subscribe()
    }

    pub async fn state(&self) -> Value {
        self.state.read().await.clone()
    }

    /// Render a subscriber's view of the current state.
    pub async fn render(&self, actor: &str) -> Result<beck_core::Html> {
        let state = self.state.read().await.clone();
        timed(&telemetry().view, || self.runtime.view(&state, actor))
    }

    /// Propose a command. Returns the `seq` its events landed at.
    ///
    /// The reply is the **ack**, and it means *committed* — not "your view has caught up". Phase 0
    /// found out the hard way that those are different facts (§18.5 item 1).
    pub async fn propose(&self, id: String, actor: String, command: Value) -> Result<Seq, String> {
        let (reply, rx) = oneshot::channel();
        let at = Instant(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        );
        self.ingress
            .send(Proposal {
                id,
                at,
                actor,
                command,
                reply,
            })
            .await
            .map_err(|_| "ingress is closed".to_string())?;
        rx.await
            .map_err(|_| "ingress dropped the proposal".to_string())?
    }

    /// The state as of `seq`, for a resuming subscriber.
    pub async fn state_at(&self, seq: Seq) -> Result<Value> {
        let (state, _) = replay_to(&self.runtime, self.store.as_ref(), seq).await?;
        Ok(state)
    }

    pub async fn floor(&self) -> Result<Seq> {
        self.store.floor().await
    }
}

/// The single writer. Everything about the total order lives in this one task.
async fn sequencer(app: Arc<App>, mut rx: mpsc::Receiver<Proposal>, config: AppConfig) {
    let mut seen: VecDeque<String> = VecDeque::with_capacity(config.dedup_capacity);
    let mut since_snapshot = 0u64;
    let mut batch: Vec<Proposal> = Vec::with_capacity(config.max_batch);

    while let Some(first) = rx.recv().await {
        batch.clear();
        batch.push(first);
        // Whatever else has already arrived rides along. The batch is exactly "what queued while
        // the last append was in flight", so the system self-tunes: latency at low load,
        // throughput at high load.
        while batch.len() < config.max_batch {
            match rx.try_recv() {
                Ok(p) => batch.push(p),
                Err(_) => break,
            }
        }

        // The write lock is held across validation *and* the append, because validation must see
        // the batch it is inside: `Add(x)` followed by `Toggle(x)` in one batch must work
        // (§18.5 item 5).
        let mut state = app.state.write().await;
        let mut pending: Vec<Pending> = Vec::new();
        let mut replies: Vec<(oneshot::Sender<Result<Seq, String>>, usize)> = Vec::new();
        let mut rejected: Vec<(oneshot::Sender<Result<Seq, String>>, String)> = Vec::new();
        let mut speculative = state.clone();
        let base = app.head.load(Ordering::Relaxed);

        for p in batch.drain(..) {
            if seen.contains(&p.id) {
                // Idempotency by envelope identity: a retry after a reconnect is safe (§4.3).
                telemetry().deduplicated.incr();
                rejected.push((p.reply, "duplicate".into()));
                continue;
            }
            let proposal = app.runtime.proposal(&p.actor, p.command.clone());
            match app.runtime.validate(&speculative, &proposal) {
                Ok(events) => {
                    if events.is_empty() {
                        rejected.push((p.reply, "no events".into()));
                        continue;
                    }
                    let mut failure: Option<String> = None;
                    for e in events {
                        let seq = base + pending.len() as u64 + 1;
                        // Encoded *before* the fold advances, so an event that cannot be written
                        // durably is refused rather than folded into a state the log cannot
                        // reproduce. A rejection here is a program that should not have compiled —
                        // Phase 2's effect rows should prove it cannot happen — but until they can,
                        // the boundary refuses instead of writing something lossy.
                        let body = match beck_core::core::value_to_repr(&e) {
                            Ok(body) => body,
                            Err(why) => {
                                failure = Some(why.to_string());
                                break;
                            }
                        };
                        let env = crate::log::Envelope {
                            seq,
                            at: p.at,
                            actor: p.actor.clone(),
                            body,
                        };
                        // Apply as we validate, so the next command in the batch sees it:
                        // `Add(x)` followed by `Toggle(x)` in one batch must work.
                        match timed(&telemetry().fold, || {
                            app.runtime.fold(&speculative, &env, e.clone())
                        }) {
                            Ok(next) => speculative = next,
                            Err(err) => {
                                failure = Some(err.to_string());
                                break;
                            }
                        }
                        pending.push(Pending {
                            at: p.at,
                            actor: p.actor.clone(),
                            body: e,
                        });
                    }
                    match failure {
                        Some(why) => rejected.push((p.reply, why)),
                        None => {
                            if seen.len() >= config.dedup_capacity {
                                seen.pop_front();
                            }
                            seen.push_back(p.id);
                            replies.push((p.reply, pending.len()));
                        }
                    }
                }
                Err(why) => rejected.push((p.reply, why)),
            }
        }

        for (reply, why) in rejected {
            telemetry().rejected.incr();
            let _ = reply.send(Err(why));
        }
        if pending.is_empty() {
            continue;
        }

        let append_started = std::time::Instant::now();
        let appended = app.store.append(&pending).await;
        telemetry().append.record(append_started.elapsed());
        match appended {
            Ok(stamped) => {
                // The predicted seqs must match what the store assigned. That assertion is how a
                // second writer would be caught (§18.5 item 5).
                for (i, env) in stamped.iter().enumerate() {
                    if env.seq != base + i as u64 + 1 {
                        tracing::error!(
                            expected = base + i as u64 + 1,
                            actual = env.seq,
                            "the log assigned a seq the sequencer did not predict"
                        );
                        std::process::abort();
                    }
                }
                let head = stamped.last().map(|e| e.seq).unwrap_or(base);
                telemetry().events_appended.add(stamped.len() as u64);
                telemetry().head.set(head);
                *state = speculative;
                app.head.store(head, Ordering::Relaxed);
                drop(state);

                for (reply, offset) in replies {
                    let _ = reply.send(Ok(base + offset as u64));
                }
                let _ = app.version.send(head);

                since_snapshot += stamped.len() as u64;
                if since_snapshot >= config.snapshot_every {
                    since_snapshot = 0;
                    let snapshot = Snapshot {
                        seq: head,
                        state: app.state.read().await.clone(),
                    };
                    let started = std::time::Instant::now();
                    let put = app.store.put_snapshot(&snapshot).await;
                    telemetry().snapshot.record(started.elapsed());
                    match put {
                        Ok(()) => tracing::info!(seq = head, "snapshot written"),
                        Err(e) => {
                            telemetry().snapshot_failures.incr();
                            tracing::warn!(
                                error = %e, seq = head,
                                "snapshot failed; the log is still the truth"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                // "A failed append has no repair path, and that is correct." The process's state
                // is ahead of the durable truth and there is nothing to reconcile: abort, and the
                // next process folds the log (§18.5 item 6).
                telemetry().append_failures.incr();
                tracing::error!(error = %e, seq = base + 1, "append failed after the fold advanced; aborting");
                std::process::abort();
            }
        }
    }
}

/// Fold the log from the best available starting point — a snapshot if there is one, genesis
/// otherwise. This is `beck replay`, and the resumption path uses it to reconstruct the view a
/// reconnecting subscriber last saw.
pub async fn replay_to(
    runtime: &Runtime,
    store: &dyn LogStore,
    target: Seq,
) -> Result<(Value, Seq)> {
    let (mut state, mut at) = match store.snapshot_at_or_before(target).await? {
        Some(s) => (s.state, s.seq),
        None => (runtime.initial_state()?, 0),
    };

    const CHUNK: usize = 4096;
    while at < target {
        let batch = store.read(at, CHUNK.min((target - at) as usize)).await?;
        if batch.is_empty() {
            break;
        }
        for env in &batch {
            let event = env.event()?;
            state = runtime.fold(&state, env, event)?;
            at = env.seq;
        }
    }
    Ok((state, at))
}

/// Fold the whole log from genesis, ignoring snapshots.
///
/// D3's genesis-replay discipline: snapshots are an optimisation, and a snapshot that disagrees
/// with the log is a bug we want CI to find, not a fact we want to trust.
pub async fn replay_from_genesis(runtime: &Runtime, store: &dyn LogStore) -> Result<(Value, Seq)> {
    let started = std::time::Instant::now();
    let mut state = runtime.initial_state()?;
    let mut at = 0;
    loop {
        let batch = store.read(at, 4096).await?;
        if batch.is_empty() {
            // Recorded here rather than per event: a replay is one operation from the operator's
            // point of view — "how long was this pod down for" — and the per-event cost is what
            // `tests/scaling.rs` measures.
            telemetry().replay.record(started.elapsed());
            return Ok((state, at));
        }
        for env in &batch {
            let event = env.event()?;
            state = runtime.fold(&state, env, event)?;
            at = env.seq;
        }
    }
}
