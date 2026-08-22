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
/// definition of what the quota refuses. `docs/82` §82.10 is the sharper version of the same point:
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
    // measured is the *shape* of a fold and the quota is not the thing under test (`docs/82` §82.4).
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
    // it is O(rows) by design. A live subscription maintains its view instead (docs/23); what must
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

/// **A loop whose body reads the state costs the change, not the collection.**
///
/// [`docs/99-the-data-tier-means-of-combination.md`](../../../../docs/99-the-data-tier-means-of-combination.md)
/// §99.9's first item, and it was written to go **red**: `corpus/27-review.beck` loops over its
/// notes and asks its verdicts about each one, which is an equi-join nobody wrote as one. The
/// engine had no operator for it, so the loop's per-element function *captured the accumulator* —
/// and a captured node that moves on every event makes the function a different function on every
/// event, so every note was reconsidered whatever changed. A nested-loop join with no index, per
/// event, and every gate in this file was blind to it.
///
/// A shape rather than a rate, for [`docs/13`](../../../../docs/13-testing.md) §13.7's reason, and
/// with no clock in it at all: [`beck_core::engine::Work`] counts applications, entries touched and
/// operators recomputed, so this is the deterministic instrument the read-model gate below uses.
///
/// # It measures both settings, which is what stops it being a claim
///
/// [`Relate::Refuse`] is the off switch [`docs/08`](../../../../docs/08-roadmap.md) §8.3 item 8
/// requires of a choice the compiler makes unbidden, and running it here does two jobs at once: it
/// is the proof the switch works, and it is the gate's own evidence that it **can fail** — the
/// pattern [`docs/82`](../../../../docs/82-the-edge-report.md) §82.10 says four of this project's
/// gates lacked. The refused path is asserted to grow, so a green run states the difference the
/// operator makes rather than only that today's number is small.
///
/// `materialised` is excluded from both and the other three counters are not. Assembling the page's
/// children is `O(n)` by design (§23.8) and would swamp the signal; `recomputed` is *included* so
/// that a "fix" which moved the work into a pointwise operator would still be caught.
#[test]
fn maintaining_a_view_whose_loop_looks_something_up_costs_the_same_at_any_size() {
    use beck_core::core::Fields;
    use beck_core::engine::{Engine, Prepared};
    use beck_core::plan::{Op, Plan, Relate};
    use beck_core::Value;
    use beck_rt::{Envelope, Instant, Runtime};

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/27-review.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let (placed, diags, map) = beck_core::compile_str("27-review.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the corpus program compiles");

    let has_join = |relate: Relate| {
        Plan::compile_with(&placed, relate)
            .nodes
            .iter()
            .any(|n| matches!(n.op, Op::Join { .. }))
    };
    // Both directions. Without the first, a program edited out of the shape would leave this gate
    // measuring a loop that relates nothing and passing; without the second, the switch could have
    // stopped working and the comparison below would be one path measured twice.
    assert!(
        has_join(Relate::Recognise),
        "27-review.beck no longer compiles to a join, so this gate measures something else"
    );
    assert!(
        !has_join(Relate::Refuse),
        "`Relate::Refuse` left a join in the plan, so the off switch is not one"
    );

    let submitted = |i: usize| {
        let mut fields = Fields::new();
        fields.insert(Arc::from("id"), Value::str_(format!("n{i:06}")));
        fields.insert(Arc::from("text"), Value::str_(format!("note {i}")));
        Value::data(Arc::from("Event"), Some(Arc::from("Submitted")), fields)
    };

    // What maintaining one more event costs, once `n` notes are already there.
    let work_at = |relate: Relate, n: usize| -> u64 {
        let backend = beck_eval::backend(&placed);
        let plan = Arc::new(Plan::compile_with(&placed, relate));
        let prepared = Arc::new(Prepared::new(plan, backend.as_ref()).expect("the plan prepares"));
        let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut engine = Engine::new(prepared);

        let fold = |state: &Value, seq: usize| {
            let event = submitted(seq);
            let env = Envelope {
                seq: seq as u64,
                at: Instant(seq as i64),
                actor: "ana".to_string(),
                body: event.clone(),
            };
            runtime.fold(state, &env, event).expect("folds")
        };
        let mut state = runtime.initial_state().expect("an initial accumulator");
        for i in 0..n {
            state = fold(&state, i + 1);
        }
        // The cold render builds every arrangement — `O(n)`, once, and not what is measured.
        engine.render(&state, &session, &here).expect("renders");
        state = fold(&state, n + 1);
        engine.render(&state, &session, &here).expect("renders");

        let work = engine.work();
        println!(
            "{relate:?} at {n:>5} notes: {:>5} applications, {:>5} touched, {:>3} recomputed, \
             {:>5} materialised",
            work.applications, work.touched, work.recomputed, work.materialised
        );
        work.applications + work.touched + work.recomputed
    };

    let (small, large) = (
        work_at(Relate::Recognise, 200),
        work_at(Relate::Recognise, 1_600),
    );
    let (refused_small, refused_large) =
        (work_at(Relate::Refuse, 200), work_at(Relate::Refuse, 1_600));

    assert!(
        large <= small * 3,
        "eight times the notes cost {large} units of maintenance against {small}. A loop whose \
         body looks up in another collection is an equi-join; without the operator its function \
         captures the accumulator and every event reconsiders every row — docs/99 §99.3"
    );
    assert!(
        refused_large > refused_small * 3,
        "with the join refused, eight times the notes cost {refused_large} units against \
         {refused_small} — which is not the nested loop this gate exists to say the operator \
         removes. Either the loop stopped being one, or something else made the refused path fast; \
         either way the comparison above no longer means what it says"
    );
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
/// (`docs/46` §46.14). Only wall clock sees it.
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
/// [`docs/23-incremental-views-report.md`](../../../../docs/23-incremental-views-report.md)
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

/// The board example, compiled once for the three gates below.
fn board_program() -> beck_core::Placed {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/board.beck");
    let src = std::fs::read_to_string(&path).expect("the board example is readable");
    let (placed, diags, map) = beck_core::compile_str("board.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("the board example compiles")
}

/// What one more card costs the board, once `n` are already on it.
///
/// `elsewhere` sends every earlier card to a column the last event does not touch, which holds the
/// probed group's size fixed while the collection grows; without it every card lands in the column
/// the last one does, and the group *is* the collection.
fn board_event_cost(
    placed: &beck_core::Placed,
    relate: beck_core::plan::Relate,
    n: usize,
    elsewhere: bool,
) -> beck_core::engine::Work {
    use beck_core::core::Fields;
    use beck_core::engine::{Engine, Prepared};
    use beck_core::plan::Plan;
    use beck_core::Value;
    use beck_rt::{Envelope, Instant, Runtime};

    let added = |i: usize| {
        let mut fields = Fields::new();
        fields.insert(Arc::from("id"), Value::str_(format!("c{i:06}")));
        fields.insert(Arc::from("text"), Value::str_(format!("card {i}")));
        Value::data(Arc::from("Event"), Some(Arc::from("Added")), fields)
    };
    let moved = |i: usize, column: i64| {
        let mut fields = Fields::new();
        fields.insert(Arc::from("id"), Value::str_(format!("c{i:06}")));
        fields.insert(Arc::from("column"), Value::Int(column));
        Value::data(Arc::from("Event"), Some(Arc::from("Moved")), fields)
    };

    let backend = beck_eval::backend(placed);
    let plan = Arc::new(Plan::compile_with(placed, relate));
    let prepared = Arc::new(Prepared::new(plan, backend.as_ref()).expect("the plan prepares"));
    let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
    let session = runtime.session("ana");
    let here = beck_core::edge::presence_of("ana");
    let mut engine = Engine::new(prepared);

    let mut seq = 0u64;
    let mut state = runtime.initial_state().expect("an initial accumulator");
    let fold = |state: &Value, body: Value, seq: u64| {
        let env = Envelope {
            seq,
            at: Instant(seq as i64),
            actor: "ana".to_string(),
            body: body.clone(),
        };
        runtime.fold(state, &env, body).expect("folds")
    };
    for i in 0..n {
        seq += 1;
        state = fold(&state, added(i), seq);
        if elsewhere {
            seq += 1;
            state = fold(&state, moved(i, 1 + (i as i64 % 2)), seq);
        }
    }
    // The cold render builds every arrangement — `O(n)`, once, and not what is measured.
    engine.render(&state, &session, &here).expect("renders");
    seq += 1;
    state = fold(&state, added(n + 1), seq);
    engine.render(&state, &session, &here).expect("renders");

    let work = engine.work();
    println!(
        "{relate:?} at {n:>5} cards, group held {}: {:>7} steps, {:>5} materialised ({} \
         applications, {} touched, {} recomputed)",
        if elsewhere { "fixed  " } else { "growing" },
        work.steps,
        work.materialised,
        work.applications,
        work.touched,
        work.recomputed
    );
    work
}

/// **A loop that filters another collection by an equality pays for the group, not the collection.**
///
/// [`docs/99-the-data-tier-means-of-combination.md`](../../../../docs/99-the-data-tier-means-of-combination.md)
/// §99.9 item 3, and the program it names: `examples/board.beck` renders three columns out of one
/// map of cards, and each column is `filter_list(map_values(b.cards), lambda c: c.column == n)`
/// inside a loop over the columns. That is a many-to-one equi-join over an index nobody built, so
/// the loop's per-element function captured the accumulator and every event re-scanned every card
/// once per column. `arrange_by` builds the index and the join probes the range under one key.
///
/// # Both settings, which is what stops the paragraph above being a claim
///
/// [`Relate::Refuse`] is the off switch [`docs/08`](../../../../docs/08-roadmap.md) §8.3 item 8
/// requires of a choice the compiler makes unbidden, and the gate measures it: with the probed
/// group's size held fixed, the recognised plan's per-event work does not move between 200 cards
/// and 1,600, and the refused plan's grows with the collection. The instrument is
/// [`beck_core::engine::Work::steps`] — what the backend executed — because the refused plan does
/// all of its work *inside* one per-element function, where the arrangement counters cannot see it
/// and where this gate used to have to look away.
///
/// # And what the operator does **not** remove, in the same run
///
/// `materialised` counts entries copied out of an arrangement to hand a consumer a `Value::List`,
/// which is what a group is. Held with the group fixed it does not move; with every card in the one
/// column the event touches it grows with the collection, because then the group *is* the
/// collection. That row is this operator's honest ceiling and §99.9 item 6 is what takes it.
#[test]
fn a_group_a_loop_filters_for_costs_the_group_and_not_the_collection() {
    use beck_core::plan::{Matching, Op, Plan, Relate};

    let placed = board_program();

    let ops = |relate: Relate| -> (bool, bool) {
        let plan = Plan::compile_with(&placed, relate);
        (
            plan.nodes
                .iter()
                .any(|n| matches!(n.op, Op::ArrangeBy { .. })),
            plan.nodes.iter().any(|n| {
                matches!(
                    n.op,
                    Op::Join {
                        matched: Matching::Group,
                        ..
                    }
                )
            }),
        )
    };
    assert_eq!(
        ops(Relate::Recognise),
        (true, true),
        "examples/board.beck no longer compiles to an `arrange_by` and a join that answers with a \
         group, so this gate measures something else"
    );
    assert_eq!(
        ops(Relate::Refuse),
        (false, false),
        "`Relate::Refuse` left the index in the plan, so the off switch is not one"
    );

    let recognised = |n| board_event_cost(&placed, Relate::Recognise, n, true);
    let refused = |n| board_event_cost(&placed, Relate::Refuse, n, true);
    let (small, large) = (recognised(200), recognised(1_600));
    let (refused_small, refused_large) = (refused(200), refused(1_600));

    assert!(
        large.steps <= small.steps,
        "eight times the cards cost {} backend steps against {}, with the group the last event \
         touches the same size in both. A loop that filters another collection by an equality is a \
         many-to-one equi-join; without the index it re-scans the collection once per element of \
         the loop — docs/99 §99.9 item 3",
        large.steps,
        small.steps
    );
    assert!(
        refused_large.steps > refused_small.steps * 3,
        "with the join refused, eight times the cards cost {} steps against {} — which is not the \
         re-scan this gate exists to say the operator removes. Either the loop stopped being one, \
         or something else made the refused path cheap; either way the flat row above no longer \
         means what it says",
        refused_large.steps,
        refused_small.steps
    );

    // The ceiling, in the counter that sees a group being built rather than a collection scanned.
    let growing = |n| board_event_cost(&placed, Relate::Recognise, n, false);
    let (growing_small, growing_large) = (growing(200), growing(1_600));
    assert!(
        growing_large.materialised > growing_small.materialised * 3,
        "with every card in one column, eight times the cards copied {} entries against {} — so \
         the counter that reads a group did not follow the group, and this gate's claim about what \
         `arrange_by` leaves behind says nothing",
        growing_large.materialised,
        growing_small.materialised
    );
    assert_eq!(
        (small.materialised, large.materialised),
        (4, 4),
        "with the group held fixed the page copies its three sections and the one card in the \
         group the event touched, whatever the collection holds"
    );
}

/// **Asking a group how big it is does not build the group.**
///
/// [`docs/99-the-data-tier-means-of-combination.md`](../../../../docs/99-the-data-tier-means-of-combination.md)
/// §99.9 item 6's first aggregate, and the leftover item 3 handed it. `corpus/35-workload.beck`
/// shows every person and how many issues name them. Item 3 turned the scan into an index probe and
/// then **materialised the group in order to count it**, so an event still cost the size of the pile
/// it landed on. The join now keeps a count per key and moves it by ±1 as the index moves.
///
/// Two instruments, and each answers a different half:
///
/// * [`beck_core::engine::Work::steps`] against [`Relate::Refuse`] — what the whole plan cost,
///   including the per-element function the refused plan does everything inside. Constant against
///   linear is the property, and the off switch is what makes it a measurement rather than a claim
///   ([`docs/08`](../../../../docs/08-roadmap.md) §8.3 item 8).
/// * `materialised` — entries copied out of an arrangement, which is what a group *is*. Held to the
///   exact number rather than to a bound: **one**, which is this page's own single `<li>` being
///   assembled (§23.8's remaining constant factor) and not a group. Two would mean the group was
///   built.
#[test]
fn counting_a_group_does_not_build_it() {
    use beck_core::core::Fields;
    use beck_core::engine::{Engine, Prepared, Work};
    use beck_core::plan::{Matching, Op, Plan, Relate};
    use beck_core::Value;
    use beck_rt::{Envelope, Instant, Runtime};

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/35-workload.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let (placed, diags, map) = beck_core::compile_str("35-workload.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the corpus program compiles");

    let counts = |relate: Relate| {
        Plan::compile_with(&placed, relate).nodes.iter().any(|n| {
            matches!(
                n.op,
                Op::Join {
                    matched: Matching::Count,
                    ..
                }
            )
        })
    };
    assert!(
        counts(Relate::Recognise),
        "corpus/35-workload.beck no longer compiles to a join that answers with a count, so this \
         gate measures something else"
    );
    assert!(
        !counts(Relate::Refuse),
        "`Relate::Refuse` left the aggregate in the plan, so the off switch is not one"
    );

    let event = |variant: &str, fields: &[(&str, &str)]| {
        let mut f = Fields::new();
        for (k, v) in fields {
            f.insert(Arc::from(*k), Value::str_(*v));
        }
        Value::data(Arc::from("Event"), Some(Arc::from(variant)), f)
    };

    // What one more issue costs, once `n` are already filed against the same person. The pile the
    // event lands on is the whole collection, which is the worst case for an aggregate that builds
    // its group and no case at all for one that counts.
    let cost_at = |relate: Relate, n: usize| -> Work {
        let backend = beck_eval::backend(&placed);
        let plan = Arc::new(Plan::compile_with(&placed, relate));
        let prepared = Arc::new(Prepared::new(plan, backend.as_ref()).expect("the plan prepares"));
        let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut engine = Engine::new(prepared);

        let mut seq = 0u64;
        let mut state = runtime.initial_state().expect("an initial accumulator");
        let fold = |state: &Value, body: Value, seq: u64| {
            let env = Envelope {
                seq,
                at: Instant(seq as i64),
                actor: "ana".to_string(),
                body: body.clone(),
            };
            runtime.fold(state, &env, body).expect("folds")
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
        // The cold render builds every arrangement — `O(n)`, once, and not what is measured.
        engine.render(&state, &session, &here).expect("renders");
        seq += 1;
        let id = format!("i{n:06}");
        state = fold(
            &state,
            event("Filed", &[("id", &id), ("title", &id), ("assignee", "p1")]),
            seq,
        );
        engine.render(&state, &session, &here).expect("renders");

        let work = engine.work();
        println!(
            "{relate:?} at {n:>5} issues: {:>6} steps, {:>5} materialised ({} applications, {} \
             touched, {} recomputed)",
            work.steps, work.materialised, work.applications, work.touched, work.recomputed
        );
        work
    };

    let (small, large) = (
        cost_at(Relate::Recognise, 200),
        cost_at(Relate::Recognise, 1_600),
    );
    let (refused_small, refused_large) =
        (cost_at(Relate::Refuse, 200), cost_at(Relate::Refuse, 1_600));

    assert!(
        large.steps <= small.steps,
        "eight times the issues cost {} backend steps against {}. Asking a group how big it is is \
         a question the group does not have to exist to answer — docs/99 §99.9 item 6",
        large.steps,
        small.steps
    );
    assert!(
        refused_large.steps > refused_small.steps * 3,
        "with the aggregate refused, eight times the issues cost {} steps against {} — which is \
         not the pile-building this gate exists to say the count removes, so the flat row above \
         says nothing",
        refused_large.steps,
        refused_small.steps
    );
    assert_eq!(
        (small.materialised, large.materialised),
        (1, 1),
        "the page asks for no group's rows and has one row of its own, so one entry is copied out \
         of an arrangement whatever the pile holds — docs/99 §99.9 item 6"
    );
}

/// **What a render reports includes what happened inside its per-element functions.**
///
/// The counters [`beck_core::engine::Work`] keeps are what the engine did *to* its arrangements,
/// and they stop at the boundary of a call: one application is one application whether the function
/// it ran read a field or rebuilt a page. So a plan that hides its work in one opaque operator
/// reported the *same four numbers* however much work it did, and every shape gate in this file was
/// blind to exactly the pessimisation an opaque operator can hide — the silent kind, in the
/// direction that flatters.
///
/// This is that case, asserted from both ends. `examples/board.beck` with the join refused rebuilds
/// all three columns inside one per-element function on every event, so:
///
/// * the four counters are **identical** at 200 cards and at 1,600 — the blindness, still true and
///   still fine, because those counters are answering a different question; and
/// * `steps` — what the backend executed, [`beck_core::backend::Steps`] — **grows with the
///   collection**, which is the answer the clock has always given and no counter could.
///
/// Both halves are asserted, because a `steps` that grew while the four moved too would mean the
/// engine had started counting the same work twice, and a `steps` of zero would pass a
/// grows-with-`n` test written only as "not constant".
#[test]
fn the_work_a_render_reports_includes_what_happened_inside_it() {
    use beck_core::plan::Relate;

    let board = board_program();
    let (small, large) = (
        board_event_cost(&board, Relate::Refuse, 200, true),
        board_event_cost(&board, Relate::Refuse, 1_600, true),
    );

    assert_eq!(
        (
            small.applications,
            small.touched,
            small.materialised,
            small.recomputed
        ),
        (
            large.applications,
            large.touched,
            large.materialised,
            large.recomputed
        ),
        "the four arrangement counters are supposed to be identical here — one opaque operator, \
         applied the same number of times at either size. If they have started to differ, this \
         gate is no longer about the thing it was written for"
    );
    assert!(
        large.steps > small.steps * 3,
        "eight times the cards cost {} backend steps against {}, from a plan that rebuilds the \
         whole page inside one per-element function. `Work` is reporting the shape of the plan \
         rather than the shape of the work — which is what it did before it counted what a call \
         did, and what made every gate in this file blind to an opaque operator",
        large.steps,
        small.steps
    );
}

/// **Asking a group for one of its ends does not build the group.**
///
/// [`docs/99-the-data-tier-means-of-combination.md`](../../../../docs/99-the-data-tier-means-of-combination.md)
/// §99.9 item 6's other two aggregates, and the sibling of `counting_a_group_does_not_build_it`
/// above. `corpus/36-auction.beck` shows the lowest and the highest bid on every lot;
/// [`beck_core::plan::Op::GroupBy`] keeps a multiset per group and reads an end of it, so a bid
/// costs two applications and a tree insert whatever the pile it lands on holds.
///
/// The measured event is a **new low**, on purpose: it is the worst case for the operator, because
/// the answer moves and the page is reassembled. An event that changed neither end would leave the
/// whole plan below the aggregate idle and make this row true for a reason that has nothing to do
/// with the group's size — which is a different property, gated in
/// `incremental_engine.rs::a_bid_between_the_ends_does_not_re_render_the_page`.
///
/// Two instruments, as above: [`beck_core::engine::Work::steps`] against [`Relate::Refuse`], which
/// is the off switch [`docs/08`](../../../../docs/08-roadmap.md) §8.3 item 8 requires of a choice
/// the compiler makes unbidden; and `materialised`, held to the exact number, because a group
/// copied out of an arrangement in order to walk it is what the operator exists not to do.
#[test]
fn asking_a_group_for_one_end_does_not_build_it() {
    use beck_core::core::Fields;
    use beck_core::engine::{Engine, Prepared, Work};
    use beck_core::plan::{Agg, Op, Plan, Relate};
    use beck_core::Value;
    use beck_rt::{Envelope, Instant, Runtime};

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/36-auction.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let (placed, diags, map) = beck_core::compile_str("36-auction.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the corpus program compiles");

    let ends = |relate: Relate| -> Vec<Agg> {
        Plan::compile_with(&placed, relate)
            .nodes
            .iter()
            .filter_map(|n| match n.op {
                Op::GroupBy { agg, .. } => Some(agg),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        ends(Relate::Recognise),
        vec![Agg::Min, Agg::Max],
        "corpus/36-auction.beck no longer compiles to a `group_by` at each end, so this gate \
         measures something else"
    );
    assert!(
        ends(Relate::Refuse).is_empty(),
        "`Relate::Refuse` left the aggregate in the plan, so the off switch is not one"
    );

    let event = |variant: &str, fields: &[(&str, Value)]| {
        let mut f = Fields::new();
        for (k, v) in fields {
            f.insert(Arc::from(*k), v.clone());
        }
        Value::data(Arc::from("Event"), Some(Arc::from(variant)), f)
    };
    let offered = |id: &str, amount: i64| {
        event(
            "Offered",
            &[
                ("id", Value::str_(id)),
                ("lot", Value::str_("l1")),
                ("amount", Value::Int(amount)),
            ],
        )
    };

    // What one more bid costs once `n` are already on the same lot. The pile the event lands on is
    // the whole collection, which is the worst case for an aggregate that walks its group and no
    // case at all for one that keeps it.
    let cost_at = |relate: Relate, n: i64| -> Work {
        let backend = beck_eval::backend(&placed);
        let plan = Arc::new(Plan::compile_with(&placed, relate));
        let prepared = Arc::new(Prepared::new(plan, backend.as_ref()).expect("the plan prepares"));
        let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut engine = Engine::new(prepared);

        let mut seq = 0u64;
        let mut state = runtime.initial_state().expect("an initial accumulator");
        let fold = |state: &Value, body: Value, seq: u64| {
            let env = Envelope {
                seq,
                at: Instant(seq as i64),
                actor: "ana".to_string(),
                body: body.clone(),
            };
            runtime.fold(state, &env, body).expect("folds")
        };
        seq += 1;
        state = fold(
            &state,
            event(
                "Opened",
                &[("id", Value::str_("l1")), ("title", Value::str_("a lamp"))],
            ),
            seq,
        );
        for i in 0..n {
            seq += 1;
            state = fold(&state, offered(&format!("b{i:06}"), 1_000 + i), seq);
        }
        // The cold render builds every arrangement — `O(n)`, once, and not what is measured.
        engine.render(&state, &session, &here).expect("renders");
        // A new low: the answer moves, the page is reassembled, and none of that is the group's
        // size.
        seq += 1;
        state = fold(&state, offered("b999999", 5), seq);
        engine.render(&state, &session, &here).expect("renders");

        let work = engine.work();
        println!(
            "{relate:?} at {n:>5} bids: {:>6} steps, {:>5} materialised ({} applications, {} \
             touched, {} recomputed)",
            work.steps, work.materialised, work.applications, work.touched, work.recomputed
        );
        work
    };

    let (small, large) = (
        cost_at(Relate::Recognise, 200),
        cost_at(Relate::Recognise, 1_600),
    );
    let (refused_small, refused_large) =
        (cost_at(Relate::Refuse, 200), cost_at(Relate::Refuse, 1_600));

    assert!(
        large.steps <= small.steps,
        "eight times the bids cost {} backend steps against {}. The smallest of a group is one \
         end of a tree the operator keeps, and neither end is the collection — docs/99 §99.9 item 6",
        large.steps,
        small.steps
    );
    assert!(
        refused_large.steps > refused_small.steps * 3,
        "with the aggregate refused, eight times the bids cost {} steps against {} — which is not \
         the pile-walking this gate exists to say the operator removes, so the flat row above says \
         nothing",
        refused_large.steps,
        refused_small.steps
    );
    assert_eq!(
        (small.materialised, large.materialised),
        (1, 1),
        "the page asks for no group's rows and has one lot of its own, so one entry is copied out \
         of an arrangement whatever the pile holds — docs/99 §99.9 item 6"
    );
}

/// **A group's total is maintained, and the group is never built.**
///
/// [`docs/99-the-data-tier-means-of-combination.md`](../../../../docs/99-the-data-tier-means-of-combination.md)
/// §99.9 item 6's last aggregate, and the sibling of `asking_a_group_for_one_end_does_not_build_it`
/// above. `corpus/37-ledger.beck` shows every account's balance;
/// [`beck_core::plan::Op::GroupBy`] keeps a running total per group and moves it by `±n`, so a
/// posting costs two applications whatever the account it lands on already holds.
///
/// **There is no worst case to choose here, and that is the difference worth measuring.** The
/// extremes above are gated on a *new low* because an event that changes neither end leaves the
/// whole plan below the aggregate idle, and a flat row measured on one of those would be flat for a
/// reason that has nothing to do with the operator. Every posting moves its account's total, so
/// every event is the reassembling case and an ordinary one is the honest measurement.
///
/// Two instruments, as above: [`beck_core::engine::Work::steps`] against [`Relate::Refuse`], the
/// off switch [`docs/08`](../../../../docs/08-roadmap.md) §8.3 item 8 requires of a choice the
/// compiler makes unbidden; and `materialised`, held to the exact number, because a group copied
/// out of an arrangement in order to add it up is what the operator exists not to do.
#[test]
fn totalling_a_group_does_not_build_it() {
    use beck_core::core::Fields;
    use beck_core::engine::{Engine, Prepared, Work};
    use beck_core::plan::{Agg, Op, Plan, Relate};
    use beck_core::Value;
    use beck_rt::{Envelope, Instant, Runtime};

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/37-ledger.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let (placed, diags, map) = beck_core::compile_str("37-ledger.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the corpus program compiles");

    let totals = |relate: Relate| -> Vec<Agg> {
        Plan::compile_with(&placed, relate)
            .nodes
            .iter()
            .filter_map(|n| match n.op {
                Op::GroupBy { agg, .. } => Some(agg),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        totals(Relate::Recognise),
        vec![Agg::Sum],
        "corpus/37-ledger.beck no longer compiles to a `group_by` holding a total, so this gate \
         measures something else"
    );
    assert!(
        totals(Relate::Refuse).is_empty(),
        "`Relate::Refuse` left the aggregate in the plan, so the off switch is not one"
    );

    let event = |variant: &str, fields: &[(&str, Value)]| {
        let mut f = Fields::new();
        for (k, v) in fields {
            f.insert(Arc::from(*k), v.clone());
        }
        Value::data(Arc::from("Event"), Some(Arc::from(variant)), f)
    };
    let posted = |id: &str, amount: i64| {
        event(
            "Posted",
            &[
                ("id", Value::str_(id)),
                ("account", Value::str_("a1")),
                ("amount", Value::Int(amount)),
            ],
        )
    };

    // What one more posting costs once `n` are already on the same account.
    let cost_at = |relate: Relate, n: i64| -> Work {
        let backend = beck_eval::backend(&placed);
        let plan = Arc::new(Plan::compile_with(&placed, relate));
        let prepared = Arc::new(Prepared::new(plan, backend.as_ref()).expect("the plan prepares"));
        let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut engine = Engine::new(prepared);

        let mut seq = 0u64;
        let mut state = runtime.initial_state().expect("an initial accumulator");
        let fold = |state: &Value, body: Value, seq: u64| {
            let env = Envelope {
                seq,
                at: Instant(seq as i64),
                actor: "ana".to_string(),
                body: body.clone(),
            };
            runtime.fold(state, &env, body).expect("folds")
        };
        seq += 1;
        state = fold(
            &state,
            event(
                "Opened",
                &[("id", Value::str_("a1")), ("name", Value::str_("cash"))],
            ),
            seq,
        );
        for i in 0..n {
            seq += 1;
            state = fold(&state, posted(&format!("p{i:06}"), 100 + i), seq);
        }
        // The cold render builds every arrangement — `O(n)`, once, and not what is measured.
        engine.render(&state, &session, &here).expect("renders");
        seq += 1;
        state = fold(&state, posted("p999999", 250), seq);
        engine.render(&state, &session, &here).expect("renders");

        let work = engine.work();
        println!(
            "{relate:?} at {n:>5} postings: {:>6} steps, {:>5} materialised ({} applications, {} \
             touched, {} recomputed)",
            work.steps, work.materialised, work.applications, work.touched, work.recomputed
        );
        work
    };

    let (small, large) = (
        cost_at(Relate::Recognise, 200),
        cost_at(Relate::Recognise, 1_600),
    );
    let (refused_small, refused_large) =
        (cost_at(Relate::Refuse, 200), cost_at(Relate::Refuse, 1_600));

    assert!(
        large.steps <= small.steps,
        "eight times the postings cost {} backend steps against {}. A total is a number the \
         operator keeps, and adding to it is not reading the group — docs/99 §99.9 item 6",
        large.steps,
        small.steps
    );
    assert!(
        refused_large.steps > refused_small.steps * 3,
        "with the aggregate refused, eight times the postings cost {} steps against {} — which is \
         not the pile-adding this gate exists to say the operator removes, so the flat row above \
         says nothing",
        refused_large.steps,
        refused_small.steps
    );
    assert_eq!(
        (small.materialised, large.materialised),
        (1, 1),
        "the page asks for no account's postings and has one account of its own, so one entry is \
         copied out of an arrangement whatever the ledger holds — docs/99 §99.9 item 6"
    );
}

/// **A difference is maintained from the side that moved, and neither side is the collection.**
///
/// [`docs/99-the-data-tier-means-of-combination.md`](../../../../docs/99-the-data-tier-means-of-combination.md)
/// §99.9 item 7. `corpus/38-backorders.beck` shows the orders for something in stock and the orders
/// for something not, which is one `filter_list` and its negation over a predicate that reads the
/// stock — so **a delivery is a different predicate**, and without the operator every order ever
/// placed is reconsidered by it.
///
/// **The event measured is a delivery and not an order, and choosing the other one would have
/// measured nothing.** An order arriving moves the left side, which the refused `filter_list`
/// already handles per delta — its capture did not move, so it does not rebuild. The whole cost
/// this operator removes is on the *right*: an item stocked, which is the half no test over a
/// single collection can see. Two orders wait on the sku that arrives at both sizes, so what moves
/// is fixed and what is measured is everything else.
///
/// Two instruments, as the aggregates' gates above: [`beck_core::engine::Work::steps`] against
/// [`Relate::Refuse`], the off switch [`docs/08`](../../../../docs/08-roadmap.md) §8.3 item 8
/// requires of a choice the compiler makes unbidden; and the plan itself, because a gate that only
/// counted steps would stay green if the operator quietly stopped being emitted.
#[test]
fn stocking_one_item_does_not_reconsider_every_order() {
    use beck_core::core::Fields;
    use beck_core::engine::{Engine, Prepared, Work};
    use beck_core::plan::{Op, Plan, Presence, Relate};
    use beck_core::Value;
    use beck_rt::{Envelope, Instant, Runtime};

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/38-backorders.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let (placed, diags, map) = beck_core::compile_str("38-backorders.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the corpus program compiles");

    let restricts = |relate: Relate| -> Vec<Presence> {
        Plan::compile_with(&placed, relate)
            .nodes
            .iter()
            .filter_map(|n| match n.op {
                Op::Restrict { keep, .. } => Some(keep),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        restricts(Relate::Recognise),
        vec![Presence::In, Presence::NotIn],
        "corpus/38-backorders.beck no longer compiles to a semi-join and an anti-join, so this \
         gate measures something else"
    );
    assert!(
        restricts(Relate::Refuse).is_empty(),
        "`Relate::Refuse` left the operator in the plan, so the off switch is not one"
    );

    let event = |variant: &str, fields: &[(&str, Value)]| {
        let mut f = Fields::new();
        for (k, v) in fields {
            f.insert(Arc::from(*k), v.clone());
        }
        Value::data(Arc::from("Event"), Some(Arc::from(variant)), f)
    };

    // What one delivery costs once `n` orders are already placed. Two orders share each sku, so
    // the delivery moves two of them whatever `n` is — and every other order is what the gate is
    // about.
    let cost_at = |relate: Relate, n: i64| -> Work {
        let backend = beck_eval::backend(&placed);
        let plan = Arc::new(Plan::compile_with(&placed, relate));
        let prepared = Arc::new(Prepared::new(plan, backend.as_ref()).expect("the plan prepares"));
        let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut engine = Engine::new(prepared);

        let mut seq = 0u64;
        let mut state = runtime.initial_state().expect("an initial accumulator");
        let fold = |state: &Value, body: Value, seq: u64| {
            let env = Envelope {
                seq,
                at: Instant(seq as i64),
                actor: "ana".to_string(),
                body: body.clone(),
            };
            runtime.fold(state, &env, body).expect("folds")
        };
        for i in 0..n {
            seq += 1;
            state = fold(
                &state,
                event(
                    "Placed",
                    &[
                        ("id", Value::str_(format!("o{i:06}"))),
                        ("customer", Value::str_(format!("c{i:06}"))),
                        ("sku", Value::str_(format!("s{:06}", i / 2))),
                        ("qty", Value::Int(1 + i % 5)),
                    ],
                ),
                seq,
            );
        }
        // The cold render builds every arrangement — `O(n)`, once, and not what is measured.
        engine.render(&state, &session, &here).expect("renders");
        seq += 1;
        state = fold(
            &state,
            event(
                "Stocked",
                &[
                    ("sku", Value::str_("s000000")),
                    ("name", Value::str_("figs")),
                ],
            ),
            seq,
        );
        engine.render(&state, &session, &here).expect("renders");

        let work = engine.work();
        println!(
            "{relate:?} at {n:>5} orders: {:>6} steps ({} applications, {} touched, {} \
             materialised, {} recomputed)",
            work.steps, work.applications, work.touched, work.materialised, work.recomputed
        );
        work
    };

    let (small, large) = (
        cost_at(Relate::Recognise, 200),
        cost_at(Relate::Recognise, 1_600),
    );
    let (refused_small, refused_large) =
        (cost_at(Relate::Refuse, 200), cost_at(Relate::Refuse, 1_600));

    assert!(
        large.steps <= small.steps,
        "eight times the orders cost {} backend steps against {}. A delivery reaches the orders \
         waiting on its sku through the operator's reverse index and no others — docs/99 §99.9 \
         item 7",
        large.steps,
        small.steps
    );
    assert!(
        large.applications <= small.applications,
        "eight times the orders cost {} applications against {}, so something is being applied to \
         the collection rather than to what moved",
        large.applications,
        small.applications
    );
    assert!(
        refused_large.steps > refused_small.steps * 3,
        "with the operator refused, eight times the orders cost {} steps against {} — which is not \
         the whole-collection reconsideration this gate exists to say the operator removes, so the \
         flat row above says nothing",
        refused_large.steps,
        refused_small.steps
    );
}

/// **The values in use are maintained, and the fold that spells the same thing is not.**
///
/// [`docs/99-the-data-tier-means-of-combination.md`](../../../../docs/99-the-data-tier-means-of-combination.md)
/// §99.9 item 7's second half. `corpus/39-topics.beck` shows the topics its notes are filed under,
/// each once, written `list_unique(map_list(…))` — one primitive, so
/// [`beck_core::plan::Op::Distinct`] maintains it.
///
/// **The control is the same program with the dedup written as a fold**, which is what
/// `lib/collections.beck`'s `unique` was before this operator and what anybody would write without
/// a primitive to reach for. It computes the same list; it is a recursion, so the plan cannot see
/// into it, and one note arriving rebuilds it from every note on the board. That is the whole
/// argument for the primitive — `lib/README.md`'s division admits one for a combining form the
/// view engine has to recognise — and it is measured here rather than asserted.
///
/// The event measured is the **worst case**: a note whose text sorts before every other, under a
/// topic that already exists, so the published occurrence of that topic *moves* and the chip row is
/// reassembled. A note that changed no answer would be flat for a reason that has nothing to do
/// with the operator.
#[test]
fn the_values_in_use_are_maintained_and_a_fold_over_them_is_not() {
    use beck_core::core::Fields;
    use beck_core::engine::{Engine, Prepared, Work};
    use beck_core::plan::{Op, Plan};
    use beck_core::Value;
    use beck_rt::{Envelope, Instant, Runtime};

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/39-topics.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");

    // The same program with the dedup spelled as the fold it used to be.
    let folded = src.replace(
        "    return list_unique(map_list(notes, lambda n: n.topic))",
        "    return seen_once(map_list(notes, lambda n: n.topic), {}, [])\n\
         \n\
         def seen_once(xs: list[Str], seen: Map[Str, Bool], acc: list[Str]) -> list[Str]:\n    \
             match xs:\n        \
                 case []:\n            \
                     return acc\n        \
                 case [first, *rest]:\n            \
                     if map_contains(seen, first):\n                \
                         return seen_once(rest, seen, acc)\n            \
                     return seen_once(rest, map_insert(seen, first, true), \
                                      list_append(acc, first))",
    );
    assert_ne!(
        folded, src,
        "corpus/39-topics.beck no longer spells its question the way this gate's control replaces"
    );

    let compile = |name: &str, text: &str| {
        let (placed, diags, map) = beck_core::compile_str(name, text);
        assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
        placed.expect("compiles")
    };
    let maintained = compile("39-topics.beck", &src);
    let recomputed = compile("39-folded.beck", &folded);

    assert!(
        Plan::compile(&maintained)
            .nodes
            .iter()
            .any(|n| matches!(n.op, Op::Distinct)),
        "corpus/39-topics.beck no longer compiles to a `distinct`, so this gate measures something \
         else"
    );
    assert!(
        !Plan::compile(&recomputed)
            .nodes
            .iter()
            .any(|n| matches!(n.op, Op::Distinct)),
        "the control compiled to the operator, so it is not a control"
    );

    let event = |variant: &str, fields: &[(&str, Value)]| {
        let mut f = Fields::new();
        for (k, v) in fields {
            f.insert(Arc::from(*k), v.clone());
        }
        Value::data(Arc::from("Event"), Some(Arc::from(variant)), f)
    };

    // What one note costs once `n` are already on the board, under four topics.
    let cost_at = |placed: &beck_core::Placed, n: i64| -> Work {
        let backend = beck_eval::backend(placed);
        let prepared =
            Arc::new(Prepared::compile(placed, backend.as_ref()).expect("the plan prepares"));
        let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut engine = Engine::new(prepared);

        let mut seq = 0u64;
        let mut state = runtime.initial_state().expect("an initial accumulator");
        let fold = |state: &Value, body: Value, seq: u64| {
            let env = Envelope {
                seq,
                at: Instant(seq as i64),
                actor: "ana".to_string(),
                body: body.clone(),
            };
            runtime.fold(state, &env, body).expect("folds")
        };
        for i in 0..n {
            seq += 1;
            state = fold(
                &state,
                event(
                    "Written",
                    &[
                        ("id", Value::str_(format!("n{i:06}"))),
                        ("text", Value::str_(format!("note-{i:06}"))),
                        ("topic", Value::str_(format!("t{}", i % 4))),
                    ],
                ),
                seq,
            );
        }
        // The cold render builds every arrangement — `O(n)`, once, and not what is measured.
        engine.render(&state, &session, &here).expect("renders");
        seq += 1;
        state = fold(
            &state,
            event(
                "Written",
                &[
                    ("id", Value::str_("n999999")),
                    ("text", Value::str_("aardvark")),
                    ("topic", Value::str_("t0")),
                ],
            ),
            seq,
        );
        engine.render(&state, &session, &here).expect("renders");

        let work = engine.work();
        println!(
            "at {n:>5} notes: {:>6} steps ({} applications, {} touched, {} materialised, {} \
             recomputed)",
            work.steps, work.applications, work.touched, work.materialised, work.recomputed
        );
        work
    };

    println!("maintained:");
    let (small, large) = (cost_at(&maintained, 200), cost_at(&maintained, 1_600));
    println!("as a fold:");
    let (folded_small, folded_large) = (cost_at(&recomputed, 200), cost_at(&recomputed, 1_600));

    assert!(
        large.steps <= small.steps,
        "eight times the notes cost {} backend steps against {}. A note joins the values in use or \
         it does not, and neither answer is read off the collection — docs/99 §99.9 item 7",
        large.steps,
        small.steps
    );
    assert!(
        folded_large.steps > folded_small.steps * 3,
        "with the dedup written as a fold, eight times the notes cost {} steps against {} — which \
         is not the whole-collection rebuild this gate exists to say the operator removes, so the \
         flat row above says nothing",
        folded_large.steps,
        folded_small.steps
    );
}

/// **Reconciling a reordered keyed list costs the same per row however many rows it has.**
///
/// The client applies a patch against the children it already holds, so every index the differ
/// emits has to be the one that child occupies *at that point in the stream* — and reading that
/// off the list is a scan. One scan per child is quadratic over the list: invisible at the three
/// and four rows a hand-written differ test uses, and severe at the size a real table reaches.
/// Reversing 4,000 keyed rows cost 25 ms of diffing before `diff::Unclaimed` replaced the scan
/// with a rank query, and each doubling of the rows roughly quadrupled the cost per row.
///
/// A reorder is the case that matters because it is the one the trim cannot help with: sorting a
/// table by another column shares no prefix or suffix between the two pages, so the whole list is
/// the window.
///
/// This gates the *shape* and not the rate (`docs/64`): the per-row cost at the larger size is
/// compared against the per-row cost at the smaller, so a slow machine moves both and only a
/// change in the order of growth fails it.
#[test]
fn reordering_a_keyed_list_costs_the_same_per_row_however_long_it_gets() {
    use beck_core::html::Html;
    use std::sync::Arc;

    fn list(keys: impl Iterator<Item = usize>) -> Html {
        let mut ul = Html::el("ul");
        for k in keys {
            ul = ul.child(
                Html::el("li")
                    .key(format!("k{k}"))
                    .child(Html::text(format!("row {k}"))),
            );
        }
        ul
    }

    let per_row_ns = |n: usize| -> f64 {
        let old = list(0..n);
        // The same children in the opposite order — shared handles, so this measures
        // reconciliation and not the rebuilding of the rows.
        let mut rev = Html::el("ul");
        if let Html::Element { children, .. } = &old {
            for c in children.iter().rev() {
                rev = rev.child_shared(Arc::clone(c));
            }
        }
        let mut best = f64::MAX;
        for _ in 0..3 {
            let started = Instant::now();
            let ops = beck_core::diff(&old, &rev);
            let elapsed = started.elapsed().as_secs_f64() * 1e9;
            assert!(
                ops.len() >= n - 1,
                "a full reversal of {n} rows should move nearly every row, not {} ops — the \
                 measurement is not reconciling anything",
                ops.len()
            );
            best = best.min(elapsed);
        }
        best / n as f64
    };

    let short = per_row_ns(500);
    let long = per_row_ns(4_000);
    println!("keyed reorder: {short:.0} ns/row at 500 and {long:.0} ns/row at 4,000");
    assert!(
        long < short * 3.0,
        "eight times the rows cost {:.1}× as much per row ({short:.0} ns → {long:.0} ns), which \
         is the shape of a scan per child rather than a rank query — see docs/23 §23.8",
        long / short
    );
}
