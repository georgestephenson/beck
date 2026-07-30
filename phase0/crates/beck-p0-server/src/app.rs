//! The application: one merge point, one sequencer, one durable fold, N per-session views.
//!
//! ```text
//!  sockets ──▶ ingress channel ──▶ [ validate ─▶ append ─▶ fold ] ──▶ version watch ──▶ diff per subscriber
//!             (merge_clients:            the single writer;              coalescing:
//!              the one place time         seq assigned once              a slow client gets
//!              enters, §3.7)              (§3.7)                         fewer, bigger patches)
//! ```
//!
//! Every arrow above is something stage 8 of the compiler will synthesise (§4.3); Phase 0 writes
//! it out by hand to find out what it costs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use uuid::Uuid;

use beck_p0_core::domain::{validate, ActorId, Command, Rejection, Session, TodoState};
use beck_p0_core::envelope::{Envelope, Instant, Seq};
use beck_p0_core::html::Html;
use beck_p0_core::protocol::{Resumption, ScopeSel};
use beck_p0_core::view::{page, Scope};
use beck_p0_log::{replay_to, LogStore, PendingEvent, Snapshot};

use crate::metrics::Metrics;

#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Snapshot the fold every N events. "`durable` is the entire database administration story"
    /// — this is the whole of it, plus retention, which Phase 0 does not exercise.
    pub snapshot_every: u64,
    /// Upper bound on group commit. The ingress task drains everything queued up to this many
    /// commands and appends their events in one statement; it is the difference between
    /// per-command fsync and a respectable events/s number.
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

struct Proposal {
    id: Uuid,
    at: Instant,
    session: Session,
    command: Command,
    reply: oneshot::Sender<Result<Seq, Rejection>>,
}

pub struct App {
    store: Arc<dyn LogStore>,
    state: RwLock<TodoState>,
    /// Bumped after every committed batch; subscribers wake on it. `watch` coalesces by design,
    /// which is exactly the backpressure behaviour we want on a slow connection.
    version: watch::Sender<Seq>,
    ingress: mpsc::Sender<Proposal>,
    metrics: Arc<Metrics>,
    head: AtomicU64,
}

impl App {
    /// Recover from the log, then open ingress.
    ///
    /// Recovery is not a special mode: it is the same fold the runtime always runs, started from
    /// the newest snapshot. A process that has just been killed mid-stream and one that has just
    /// been deployed take exactly this path.
    pub async fn start(
        store: Arc<dyn LogStore>,
        config: AppConfig,
        metrics: Arc<Metrics>,
    ) -> Result<Arc<App>> {
        let started = std::time::Instant::now();
        let (state, head) = replay_to(store.as_ref(), Seq::MAX).await?;
        metrics
            .recovery_millis
            .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        metrics.recovered_to.store(head, Ordering::Relaxed);
        tracing::info!(
            store = store.kind(),
            head,
            todos = state.len(),
            millis = started.elapsed().as_millis() as u64,
            "recovered by folding the log"
        );

        let (version, _) = watch::channel(head);
        let (ingress, rx) = mpsc::channel(4096);

        let app = Arc::new(App {
            store,
            state: RwLock::new(state),
            version,
            ingress,
            metrics,
            head: AtomicU64::new(head),
        });

        tokio::spawn(sequencer(app.clone(), rx, config));
        Ok(app)
    }

    pub fn head(&self) -> Seq {
        self.head.load(Ordering::Acquire)
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    pub fn store_kind(&self) -> &'static str {
        self.store.kind()
    }

    pub fn watch_version(&self) -> watch::Receiver<Seq> {
        self.version.subscribe()
    }

    /// `send!` from the browser: propose a command and wait for the server's decision.
    ///
    /// The wait is deliberate — Mode A has no optimistic application, so the client learns the
    /// outcome when the authoritative patch arrives (§5.1).
    pub async fn propose(
        &self,
        id: Uuid,
        actor: ActorId,
        command: Command,
    ) -> Result<Seq, Rejection> {
        let (reply, wait) = oneshot::channel();
        let proposal = Proposal {
            id,
            at: now(),
            session: Session::new(actor),
            command,
            reply,
        };
        self.metrics.commands_in.fetch_add(1, Ordering::Relaxed);
        if self.ingress.send(proposal).await.is_err() {
            // Ingress is closed: the process is draining. The client will retry on reconnect, and
            // its command id makes that retry safe.
            return Err(Rejection::NoSuchTodo);
        }
        match wait.await {
            Ok(result) => result,
            Err(_) => Err(Rejection::NoSuchTodo),
        }
    }

