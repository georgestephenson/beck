//! The Phase 0 measurement harness.
//!
//! The roadmap's exit criteria are "stated from evidence, not opinion", and this is the evidence.
//! One subcommand per criterion:
//!
//! | Criterion | Subcommand |
//! |---|---|
//! | interaction latency p50/p99 on a realistic RTT | `latency` |
//! | events/s through the sequencer; fold replay throughput | `throughput` |
//! | per-idle-session memory at 1k and 10k subscribers | `fanout` |
//! | thin-client payload and time-to-first-paint | `payload` |
//! | reconnect-after-deploy: does resumption replay the gap | `resume` |

mod client;
mod stats;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

use beck_p0_core::diff::diff;
use beck_p0_core::domain::{ActorId, Command, Id, Todo, TodoState};
use beck_p0_core::patch::{Codec, PatchFrame};
use beck_p0_core::view::{page, Scope};
use beck_p0_log::{replay_from_genesis, LogStore, MemoryLog, PgLog, RedbLog};
use beck_p0_server::app::{App, AppConfig};
use beck_p0_server::metrics::resident_bytes;
use beck_p0_server::Metrics;

use crate::client::{http_get, metric, Client};
use crate::stats::{round, Summary};

#[derive(Parser)]
#[command(name = "beck-p0-bench", about = "Phase 0 exit-criteria measurements")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// Write the measurement as JSON to this file, in addition to printing it.
    #[arg(long, global = true)]
    json: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// click → command → event → fold → patch → DOM, over a real websocket.
    Latency {
        #[arg(long, default_value = "ws://127.0.0.1:8080/socket")]
        url: String,
        #[arg(long, default_value_t = 500)]
        iterations: usize,
        /// Simulated round-trip time, applied half on each leg.
        #[arg(long, default_value_t = 0)]
        rtt_ms: u64,
        /// How many todos are already in the list.
        #[arg(long, default_value_t = 100)]
        rows: usize,
    },
    /// Events/s through the sequencer, and fold throughput on replay.
    Throughput {
        #[command(flatten)]
        store: StoreArgs,
        #[arg(long, default_value_t = 32)]
        clients: usize,
        #[arg(long, default_value_t = 20_000)]
        commands: usize,
    },
    /// Per-idle-session server memory with N connected subscribers of a per-session view.
    Fanout {
        #[arg(long, default_value_t = 1000)]
        subscribers: usize,
        #[arg(long, default_value = "mine")]
        scope: String,
        /// Drive this many events afterwards and measure interaction latency under fanout.
        #[arg(long, default_value_t = 200)]
        drive: usize,
        /// Measure a running server over real sockets instead of in-process subscriptions.
        #[arg(long)]
        url: Option<String>,
        /// Host:port of the server's HTTP endpoint, for reading its RSS in socket mode.
        #[arg(long, default_value = "127.0.0.1:8080")]
        http: String,
        /// Pre-seed this many todos per subscriber's view.
        #[arg(long, default_value_t = 0)]
        rows: usize,
    },
    /// Thin-client payload, stylesheet, SSR size and time to first paint.
    Payload {
        #[arg(long, default_value = "127.0.0.1:8080")]
        http: String,
        #[arg(long, default_value_t = 20)]
        samples: usize,
    },
    /// Reconnect-after-deploy: does `(subscription, seq)` resumption actually replay the gap?
    Resume {
        #[arg(long, default_value = "ws://127.0.0.1:8080/socket")]
        url: String,
        /// How many events to let accumulate while the subscriber is away.
        #[arg(long, default_value_t = 25)]
        gap: usize,
    },
}

#[derive(Args, Clone)]
struct StoreArgs {
    #[arg(long, value_enum, default_value_t = StoreKind::Memory)]
    store: StoreKind,
    #[arg(long, default_value = "bench.redb")]
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
    Memory,
    Redb,
    Postgres,
}

