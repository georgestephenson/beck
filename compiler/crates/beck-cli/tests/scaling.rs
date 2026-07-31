//! The asymptotics of the two things that grow: the fold, and the dependency graph.
//!
//! [`docs/19-phase-1-report.md`](../../../../docs/19-phase-1-report.md) §19.4 item 3 measured the
//! Phase 1 fold at 57,203 events/s over 200 events and 7,562 events/s over 4,818 — folding 24×
//! more events cost 7.6× more *per event*, which is not `O(log n)`. The accumulator was being
//! copied on every insert, making a fold over a log `O(events × rows)`.
//!
//! That is a semantic defect, not a backend one: it would survive into Cranelift unchanged. This
//! file is the gate that keeps it fixed.
//!
//! # Why the bound is loose
//!
//! [`docs/13-testing.md`] §13.7 and the CI workflow both say timing thresholds should not be gates,
//! because "a gate that flakes gets deleted". So this does not assert a *rate*; it asserts a
//! *shape*. Over an 8× increase in log length, a quadratic fold costs ~8× more per event and a
//! logarithmic one costs ~1.3×. The bound is 3×: far enough above the real signal to survive a
//! noisy shared runner, far enough below the failure to catch it.

use std::sync::Arc;
use std::time::Instant;

use beck_rt::{replay_from_genesis, App, AppConfig, LogStore, MemoryLog, Runtime};

mod support;
use support::{command, todo_program};

/// Record a log of `n` additions, then time a cold fold of it.
async fn fold_cost_ns_per_event(n: usize) -> (u64, f64) {
    let store = Arc::new(MemoryLog::new());
    let app = App::start(todo_program(), store.clone(), AppConfig::default())
        .await
        .expect("app starts");

    for i in 0..n {
        app.propose(
            format!("k{i}"),
            "alice".to_string(),
            command(
                "Add",
                &[("id", &format!("t{i:06}")), ("text", &format!("todo {i}"))],
            ),
        )
        .await
        .expect("accepted");
    }
    let head = store.head().await.expect("head");
    assert_eq!(
        head as usize, n,
        "the harness recorded the wrong number of events"
    );

    // Genesis, not `replay_to`: the sequencer snapshots every 1,000 events, so `replay_to` would
    // load a snapshot and fold nothing — measuring the snapshot path instead of the fold. That
    // mistake made the first version of this test report 1 ns/event and pass.
    let runtime = Runtime::new(todo_program()).expect("runtime");
    let started = Instant::now();
    let (_, at) = replay_from_genesis(&runtime, store.as_ref())
        .await
        .expect("replay");
    let elapsed = started.elapsed();
    assert_eq!(at, head);
    (head, elapsed.as_nanos() as f64 / n as f64)
}

