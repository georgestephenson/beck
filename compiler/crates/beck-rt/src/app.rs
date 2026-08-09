//! The application: one merge point, one sequencer, one durable fold, N per-session views.
//!
//! ```text
//!  sockets ──▶ ingress channel ──▶ [ validate ─▶ append ─▶ fold ] ──▶ version watch ──▶ diff per subscriber
//!             (merge_clients:            the single writer;              coalescing:
//!              the one place time         seq assigned once              a slow client gets
//!              enters, §3.7)              (§3.7)                         fewer, bigger patches)
//! ```
//!
//! Phase 0 wrote this by hand to find out what it costs; Phase 1 keeps the shape — `docs/18`
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
    /// Whether a subscription maintains its view by delta rather than recomputing it (§5.3).
    ///
    /// On by default, because it is what §3.8 asks for and it is ~5× faster per event. It is a
    /// *switch* rather than a fact because it is also a memory-for-time trade — about 4× the bytes
    /// a subscription already held for its page
    /// ([`docs/24-incremental-views-report.md`](../../../../../docs/24-incremental-views-report.md)
    /// §24.6) — and an operator running a fanout of a hundred thousand idle sessions over a large
    /// accumulator should be able to decide that differently without recompiling.
    pub maintain_views: bool,
    /// Whether the operators that do not read the session are held **once** for every subscriber
    /// rather than once per subscriber (§5.3).
    ///
    /// On by default. It costs one lock acquisition per render — a read lock, so subscribers do not
    /// block each other — and saves every subscriber the arrangements below the accumulator that
    /// are the same computation for all of them. How much that is depends entirely on the program:
    /// a view that filters by the session immediately below the fold shares almost nothing, and one
    /// that sorts a public feed and personalises only the greeting shares almost everything
    /// ([`docs/26-arrangement-sharing-report.md`](../../../../../docs/26-arrangement-sharing-report.md)).
    ///
    /// Ignored when `maintain_views` is off: there are no arrangements to share.
    pub share_arrangements: bool,
    /// How long the shared dataflow keeps what a subscriber might still ask for.
    ///
    /// The default releases the arrangements when the last subscription ends and keeps at most 64
    /// versions of change history while one is open — but what is actually kept is the oldest
    /// connected subscriber's lag, so both numbers are ceilings rather than costs. A deployment
    /// whose clients reconnect constantly wants `release_when_idle` off, and one with slow clients
    /// and fast events wants a deeper history; neither should have to recompile for it
    /// ([`docs/26-arrangement-sharing-report.md`](../../../../../docs/26-arrangement-sharing-report.md)
    /// §26.9 asked for exactly this).
    ///
    /// Ignored when `share_arrangements` is off: there is no shared dataflow to retain anything.
    pub retention: beck_core::engine::Retention,
    /// Where an envelope's `at` comes from.
    ///
    /// A dependency rather than a tunable, and here because the merge point is "the one place time
    /// enters" (§3.7) and this is the configuration the merge point is built from. F11's constraint
    /// is that a clock is supplied and never ambient; `beck_core::clock` says why, and says what is
    /// deliberately not on the seam yet.
    pub clock: Arc<dyn beck_core::clock::Clock>,
    /// How a claimed identity becomes a verified one.
    ///
    /// A dependency rather than a tunable, for the same reason the clock is one, and here because
    /// the merge point is where a proposal acquires its actor. `DevIdentity` by default: `beck run`
    /// on a laptop must not need a secret, and `crate::identity` is where the consequences of that
    /// default are written down.
    pub identity: Arc<dyn crate::identity::Identity>,
    /// How much one actor may turn into permanent storage — F3's channel (b).
    ///
    /// A tunable rather than a dependency, and **on by default**, which is what
    /// [`docs/14`](../../../../../docs/14-review-findings.md) F3 decided: a quota a program has to
    /// ask for is a quota most programs do not have. [`crate::quota`] is the mechanism, the numbers
    /// and what the bound is actually worth.
    pub quota: crate::quota::Quota,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            snapshot_every: 1000,
            max_batch: 256,
            dedup_capacity: 16_384,
            maintain_views: true,
            share_arrangements: true,
            retention: beck_core::engine::Retention::default(),
            clock: Arc::new(beck_core::clock::SystemClock),
            identity: Arc::new(crate::identity::DevIdentity),
            quota: crate::quota::Quota::default(),
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
    config: AppConfig,
    /// §5.3's one shared dataflow: the plan's operators that do not read the session, maintained
    /// once for every subscription rather than once inside each. Advanced lazily by whichever
    /// subscriber renders first at a new version, so a process with no subscribers does no view
    /// work at all.
    shared: Arc<beck_core::engine::SharedDataflow>,
    /// F3's per-actor write quota. Held here rather than in the sequencer because it refuses
    /// *before* the queue: a proposal that will not be admitted should not occupy a slot in it.
    limit: crate::quota::RateLimit,
    /// Set once, when this process is going away. Every subscription watches it.
    ///
    /// §5.2 lists "graceful drain (finish folds, snapshot, hand off subscriptions)" among the
    /// things the runtime must ship, and the last of those three needs a subscription to *end*:
    /// `http::serve` stops accepting on shutdown, but a websocket that was already accepted is a
    /// task of its own and went on living for as long as the process did. A client whose server
    /// has drained should find out and reconnect — which, for a Mode B client, is also the moment
    /// its offline queue matters (`docs/94` §94.13).
    draining: watch::Sender<bool>,
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
        let shared = runtime.shared_dataflow(config.retention);
        let app = Arc::new(App {
            runtime,
            store,
            state: RwLock::new(state),
            version,
            ingress: tx,
            head: AtomicU64::new(head),
            config: config.clone(),
            shared,
            limit: crate::quota::RateLimit::new(config.quota),
            draining: watch::channel(false).0,
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

    /// Tell every subscription this process is going away.
    ///
    /// Idempotent, and one-way: an application that has drained does not come back, because the
    /// thing that would bring it back is a new process.
    pub fn drain(&self) {
        let _ = self.draining.send(true);
    }

    /// Watch for the drain. `true` already means it has happened.
    pub fn draining(&self) -> watch::Receiver<bool> {
        self.draining.subscribe()
    }

    pub async fn state(&self) -> Value {
        self.state.read().await.clone()
    }

    /// Render a subscriber's view of the current state, by full recompute.
    ///
    /// Kept for the two callers that render a state nobody is subscribed to: the server-side render
    /// of the first document, and the resumption path's reconstruction of the view as of an old
    /// `seq`. A live subscription goes through [`App::maintain`] instead.
    pub async fn render(&self, actor: &str) -> Result<beck_core::Html> {
        let state = self.state.read().await.clone();
        timed(&telemetry().view, || self.runtime.view(&state, actor))
    }

    /// An engine for one new subscription, of whichever kind this application is configured for.
    ///
    /// With sharing on it owns only the per-session operators; the rest arrive from the one shared
    /// dataflow. With it off it owns the whole plan, which is what every subscription did before
    /// `docs/26-arrangement-sharing-report.md`.
    pub fn view_engine(&self) -> Result<beck_core::engine::Engine> {
        if self.config.share_arrangements {
            Ok(self.shared.subscriber())
        } else {
            self.runtime.view_engine()
        }
    }

    /// Render a subscriber's view of the current state, by maintaining it. Returns the page **and
    /// the version it reflects**.
    ///
    /// The engine belongs to the subscription, so its arrangements survive between events and the
    /// per-event work is proportional to what the event changed (§5.3). The state is cloned under
    /// the read lock and the engine runs outside it — an `Arc` bump under the lock, and no
    /// rendering while the sequencer wants to write.
    ///
    /// The version is read **under the same lock** as the state, because the sequencer publishes
    /// both under its write lock, and a page paired with a `seq` it does not reflect is a wrong DOM
    /// after the client's next reconnect: a resuming client is served the difference from the `seq`
    /// its last frame carried. This used to be `app.head()` sampled after the render, which is a
    /// larger number whenever an event landed in between.
    pub async fn maintain(
        &self,
        engine: &mut beck_core::engine::Engine,
        actor: &str,
    ) -> Result<(beck_core::Html, Seq)> {
        let (state, version) = {
            let guard = self.state.read().await;
            (guard.clone(), self.head.load(Ordering::Relaxed))
        };
        timed(&telemetry().view, || {
            if !self.config.maintain_views {
                return Ok((self.runtime.view(&state, actor)?, version));
            }
            if self.config.share_arrangements {
                self.runtime
                    .render_shared(&self.shared, engine, &state, version, actor)
            } else {
                Ok((self.runtime.render(engine, &state, actor)?, version))
            }
        })
    }

    /// Whether subscriptions maintain their views (§5.3) or recompute them.
    /// How this process decides who is asking.
    ///
    /// Public because both edges — the socket and the document handler — have to ask the same
    /// question, and because the dashboard and the startup line have to be able to *say* which
    /// provider is in force. An operator who cannot tell from the logs whether authentication is
    /// on does not have authentication (`docs/48` §48.3).
    pub fn identity(&self) -> &Arc<dyn crate::identity::Identity> {
        &self.config.identity
    }

    pub fn maintains_views(&self) -> bool {
        self.config.maintain_views
    }

    /// Whether the operators that do not read the session are held once between subscribers.
    pub fn shares_arrangements(&self) -> bool {
        self.config.maintain_views && self.config.share_arrangements
    }

    /// The shared dataflow, for a measurement that wants to know what it holds.
    pub fn shared_dataflow(&self) -> &Arc<beck_core::engine::SharedDataflow> {
        &self.shared
    }

    /// Propose a command. Returns the `seq` its events landed at.
    ///
    /// The reply is the **ack**, and it means *committed* — not "your view has caught up". Phase 0
    /// found out the hard way that those are different facts (§18.5 item 1).
    pub async fn propose(&self, id: String, actor: String, command: Value) -> Result<Seq, String> {
        let (reply, rx) = oneshot::channel();
        // §3.7: the merge point is the one place time enters. It enters *here*, from the clock the
        // process was configured with, and is data on the envelope from this line onwards — which
        // is what makes a replay of that envelope reproduce the run rather than re-read the clock.
        let at = Instant(self.config.clock.now_millis());

        // F3's quota, charged from that same instant rather than from a second reading of a clock.
        // Refused *before* the queue: a proposal nothing will admit should not occupy a slot in it,
        // which is the difference between a quota and a slower queue.
        if !self.limit.admit(&actor, at.0) {
            telemetry().throttled.incr();
            tracing::warn!(actor, "refused: over the write quota");
            return Err("over the write quota".to_string());
        }
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

    /// Run something against a consistent snapshot of the accumulator and the version it is at.
    ///
    /// The read lock is held for the whole of `f`, which is what makes it a snapshot rather than
    /// two facts read at two times: the sequencer commits under the write lock, so nothing can move
    /// the state — and therefore nothing can advance the shared dataflow past this version — while
    /// this runs. [`crate::pgwire`] is the caller, and it is the one place a *reader* needs the two
    /// together; a rendering subscriber takes a clone instead, because a render is `O(page)` and a
    /// scan is `O(rows)`.
    pub async fn read_snapshot<T>(&self, f: impl FnOnce(&Value, Seq) -> T) -> T {
        let guard = self.state.read().await;
        f(&guard, self.head.load(Ordering::Relaxed))
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
    // The commands already appended, with the position each got. **The position is the point**:
    // §4.3 makes the id an idempotency key so "a retry after a reconnect is safe", and a retry is
    // only safe if the answer to the second attempt is the answer to the first. Remembering the id
    // alone let this reply "duplicate" — a *rejection* — to a command that had been accepted, so a
    // client replaying its offline queue was told its work had been refused and took it back off
    // the page (`docs/94` §94.13).
    let mut seen: VecDeque<(String, Seq)> = VecDeque::with_capacity(config.dedup_capacity);
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
            if let Some((_, at)) = seen.iter().find(|(id, _)| *id == p.id) {
                // Idempotency by envelope identity: a retry after a reconnect is safe (§4.3), and
                // it is safe because this is an **ack** carrying the position the first attempt
                // got, not a refusal. The command is in the log; saying so twice is the whole of
                // what idempotent means.
                telemetry().deduplicated.incr();
                let _ = p.reply.send(Ok(*at));
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
                        // Checked storable *before* the fold advances, so an event that cannot be
                        // written durably is refused rather than folded into a state the log
                        // cannot reproduce. A rejection here is a program that should not have
                        // compiled — `secure::storable` proves it cannot — but the boundary
                        // refuses rather than writing something lossy.
                        if let Err(why) = beck_core::repr::Repr::of(&e) {
                            failure = Some(why.to_string());
                            break;
                        }
                        let env = crate::log::Envelope {
                            seq,
                            at: p.at,
                            actor: p.actor.clone(),
                            body: e.clone(),
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
                            // The position is `base + pending.len()`: the last event this command
                            // produced, which is the seq its reply carries below.
                            seen.push_back((p.id, base + pending.len() as u64));
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
