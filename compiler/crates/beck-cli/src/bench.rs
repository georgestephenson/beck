//! `beck bench log` — the same workload against every substrate, with the substrate named.
//!
//! # Why this exists
//!
//! "Is PostgreSQL the right store" is a question that had been answered by taste since the original
//! sketch, and it is a question the project already has the machinery to answer by measurement:
//! [`beck_rt::LogStore`] is seven methods with three implementations behind it, and its `kind()`
//! method exists precisely because "a number without its substrate is meaningless".
//!
//! [`docs/18-phase-0-report.md`](../../../../../docs/18-phase-0-report.md) §18.3.2 measured Phase 0's
//! log at 7,660 events/s on Postgres against 8,927 on redb and 140,608 in memory. Two conclusions
//! follow and both are load-bearing for the substrate decision:
//!
//! * the two **durable** stores are within 16% of each other, so the choice of database is not
//!   where the throughput is;
//! * both are ~18× off the non-durable one, so `fsync` is, and **group commit** is the lever —
//!   Phase 0 measured 11× from batching alone.
//!
//! This subcommand is the Phase 2 compiler's version of that measurement, so the numbers are about
//! *this* runtime rather than about a predecessor with a different encoding.
//!
//! # What it measures, and what it deliberately does not
//!
//! Four numbers per substrate:
//!
//! | | what it isolates |
//! |---|---|
//! | **append (batched)** | the real path: one durable commit per batch, as the sequencer drives it |
//! | **append (serial)** | one event per commit — the group-commit lever, as a ratio against the row above |
//! | **read** | range scan by `seq`, which is what replay and subscription resume do |
//! | **encode/decode** | the codec alone, with no store at all — so a store's number can be read net of it |
//!
//! It is **not** a database benchmark. There is no concurrency, because §3.7 gives the log exactly
//! one writer; there is no query mix, because the store answers two shapes of question. A number
//! here is only comparable to another number from the same run on the same machine, and the output
//! says so.
//!
//! # Postgres
//!
//! Measured only when a URL is given (`--url`, or `BECK_POSTGRES_URL`). Without one it is skipped
//! and the output says which substrates ran — the alternative is a table with a silent hole in it,
//! which is how "we measured it" becomes untrue without anybody noticing.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant as StdInstant};

use anyhow::{Context, Result};
use beck_core::Value;
use beck_rt::log::Pending;
use beck_rt::{Durability, Instant, LogStore, MemoryLog, RedbLog, SqliteLog};

/// How many events each measurement moves. Small enough to run in seconds, large enough that a
/// single `fsync` does not dominate.
const EVENTS: usize = 2_000;

/// The batch the sequencer would hand a store under load. Phase 0 measured a mean group commit of
/// 16.2 (§18.3.2), so this is that, rounded.
const BATCH: usize = 16;

pub async fn run(url: Option<&str>, dir: &Path) -> Result<()> {
    println!(
        "beck bench log — {EVENTS} events, batches of {BATCH}\n\
         one writer, no concurrency: §3.7 gives the log exactly one, so a concurrency number would\n\
         be measuring something this system does not do.\n"
    );

    codec()?;

    let mut rows: Vec<Row> = Vec::new();
    rows.push(measure(Arc::new(MemoryLog::new())).await?);

    let path = dir.join("bench.redb");
    let _ = std::fs::remove_file(&path);
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    rows.push(measure(Arc::new(RedbLog::open(&path)?)).await?);
    let _ = std::fs::remove_file(&path);

    // SQLite needs no server, so unlike Postgres it is always in the table — which is what makes
    // `docs/08`'s "measure and let the number pick rung 0's default" answerable on any machine.
    //
    // **Both** durability settings, and that is the whole point of the row rather than thoroughness:
    // measured against redb at `NORMAL`, SQLite looks 26× faster, and the comparison is measuring a
    // weaker promise rather than a faster engine (`docs/67` §67.3). Printing both is what stops the
    // flattering number being the only one anybody sees.
    for (durability, label) in [
        (Durability::Fsync, "bench.sqlite"),
        (Durability::Relaxed, "bench-relaxed.sqlite"),
    ] {
        let sqlite = dir.join(label);
        let clean = |p: &Path| {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", p.display()));
            }
        };
        clean(&sqlite);
        rows.push(measure(Arc::new(SqliteLog::open_with(&sqlite, durability)?)).await?);
        clean(&sqlite);
    }

    match url {
        Some(url) => {
            let pg = beck_rt::PgLog::connect(url)
                .await
                .context("connecting to the log store for the benchmark")?;
            pg.truncate().await?;
            rows.push(measure(Arc::new(pg)).await?);
        }
        None => {
            println!("postgres: skipped — pass `--url` or set BECK_POSTGRES_URL to include it.\n")
        }
    }

    report(&rows);
    Ok(())
}

struct Row {
    substrate: &'static str,
    durable: bool,
    batched: f64,
    serial: f64,
    read: f64,
}

