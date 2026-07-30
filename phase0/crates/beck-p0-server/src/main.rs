//! `beck-p0` — the Phase 0 binary.
//!
//! The subcommands are the ones the roadmap's exit criteria need: run the app, replay the log,
//! and verify that replay is exact. They are named after the `beck` commands they stand in for
//! (§3.7's `beck replay`), because the point of Phase 0 is to find out what those commands cost
//! before there is a compiler to emit them.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use beck_p0_core::diff::diff;
use beck_p0_core::domain::{apply_event, Command, Id, TodoState};
use beck_p0_core::envelope::Seq;
use beck_p0_core::patch::PatchFrame;
use beck_p0_core::view::{page, Scope};
use beck_p0_log::{replay_from_genesis, replay_to, LogStore, MemoryLog, PgLog, RedbLog};
use beck_p0_server::app::{App, AppConfig};
use beck_p0_server::{http, Metrics};

#[derive(Parser)]
#[command(
    name = "beck-p0",
    version,
    about = "Phase 0: the todo sketch, hand-compiled"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve the app: ingress, fold, per-session views, patch fanout.
    Run {
        #[arg(long, default_value = "0.0.0.0:8080")]
        addr: SocketAddr,
        #[command(flatten)]
        store: StoreArgs,
        #[arg(long, default_value_t = 1000)]
        snapshot_every: u64,
    },
    /// Fold the log and report the state it produces — `beck replay`.
    Replay {
        #[command(flatten)]
        store: StoreArgs,
        /// Replay to this position instead of the head.
        #[arg(long)]
        to: Option<Seq>,
        /// Ignore snapshots and fold from genesis (D3's genesis-replay discipline).
        #[arg(long)]
        genesis: bool,
    },
    /// Assert that replay is exact: same state *and* same patch stream, every time.
    Verify {
        #[command(flatten)]
        store: StoreArgs,
        /// How many events to verify the *patch stream* over. State determinism is always checked
        /// across the whole log; re-deriving the patch stream means re-rendering and diffing the
        /// view after every event, which with v0.1's full-recompute views costs O(events × rows)
        /// — quadratic on a log whose list only grows. Phase 3's incremental views remove the
        /// quadratic term; until then this is bounded and the bound is reported.
        #[arg(long, default_value_t = 2000)]
        patch_limit: u64,
    },
    /// Fold to the head and write a snapshot — what the generated snapshot `CronJob` runs.
    Snapshot {
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Append synthetic traffic through the real ingress path.
    Seed {
        #[command(flatten)]
        store: StoreArgs,
        #[arg(long, default_value_t = 1000)]
        events: usize,
        #[arg(long, default_value_t = 8)]
        actors: usize,
    },
}

#[derive(Args, Clone)]
struct StoreArgs {
    #[arg(long, value_enum, default_value_t = StoreKind::Redb)]
    store: StoreKind,
    #[arg(long, default_value = "beck-p0.redb")]
    redb_path: PathBuf,
    #[arg(
        long,
        env = "BECK_PG",
        default_value = "postgres://postgres@localhost/beck_p0"
    )]
    pg: String,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum StoreKind {
    /// Rung 0 without a file: no durability, same total order.
    Memory,
    /// Rung 0: embedded, durable, replayable (§6.6).
    Redb,
    /// The v1 durable substrate (§5.3).
    Postgres,
}

