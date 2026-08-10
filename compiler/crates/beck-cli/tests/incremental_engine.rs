//! The incremental view engine against its oracle.
//!
//! [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.3: incremental views are
//! "an optimisation with an exact correctness oracle (recompute) to test against — a luxurious
//! position for CI." This is that position, occupied.
//!
//! For every program in the corpus, and for the sketch, an event log is generated, folded one event
//! at a time, and after **every** event the maintained view is compared with the recomputed one —
//! not by a digest, by the rendered page, byte for byte, for two different subscribers. A page is
//! diffed into a patch stream and replayed (§4.8), so "nearly the same" is not a category that
//! exists here: an engine one element out of order produces a wrong DOM on every connected client.
//!
//! Three properties are checked beyond equality, because equality alone would be satisfied by an
//! engine that recomputed everything and called it maintenance:
//!
//! * a warm engine and a cold one agree, so no arrangement carries state that changes an answer;
//! * an engine rendered against an *older* state agrees, because the runtime's resumption path does
//!   exactly that when a subscriber reconnects;
//! * `list_len` over a maintained collection does not visit the collection — §3.8's "±1 per event,
//!   never by recount", asserted as a count rather than as a duration.

use std::sync::Arc;

use beck_core::engine::{Engine, Prepared, SharedDataflow};
use beck_core::gen::{arbitrary, Rng};
use beck_core::plan::{Op, Plan};
use beck_core::{Placed, Ty, Value};
use beck_rt::{Envelope, Instant, Runtime};

mod support;

const ACTORS: &[&str] = &["ana", "bo"];

fn corpus_files() -> Vec<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the corpus directory is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    out.sort();
    out
}

fn compile(name: &str, src: &str) -> Placed {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    placed.unwrap_or_else(|| panic!("{name} did not slice"))
}

/// A program, its recompute oracle, and one engine per subscriber — twice over.
///
/// `engines` are standalone: each computes the whole plan. `shared` holds §5.3's one shared
/// dataflow and `subscribers` are the per-session halves attached to it. Both are compared with
/// recompute at every event, because sharing an arrangement between subscribers is exactly the sort
/// of optimisation that is right for one subscriber and wrong for two.
struct Subject {
    name: String,
    runtime: Runtime,
    prepared: Arc<Prepared>,
    engines: Vec<Engine>,
    shared: Arc<SharedDataflow>,
    subscribers: Vec<Engine>,
    state: Value,
    seq: u64,
}

impl Subject {
    fn new(name: &str, placed: Placed) -> Subject {
        let backend = beck_eval::backend(&placed);
        let prepared =
            Arc::new(Prepared::compile(&placed, backend.as_ref()).expect("the plan prepares"));
        let engines = ACTORS
            .iter()
            .map(|_| Engine::new(prepared.clone()))
            .collect();
        let shared = Arc::new(SharedDataflow::new(prepared.clone()));
        let subscribers = ACTORS.iter().map(|_| shared.subscriber()).collect();
        let runtime = Runtime::new(placed, backend).expect("the program prepares");
        let state = runtime.initial_state().expect("an initial accumulator");
        Subject {
            name: name.to_string(),
            runtime,
            prepared,
            engines,
            shared,
            subscribers,
            state,
            seq: 0,
        }
    }

    fn fold(&mut self, event: Value) {
        self.seq += 1;
        let env = Envelope {
            seq: self.seq,
            at: Instant(self.seq as i64),
            actor: ACTORS[self.seq as usize % ACTORS.len()].to_string(),
            body: event.clone(),
        };
        self.state = self
            .runtime
            .fold(&self.state, &env, event)
            .unwrap_or_else(|e| panic!("{}: folding at seq {}: {e}", self.name, self.seq));
    }

    /// The assertion the whole engine exists to survive. Returns how many pages it compared.
    fn agrees(&mut self, at: &str) -> usize {
        for (i, actor) in ACTORS.iter().enumerate() {
            let expected = self
                .runtime
                .view(&self.state, actor)
                .unwrap_or_else(|e| panic!("{}: recompute: {e}", self.name))
                .render();
            let state = self.state.clone();
            let session = self.runtime.session(actor);
            let here = beck_core::edge::presence_of(actor);
            let got = self.engines[i]
                .render(&state, &session, &here)
                .unwrap_or_else(|e| panic!("{}: engine: {e}", self.name));
            assert_eq!(
                page(&self.name, got),
                expected,
                "{} at {at}, subscriber `{actor}`: the maintained view is not the recomputed one",
                self.name
            );
            let (got, version) = self
                .shared
                .render(&mut self.subscribers[i], &state, self.seq, &session, &here)
                .unwrap_or_else(|e| panic!("{}: shared engine: {e}", self.name));
            assert_eq!(
                page(&self.name, got),
                expected,
                "{} at {at}, subscriber `{actor}`: the view maintained over a *shared* dataflow is \
                 not the recomputed one",
                self.name
            );
            assert_eq!(
                version, self.seq,
                "{}: the shared dataflow rendered a version nobody asked for",
                self.name
            );
        }
        ACTORS.len() * 2
    }
}