    /// The current view for a scope, with the `seq` it reflects.
    pub async fn view_now(&self, scope: &Scope) -> (Seq, Html) {
        let state = self.state.read().await;
        // Read the head *inside* the lock so the frame's seq and its contents cannot disagree.
        (self.head(), page(&state, scope))
    }

    /// Establish or resume a subscription.
    ///
    /// Returns the view the client already has (so the caller can diff against it) and how that
    /// was arrived at. This is the exit criterion "does resumption actually replay the gap"; the
    /// answer is yes, by folding the log from the newest snapshot at or before the client's `seq`
    /// — which means it works across replicas and across a deploy, because it depends on log
    /// position and nothing else (§5.1).
    pub async fn resume(&self, from: Seq, scope: &Scope) -> Result<(Resumption, Html)> {
        let head = self.head();
        if from == 0 {
            self.metrics
                .resumptions_fresh
                .fetch_add(1, Ordering::Relaxed);
            return Ok((Resumption::Fresh, Html::el("main")));
        }
        if from == head {
            // The overwhelmingly common case: the client is up to date, usually because it just
            // received a server-rendered first paint stamped with this very `seq`. No replay, no
            // catch-up patch, no work proportional to the log.
            self.metrics
                .resumptions_resumed
                .fetch_add(1, Ordering::Relaxed);
            let state = self.state.read().await;
            return Ok((
                Resumption::Resumed { from, replayed: 0 },
                page(&state, scope),
            ));
        }

        let floor = self.store.floor().await?;
        if from > head || (floor > 0 && from < floor - 1) {
            self.metrics
                .resumptions_reset
                .fetch_add(1, Ordering::Relaxed);
            return Ok((Resumption::Reset { from }, Html::el("main")));
        }

        let started = std::time::Instant::now();
        let (state_then, at) = replay_to(self.store.as_ref(), from).await?;
        if at != from {
            self.metrics
                .resumptions_reset
                .fetch_add(1, Ordering::Relaxed);
            return Ok((Resumption::Reset { from }, Html::el("main")));
        }
        self.metrics
            .resumptions_resumed
            .fetch_add(1, Ordering::Relaxed);
        self.metrics
            .resume_replay_micros
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        Ok((
            Resumption::Resumed {
                from,
                replayed: head.saturating_sub(from),
            },
            page(&state_then, scope),
        ))
    }

    /// Stop accepting commands. In-flight batches finish, the fold is snapshotted, and open
    /// subscriptions are left to resume against the next process — the drain half of §6.4's
    /// deploy choreography.
    pub async fn drain(&self) {
        self.ingress.closed().await;
    }

