//! Measurements quoted in `docs/24-incremental-views-report.md`, `docs/26`, `docs/51` and
//! `docs/89-query-fusion-report.md`.
//!
//! Run with `cargo test --release --test measure_incremental -- --nocapture`. Printed, never
//! thresholded: §13.7's rule that a shared CI runner cannot hold a timing gate honestly. The claims
//! that *are* gates are counts rather than durations, and they live in `incremental_engine.rs` and
//! in `scaling.rs`.

use std::sync::Arc;
use std::time::Instant;

use beck_core::engine::{Engine, Prepared};
use beck_core::plan::{Op, Plan};
use beck_core::{Placed, Value};
use beck_rt::{Envelope, Instant as At, Runtime};

mod support;

/// The sketch, its oracle, and a maintained view of it.
struct Bench {
    runtime: Runtime,
    plan: Arc<Plan>,
    prepared: Arc<Prepared>,
}

impl Bench {
    fn new(placed: Placed) -> Bench {
        let backend = beck_eval::backend(&placed);
        let prepared = Arc::new(Prepared::compile(&placed, backend.as_ref()).expect("prepares"));
        let plan = prepared.plan().clone();
        let runtime = Runtime::new(placed, backend).expect("prepares");
        Bench {
            runtime,
            plan,
            prepared,
        }
    }

    fn engine(&self) -> Engine {
        Engine::new(self.prepared.clone())
    }

    /// A state holding `n` todos, all owned by `ana`.
    fn state_with(&self, n: usize) -> Value {
        let mut state = self.runtime.initial_state().expect("initial");
        for i in 0..n {
            state = self.add(&state, i as u64 + 1);
        }
        state
    }

    /// `n` posts on `24-feed.beck`, which has a different `Event` union from the sketch's.
    fn feed_with(&self, n: usize) -> Value {
        let mut state = self.runtime.initial_state().expect("initial");
        for i in 0..n as u64 {
            let event = Value::data(
                Arc::from("Event"),
                Some(Arc::from("Published")),
                beck_core::core::Fields::from_iter([
                    (Arc::from("id"), Value::str_(format!("p{i:05}"))),
                    (Arc::from("text"), Value::str_(format!("post {i}"))),
                ]),
            );
            let env = Envelope {
                seq: i + 1,
                at: At(i as i64 + 1),
                actor: "ana".to_string(),
                body: event.clone(),
            };
            state = self.runtime.fold(&state, &env, event).expect("fold");
        }
        state
    }

    /// `rows` todos spread over `owners` actors — §5.3's `todos.map(filter_by(session.user))`,
    /// where every connected subscriber's filter actually keeps something.
    ///
    /// `state_with` puts every todo on one actor, which is right for the per-event tables above and
    /// wrong for a fanout: a subscriber whose filter is empty has no per-session work to compare
    /// against, and the sharing would look better than it is.
    fn state_across(&self, rows: usize, owners: usize) -> Value {
        let mut state = self.runtime.initial_state().expect("initial");
        for i in 0..rows as u64 {
            state = self.add_by(&state, i + 1, &format!("u{}", i as usize % owners));
        }
        state
    }

    fn add(&self, state: &Value, n: u64) -> Value {
        self.add_by(state, n, "ana")
    }

    fn add_by(&self, state: &Value, n: u64, actor: &str) -> Value {
        let id = Value::data(
            Arc::from("Id"),
            None,
            beck_core::core::Fields::from_iter([(
                Arc::from("value"),
                Value::str_(format!("{n:08}")),
            )]),
        );
        let event = Value::data(
            Arc::from("Event"),
            Some(Arc::from("Added")),
            beck_core::core::Fields::from_iter([
                (Arc::from("id"), id),
                (Arc::from("text"), Value::str_(format!("item {n}"))),
            ]),
        );
        let env = Envelope {
            seq: n,
            at: At(n as i64),
            actor: actor.to_string(),
            body: event.clone(),
        };
        self.runtime.fold(state, &env, event).expect("fold")
    }
}

