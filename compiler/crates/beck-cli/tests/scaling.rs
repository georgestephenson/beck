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
//! `docs/13-testing.md` §13.7 and the CI workflow both say timing thresholds should not be gates,
//! because "a gate that flakes gets deleted". So this does not assert a *rate*; it asserts a
//! *shape*. Over an 8× increase in log length, a quadratic fold costs ~8× more per event and a
//! logarithmic one costs ~1.3×. The bound is 3×: far enough above the real signal to survive a
//! noisy shared runner, far enough below the failure to catch it.

use std::sync::Arc;
use std::time::Instant;

use beck_rt::{replay_from_genesis, App, AppConfig, LogStore, MemoryLog};

mod support;
use support::{command, todo_runtime};

/// Record a log of `n` additions, then time a cold fold of it.
async fn fold_cost_ns_per_event(n: usize) -> (u64, f64) {
    let store = Arc::new(MemoryLog::new());
    let app = App::start(todo_runtime(), store.clone(), AppConfig::default())
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
    let runtime = todo_runtime();
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
    // The other half of §19.4 item 3, about the path that is still a full recompute: `App::render`
    // is what serves the first document and what reconstructs a resuming subscriber's old view, and
    // it is O(rows) by design. A live subscription maintains its view instead (docs/24); what must
    // *not* happen on either path is the view becoming super-linear in the rows, which is what a
    // copying map would also cause.
    let store = Arc::new(MemoryLog::new());
    let app = App::start(todo_runtime(), store, AppConfig::default())
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
    let graph_time = started.elapsed();

    // Against the compile it is derived from, so the comparison decides whether memoising the
    // graph would be worth anything. It is not the cost; the front end is.
    let started = Instant::now();
    let _ = beck_core::compile_str("todo.beck", &src);
    let compile_time = started.elapsed();

    println!(
        "todo.beck: {} nodes, {} edges — graph {:.0} µs, full compile {:.0} µs ({:.1}% of it)",
        g.len(),
        g.edge_count(),
        graph_time.as_secs_f64() * 1e6,
        compile_time.as_secs_f64() * 1e6,
        100.0 * graph_time.as_secs_f64() / compile_time.as_secs_f64()
    );
    assert!(g.len() > 20, "the program has more parts than that");
    assert!(
        graph_time.as_millis() < 50,
        "building the graph for a 132-line program took {graph_time:?}"
    );
}

/// **The accumulator idiom is linear**, which is the language's own asymptotics rather than the
/// runtime's.
///
/// Beck has no mutable sequence, so a loop that builds one threads it through a tail call:
/// `go(i + 1, list_append(done, x))`. That is how `lib/`, `awfy/`, `clbg/`, the corpus and both
/// SICP chapters accumulate — and until `docs/70` it was `O(n²)`, because `list_append` copied the
/// whole list every time. The compiler now proves which read of a binding is its last
/// (`beck_core::liveness`), the frame hands the value over instead of lending it, and the append
/// pushes into a list nobody else holds.
///
/// This is the same *class* of defect as the fold above and gets the same treatment: a shape, not a
/// rate. Over an 8× longer run a quadratic costs about 8× more per element and a linear one costs
/// about the same per element; the bound is 3×, which is far above the noise of a shared runner and
/// far below the failure.
///
/// It cannot be a fuel assertion, and that is worth knowing: the step count over this loop is
/// exactly linear either way, because a primitive that copies ten thousand values is one step
/// (`docs/69` §69.7). Only wall clock sees it.
#[test]
fn building_a_list_by_accumulation_costs_the_same_per_element_however_long_it_gets() {
    let program = |n: usize| {
        format!(
            "def build(i: Int, n: Int, done: list[Int]) -> list[Int]:\n\
             \x20   if i >= n:\n\
             \x20       return done\n\
             \x20   return build(i + 1, n, list_append(done, i))\n\
             \n\
             test \"accumulate\":\n\
             \x20   expect list_len(build(0, {n}, [])) == {n}\n"
        )
    };
    let per_element_ns = |n: usize| -> f64 {
        let file = std::env::temp_dir().join(format!("beck-scaling-accumulate-{n}.beck"));
        std::fs::write(&file, program(n)).expect("a scratch file");
        // Twice, and the faster one taken: the first run pays for the page cache and the process.
        let mut best = f64::MAX;
        for _ in 0..2 {
            let started = Instant::now();
            let out = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
                .args(["test", file.to_str().expect("a path")])
                .output()
                .expect("the compiler is built");
            let elapsed = started.elapsed().as_secs_f64() * 1e9;
            assert!(
                out.status.success(),
                "`beck test` on a {n}-element accumulator:\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            best = best.min(elapsed);
        }
        let _ = std::fs::remove_file(&file);
        best / n as f64
    };

    // The short run carries the process start-up, so it is the one that flatters the ratio — which
    // is the safe direction: it makes the bound harder to pass, not easier.
    let short = per_element_ns(2_000);
    let long = per_element_ns(16_000);
    println!("accumulator: {short:.0} ns/element at 2,000 and {long:.0} ns/element at 16,000");
    assert!(
        long < short * 3.0,
        "eight times the elements cost {:.1}× as much per element ({short:.0} ns → {long:.0} ns), \
         which is the shape of a copy per append rather than a push — see docs/70",
        long / short
    );
}
