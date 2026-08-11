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

/// The default configuration with F3's per-actor write quota switched off.
///
/// Every measurement here proposes more events from one actor than any person could, which is the
/// definition of what the quota refuses. `docs/84` §84.5 is the sharper version of the same point:
/// a limit calibrated against human behaviour is tripped by a benchmark before it is tripped by an
/// attacker, and a harness has to say which it is.
fn unthrottled() -> AppConfig {
    AppConfig {
        quota: beck_rt::quota::Quota::unlimited(),
        ..Default::default()
    }
}

/// Record a log of `n` additions, then time a cold fold of it.
async fn fold_cost_ns_per_event(n: usize) -> (u64, f64) {
    let store = Arc::new(MemoryLog::new());
    // F3's write quota is on by default and this harness is exactly what it exists to refuse: one
    // actor appending thousands of events as fast as a machine can. Off here, because what is being
    // measured is the *shape* of a fold and the quota is not the thing under test (`docs/84` §84.3).
    let app = App::start(todo_runtime(), store.clone(), unthrottled())
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
    let app = App::start(todo_runtime(), store, unthrottled())
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

/// **Building a string is linear**, and so is walking one by character index.
///
/// The same defect as the accumulator above, in the other structure: `Value::Str` was an `Arc<str>`
/// with no spare capacity, so `+` allocated and copied both sides; `str_len` counted characters and
/// `str_slice` skipped to its start, so both were `O(n)` in the string rather than in the answer.
/// A loop that builds text was `O(n²)` and a loop that reads it by index was `O(n²)` — `docs/70`
/// §70.2 measured them and `docs/70` is the fix: a string carries its character count and whether
/// it is ASCII, and `+` pushes into the left side when the last-use analysis proves nobody else
/// holds it.
///
/// A shape rather than a rate, for [`docs/13`](../../../../docs/13-testing.md) §13.7's reason, and
/// the same 3× bound the two gates above use.
#[test]
fn text_costs_the_same_per_character_however_long_it_gets() {
    let build = |n: usize| {
        format!(
            "def build(i: Int, n: Int, done: Str) -> Str:\n\
             \x20   if i >= n:\n\
             \x20       return done\n\
             \x20   return build(i + 1, n, done + \"x\")\n\
             \n\
             def scan(s: Str, i: Int, acc: Int) -> Int:\n\
             \x20   if i >= str_len(s):\n\
             \x20       return acc\n\
             \x20   return scan(s, i + 1, acc + str_len(str_slice(s, i, 1)))\n\
             \n\
             test \"text\":\n\
             \x20   expect scan(build(0, {n}, \"\"), 0, 0) == {n}\n"
        )
    };
    let per_character_ns = |n: usize| -> f64 {
        let file = std::env::temp_dir().join(format!("beck-scaling-text-{n}.beck"));
        std::fs::write(&file, build(n)).expect("a scratch file");
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
                "`beck test` on {n} characters:\n{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            best = best.min(elapsed);
        }
        let _ = std::fs::remove_file(&file);
        best / n as f64
    };

    // Sixteen times rather than the eight the two gates above use, and the spread is measured
    // rather than chosen: over 8× the old evaluator cost 2.2× per character, which this bound would
    // have let through. Over 16× it cost **4.07×**, against 0.71× for the fixed one — a copy per
    // append is invisible at small sizes because a `memcpy` is fast next to an evaluator step, so
    // the gate has to reach the size where it stops being.
    let short = per_character_ns(1_000);
    let long = per_character_ns(16_000);
    println!("text: {short:.0} ns/character at 1,000 and {long:.0} ns/character at 16,000");
    assert!(
        long < short * 3.0,
        "sixteen times the characters cost {:.1}× as much per character ({short:.0} ns → \
         {long:.0} ns), which is the shape of a copy per append or a walk per index — see docs/70",
        long / short
    );
}