impl StoreArgs {
    async fn open(&self) -> Result<Arc<dyn LogStore>> {
        Ok(match self.store {
            StoreKind::Memory => Arc::new(MemoryLog::new()),
            StoreKind::Redb => {
                let _ = std::fs::remove_file(&self.redb_path);
                Arc::new(RedbLog::open(&self.redb_path)?)
            }
            StoreKind::Postgres => {
                let store = PgLog::connect(&self.pg).await?;
                store.truncate().await?;
                Arc::new(store)
            }
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let measurement = match cli.command {
        Cmd::Latency {
            url,
            iterations,
            rtt_ms,
            rows,
        } => latency(&url, iterations, Duration::from_millis(rtt_ms), rows).await?,
        Cmd::Throughput {
            store,
            clients,
            commands,
        } => throughput(store, clients, commands).await?,
        Cmd::Fanout {
            subscribers,
            scope,
            drive,
            url,
            http,
            rows,
        } => match url {
            Some(url) => fanout_sockets(&url, &http, subscribers, &scope).await?,
            None => fanout_inprocess(subscribers, &scope, drive, rows).await?,
        },
        Cmd::Payload { http, samples } => payload(&http, samples).await?,
        Cmd::Resume { url, gap } => resume(&url, gap).await?,
    };

    println!("{}", serde_json::to_string_pretty(&measurement)?);
    if let Some(path) = cli.json {
        std::fs::write(&path, serde_json::to_string_pretty(&measurement)?)?;
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}

/// **Interaction latency.** The full Mode A path, measured from the client: the command leaves,
/// the server validates it, appends it, folds it, re-renders this subscriber's view, diffs it, and
/// the patch comes back.
async fn latency(url: &str, iterations: usize, rtt: Duration, rows: usize) -> Result<Value> {
    let run = run_nonce();
    let mut c = Client::connect(url, rtt).await?;
    c.hello(&format!("bench-latency-{run}"), 0, "bench", "all")
        .await?;
    // Consume the frame that establishes the subscription, so the first measured interaction is
    // not credited with a patch that was already on its way.
    c.next_patch().await?;

    // Fill the list first: a diff over an empty list is not the measurement anyone cares about.
    for i in 0..rows {
        c.interact(&Command::Add {
            id: Id::from_u128(run + i as u128),
            text: format!("seed {i:04}"),
        })
        .await?;
    }

    let mut add_samples = Vec::with_capacity(iterations);
    let mut delete_samples = Vec::with_capacity(iterations);
    let mut bytes_samples = Vec::with_capacity(iterations * 2);
    let mut ops_samples = Vec::with_capacity(iterations * 2);

    for i in 0..iterations + 50 {
        let id = Id::from_u128(run + 0x0010_0000 + i as u128);
        let warmup = i < 50;

        let add = c
            .interact(&Command::Add {
                id,
                text: format!("interaction {i:06}"),
            })
            .await?;
        let delete = c.interact(&Command::Delete { id }).await?;

        if !warmup {
            add_samples.push(add.millis);
            delete_samples.push(delete.millis);
            for interaction in [&add, &delete] {
                bytes_samples.push(interaction.bytes as f64);
                ops_samples.push(interaction.ops as f64);
            }
        }
    }

    let all: Vec<f64> = add_samples
        .iter()
        .chain(delete_samples.iter())
        .copied()
        .collect();
    c.close().await?;

    Ok(json!({
        "measurement": "interaction latency (ms)",
        "mode": "A (server-side view, DOM patches)",
        "rows_in_view": rows,
        "simulated_rtt_ms": rtt.as_millis(),
        "add": Summary::of(add_samples).to_json(),
        "delete": Summary::of(delete_samples).to_json(),
        "all": Summary::of(all).to_json(),
        "patch_bytes": Summary::of(bytes_samples).to_json(),
        "patch_ops": Summary::of(ops_samples).to_json(),
    }))
}

/// **Events/s through a single sequencer**, and **fold throughput on replay**.
///
/// Measured in-process, without websockets, because the criterion is about the sequencer and the
/// log substrate — the socket path is what `latency` measures.
async fn throughput(store: StoreArgs, clients: usize, commands: usize) -> Result<Value> {
    let kind = store.store;
    let store = store.open().await?;
    let store_kind = store.kind();
    let app = App::start(
        store.clone(),
        AppConfig::default(),
        Arc::new(Metrics::default()),
    )
    .await?;

    let per_client = commands / clients.max(1);
    let started = Instant::now();
    let mut tasks = Vec::with_capacity(clients);
    for c in 0..clients {
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            let actor = format!("actor{c}");
            for i in 0..per_client {
                let id = Id::from_u128((c as u128) << 64 | i as u128);
                let _ = app
                    .propose_blocking(
                        &actor,
                        Command::Add {
                            id,
                            text: format!("c{c} i{i}"),
                        },
                    )
                    .await;
            }
        }));
    }
    for task in tasks {
        task.await?;
    }
    let append_elapsed = started.elapsed();
    let appended = app.head();

    let metrics = app.metrics();
    let batches = metrics
        .batches
        .load(std::sync::atomic::Ordering::Relaxed)
        .max(1);
    let batched = metrics
        .batched_commands
        .load(std::sync::atomic::Ordering::Relaxed);

    // Fold throughput on replay: the same events, folded from genesis with no snapshot.
    let started = Instant::now();
    let (state, at) = replay_from_genesis(store.as_ref()).await?;
    let replay_elapsed = started.elapsed();

    Ok(json!({
        "measurement": "sequencer throughput and fold replay",
        "store": store_kind,
        "durable": !matches!(kind, StoreKind::Memory),
        "concurrent_clients": clients,
        "events": appended,
        "append": {
            "seconds": round(append_elapsed.as_secs_f64()),
            "events_per_second": round(appended as f64 / append_elapsed.as_secs_f64()),
            "group_commits": batches,
            "mean_batch_size": round(batched as f64 / batches as f64),
        },
        "replay": {
            "events": at,
            "todos": state.len(),
            "seconds": round(replay_elapsed.as_secs_f64()),
            "events_per_second": round(at as f64 / replay_elapsed.as_secs_f64()),
        },
    }))
}

/// **Per-idle-session server memory** — the fanout number, "the one that kills LiveView-shaped
/// systems".
///
/// In-process: each subscriber is a real subscription — the same `session::run`, the same
/// websocket codec — over an in-memory duplex instead of a socket. That removes the kernel socket
/// buffers (and the file-descriptor ceiling) from the measurement, so this reports the *runtime's*
/// per-session cost. `fanout --url` measures the same thing with real sockets, and the report
/// quotes both.
async fn fanout_inprocess(
    subscribers: usize,
    scope: &str,
    drive: usize,
    rows: usize,
) -> Result<Value> {
    use tokio_tungstenite::tungstenite::protocol::Role;
    use tokio_tungstenite::WebSocketStream;

    let app = App::start(
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
        Arc::new(Metrics::default()),
    )
    .await?;

    // Seed the shared list, so per-session views are not trivially empty.
    for i in 0..rows {
        app.propose_blocking(
            "seed",
            Command::Add {
                id: Id::from_u128(0x5eed_0000 + i as u128),
                text: format!("seed {i:04}"),
            },
        )
        .await
        .ok();
    }

    let baseline = resident_bytes().unwrap_or(0);
    let mut clients = Vec::with_capacity(subscribers);

    for i in 0..subscribers {
        let (server_side, client_side) = tokio::io::duplex(8192);
        let app = app.clone();
        tokio::spawn(async move {
            let socket = WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let _ = beck_p0_server::session::run(app, socket).await;
        });

        let mut socket = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
        let actor = format!("actor{i}");
        let hello = json!({"t": "hello", "sub": format!("sub{i}"), "seq": 0, "actor": actor, "scope": scope});
        use futures_util::{SinkExt, StreamExt};
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                hello.to_string(),
            ))
            .await?;
        // Drain the welcome and the initial frame so the subscription is fully established.
        let mut seen_frame = false;
        while !seen_frame {
            match socket.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text)?;
                    seen_frame = value["t"] == "p";
                }
                Some(Ok(_)) => continue,
                _ => bail!("subscription {i} closed during handshake"),
            }
        }
        clients.push(socket);
    }

    // Let allocation settle before reading RSS.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let with_subscribers = resident_bytes().unwrap_or(0);
    let per_session = (with_subscribers.saturating_sub(baseline)) as f64 / subscribers as f64;

    // What the runtime itself accounts for: the rendered view held per subscriber.
    let scope_value = if scope == "mine" {
        Scope::Mine(ActorId::new("actor0"))
    } else {
        Scope::Everyone
    };
    let (_, view) = app.view_now(&scope_value).await;
    let accounted = beck_p0_server::session::view_bytes(&view);

    // Interaction latency with the fanout in place: every subscriber wakes on every event, even
    // when its own view did not change. That wakeup is the cost this benchmark exists to expose.
    let mut samples = Vec::with_capacity(drive);
    for i in 0..drive {
        let started = Instant::now();
        app.propose_blocking(
            "driver",
            Command::Add {
                id: Id::from_u128(0xd00d_0000 + i as u128),
                text: format!("driven {i}"),
            },
        )
        .await
        .ok();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }

    Ok(json!({
        "measurement": "per-idle-session memory (in-process subscriptions)",
        "subscribers": subscribers,
        "scope": scope,
        "rows_in_shared_state": rows,
        "rss_baseline_bytes": baseline,
        "rss_with_subscribers_bytes": with_subscribers,
        "per_session_bytes": round(per_session),
        "per_session_view_bytes_accounted": accounted,
        "note": "in-process subscriptions exclude kernel socket buffers; see the socket-mode run",
        "commit_latency_under_fanout_ms": Summary::of(samples).to_json(),
    }))
}

