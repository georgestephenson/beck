//! Measurements quoted in `docs/23-incremental-views-report.md`, `docs/23`, `docs/23` and
//! `docs/23-incremental-views-report.md`.
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
    let here = beck_core::edge::presence_of("ana");

    println!(
        "\n{:>7}  {:>10} {:>10} {:>8}  {:>12} {:>12} {:>6}",
        "rows", "delta", "materialise", "recomp", "maintain µs", "recompute µs", "ratio"
    );
    for n in [10usize, 100, 1_000, 5_000] {
        let state = bench.state_with(n);
        let next = bench.add(&state, n as u64 + 1);

        let mut engine = bench.engine();
        engine.render(&state, &session, &here).expect("warm");

        // One event, measured on its own.
        let mut maintain = u128::MAX;
        let mut work = engine.work();
        for _ in 0..5 {
            let mut e = bench.engine();
            e.render(&state, &session, &here).expect("warm");
            let t = Instant::now();
            e.render(&next, &session, &here).expect("step");
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
    let here = beck_core::edge::presence_of("ana");
    println!(
        "\n{:>7}  {:>9} {:>12} {:>12} {:>12} {:>7}",
        "rows", "arranged", "engine KB", "of it shared", "page KB", "×"
    );
    for n in [10usize, 100, 1_000, 5_000] {
        let state = bench.state_with(n);
        let mut engine = bench.engine();
        engine.render(&state, &session, &here).expect("render");
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

/// The measurement `docs/23-incremental-views-report.md` quotes: what the rewrite is worth.
///
/// Two tables, and the first is the honest one. Counts — operators, arrangements, entries held,
/// work per event — are properties of the plan and of the engine's own counters, so they are the
/// same on every machine. The wall clock is second because it is the one a shared runner argues
/// with, and it is measured with the two plans **alternating** rather than one after the other:
/// `docs/70` §70.7 found that a fixed A-then-B order biases a comparison by as much as the effects
/// this project reports.
#[test]
fn what_query_fusion_is_worth() {
    println!(
        "\n{:<24} {:>9} {:>9} {:>13} {:>13}",
        "program", "before", "after", "arr. before", "arr. after"
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
            "{name:<24} {:>9} {:>9} {:>13} {:>13}",
            f.operators.0, f.operators.1, f.arrangements.0, f.arrangements.1
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
    let here = beck_core::edge::presence_of("ana");

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
                e.render(&state, &session, &here).expect("warm");
                let t = Instant::now();
                e.render(&next, &session, &here).expect("step");
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

/// The measurement `docs/23-incremental-views-report.md` quotes: what a fanout costs with §5.3's
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
            let here = beck_core::edge::presence([]);

            let mut alone: Vec<Engine> = (0..n).map(|_| bench.engine()).collect();
            let started = Instant::now();
            for (e, s) in alone.iter_mut().zip(&sessions) {
                e.render(&state, s, &here).expect("a standalone render");
            }
            let alone_us = started.elapsed().as_micros();
            let unshared = fanout_footprint(&state, None, &alone.iter().collect::<Vec<_>>()).bytes;

            let dataflow = Arc::new(SharedDataflow::new(bench.prepared.clone()));
            let mut engines: Vec<Engine> = (0..n).map(|_| dataflow.subscriber()).collect();
            let started = Instant::now();
            for (e, s) in engines.iter_mut().zip(&sessions) {
                dataflow
                    .render(e, &state, 1, s, &here)
                    .expect("a shared render");
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
            let here = beck_core::edge::presence([]);

            let mut alone: Vec<Engine> = (0..n).map(|_| bench.engine()).collect();
            for (e, s) in alone.iter_mut().zip(&sessions) {
                e.render(&state, s, &here).expect("warm");
            }
            let started = Instant::now();
            for (e, s) in alone.iter_mut().zip(&sessions) {
                e.render(&next, s, &here).expect("step");
            }
            let alone_us = started.elapsed().as_micros();
            let alone_work: u64 = alone.iter().map(|e| e.work().total()).sum();

            let dataflow = Arc::new(SharedDataflow::new(bench.prepared.clone()));
            let mut engines: Vec<Engine> = (0..n).map(|_| dataflow.subscriber()).collect();
            for (e, s) in engines.iter_mut().zip(&sessions) {
                dataflow.render(e, &state, 1, s, &here).expect("warm");
            }
            let started = Instant::now();
            for (e, s) in engines.iter_mut().zip(&sessions) {
                dataflow.render(e, &next, 2, s, &here).expect("step");
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
/// [`docs/23-incremental-views-report.md`](../../../../docs/23-incremental-views-report.md)
/// §23.19 left two numbers unmeasured: what the shared dataflow retains for a fanout that has gone
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
        let here = beck_core::edge::presence([]);

        let dataflow = Arc::new(SharedDataflow::new(bench.prepared.clone()));
        let mut engines: Vec<Engine> = (0..FANOUT).map(|_| dataflow.subscriber()).collect();
        for (e, s) in engines.iter_mut().zip(&sessions) {
            dataflow
                .render(e, &state, 1, s, &here)
                .expect("a shared render");
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
        let here = beck_core::edge::presence_of("u0");
        let mut keen = dataflow.subscriber();
        let mut slow = (lag > 0).then(|| dataflow.subscriber());
        for v in 0..=VERSIONS {
            dataflow
                .render(&mut keen, &states[v as usize], v, &session, &here)
                .expect("the keen subscriber renders");
            // The laggard's last render, after which it stops looking and falls `lag` behind.
            if let (Some(slow), true) = (slow.as_mut(), v + lag == VERSIONS) {
                dataflow
                    .render(slow, &states[v as usize], v, &session, &here)
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

/// What the index under a filtered loop is worth, in a clock the counters cannot supply.
///
/// `docs/99` §99.9 item 3. `scaling.rs::a_group_a_loop_filters_for_costs_the_group_and_not_the_collection`
/// holds the *shape*, in `Work::steps` — the group is paid for and the collection is not, against a
/// refused plan whose cost grows with the collection. This is the table that says what the
/// difference costs in **time**, which is a rate and therefore printed rather than thresholded
/// (§13.7); the counters are printed beside it because a table about what an operator saved should
/// show that its two instruments agree.
///
/// Two workloads, because they are the two ends of what the operator can be worth: `spread` puts
/// the cards across the columns, which is what a board looks like, and `one column` puts every card
/// in one, where the group *is* the collection and the index has nothing left to exclude.
#[test]
fn what_a_grouped_join_is_worth() {
    use beck_core::plan::Relate;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/board.beck");
    let src = std::fs::read_to_string(&path).expect("the board example is readable");
    let (placed, diags, map) = beck_core::compile_str("board.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the board example compiles");

    let event = |variant: &str, i: usize, column: i64| {
        let mut fields = beck_core::core::Fields::new();
        fields.insert(Arc::from("id"), Value::str_(format!("c{i:06}")));
        if variant == "Added" {
            fields.insert(Arc::from("text"), Value::str_(format!("card {i}")));
        } else {
            fields.insert(Arc::from("column"), Value::Int(column));
        }
        Value::data(Arc::from("Event"), Some(Arc::from(variant)), fields)
    };

    // The clock, and the counters beside it. `Work::steps` is what the backend executed, so the
    // two now tell the same story in different units — which is the check worth having in a table
    // whose whole subject is what an operator saved.
    let once = |relate: Relate, n: usize, spread: bool| -> (u128, beck_core::engine::Work) {
        let backend = beck_eval::backend(&placed);
        let plan = Arc::new(Plan::compile_with(&placed, relate));
        let prepared = Arc::new(Prepared::new(plan, backend.as_ref()).expect("prepares"));
        let runtime = Runtime::new(placed.clone(), backend).expect("prepares");
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut engine = Engine::new(prepared);

        let mut seq = 0u64;
        let mut state = runtime.initial_state().expect("initial");
        let fold = |state: &Value, body: Value, seq: u64| {
            let env = Envelope {
                seq,
                at: At(seq as i64),
                actor: "ana".to_string(),
                body: body.clone(),
            };
            runtime.fold(state, &env, body).expect("fold")
        };
        for i in 0..n {
            seq += 1;
            state = fold(&state, event("Added", i, 0), seq);
            if spread {
                seq += 1;
                state = fold(&state, event("Moved", i, 1 + (i as i64 % 2)), seq);
            }
        }
        engine.render(&state, &session, &here).expect("warm");
        seq += 1;
        let next = fold(&state, event("Added", n + 1, 0), seq);
        let t = Instant::now();
        engine.render(&next, &session, &here).expect("step");
        (t.elapsed().as_micros(), engine.work())
    };

    // The first thing measured in a process pays for warm-up, and this suite has been caught by
    // that before (CHANGELOG 2026-08-17).
    let _ = once(Relate::Recognise, 200, true);

    println!(
        "\n{:>8}  {:>7}  {:>13} {:>11} {:>7}  {:>22} {:>22}",
        "cards",
        "layout",
        "arrange_by µs",
        "refused µs",
        "ratio",
        "arrange_by a/t/m/r",
        "refused a/t/m/r"
    );
    let counted = |w: beck_core::engine::Work| {
        format!(
            "{}/{}/{}/{} +{}",
            w.applications, w.touched, w.materialised, w.recomputed, w.steps
        )
    };
    for spread in [true, false] {
        for n in [200usize, 1_600] {
            let runs = |relate: Relate| {
                let mut out: Vec<(u128, beck_core::engine::Work)> =
                    (0..3).map(|_| once(relate, n, spread)).collect();
                out.sort_by_key(|(t, _)| *t);
                out.remove(0)
            };
            let (with, with_work) = runs(Relate::Recognise);
            let (without, without_work) = runs(Relate::Refuse);
            println!(
                "{n:>8}  {:>7}  {with:>13} {without:>11} {:>6.1}×  {:>22} {:>22}",
                if spread { "spread" } else { "one col" },
                without as f64 / with.max(1) as f64,
                counted(with_work),
                counted(without_work)
            );
        }
    }
    println!(
        "\n  spread  — the cards across the three columns, which is what a board looks like.\n\
         \x20 one col — every card in the column the last event touches, so the group is the\n\
         \x20           whole collection and the index has nothing left to exclude. That row is\n\
         \x20           the honest ceiling on this operator: it removes the scan, not the group\n\
         \x20           (docs/99 §99.9 item 6 is what removes the group).\n\
         \x20 a/t/m/r — applications, entries touched, entries materialised, operators\n\
         \x20           recomputed, and `steps` is what the backend executed inside all of\n\
         \x20           them. The refused column's first four are the same at both sizes,\n\
         \x20           because it rebuilds the page inside one per-element function and one\n\
         \x20           application is one application; its `steps` and its clock both move\n\
         \x20           with the collection, which is the work those four cannot see."
    );
}

/// What answering a group's size from a count rather than from the group saves, in a clock.
///
/// `docs/99` §99.9 item 6. `scaling.rs::counting_a_group_does_not_build_it` holds the shape — one
/// entry copied at any pile size against a number that grows — and this is the rate, which is
/// printed rather than thresholded (§13.7).
///
/// The contrast here is **not** `Relate::Refuse`, and that is a scoping choice rather than a
/// limitation: the refused plan has item 3's scan in it too, so comparing against it would credit
/// this change with the previous one's win. The variant instead writes the same count as
/// `list_len(sort_by(filter_list(…), …))`, which prints the same page and is no longer an aggregate
/// the recogniser can see — item 3's index, item 6's aggregate withheld. `scaling.rs` is where the
/// off switch itself is measured.
#[test]
fn what_counting_a_group_saves() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/35-workload.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let compiled = |name: &str, text: &str| {
        let (placed, diags, map) = beck_core::compile_str(name, text);
        assert!(!diags.has_errors(), "{}", diags.render(&map));
        placed.expect("it compiles")
    };
    let counted = compiled("35-workload.beck", &src);
    let grouped = compiled(
        "35-workload-built.beck",
        &src.replace(
            "return list_len(filter_list(map_values(s.issues), lambda i: i.assignee == who))",
            "return list_len(sort_by(filter_list(map_values(s.issues), lambda i: i.assignee == \
             who), lambda i: i.title))",
        ),
    );

    let event = |variant: &str, fields: &[(&str, &str)]| {
        let mut f = beck_core::core::Fields::new();
        for (k, v) in fields {
            f.insert(Arc::from(*k), Value::str_(*v));
        }
        Value::data(Arc::from("Event"), Some(Arc::from(variant)), f)
    };

    let once = |subject: &beck_core::Placed, n: usize| -> u128 {
        let backend = beck_eval::backend(subject);
        let prepared = Arc::new(
            Prepared::new(Arc::new(Plan::compile(subject)), backend.as_ref()).expect("prepares"),
        );
        let runtime = Runtime::new(subject.clone(), backend).expect("prepares");
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut engine = Engine::new(prepared);

        let mut seq = 0u64;
        let mut state = runtime.initial_state().expect("initial");
        let fold = |state: &Value, body: Value, seq: u64| {
            let env = Envelope {
                seq,
                at: At(seq as i64),
                actor: "ana".to_string(),
                body: body.clone(),
            };
            runtime.fold(state, &env, body).expect("fold")
        };
        seq += 1;
        state = fold(
            &state,
            event("Hired", &[("id", "p1"), ("name", "Ada")]),
            seq,
        );
        for i in 0..n {
            seq += 1;
            let id = format!("i{i:06}");
            state = fold(
                &state,
                event("Filed", &[("id", &id), ("title", &id), ("assignee", "p1")]),
                seq,
            );
        }
        engine.render(&state, &session, &here).expect("warm");
        seq += 1;
        let id = format!("i{n:06}");
        let next = fold(
            &state,
            event("Filed", &[("id", &id), ("title", &id), ("assignee", "p1")]),
            seq,
        );
        let t = Instant::now();
        engine.render(&next, &session, &here).expect("step");
        t.elapsed().as_micros()
    };

    // The first thing measured in a process pays for warm-up (CHANGELOG 2026-08-17).
    let _ = once(&counted, 200);

    println!(
        "\n{:>8}  {:>12} {:>12} {:>7}",
        "issues", "counted µs", "grouped µs", "ratio"
    );
    for n in [200usize, 1_600] {
        let with = (0..3).map(|_| once(&counted, n)).min().expect("three runs");
        let without = (0..3).map(|_| once(&grouped, n)).min().expect("three runs");
        println!(
            "{n:>8}  {with:>12} {without:>12} {:>6.1}×",
            without as f64 / with.max(1) as f64
        );
    }
    println!(
        "\n  Both render the same page. `counted` answers each person's pile from a tally the\n\
         \x20 join keeps; `grouped` builds the pile and measures it, which is what item 3 left\n\
         \x20 behind and what this closes. The ratio grows with the pile because one side is\n\
         \x20 constant in it and the other is linear."
    );
}