#[test]
fn what_one_event_costs_against_the_size_of_the_collection() {
    // The claim under measurement is §3.8's: "`remaining` updates by ±1 per event, never by
    // recount." What the table has to show is *both* halves of the honest answer — the delta work
    // that does not grow, and the assembly of the page's children, which still does.
    let bench = Bench::new(support::todo_program());
    let session = bench.runtime.session("ana");

    println!(
        "\n{:>7}  {:>10} {:>10} {:>8}  {:>12} {:>12} {:>6}",
        "rows", "delta", "materialise", "recomp", "maintain µs", "recompute µs", "ratio"
    );
    for n in [10usize, 100, 1_000, 5_000] {
        let state = bench.state_with(n);
        let next = bench.add(&state, n as u64 + 1);

        let mut engine = bench.engine();
        engine.render(&state, &session).expect("warm");

        // One event, measured on its own.
        let mut maintain = u128::MAX;
        let mut work = engine.work();
        for _ in 0..5 {
            let mut e = bench.engine();
            e.render(&state, &session).expect("warm");
            let t = Instant::now();
            e.render(&next, &session).expect("step");
            maintain = maintain.min(t.elapsed().as_micros());
            work = e.work();
        }
        let mut recompute = u128::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            bench.runtime.view(&next, "ana").expect("recompute");
            recompute = recompute.min(t.elapsed().as_micros());
        }
        println!(
            "{n:>7}  {:>10} {:>10} {:>8}  {maintain:>12} {recompute:>12} {:>5.1}×",
            work.applications + work.touched,
            work.materialised,
            work.recomputed,
            recompute as f64 / maintain.max(1) as f64,
        );
    }
    println!(
        "\n  delta       — per-element functions applied plus arrangement entries moved.\n  \
         materialise — entries copied to hand a pointwise operator a `list`. This is the O(n)\n  \
                       that remains, and it is the page's children being assembled.\n  \
         recomp      — pointwise operators re-evaluated (the plan has {} of them).",
        bench
            .plan
            .nodes
            .iter()
            .filter(|n| matches!(n.op, Op::Pointwise { .. }))
            .count()
    );
}

#[test]
fn what_a_subscriber_holds() {
    // §5.3's per-session memory, which is the metric the whole fanout question turns on: an engine
    // per subscription is a memory-for-time trade, and a trade whose cost is not measured is a
    // claim rather than a trade. Phase 0's kill gate was ~50 KB per idle session and it measured
    // ~5 KB (`docs/18-phase-0-report.md` §18.3).
    //
    // Computed, not sampled. The first version of this read `/proc/self/statm` around 32 live
    // subscriptions and reported a ratio that swung between 2.0× and 4.9× across runs of the same
    // binary, because the resident set moves with the allocator's arena rather than with the data.
    // `Engine::footprint` walks what is retained and counts shared structure once; it excludes
    // per-allocation overhead, so it is a floor.
    let bench = Bench::new(support::todo_program());
    let session = bench.runtime.session("ana");
    println!(
        "\n{:>7}  {:>9} {:>12} {:>12} {:>12} {:>7}",
        "rows", "arranged", "engine KB", "of it shared", "page KB", "×"
    );
    for n in [10usize, 100, 1_000, 5_000] {
        let state = bench.state_with(n);
        let mut engine = bench.engine();
        engine.render(&state, &session).expect("render");
        let f = engine.footprint(&state);
        // What a subscription already held before any of this: the last rendered page, kept so the
        // next one can be diffed against it.
        let page = bench.runtime.view(&state, "ana").expect("view");
        let page_bytes = beck_core::engine::html_footprint(&page);
        println!(
            "{n:>7}  {:>9} {:>12.1} {:>12.1} {:>12.1} {:>6.1}×",
            f.entries,
            f.bytes as f64 / 1024.0,
            f.shared_bytes as f64 / 1024.0,
            page_bytes as f64 / 1024.0,
            f.bytes as f64 / page_bytes.max(1) as f64,
        );
    }
    let shared = bench.plan.shared().len();
    println!(
        "\n  arranged     — entries across every arrangement one subscriber holds.\n  \
           engine KB    — bytes this subscription retains beyond the accumulator itself, which it\n  \
                          shares by `Arc` rather than copies.\n  \
           of it shared — the part in operators that do not read the session, which §5.3 says a\n  \
                          thousand subscribers hold once between them. This engine is standalone,\n  \
                          so it holds them itself; the fanout table below is the shared one.\n  \
           page KB      — the rendered page alone, which a subscription already held for the diff.\n  \
           {shared} of this plan's {} operators do not read the session.",
        bench.plan.nodes.len()
    );
}