/// The same measurement with real sockets against a running server — closer to production, and
/// bounded by the file-descriptor limit of the environment.
async fn fanout_sockets(url: &str, http: &str, subscribers: usize, scope: &str) -> Result<Value> {
    let baseline = metric(http, "beck_process_resident_bytes").await?;
    let mut clients = Vec::with_capacity(subscribers);
    for i in 0..subscribers {
        let mut c = Client::connect(url, Duration::ZERO).await?;
        c.hello(&format!("sub{i}"), 0, &format!("actor{i}"), scope)
            .await?;
        // Consume the initial frame so the subscription is established and quiescent.
        c.next_patch().await?;
        clients.push(c);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let with_subscribers = metric(http, "beck_process_resident_bytes").await?;
    let live = metric(http, "beck_subscriptions").await?;

    Ok(json!({
        "measurement": "per-idle-session memory (real websockets)",
        "subscribers": subscribers,
        "server_reported_subscriptions": live,
        "scope": scope,
        "rss_baseline_bytes": baseline,
        "rss_with_subscribers_bytes": with_subscribers,
        "per_session_bytes": round((with_subscribers - baseline) / subscribers as f64),
    }))
}

/// **Thin-client payload and time to first paint.**
async fn payload(http: &str, samples: usize) -> Result<Value> {
    let (status, js, _, _) = http_get(http, "/beck.js").await?;
    if status != 200 {
        bail!("GET /beck.js returned {status}");
    }
    let (_, css, _, _) = http_get(http, "/app.css").await?;

    let mut ttfb = Vec::with_capacity(samples);
    let mut total = Vec::with_capacity(samples);
    let mut ssr_bytes = 0;
    for _ in 0..samples {
        let (status, body, first, whole) = http_get(http, "/?actor=bench").await?;
        if status != 200 {
            bail!("GET / returned {status}");
        }
        ssr_bytes = body.len();
        ttfb.push(first.as_secs_f64() * 1000.0);
        total.push(whole.as_secs_f64() * 1000.0);
    }

    // Patch sizes for a realistic edit at several list sizes, in both encodings (§4.4).
    let mut frames = Vec::new();
    for rows in [10usize, 100, 1000] {
        let before = synthetic_state(rows);
        let mut after = before.clone();
        after
            .todos
            .get_mut(&Id::from_u128(rows as u128 / 2))
            .expect("mid row exists")
            .done = true;
        let ops = diff(
            &page(&before, &Scope::Everyone),
            &page(&after, &Scope::Everyone),
        );
        let frame = PatchFrame::new(rows as u64, ops);
        let full = page(&after, &Scope::Everyone).render();
        frames.push(json!({
            "rows": rows,
            "ops": frame.ops.len(),
            "json_bytes": Codec::Json.encode(&frame).len(),
            "postcard_bytes": Codec::Postcard.encode(&frame).len(),
            "full_page_html_bytes": full.len(),
        }));
    }

    Ok(json!({
        "measurement": "thin-client payload and first paint",
        "thin_client": {
            "raw_bytes": js.len(),
            "brotli_bytes": brotli_size(&js),
            "budget_brotli_bytes": 10 * 1024,
        },
        "stylesheet": {
            "raw_bytes": css.len(),
            "brotli_bytes": brotli_size(&css),
        },
        "ssr": {
            "bytes": ssr_bytes,
            "brotli_bytes_estimate": null,
            "time_to_first_byte_ms": Summary::of(ttfb).to_json(),
            "time_to_last_byte_ms": Summary::of(total).to_json(),
        },
        "patch_frames": frames,
    }))
}

/// **Reconnect-after-deploy.** Subscribe, disconnect, let the log move, reconnect with the last
/// `seq`, and check that what comes back is a gap patch rather than the world.
async fn resume(url: &str, gap: usize) -> Result<Value> {
    let run = run_nonce();
    let mut subscriber = Client::connect(url, Duration::ZERO).await?;
    let (seq_at_subscribe, _) = subscriber
        .hello(&format!("bench-resume-{run}"), 0, "watcher", "all")
        .await?;
    subscriber.next_patch().await?;
    let full_frame_bytes = {
        // Size of the frame a *fresh* subscriber pays for, as the comparison point.
        let mut fresh = Client::connect(url, Duration::ZERO).await?;
        fresh
            .hello(&format!("bench-resume-fresh-{run}"), 0, "watcher2", "all")
            .await?;
        let (_, bytes, ops) = fresh.next_patch().await?;
        fresh.close().await?;
        (bytes, ops)
    };

    // The subscriber goes away — a dropped connection, or a deploy taking the process with it.
    subscriber.close().await?;

    let mut driver = Client::connect(url, Duration::ZERO).await?;
    driver
        .hello(&format!("bench-resume-driver-{run}"), 0, "driver", "all")
        .await?;
    driver.next_patch().await?;
    let mut last_seq = 0;
    for i in 0..gap {
        last_seq = driver
            .interact(&Command::Add {
                id: Id::from_u128(run + i as u128),
                text: format!("while you were out {i}"),
            })
            .await?
            .seq;
    }
    driver.close().await?;

    let started = Instant::now();
    let mut back = Client::connect(url, Duration::ZERO).await?;
    let (_, how) = back
        .hello(
            &format!("bench-resume-{run}"),
            seq_at_subscribe,
            "watcher",
            "all",
        )
        .await?;
    let (seq, bytes, ops) = back.next_patch().await?;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    back.close().await?;

    Ok(json!({
        "measurement": "reconnect-after-deploy resumption",
        "subscribed_at_seq": seq_at_subscribe,
        "events_missed": gap,
        "resumed_to_seq": seq,
        "server_says": how,
        "gap_patch": {"bytes": bytes, "ops": ops},
        "fresh_subscription_frame": {"bytes": full_frame_bytes.0, "ops": full_frame_bytes.1},
        "reconnect_to_caught_up_ms": round(elapsed),
        "replayed_the_gap": how == "resumed" && seq == last_seq,
    }))
}

/// Ids are minted by the client, and the server accepts a client-minted id only if it is *fresh*
/// (first writer wins, F2). A benchmark run must therefore not reuse the ids of a previous run
/// against the same log — the second run would be correctly rejected.
fn run_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after 1970")
        .as_nanos()
        << 24
}

fn synthetic_state(rows: usize) -> TodoState {
    let mut state = TodoState::new();
    for i in 0..rows {
        let id = Id::from_u128(i as u128);
        state.todos.insert(
            id,
            Todo {
                id,
                text: format!("todo {i:06}"),
                done: false,
                owner: ActorId::new("bench"),
            },
        );
    }
    state
}

fn brotli_size(input: &[u8]) -> usize {
    use std::io::Write;
    let mut out = Vec::new();
    {
        // Quality 11, window 22: what a CDN or `Content-Encoding: br` would do to this asset.
        let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
        writer.write_all(input).expect("in-memory write");
    }
    out.len()
}