impl StoreArgs {
    async fn open(&self) -> Result<Arc<dyn LogStore>> {
        Ok(match self.store {
            StoreKind::Memory => Arc::new(MemoryLog::new()),
            StoreKind::Redb => Arc::new(RedbLog::open(&self.redb_path)?),
            StoreKind::Postgres => Arc::new(PgLog::connect(&self.pg).await?),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    match Cli::parse().command {
        Cmd::Run {
            addr,
            store,
            snapshot_every,
        } => run(addr, store, snapshot_every).await,
        Cmd::Replay { store, to, genesis } => replay(store, to, genesis).await,
        Cmd::Verify { store, patch_limit } => verify(store, patch_limit).await,
        Cmd::Snapshot { store } => snapshot(store).await,
        Cmd::Seed {
            store,
            events,
            actors,
        } => seed(store, events, actors).await,
    }
}

async fn run(addr: SocketAddr, store: StoreArgs, snapshot_every: u64) -> Result<()> {
    let store = store.open().await?;
    let metrics = Arc::new(Metrics::default());
    let app = App::start(
        store,
        AppConfig {
            snapshot_every,
            ..AppConfig::default()
        },
        metrics,
    )
    .await?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let serving = tokio::spawn(http::serve(app.clone(), addr, shutdown_rx));

    // The drain half of §6.4's deploy choreography: stop taking traffic, snapshot, and leave the
    // log as the only thing that matters. Subscribers resume against the next process by
    // (subscription, seq) — which is the behaviour the Phase 0 exit criteria ask us to prove.
    wait_for_termination().await;
    tracing::info!("termination signal: draining");
    let _ = shutdown_tx.send(true);
    if let Err(e) = app.snapshot_now().await {
        tracing::warn!(error = %e, "snapshot on drain failed; the log is still authoritative");
    }
    serving.abort();
    tracing::info!(head = app.head(), "drained");
    Ok(())
}

async fn wait_for_termination() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn replay(store: StoreArgs, to: Option<Seq>, genesis: bool) -> Result<()> {
    let store = store.open().await?;
    let started = std::time::Instant::now();
    let (state, at) = if genesis {
        replay_from_genesis(store.as_ref()).await?
    } else {
        replay_to(store.as_ref(), to.unwrap_or(Seq::MAX)).await?
    };
    let elapsed = started.elapsed();

    println!("store        {}", store.kind());
    println!("replayed to  {at}");
    println!("todos        {}", state.len());
    println!("digest       {}", hex(&state.digest()));
    println!(
        "took         {:.3} ms ({:.0} events/s)",
        elapsed.as_secs_f64() * 1000.0,
        at as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
    );
    Ok(())
}

/// The replay-determinism harness (§4.8): fold the same log twice and assert bit-identical states
/// *and* patch streams, then assert that the snapshot path agrees with genesis.
async fn verify(store: StoreArgs, patch_limit: u64) -> Result<()> {
    let store = store.open().await?;
    let head = store.head().await?;
    if head == 0 {
        bail!("the log is empty: seed it first (`beck-p0 seed --events 1000`)");
    }

    // State determinism over the whole log: two folds, no snapshots, compare digests.
    let started = std::time::Instant::now();
    let (first_state, _) = replay_from_genesis(store.as_ref()).await?;
    let (second_state, _) = replay_from_genesis(store.as_ref()).await?;
    let state_seconds = started.elapsed().as_secs_f64() / 2.0;

    // Patch-stream determinism over a bounded prefix (see `--patch-limit`).
    let limit = patch_limit.min(head);
    let started = std::time::Instant::now();
    let first = fold_and_hash(store.as_ref(), limit).await?;
    let second = fold_and_hash(store.as_ref(), limit).await?;
    let patch_seconds = started.elapsed().as_secs_f64() / 2.0;

    let (from_snapshot, at) = replay_to(store.as_ref(), head).await?;

    println!("store              {}", store.kind());
    println!("head               {head}");
    println!("state digest       {}", hex(&first_state.digest()));
    println!("todos              {}", first_state.len());
    println!(
        "state fold         {:.3} s ({:.0} events/s)",
        state_seconds,
        head as f64 / state_seconds.max(f64::EPSILON)
    );
    println!("patch limit        {limit}");
    println!("patch digest       {}", hex(&first.patches));
    println!("frames             {}", first.frames);
    println!("patch bytes        {}", first.patch_bytes);
    println!(
        "patch fold         {:.3} s ({:.0} events/s — full recompute per event, O(events × rows))",
        patch_seconds,
        limit as f64 / patch_seconds.max(f64::EPSILON)
    );

    if first_state.digest() != second_state.digest() {
        bail!("replay produced a different state the second time");
    }
    if first.patches != second.patches {
        bail!("replay produced a different patch stream the second time");
    }
    if at != head || from_snapshot.digest() != first_state.digest() {
        bail!("the snapshot path disagrees with genesis replay");
    }
    println!("\nreplay is exact: state and patch stream are bit-identical, and the");
    println!("snapshot path agrees with a fold from genesis.");
    Ok(())
}

struct FoldHash {
    patches: [u8; 32],
    frames: u64,
    patch_bytes: u64,
}

/// Fold from genesis, rendering the broadcast view after every event, and hash the resulting patch
/// stream. If this is stable, then time-travel debugging, `beck fork` and log-backed property
/// tests are all just consequences (§3.7).
async fn fold_and_hash(store: &dyn LogStore, limit: Seq) -> Result<FoldHash> {
    let mut state = TodoState::new();
    let mut view = page(&state, &Scope::Everyone);
    let mut hasher = blake3::Hasher::new();
    let mut frames = 0u64;
    let mut patch_bytes = 0u64;
    let mut at: Seq = 0;

    while at < limit {
        let batch = store.read(at, 4096).await?;
        if batch.is_empty() {
            break;
        }
        for envelope in &batch {
            if envelope.seq > limit {
                break;
            }
            apply_event(&mut state, envelope);
            at = envelope.seq;
            let next = page(&state, &Scope::Everyone);
            let ops = diff(&view, &next);
            view = next;
            if ops.is_empty() {
                continue;
            }
            let bytes = PatchFrame::new(at, ops).to_json().to_string();
            patch_bytes += bytes.len() as u64;
            frames += 1;
            hasher.update(bytes.as_bytes());
        }
    }

    Ok(FoldHash {
        patches: *hasher.finalize().as_bytes(),
        frames,
        patch_bytes,
    })
}

/// Snapshotting out of band, as the generated `CronJob` does it: fold to the head, write the
/// accumulator, exit. Nothing here is privileged — a snapshot is an optimisation over a log that
/// remains authoritative, so a failed job costs a slower recovery and nothing else.
async fn snapshot(store: StoreArgs) -> Result<()> {
    let store = store.open().await?;
    let head = store.head().await?;
    if head == 0 {
        println!("nothing to snapshot: the log is empty");
        return Ok(());
    }
    let started = std::time::Instant::now();
    let (state, at) = replay_to(store.as_ref(), head).await?;
    store
        .put_snapshot(&beck_p0_log::Snapshot { seq: at, state })
        .await?;
    println!(
        "snapshotted {} at seq {at} in {:.3} s",
        store.kind(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

async fn seed(store: StoreArgs, events: usize, actors: usize) -> Result<()> {
    let store = store.open().await?;
    let app = App::start(store, AppConfig::default(), Arc::new(Metrics::default())).await?;
    let base = app.head() as u128;
    let started = std::time::Instant::now();

    for i in 0..events {
        let actor = format!("actor{}", i % actors.max(1));
        let id = Id::from_u128(base + i as u128 + 1);
        app.propose_blocking(
            &actor,
            Command::Add {
                id,
                text: format!("todo {}", base + i as u128 + 1),
            },
        )
        .await
        .map_err(|r| anyhow::anyhow!("seeding rejected: {r}"))?;
    }

    let elapsed = started.elapsed();
    println!(
        "seeded {events} events in {:.3} s ({:.0} events/s, head {})",
        elapsed.as_secs_f64(),
        events as f64 / elapsed.as_secs_f64().max(f64::EPSILON),
        app.head()
    );
    app.snapshot_now().await?;
    Ok(())
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