    pub async fn snapshot_now(&self) -> Result<()> {
        let state = self.state.read().await.clone();
        let seq = self.head();
        self.store.put_snapshot(&Snapshot { seq, state }).await?;
        self.metrics.snapshots.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Test/bench hook: propose without a socket in the way.
    pub async fn propose_blocking(&self, actor: &str, command: Command) -> Result<Seq, Rejection> {
        self.propose(Uuid::new_v4(), ActorId::new(actor), command)
            .await
    }
}

/// The single writer. Everything downstream of it is deterministic.
async fn sequencer(app: Arc<App>, mut rx: mpsc::Receiver<Proposal>, config: AppConfig) {
    let mut dedup = Dedup::new(config.dedup_capacity);
    let mut batch: Vec<Proposal> = Vec::with_capacity(config.max_batch);
    let mut last_snapshot = app.head();

    while let Some(first) = rx.recv().await {
        batch.push(first);
        while batch.len() < config.max_batch {
            match rx.try_recv() {
                Ok(p) => batch.push(p),
                Err(_) => break,
            }
        }
        app.metrics.batches.fetch_add(1, Ordering::Relaxed);
        app.metrics
            .batched_commands
            .fetch_add(batch.len() as u64, Ordering::Relaxed);

        if let Err(e) = commit(&app, &mut batch, &mut dedup).await {
            // The in-memory fold has advanced past a log that refused the write. There is no
            // repair path and there should not be one: the fold is downstream of the log by
            // construction, so the only correct recovery is to die and replay.
            tracing::error!(error = %e, "log append failed; aborting so recovery replays the log");
            std::process::abort();
        }
        batch.clear();

        let head = app.head();
        if head.saturating_sub(last_snapshot) >= config.snapshot_every {
            last_snapshot = head;
            if let Err(e) = app.snapshot_now().await {
                tracing::warn!(error = %e, "snapshot failed; the log is still authoritative");
            }
        }
    }
}

async fn commit(app: &Arc<App>, batch: &mut [Proposal], dedup: &mut Dedup) -> Result<()> {
    let mut state = app.state.write().await;
    let head = app.head();

    let mut pending: Vec<PendingEvent> = Vec::with_capacity(batch.len());
    // (proposal index, seq the proposal's last event will land at)
    let mut outcomes: Vec<(usize, Result<Seq, Rejection>)> = Vec::with_capacity(batch.len());

    for (i, proposal) in batch.iter().enumerate() {
        if let Some(seq) = dedup.get(&proposal.id) {
            // A retry after a reconnect: the command already happened exactly once (§4.3).
            app.metrics.commands_deduped.fetch_add(1, Ordering::Relaxed);
            outcomes.push((i, Ok(seq)));
            continue;
        }
        match validate(&state, &proposal.session, &proposal.command) {
            Ok(events) => {
                let mut last = 0;
                for event in events {
                    let seq = head + pending.len() as Seq + 1;
                    let envelope = Envelope::new(
                        seq,
                        proposal.at,
                        proposal.session.actor.clone(),
                        event.clone(),
                    );
                    // Applied here, under the same lock that will hold across the append: no
                    // reader can observe the fold ahead of the log, and validation of later
                    // commands in this batch sees the effects of earlier ones.
                    beck_p0_core::domain::apply_event(&mut state, &envelope);
                    pending.push(PendingEvent::new(
                        proposal.at,
                        proposal.session.actor.clone(),
                        event,
                    ));
                    last = seq;
                }
                dedup.insert(proposal.id, last);
                outcomes.push((i, Ok(last)));
            }
            Err(rejection) => {
                // Rejected traffic is never logged (F3).
                app.metrics
                    .commands_rejected
                    .fetch_add(1, Ordering::Relaxed);
                outcomes.push((i, Err(rejection)));
            }
        }
    }

    if !pending.is_empty() {
        let stamped = app.store.append(&pending).await?;
        let first = stamped.first().expect("non-empty batch").seq;
        let last = stamped.last().expect("non-empty batch").seq;
        if first != head + 1 || last != head + stamped.len() as Seq {
            bail!(
                "log assigned {first}..={last}, expected {}..={}; a second writer exists",
                head + 1,
                head + stamped.len() as Seq
            );
        }
        app.head.store(last, Ordering::Release);
        app.metrics
            .events_committed
            .fetch_add(stamped.len() as u64, Ordering::Relaxed);
    }

    // Drop the write lock before waking subscribers, so a thundering herd of diffs takes read
    // locks against a state nobody is writing.
    drop(state);

    for (i, outcome) in outcomes {
        let proposal = &mut batch[i];
        let (reply, _) = oneshot::channel();
        let reply = std::mem::replace(&mut proposal.reply, reply);
        let _ = reply.send(outcome);
    }

    if !pending.is_empty() {
        let _ = app.version.send(app.head());
    }
    Ok(())
}

/// Bounded idempotency memory: command id → the seq it produced.
struct Dedup {
    capacity: usize,
    order: std::collections::VecDeque<Uuid>,
    seen: std::collections::HashMap<Uuid, Seq>,
}

impl Dedup {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: std::collections::VecDeque::with_capacity(capacity),
            seen: std::collections::HashMap::with_capacity(capacity),
        }
    }

    fn get(&self, id: &Uuid) -> Option<Seq> {
        self.seen.get(id).copied()
    }

    fn insert(&mut self, id: Uuid, seq: Seq) {
        if self.seen.insert(id, seq).is_none() {
            self.order.push_back(id);
            if self.order.len() > self.capacity {
                if let Some(old) = self.order.pop_front() {
                    self.seen.remove(&old);
                }
            }
        }
    }
}