#[test]
fn how_much_of_each_corpus_program_is_maintained() {
    println!(
        "\n{:<24} {:>7} {:>12} {:>11} {:>8}",
        "program", "nodes", "maintained", "recomputed", "shared"
    );
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    files.sort();
    let mut totals = (0usize, 0usize);
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("readable");
        let (placed, d, map) = beck_core::compile_str(&name, &src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let plan = Plan::compile(&placed.expect("slices"));
        let (maintained, recomputed) = plan.counts();
        totals.0 += maintained;
        totals.1 += recomputed;
        println!(
            "{name:<24} {:>7} {maintained:>12} {recomputed:>11} {:>8}",
            plan.nodes.len(),
            plan.shared().len(),
        );
    }
    println!(
        "\n  {} maintained operators and {} recomputed ones across the corpus.",
        totals.0, totals.1
    );
}

/// The measurement `docs/89-query-fusion-report.md` quotes: what the rewrite is worth.
///
/// Two tables, and the first is the honest one. Counts — operators, arrangements, entries held,
/// work per event — are properties of the plan and of the engine's own counters, so they are the
/// same on every machine. The wall clock is second because it is the one a shared runner argues
/// with, and it is measured with the two plans **alternating** rather than one after the other:
/// `docs/78` §78.6 found that a fixed A-then-B order biases a comparison by as much as the effects
/// this project reports.
#[test]
fn what_query_fusion_is_worth() {
    println!(
        "\n{:<24} {:>9} {:>9} {:>13} {:>13} {:>6}",
        "program", "before", "after", "arr. before", "arr. after", "dead"
    );
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    files.sort();
    let mut programs: Vec<(String, Placed)> =
        vec![("examples/todo.beck".to_string(), support::todo_program())];
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("readable");
        let (placed, d, map) = beck_core::compile_str(&name, &src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        programs.push((name, placed.expect("slices")));
    }
    let (mut moved, mut arrangements) = (0usize, 0usize);
    for (name, placed) in &programs {
        let f = beck_core::fuse::fuse(Plan::unfused(placed)).1;
        if f.operators.0 != f.operators.1 {
            moved += 1;
        }
        arrangements += f.arrangements.0 - f.arrangements.1;
        println!(
            "{name:<24} {:>9} {:>9} {:>13} {:>13} {:>6}",
            f.operators.0, f.operators.1, f.arrangements.0, f.arrangements.1, f.unreachable
        );
    }
    println!(
        "\n  {moved} of {} programs have an operator removed; {arrangements} arrangements in all.",
        programs.len()
    );

    // What one of those arrangements costs, on the sketch, whose `for t in mine:` is the shape the
    // rewrite is for.
    let placed = support::todo_program();
    let backend = beck_eval::backend(&placed);
    let plans = [
        ("unfused", Plan::unfused(&placed)),
        ("fused", Plan::compile(&placed)),
    ];
    let prepared: Vec<Arc<Prepared>> = plans
        .iter()
        .map(|(_, p)| Arc::new(Prepared::new(Arc::new(p.clone()), backend.as_ref()).expect("prep")))
        .collect();
    let runtime = Runtime::new(placed, backend).expect("prepares");
    let bench = Bench::new(support::todo_program());
    let session = runtime.session("ana");

    println!("\n{:>7}  {:^38}  {:^38}  {:>6}", "", "unfused", "fused", "");
    println!(
        "{:>7}  {:>13} {:>12} {:>10}  {:>13} {:>12} {:>10}  {:>6}",
        "rows",
        "entries held",
        "work/event",
        "µs/event",
        "entries held",
        "work/event",
        "µs/event",
        "held"
    );
    for n in [10usize, 100, 1_000, 5_000] {
        let state = bench.state_with(n);
        let next = bench.add(&state, n as u64 + 1);
        let mut held = [0u64; 2];
        let mut work = [0u64; 2];
        let mut micros = [u128::MAX; 2];
        // Alternating, best of five: the two plans see the same machine in the same state.
        for _ in 0..5 {
            for k in 0..2 {
                let mut e = Engine::new(prepared[k].clone());
                e.render(&state, &session).expect("warm");
                let t = Instant::now();
                e.render(&next, &session).expect("step");
                micros[k] = micros[k].min(t.elapsed().as_micros());
                held[k] = e.arranged();
                work[k] = e.work().total();
            }
        }
        println!(
            "{n:>7}  {:>13} {:>12} {:>10}  {:>13} {:>12} {:>10}  {:>5.0}%",
            held[0],
            work[0],
            micros[0],
            held[1],
            work[1],
            micros[1],
            100.0 * held[1] as f64 / held[0].max(1) as f64,
        );
    }
    println!(
        "\n  entries held — every arrangement's entries, per subscriber. The rewrite removes one"
    );
    println!("                 arrangement of the collection's size, which is the column to read.");
    println!(
        "  work/event   — applications + entries touched + entries copied + operators recomputed."
    );
}