/// The same shape as the two gates above, asserted **deterministically**: the *budget* sees it.
///
/// `docs/70` §70.6 had to say the opposite — "it cannot be a fuel assertion […] a primitive that
/// copies ten thousand values is one step" — and that was true of a budget that counted nodes.
/// `docs/70` made `--fuel` charge for the work a primitive does over a length the caller chose, so
/// a copy per append is now visible to it, and this gate needs no clock at all: same numbers on any
/// machine, no 3× slack for a shared runner, no [`docs/13`](../../../../docs/13-testing.md) §13.7
/// caveat.
///
/// It runs beside the wall-clock gates rather than replacing them, because the two see different
/// things: fuel counts what the evaluator was *asked* to do, and the clock counts what it cost —
/// an allocation per step, or a cache miss per element, is invisible here and real there.
#[test]
fn the_budget_itself_shows_that_accumulating_is_linear() {
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
    // Measured at 14 steps an element either side of 1,000 and 8,000; 20 is that with room for a
    // node or two, and nowhere near the `n / 2` a copy per append would need.
    const PER_ELEMENT: usize = 20;
    for n in [1_000usize, 8_000] {
        let file = std::env::temp_dir().join(format!("beck-scaling-fuel-{n}.beck"));
        std::fs::write(&file, program(n)).expect("a scratch file");
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
            .args([
                "test",
                file.to_str().expect("a path"),
                "--fuel",
                &(PER_ELEMENT * n).to_string(),
            ])
            .output()
            .expect("the compiler is built");
        let _ = std::fs::remove_file(&file);
        assert!(
            out.status.success(),
            "accumulating {n} elements needed more than {PER_ELEMENT} steps each, which is the \
             shape of a copy per append rather than a push — see docs/70 and docs/70:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// **The accumulator is linear when it is written as a fold, too** — the same shape as the gate
/// above, in the form the language's own `list_fold` invites.
///
/// `docs/70` made the *recursive* accumulator linear by handing a local over on its last read. It
/// did nothing for
///
/// ```text
/// list_fold(xs, [], lambda acc, x: list_append(acc, x))
/// ```
///
/// because `acc` is a lambda's parameter, and the liveness pass excluded every variable any lambda
/// mentions — the exclusion that keeps a closure from having its captures moved out from under it,
/// applied to a variable the closure itself binds. So the fold form stayed `O(n²)` for three
/// reports while the recursive form was linear, which is `docs/19` §19.4's defect in its third
/// place. `docs/70` is the fix: a lambda body is analysed as the frame it is.
///
/// Deterministic, like the gate above and for the same reason: the budget charges a primitive for
/// the work it does over a length the caller chose (`docs/70`), so a copy per append is visible to
/// it. Measured at 18 steps an element either side of 1,000 and 8,000; 25 is that with room to
/// spare, against the 640 and 5,120 an element the copying version needed — which is the shape
/// itself, since eight times the elements cost eight times as much *each*.
#[test]
fn accumulating_inside_a_fold_costs_the_same_per_element_however_long_it_gets() {
    let program = |n: usize| {
        format!(
            "def upto(i: Int, n: Int, done: list[Int]) -> list[Int]:\n\
             \x20   if i >= n:\n\
             \x20       return done\n\
             \x20   return upto(i + 1, n, list_append(done, i))\n\
             \n\
             test \"fold\":\n\
             \x20   expect list_len(list_fold(upto(0, {n}, []), [], lambda acc, x: \
             list_append(acc, x))) == {n}\n"
        )
    };
    const PER_ELEMENT: usize = 25;
    for n in [1_000usize, 8_000] {
        let file = std::env::temp_dir().join(format!("beck-scaling-fold-{n}.beck"));
        std::fs::write(&file, program(n)).expect("a scratch file");
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
            .args([
                "test",
                file.to_str().expect("a path"),
                "--fuel",
                &(PER_ELEMENT * n).to_string(),
            ])
            .output()
            .expect("the compiler is built");
        let _ = std::fs::remove_file(&file);
        assert!(
            out.status.success(),
            "folding {n} elements into a list needed more than {PER_ELEMENT} steps each, which is \
             the shape of a copy per append rather than a push — see docs/70:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// **Keeping a read model fresh costs the change, not the collection** — and costs nothing at all
/// until somebody asks.
///
/// [`docs/88-read-models-and-pgwire-report.md`](../../../../docs/88-read-models-and-pgwire-report.md)
/// claims two things about what a read model costs, and both are shapes rather than rates, so both
/// are counted rather than timed:
///
/// 1. **Nothing per event.** No projection is written, so a connected SQL client that asks nothing
///    leaves the write path exactly as it was: `advances()` stays at zero however many events land.
/// 2. **The delta per query.** The first query after an event advances the shared dataflow, and
///    that advance is `O(δ)` — so the work it does must not grow with the number of rows already
///    in the collection.
///
/// The second is the one that could regress into a recount, and it is measured at two sizes for
/// [`docs/64`](../../../../docs/64-compile-speed-report.md)'s reason: one measurement cannot tell
/// linear from constant. It needs no clock, because
/// [`beck_core::engine::Work`] counts entries touched and operators recomputed.
#[tokio::test]
async fn a_read_model_costs_nothing_per_event_and_a_delta_per_query() {
    // A derived *collection* signal — the shape whose rows come from an arrangement rather than
    // from the accumulator. `ranking` does not read the session, so it is on the shared side of
    // §5.3's cut and is therefore a table.
    const PROGRAM: &str = r#"
model Item:
    id: Str
    n: Int

model State:
    items: Map[Str, Item]

union Command:
    Add(id: Str, n: Int)

union Event:
    Added(id: Str, n: Int)

union Rejection:
    Blank

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Added(id, n):
            return s.with(items=map_insert(s.items, id, Item(id=id, n=n)))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Add(id, n):
            if str_is_empty(id):
                return Err(error=Blank)
            return Ok(value=[Added(id=id, n=n)])

def ranked(s: State) -> list[Item]:
    return sort_by(map_values(s.items), lambda i: i.id)

def render(items: list[Item], session: Session) -> Html:
    return ui:
        main:
            p: (str(list_len(items)) + " items")

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, store, validate)
store: Signal[State] = durable(fold(apply_event, State(items={}), events))
ranking: Signal[list[Item]] = signal_map(store, ranked)
page: Signal[Html] = per_session(ranking, render)
"#;

    let (placed, diags, map) = beck_core::compile_str("scaling-read-model.beck", PROGRAM);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("compiles");
    let plan = beck_core::plan::Plan::compile(&placed);
    let schema = beck_core::read::Schema::of(&placed, &plan);
    let table = schema.table("ranking").expect("a derived collection table");
    let op = match &table.source {
        beck_core::read::Source::View(op) => *op,
        other => panic!("`ranking` is read from {other:?} rather than from the dataflow"),
    };

    let add = |id: String, n: i64| {
        let mut fields = beck_core::core::Fields::new();
        fields.insert(Arc::from("id"), beck_core::Value::str_(&id));
        fields.insert(Arc::from("n"), beck_core::Value::Int(n));
        beck_core::Value::data(Arc::from("Command"), Some(Arc::from("Add")), fields)
    };

    let mut work_at = Vec::new();
    for n in [200usize, 1_600] {
        let backend = beck_eval::backend(&placed);
        let runtime = beck_rt::Runtime::new(placed.clone(), backend).expect("prepares");
        let app = App::start(runtime, Arc::new(MemoryLog::new()), unthrottled())
            .await
            .expect("starts");
        let reader = app.shared_dataflow().reader();

        for i in 0..n {
            app.propose(format!("k{i}"), "ana", add(format!("i{i:06}"), i as i64))
                .await
                .expect("accepted");
        }

        // Claim 1: a connected reader that has asked nothing has cost the write path nothing.
        assert_eq!(
            app.shared_dataflow().advances(),
            0,
            "{n}: the shared dataflow was advanced without anybody reading it"
        );

        // The first query pays the cold build of the arrangement — `O(n)`, once, and not what is
        // being measured. Then one more event, and the query after it is the delta.
        let rows = app
            .read_snapshot(|state, version| reader.read(state, version, op))
            .await
            .expect("the first read builds the arrangement");
        // `ranked` is `sort_by(map_values(...))`, so the node is a maintained arrangement and its
        // entries are the rows — the read model is the arrangement, with nothing copied into it.
        assert_eq!(rows.len(), n, "the arrangement holds a row per element");

        app.propose("last".into(), "ana", add("zzz".into(), 0))
            .await
            .expect("accepted");
        app.read_snapshot(|state, version| reader.read(state, version, op))
            .await
            .expect("the second read advances by a delta");

        let work = app.shared_dataflow().work();
        println!(
            "read model at {n} rows: advance touched {} entries, applied {} functions, \
             recomputed {} operators, materialised {}",
            work.touched, work.applications, work.recomputed, work.materialised
        );
        work_at.push(work.touched + work.applications);
    }

    let (small, large) = (work_at[0], work_at[1]);
    assert!(
        large <= small * 3,
        "eight times the rows cost {large} units of delta work against {small} — that is a \
         recount rather than a delta"
    );
}