#[tokio::test(flavor = "multi_thread")]
async fn folding_a_log_is_not_quadratic() {
    let (small_n, small) = fold_cost_ns_per_event(500).await;
    let (large_n, large) = fold_cost_ns_per_event(4_000).await;
    let growth = large / small;
    let longer = large_n as f64 / small_n as f64;

    println!(
        "fold cost per event: {small_n} events → {small:.0} ns, \
         {large_n} events → {large:.0} ns ({growth:.2}× over a {longer:.0}× longer log)"
    );

    assert!(
        growth < 3.0,
        "the per-event fold cost grew {growth:.2}× over a {longer:.0}× longer log \
         ({small:.0} ns → {large:.0} ns). A fold that copies its accumulator grows ~{longer:.0}×; \
         one that shares structure grows ~1×. See docs/19 §19.4 item 3."
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_over_a_large_state_is_still_one_pass() {
    // The other half of §19.4 item 3: views are full recompute per event, which is O(rows) by
    // design and stays that way until Phase 3 makes them incremental. What must *not* happen is
    // the view becoming super-linear in the rows, which is what a copying map would also cause.
    let store = Arc::new(MemoryLog::new());
    let app = App::start(todo_program(), store, AppConfig::default())
        .await
        .expect("app starts");
    for i in 0..2_000 {
        app.propose(
            format!("k{i}"),
            "alice".to_string(),
            command(
                "Add",
                &[("id", &format!("t{i:06}")), ("text", &format!("todo {i}"))],
            ),
        )
        .await
        .expect("accepted");
    }

    let state = app.state().await;
    let started = Instant::now();
    let html = app.runtime().view(&state, "alice").expect("view");
    let elapsed = started.elapsed();
    println!(
        "view over 2,000 rows: {:.1} ms",
        elapsed.as_secs_f64() * 1e3
    );
    assert!(html.render().contains("todo 1999"));
}

/// Build a graph of `n` definitions in a chain, each also depending on a shared root, and time it.
fn graph_cost_ns_per_node(n: usize) -> f64 {
    use beck_core::graph::{EdgeKind, GraphBuilder, GraphNode, NodeId, NodeKind};
    use beck_core::Tier;

    let mut b = GraphBuilder::new();
    for i in 0..n {
        b.node(GraphNode {
            name: Arc::from(format!("n{i}").as_str()),
            kind: NodeKind::Function,
            tier: Tier::Any,
            effects: Vec::new(),
            because: String::new(),
            span: Default::default(),
        });
    }
    // A chain, plus a shared root everything calls: one long path for the SCC pass to walk and one
    // high-degree vertex, which are the two shapes a real program has. `2..` for the root edges
    // because node 1's chain edge already goes to node 0, and duplicates collapse.
    for i in 1..n {
        b.edge(NodeId(i as u32), NodeId(i as u32 - 1), EdgeKind::Calls);
    }
    for i in 2..n {
        b.edge(NodeId(i as u32), NodeId(0), EdgeKind::Calls);
    }

    let started = Instant::now();
    let g = b.finish();
    let elapsed = started.elapsed();
    assert_eq!(g.len(), n);
    assert_eq!(g.edge_count(), (n - 1) + (n - 2));
    elapsed.as_nanos() as f64 / n as f64
}

#[test]
fn building_the_dependency_graph_is_linear() {
    // "Almost instant" is not a target to chase; it is what `O(V + E)` construction and CSR
    // adjacency make unavoidable. This is the gate that says so — a representation change that
    // introduced a per-node scan (a `contains` inside a loop, a sort per vertex) would show up
    // here as growth and nowhere else.
    let small = graph_cost_ns_per_node(10_000);
    let large = graph_cost_ns_per_node(200_000);
    let growth = large / small;

    println!(
        "graph build: 10,000 nodes → {small:.0} ns/node, 200,000 nodes → {large:.0} ns/node \
         ({growth:.2}× over a 20× larger program)"
    );
    assert!(
        growth < 3.0,
        "per-node graph build cost grew {growth:.2}× over a 20× larger program \
         ({small:.0} ns → {large:.0} ns). Linear construction grows ~1×; anything quadratic \
         grows ~20×."
    );
}

#[test]
fn the_whole_todo_program_graph_is_built_in_well_under_a_millisecond() {
    // The number that matters in practice: not the synthetic 200,000-node case, but the real
    // program, rebuilt from scratch on every keystroke in the worst case.
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/todo.beck"
    ))
    .expect("the example is where the CLI says it is");
    let (placed, diags, map) = beck_core::compile_str("todo.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the example compiles");

    let started = Instant::now();
    let g = beck_infra::dependency_graph(&placed);
    let elapsed = started.elapsed();
    println!(
        "todo.beck: {} nodes, {} edges, built in {:.0} µs",
        g.len(),
        g.edge_count(),
        elapsed.as_secs_f64() * 1e6
    );
    assert!(g.len() > 20, "the program has more parts than that");
    assert!(
        elapsed.as_millis() < 50,
        "building the graph for a 132-line program took {elapsed:?}"
    );
}