/// The measurement `docs/26-arrangement-sharing-report.md` quotes: what a fanout costs with §5.3's
/// shared dataflow and what it cost without one.
///
/// Two programs, because the answer is entirely a property of the program and quoting one number
/// would be quoting the more flattering one. The sketch filters by `session.actor` immediately
/// below the accumulator, so almost nothing is above the cut; `24-feed.beck` sorts a public feed
/// and personalises only the greeting, so almost everything is.
///
/// Bytes are `fanout_footprint`, which walks the accumulator, the shared side and every subscriber
/// with **one** exclusion set — summing per-subscriber footprints would charge every subscriber for
/// the page subtrees they now hold by `Arc` between them, which is the saving under measurement.
#[test]
fn what_a_fanout_costs_with_and_without_a_shared_dataflow() {
    use beck_core::engine::{fanout_footprint, SharedDataflow};

    for (label, placed, feed) in [
        ("examples/todo.beck", support::todo_program(), false),
        ("24-feed.beck", feed_program(), true),
    ] {
        const ROWS: usize = 200;
        const OWNERS: usize = 8;
        let bench = Bench::new(placed);
        let state = if feed {
            bench.feed_with(ROWS)
        } else {
            bench.state_across(ROWS, OWNERS)
        };
        let plan = bench.plan.clone();
        println!(
            "\n{label}, {ROWS} rows: {} of {} operators do not read the session",
            plan.shared().len(),
            plan.nodes.len()
        );
        println!(
            "{:>12}  {:>12} {:>12} {:>8}  {:>12} {:>12}",
            "subscribers", "unshared KB", "shared KB", "×", "unshared µs", "shared µs"
        );
        for n in [1usize, 8, 64, 256] {
            // Subscribers drawn from the same actors that own the rows, so every per-session
            // filter keeps a share of the collection rather than nothing.
            let sessions: Vec<Value> = (0..n)
                .map(|i| bench.runtime.session(&format!("u{}", i % OWNERS)))
                .collect();

            let mut alone: Vec<Engine> = (0..n).map(|_| bench.engine()).collect();
            let started = Instant::now();
            for (e, s) in alone.iter_mut().zip(&sessions) {
                e.render(&state, s).expect("a standalone render");
            }
            let alone_us = started.elapsed().as_micros();
            let unshared = fanout_footprint(&state, None, &alone.iter().collect::<Vec<_>>()).bytes;

            let dataflow = Arc::new(SharedDataflow::new(bench.prepared.clone()));
            let mut engines: Vec<Engine> = (0..n).map(|_| dataflow.subscriber()).collect();
            let started = Instant::now();
            for (e, s) in engines.iter_mut().zip(&sessions) {
                dataflow.render(e, &state, 1, s).expect("a shared render");
            }
            let shared_us = started.elapsed().as_micros();
            let shared =
                fanout_footprint(&state, Some(&dataflow), &engines.iter().collect::<Vec<_>>())
                    .bytes;

            println!(
                "{n:>12}  {:>12} {:>12} {:>7.1}×  {alone_us:>12} {shared_us:>12}",
                unshared / 1024,
                shared / 1024,
                unshared as f64 / shared.max(1) as f64,
            );
        }
    }
    println!(
        "\n  A cold fanout: every subscriber's first render. The steady-state question — what one\n  \
         event costs a connected fanout — is the table above this one, per subscriber, times the\n  \
         subscribers, minus the shared prefix, which is advanced once."
    );
}

