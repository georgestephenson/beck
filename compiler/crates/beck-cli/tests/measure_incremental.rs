//! Measurements quoted in `docs/24-incremental-views-report.md`.
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

    fn add(&self, state: &Value, n: u64) -> Value {
        let id = Value::Data {
            ty: Arc::from("Id"),
            variant: None,
            fields: Arc::new(std::collections::BTreeMap::from([(
                Arc::from("value"),
                Value::str_(format!("{n:08}")),
            )])),
        };
        let event = Value::Data {
            ty: Arc::from("Event"),
            variant: Some(Arc::from("Added")),
            fields: Arc::new(std::collections::BTreeMap::from([
                (Arc::from("id"), id),
                (Arc::from("text"), Value::str_(format!("item {n}"))),
            ])),
        };
        let env = Envelope {
            seq: n,
            at: At(n as i64),
            actor: "ana".to_string(),
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
    // ~5 KB ([`docs/18-phase-0-report.md`] §18.3).
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
                          thousand subscribers should hold once between them and this engine holds\n  \
                          once each (docs/24 §24.7).\n  \
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