fn page(name: &str, v: Value) -> String {
    match v {
        Value::Html(h) => h.render(),
        other => panic!("{name}: the engine produced {}", other.display()),
    }
}

/// A deterministic log for a program, from its own `Event` union.
///
/// Generated rather than hand-written because the point is to run *this* engine against programs
/// nobody wrote it for. Seeded by the file's name, so a failure is a reproducible failure.
fn log_for(placed: &Placed, name: &str, n: usize) -> Vec<Value> {
    let mut rng = Rng::seeded(name, 1);
    let ty = Ty::con(
        placed
            .roles
            .event_ty
            .con_name()
            .expect("an event type with a name"),
    );
    (0..n)
        .filter_map(|_| arbitrary(&ty, &placed.program.types, &mut rng).ok())
        .collect()
}

fn subjects() -> Vec<(Subject, Vec<Value>)> {
    let mut out = Vec::new();
    let sketch = support::todo_program();
    let log = log_for(&sketch, "examples/todo.beck", 40);
    out.push((Subject::new("examples/todo.beck", sketch), log));
    for path in corpus_files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("a readable corpus program");
        let placed = compile(&name, &src);
        let log = log_for(&placed, &name, 30);
        out.push((Subject::new(&name, placed), log));
    }
    out
}

#[test]
fn the_maintained_view_is_the_recomputed_view_for_every_corpus_program() {
    let all = subjects();
    assert!(
        all.len() >= 24,
        "only {} programs were exercised; the corpus is the measurement",
        all.len()
    );
    let (mut folded, mut compared) = (0usize, 0usize);
    for (mut subject, log) in all {
        // The empty state is the first thing a fresh subscriber sees, so it is the first thing
        // compared.
        compared += subject.agrees("the initial state");
        assert!(
            !log.is_empty(),
            "{}: no events could be generated, so nothing was compared",
            subject.name
        );
        for (i, event) in log.into_iter().enumerate() {
            subject.fold(event);
            compared += subject.agrees(&format!("event {}", i + 1));
            folded += 1;
        }
    }
    // Printed so that docs/24 §24.4's count is reproducible rather than remembered.
    println!("{folded} events folded, {compared} pages compared");
    assert!(folded > 500, "only {folded} events were compared");
}

#[test]
fn a_cold_engine_and_a_warm_one_produce_the_same_page() {
    // The failure this rules out: an arrangement that is right only because of the order the events
    // arrived in. A cold engine has no history at all, so if the two agree at a state reached by
    // forty events, no history is being relied on.
    for (mut subject, log) in subjects() {
        for event in log {
            subject.fold(event);
        }
        for actor in ACTORS {
            let session = subject.runtime.session(actor);
            let here = beck_core::edge::presence_of(actor);
            let mut cold = Engine::new(subject.prepared.clone());
            let fresh = cold
                .render(&subject.state, &session, &here)
                .expect("a cold render");
            let warm = subject.engines[0]
                .render(&subject.state, &subject.runtime.session(actor), &here)
                .expect("a warm render");
            assert_eq!(
                fresh, warm,
                "{}: a cold engine disagrees with a warm one for `{actor}`",
                subject.name
            );
        }
    }
}

#[test]
fn an_engine_rendered_against_an_older_state_is_still_right() {
    // `beck-rt`'s resumption path renders the state as of the `seq` a reconnecting subscriber last
    // saw, which is *older* than the one the engine has been tracking. An engine that assumed the
    // state only ever moves forward would serve that subscriber a page from the future.
    let mut subject = Subject::new("examples/todo.beck", support::todo_program());
    let log = log_for(subject.runtime.placed(), "examples/todo.beck", 30);
    let mut history = vec![subject.state.clone()];
    for event in log {
        subject.fold(event);
        history.push(subject.state.clone());
    }
    let session = subject.runtime.session(ACTORS[0]);
    let here = beck_core::edge::presence_of(ACTORS[0]);
    // Backwards, then forwards again, on the engine that has already seen the whole log.
    for state in history.iter().rev().chain(history.iter()) {
        let expected = subject.runtime.view(state, ACTORS[0]).expect("recompute");
        let got = subject.engines[0]
            .render(state, &session, &here)
            .expect("a render against an arbitrary state");
        let Value::Html(got) = got else {
            panic!("the engine produced a non-Html value")
        };
        assert_eq!(got.render(), expected.render());
    }
}