fn feed_program() -> Placed {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/24-feed.beck");
    let src = std::fs::read_to_string(&path).expect("24-feed.beck is in the corpus");
    let (placed, d, map) = beck_core::compile_str("24-feed.beck", &src);
    assert!(!d.has_errors(), "{}", d.render(&map));
    placed.expect("24-feed.beck slices")
}

/// What **one event** costs a fanout that is already connected — the number an operator running a
/// thousand sessions actually pays, and the one the cold table above does not answer.
///
/// With no sharing, every subscriber flows the delta through its own copy of the whole plan, so the
/// per-event cost of the shared prefix is paid `n` times. With sharing it is paid once and every
/// subscriber pays only what is below the session.
#[test]
fn what_one_event_costs_a_connected_fanout() {
    use beck_core::engine::SharedDataflow;

    for (label, placed, feed) in [
        ("examples/todo.beck", support::todo_program(), false),
        ("24-feed.beck", feed_program(), true),
    ] {
        const ROWS: usize = 200;
        const OWNERS: usize = 8;
        let bench = Bench::new(placed);
        let (state, next) = if feed {
            (bench.feed_with(ROWS), bench.feed_with(ROWS + 1))
        } else {
            let s = bench.state_across(ROWS, OWNERS);
            let n = bench.add_by(&s, ROWS as u64 + 1, "u0");
            (s, n)
        };
        println!("\n{label}, one event over {ROWS} rows");
        println!(
            "{:>12}  {:>12} {:>12} {:>8}  {:>12} {:>12} {:>8}",
            "subscribers", "unshared µs", "shared µs", "×", "unshared work", "shared work", "×"
        );
        for n in [1usize, 8, 64, 256] {
            let sessions: Vec<Value> = (0..n)
                .map(|i| bench.runtime.session(&format!("u{}", i % OWNERS)))
                .collect();

            let mut alone: Vec<Engine> = (0..n).map(|_| bench.engine()).collect();
            for (e, s) in alone.iter_mut().zip(&sessions) {
                e.render(&state, s).expect("warm");
            }
            let started = Instant::now();
            for (e, s) in alone.iter_mut().zip(&sessions) {
                e.render(&next, s).expect("step");
            }
            let alone_us = started.elapsed().as_micros();
            let alone_work: u64 = alone.iter().map(|e| e.work().total()).sum();

            let dataflow = Arc::new(SharedDataflow::new(bench.prepared.clone()));
            let mut engines: Vec<Engine> = (0..n).map(|_| dataflow.subscriber()).collect();
            for (e, s) in engines.iter_mut().zip(&sessions) {
                dataflow.render(e, &state, 1, s).expect("warm");
            }
            let started = Instant::now();
            for (e, s) in engines.iter_mut().zip(&sessions) {
                dataflow.render(e, &next, 2, s).expect("step");
            }
            let shared_us = started.elapsed().as_micros();
            let shared_work: u64 =
                dataflow.work().total() + engines.iter().map(|e| e.work().total()).sum::<u64>();

            println!(
                "{n:>12}  {alone_us:>12} {shared_us:>12} {:>7.1}×  {alone_work:>13} \
                 {shared_work:>11} {:>7.1}×",
                alone_us as f64 / shared_us.max(1) as f64,
                alone_work as f64 / shared_work.max(1) as f64,
            );
        }
    }
    println!(
        "\n  work — `Engine::work().total()`: per-element applications, arrangement entries moved,\n  \
                entries copied into a `list`, and pointwise operators re-evaluated. A count rather\n  \
                than a duration, so it is the same on any machine (§13.7)."
    );
}