/// The codec on its own, so a store's number can be read net of the encoding.
fn codec() -> Result<()> {
    let v = event(0);
    let bytes = beck_core::repr::to_bytes(&v)?;
    let json = beck_core::core::value_to_repr(&v)?.to_string();

    let n = 200_000;
    let start = StdInstant::now();
    for i in 0..n {
        let _ = beck_core::repr::to_bytes(&event(i as i64))?;
    }
    let encode = rate(n, start.elapsed());

    let start = StdInstant::now();
    for _ in 0..n {
        let _ = beck_core::repr::from_bytes(&bytes)?;
    }
    let decode = rate(n, start.elapsed());

    // The encoding this replaced, measured beside it. Phase 1 and Phase 2 stored every event as
    // the JSON repr, serialised to text — so this row is what the change bought, on the serial
    // path, per event, rather than an assertion that binary is faster.
    let start = StdInstant::now();
    for i in 0..n {
        let _ = beck_core::core::value_to_repr(&event(i as i64))?.to_string();
    }
    let encode_json = rate(n, start.elapsed());

    let start = StdInstant::now();
    for _ in 0..n {
        let repr: serde_json::Value = serde_json::from_str(&json)?;
        let _ = beck_core::core::value_from_repr(&repr).context("the json repr decodes")?;
    }
    let decode_json = rate(n, start.elapsed());

    println!("codec (no store in the way)");
    println!(
        "  {:<14} {:>12} {:>12}  {:>7}",
        "", "postcard", "json (was)", "ratio"
    );
    println!(
        "  {:<14} {:>10} B {:>10} B  {:>6.2}×",
        "event size",
        bytes.len(),
        json.len(),
        json.len() as f64 / bytes.len() as f64
    );
    println!(
        "  {:<14} {:>10.0} /s {:>9.0} /s  {:>6.2}×",
        "encode",
        encode,
        encode_json,
        encode / encode_json.max(1.0)
    );
    println!(
        "  {:<14} {:>10.0} /s {:>9.0} /s  {:>6.2}×\n",
        "decode",
        decode,
        decode_json,
        decode / decode_json.max(1.0)
    );
    Ok(())
}

async fn measure(store: Arc<dyn LogStore>) -> Result<Row> {
    let kind = store.kind();

    // Batched: what the sequencer actually does — one durable commit per batch of whatever arrived
    // while the last one was in flight.
    let start = StdInstant::now();
    let mut n = 0usize;
    while n < EVENTS {
        let batch: Vec<Pending> = (0..BATCH.min(EVENTS - n))
            .map(|i| pending((n + i) as i64))
            .collect();
        store.append(&batch).await?;
        n += batch.len();
    }
    let batched = rate(EVENTS, start.elapsed());

    // Serial: one event per commit. The ratio against the row above is the group-commit lever, and
    // it is the number that decides where effort goes — Phase 0 measured 11×.
    let serial_events = EVENTS / 10;
    let start = StdInstant::now();
    for i in 0..serial_events {
        store.append(&[pending(i as i64)]).await?;
    }
    let serial = rate(serial_events, start.elapsed());

    // Read: the range scan replay and subscription resume both perform.
    let head = store.head().await?;
    let start = StdInstant::now();
    let mut at = 0;
    let mut read = 0usize;
    while at < head {
        let page = store.read(at, 512).await?;
        if page.is_empty() {
            break;
        }
        at = page.last().expect("non-empty").seq;
        read += page.len();
    }
    let read_rate = rate(read, start.elapsed());

    Ok(Row {
        substrate: kind,
        durable: kind != "memory",
        batched,
        serial,
        read: read_rate,
    })
}

fn report(rows: &[Row]) {
    println!(
        "{:<15} {:>8}  {:>14}  {:>14}  {:>14}  {:>8}",
        "substrate", "durable", "append (batch)", "append (serial)", "read", "batch ×"
    );
    for r in rows {
        println!(
            "{:<15} {:>8}  {:>11.0} /s  {:>11.0} /s  {:>11.0} /s  {:>7.1}×",
            r.substrate,
            if r.durable { "yes" } else { "no" },
            r.batched,
            r.serial,
            r.read,
            r.batched / r.serial.max(1.0),
        );
    }
    println!(
        "\nRead these against each other, not against another machine: they are one run on one\n\
         host with whatever else it was doing. What is stable across runs is the *shape* — the\n\
         durable substrates close together, the non-durable one far above, and the batch ratio\n\
         larger than either gap. That shape is the substrate argument: `fsync` is the cost, group\n\
         commit is the lever, and the choice of database is worth less than both."
    );
    if let (Some(a), Some(b)) = (
        rows.iter().find(|r| r.substrate == "redb"),
        rows.iter().find(|r| r.substrate == "postgres"),
    ) {
        println!(
            "\nredb is {:.2}× postgres on batched append.",
            a.batched / b.batched.max(1.0)
        );
    }
}

fn rate(n: usize, took: Duration) -> f64 {
    n as f64 / took.as_secs_f64().max(f64::EPSILON)
}

fn pending(i: i64) -> Pending {
    Pending {
        at: Instant(1_700_000_000_000 + i),
        actor: "bench".to_string(),
        body: event(i),
    }
}

/// An event the size of a real one: a union variant with a few fields, not a scalar.
///
/// `Toggled(id)` is the smallest thing a program logs and `Added(id, text, owner)` is typical, so
/// the shape here is the second — a benchmark on the smallest possible event would flatter every
/// encoding equally and tell nobody anything.
fn event(i: i64) -> Value {
    Value::data(
        Arc::from("Event"),
        Some(Arc::from("Added")),
        beck_core::core::Fields::from_iter([
            (Arc::from("id"), Value::str_(format!("todo-{i}"))),
            (Arc::from("text"), Value::str_("buy milk on the way home")),
            (Arc::from("owner"), Value::str_("ana")),
            (Arc::from("at"), Value::Int(1_700_000_000_000 + i)),
            (Arc::from("done"), Value::Bool(false)),
        ]),
    )
}