#[test]
fn the_sketch_decomposes_into_the_operators_section_3_8_names() {
    // A guard on the *plan*, not on the engine: every assertion above would still pass if the
    // decomposition silently gave up and made the whole view one opaque recompute, because that is
    // the fallback and the fallback is correct. This is the test that says it did not.
    let placed = support::todo_program();
    // What the decomposition produces: `for t in mine:` is a `map_list` inside a `flatten`, one
    // operator per construct the source names.
    let built = Plan::unfused(&placed);
    let names = |p: &Plan| -> Vec<&'static str> { p.nodes.iter().map(|n| n.op.name()).collect() };
    for wanted in [
        "map_values",
        "filter_list",
        "sort_by",
        "map_list",
        "flatten",
        "list_len",
    ] {
        assert!(
            names(&built).contains(&wanted),
            "the sketch's view has no `{wanted}` operator; the plan is {:?}",
            names(&built)
        );
    }
    // What the engine runs is the *fused* plan, where that pair is one operator and the
    // arrangement between them is never built (docs/89). `fusion.rs` is the gate on the rewrite;
    // this is the gate on the sketch still decomposing.
    let plan = Plan::compile(&placed);
    let ops = names(&plan);
    for wanted in [
        "map_values",
        "filter_list",
        "sort_by",
        "flat_map",
        "list_len",
    ] {
        assert!(
            ops.contains(&wanted),
            "the sketch's fused view has no `{wanted}` operator; the plan is {ops:?}"
        );
    }
    let (maintained, recomputed) = plan.counts();
    assert!(
        maintained >= 6,
        "only {maintained} of {} operators are maintained",
        maintained + recomputed
    );
    // The accumulator is read by the plan's one source and nothing else reads the session before
    // the filter, so the arrangement above `filter_list` is shareable between subscribers.
    let shared = plan.shared();
    assert!(
        shared.len() >= 3,
        "nothing above the per-session filter is shared: {shared:?}"
    );
}

#[test]
fn a_count_over_a_maintained_collection_does_not_visit_the_collection() {
    // §3.8, verbatim: "`remaining` updates by ±1 per event, never by recount." The sketch's
    // `remaining` is `list_len(filter_list(mine(s, session), …))`, so this is that sentence about
    // that program.
    //
    // Counted, not timed. The number that must not grow with `n` is the work the *maintained*
    // operators do; the page's `html_el` still assembles `n` children, and that is stated in
    // docs/24 rather than hidden here.
    let placed = support::todo_program();
    let backend = beck_eval::backend(&placed);
    let prepared = Arc::new(Prepared::compile(&placed, backend.as_ref()).expect("prepares"));
    let runtime = Runtime::new(placed, backend).expect("prepares");

    let count_of = |n: usize| -> u64 {
        let mut engine = Engine::new(prepared.clone());
        let session = runtime.session("ana");
        let here = beck_core::edge::presence_of("ana");
        let mut state = runtime.initial_state().expect("initial");
        for i in 0..n {
            state = fold_added(&runtime, &state, i as u64 + 1, "ana");
        }
        // Warm the engine at `n` rows, then add one more and measure only that step.
        engine.render(&state, &session, &here).expect("warm");
        let next = fold_added(&runtime, &state, n as u64 + 1, "ana");
        engine.render(&next, &session, &here).expect("step");
        engine.work().applications + engine.work().touched
    };

    let small = count_of(20);
    let large = count_of(400);
    assert!(
        large <= small * 2,
        "one event over 400 rows did {large} units of delta work against {small} over 20; \
         a recount would have grown with the collection"
    );
    assert!(
        large < 40,
        "one event over 400 rows did {large} units of delta work; it should be a handful"
    );
}

/// Fold one `Added` event into the sketch's accumulator.
fn fold_added(runtime: &Runtime, state: &Value, n: u64, actor: &str) -> Value {
    let id = Value::data(
        Arc::from("Id"),
        None,
        beck_core::core::Fields::from_iter([(Arc::from("value"), Value::str_(format!("{n:06}")))]),
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
        at: Instant(n as i64),
        actor: actor.to_string(),
        body: event.clone(),
    };
    runtime.fold(state, &env, event).expect("fold")
}

#[test]
fn every_operator_the_plan_falls_back_on_says_which_construct_forced_it() {
    // A fallback with no reason is a fallback nobody will ever remove.
    for path in corpus_files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("readable");
        let plan = Plan::compile(&compile(&name, &src));
        for node in &plan.nodes {
            if let Op::Pointwise { .. } = &node.op {
                continue;
            }
            assert!(
                node.because.is_none(),
                "{name}: a maintained operator carries a fallback reason"
            );
        }
    }
}