/// What a process holds when nobody is looking, and how much change history a fanout pins.
///
/// [`docs/26-arrangement-sharing-report.md`](../../../../docs/26-arrangement-sharing-report.md)
/// §26.9 left two numbers unmeasured: what the shared dataflow retains for a fanout that has gone
/// away, and where the change history's knee actually is. Both are now a function of the reader
/// set rather than of a constant, and this is the table that says by how much.
///
/// The first half is bytes, because "the arrangements are dropped" is a memory claim and entries
/// are not bytes. The second is versions, because that is the unit the retention ceiling is in.
#[test]
fn what_the_arrangement_lifecycle_gives_back() {
    use beck_core::engine::{fanout_footprint, SharedDataflow};

    const ROWS: usize = 200;
    const OWNERS: usize = 8;
    const FANOUT: usize = 64;

    println!("\nWhat an idle process holds — {FANOUT} subscribers, then none");
    println!(
        "{:>20}  {:>12} {:>12} {:>12} {:>8}",
        "program", "connected KB", "shared KB", "idle KB", "given back"
    );
    for (label, placed, feed) in [
        ("examples/todo.beck", support::todo_program(), false),
        ("24-feed.beck", feed_program(), true),
    ] {
        let bench = Bench::new(placed);
        let state = if feed {
            bench.feed_with(ROWS)
        } else {
            bench.state_across(ROWS, OWNERS)
        };
        let sessions: Vec<Value> = (0..FANOUT)
            .map(|i| bench.runtime.session(&format!("u{}", i % OWNERS)))
            .collect();

        let dataflow = Arc::new(SharedDataflow::new(bench.prepared.clone()));
        let mut engines: Vec<Engine> = (0..FANOUT).map(|_| dataflow.subscriber()).collect();
        for (e, s) in engines.iter_mut().zip(&sessions) {
            dataflow.render(e, &state, 1, s).expect("a shared render");
        }
        let connected =
            fanout_footprint(&state, Some(&dataflow), &engines.iter().collect::<Vec<_>>()).bytes;
        let shared = dataflow.footprint(&state).bytes;

        // Every subscription ends. This is the state a process sits in between fanouts, and until
        // now it was the same as the line above it.
        drop(engines);
        let idle = dataflow.footprint(&state).bytes;
        assert_eq!(dataflow.readers(), 0);

        println!(
            "{label:>20}  {:>12} {:>12} {:>12} {:>7.1}%",
            connected / 1024,
            shared / 1024,
            idle / 1024,
            100.0 * (shared - idle) as f64 / shared.max(1) as f64,
        );
    }
    println!(
        "  `connected` is the whole fanout, `shared` the part of it held once, `idle` what is left\n  \
         after the last subscription ends. The percentage is of `shared`: the per-subscriber half\n  \
         goes with the subscribers whether or not anything is released."
    );

    println!("\nHow much change history a fanout pins — 24-feed.beck, one laggard");
    println!(
        "{:>14}  {:>10} {:>12} {:>10}",
        "laggard's lag", "retained", "the ceiling", "saved"
    );
    let bench = Bench::new(feed_program());
    const VERSIONS: u64 = 80;
    let states: Vec<Value> = (0..=VERSIONS)
        .map(|k| bench.feed_with(ROWS + k as usize))
        .collect();
    // A subscriber lags by rendering *less often*, not by asking for an older version: asking for
    // an older version is served the current one, which is the documented behaviour and means a
    // subscriber cannot be made to lag that way.
    for lag in [0u64, 1, 4, 16, 70] {
        let dataflow = Arc::new(SharedDataflow::new(bench.prepared.clone()));
        let session = bench.runtime.session("u0");
        let mut keen = dataflow.subscriber();
        let mut slow = (lag > 0).then(|| dataflow.subscriber());
        for v in 0..=VERSIONS {
            dataflow
                .render(&mut keen, &states[v as usize], v, &session)
                .expect("the keen subscriber renders");
            // The laggard's last render, after which it stops looking and falls `lag` behind.
            if let (Some(slow), true) = (slow.as_mut(), v + lag == VERSIONS) {
                dataflow
                    .render(slow, &states[v as usize], v, &session)
                    .expect("the laggard renders");
            }
        }
        let retained = dataflow.retained();
        let ceiling = (VERSIONS as usize).min(dataflow.retention().depth);
        println!(
            "{lag:>14}  {retained:>10} {ceiling:>12} {:>9.1}×",
            ceiling as f64 / retained.max(1) as f64,
        );
        drop(slow);
    }
    println!(
        "  `the ceiling` is what the constant retained before this: every version, up to 64. What\n  \
         is retained now is the laggard's own lag — the ceiling only bites past it, which is why\n  \
         a lag of 70 retains 64 and that subscriber rebuilds instead."
    );
}