/// Wall-clock at ingress, captured as data. The only clock read in the whole application, and it
/// happens *outside* every fold (§3.7).
pub fn now() -> Instant {
    Instant(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis() as i64,
    )
}

pub fn scope_of(sel: ScopeSel, actor: &ActorId) -> Scope {
    match sel {
        ScopeSel::All => Scope::Everyone,
        ScopeSel::Mine => Scope::Mine(actor.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_p0_core::domain::Id;
    use beck_p0_log::MemoryLog;

    async fn app() -> Arc<App> {
        App::start(
            Arc::new(MemoryLog::new()),
            AppConfig::default(),
            Arc::new(Metrics::default()),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_command_becomes_an_event_a_fold_and_a_view() {
        let app = app().await;
        let id = Id::from_u128(1);
        let seq = app
            .propose_blocking(
                "alice",
                Command::Add {
                    id,
                    text: "write the runtime".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(seq, 1);

        let (at, html) = app.view_now(&Scope::Everyone).await;
        assert_eq!(at, 1);
        assert!(html.render().contains("write the runtime"));
    }

    #[tokio::test]
    async fn later_commands_in_a_batch_see_earlier_ones() {
        let app = app().await;
        let id = Id::from_u128(1);
        // Issued concurrently so they land in the same group commit.
        let (add, toggle) = tokio::join!(
            app.propose_blocking(
                "alice",
                Command::Add {
                    id,
                    text: "x".into()
                }
            ),
            async {
                // Give the Add a head start within the same batch window.
                tokio::task::yield_now().await;
                app.propose_blocking("alice", Command::Toggle { id }).await
            }
        );
        assert!(add.is_ok());
        assert_eq!(toggle, Ok(2), "a toggle must see the add that preceded it");
    }

    #[tokio::test]
    async fn retrying_a_command_id_does_not_duplicate_the_event() {
        let app = app().await;
        let id = Id::from_u128(1);
        let command_id = Uuid::new_v4();
        let cmd = Command::Add {
            id,
            text: "once".into(),
        };
        let first = app
            .propose(command_id, ActorId::new("alice"), cmd.clone())
            .await
            .unwrap();
        let retry = app
            .propose(command_id, ActorId::new("alice"), cmd)
            .await
            .unwrap();
        assert_eq!(first, retry);
        assert_eq!(app.head(), 1, "the retry must not have appended anything");
    }

    #[tokio::test]
    async fn resumption_replays_the_gap_rather_than_resetting() {
        let app = app().await;
        for i in 1..=5u128 {
            app.propose_blocking(
                "alice",
                Command::Add {
                    id: Id::from_u128(i),
                    text: format!("todo {i}"),
                },
            )
            .await
            .unwrap();
        }
        let (how, view_then) = app.resume(3, &Scope::Everyone).await.unwrap();
        assert_eq!(
            how,
            Resumption::Resumed {
                from: 3,
                replayed: 2
            }
        );

        let (seq, view_now) = app.view_now(&Scope::Everyone).await;
        assert_eq!(seq, 5);
        let ops = beck_p0_core::diff(&view_then, &view_now);
        // Two todos arrived while the client was away: two inserts and the footer text.
        assert!(!ops.is_empty());
        assert!(ops.len() <= 4, "gap patch should be small, got {ops:#?}");
        assert_eq!(beck_p0_core::diff::apply(&view_then, &ops), view_now);
    }

    #[tokio::test]
    async fn a_seq_from_the_future_resets_instead_of_lying() {
        let app = app().await;
        let (how, _) = app.resume(99, &Scope::Everyone).await.unwrap();
        assert_eq!(how, Resumption::Reset { from: 99 });
    }
}
