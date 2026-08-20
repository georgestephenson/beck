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
            // The rosters `Runtime::view` renders the oracle against: one connection, this actor's
            // own. An engine given an empty awareness roster would be held to a different page.
            let aware = self
                .runtime
                .contribution(actor)
                .unwrap_or_else(|e| panic!("{}: awareness: {e}", self.name));
            let got = self.engines[i]
                .render_all(&state, &session, &here, &aware)
                .unwrap_or_else(|e| panic!("{}: engine: {e}", self.name));
            assert_eq!(
                page(&self.name, got),
                expected,
                "{} at {at}, subscriber `{actor}`: the maintained view is not the recomputed one",
                self.name
            );
            let (got, version) = self
                .shared
                .render_all(
                    &mut self.subscribers[i],
                    &state,
                    self.seq,
                    &session,
                    &here,
                    &aware,
                )
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
    // The board is not in the corpus and is the only program that compiles to `arrange_by` and to
    // a join answering with a group (docs/99 §99.9 item 3), so the maintained-against-recomputed
    // oracle would not otherwise reach either.
    let board = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/board.beck");
    let src = std::fs::read_to_string(&board).expect("the board example is readable");
    let placed = compile("board.beck", &src);
    let log = log_for(&placed, "examples/board.beck", 40);
    out.push((Subject::new("examples/board.beck", placed), log));
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
    // Printed so that docs/23 §23.7's count is reproducible rather than remembered.
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
            let aware = subject.runtime.contribution(actor).expect("awareness");
            let mut cold = Engine::new(subject.prepared.clone());
            let fresh = cold
                .render_all(&subject.state, &session, &here, &aware)
                .expect("a cold render");
            let warm = subject.engines[0]
                .render_all(
                    &subject.state,
                    &subject.runtime.session(actor),
                    &here,
                    &aware,
                )
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
    // arrangement between them is never built (docs/23). `fusion.rs` is the gate on the rewrite;
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
    // docs/23 rather than hidden here.
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

/// A group's **count** is maintained, and the events that take one *down* are covered on purpose
/// rather than by the generator's luck.
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6:
/// `corpus/35-workload.beck` asks each person how many issues name them, and the join answers from a
/// count it keeps rather than by building the group. A tally maintained by `+1` on an entry that
/// arrived and `-1` on one that left is checked by
/// `the_maintained_view_is_the_recomputed_view_for_every_corpus_program` too — deleting the
/// decrement turns that harness red as well, which was measured rather than assumed. But it turns
/// red **by luck**: that harness folds an arbitrary log, and whether a `Closed(id)` names an issue
/// some earlier `Filed(id)` created is a property of the generator's seed rather than of the test.
/// A seed that never collided would leave a count that only goes up passing.
///
/// So this log is written rather than generated, and every event in it lands: two people, four
/// issues between them, two closed, one refiled, one reassigned by being filed again under the same
/// id — which moves a tally down and another up in one event. The oracle is the same one every
/// other test here uses, the recomputed page byte for byte, and the closing assertion is on the
/// **end state**: both piles empty, which is what a tally that never decremented cannot produce.
#[test]
fn a_maintained_count_per_group_survives_the_events_that_take_it_down() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/35-workload.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let mut subject = Subject::new("corpus/35-workload.beck", compile("35-workload.beck", &src));

    let event = |variant: &str, fields: Vec<(&str, &str)>| {
        Value::data(
            Arc::from("Event"),
            Some(Arc::from(variant)),
            beck_core::core::Fields::from_iter(
                fields
                    .into_iter()
                    .map(|(k, v)| (Arc::from(k), Value::str_(v))),
            ),
        )
    };
    let log = vec![
        event("Hired", vec![("id", "p1"), ("name", "Ada")]),
        event("Hired", vec![("id", "p2"), ("name", "Bo")]),
        event(
            "Filed",
            vec![("id", "i1"), ("title", "one"), ("assignee", "p1")],
        ),
        event(
            "Filed",
            vec![("id", "i2"), ("title", "two"), ("assignee", "p1")],
        ),
        event(
            "Filed",
            vec![("id", "i3"), ("title", "three"), ("assignee", "p2")],
        ),
        event(
            "Filed",
            vec![("id", "i4"), ("title", "four"), ("assignee", "p1")],
        ),
        // Down: two of Ada's three go, which is the half a generated log never reaches.
        event("Closed", vec![("id", "i1")]),
        event("Closed", vec![("id", "i2")]),
        // Back up, and then across: filing `i4` again under `p2` moves one entry from one group to
        // the other, so one tally goes down and another up in the same event.
        event(
            "Filed",
            vec![("id", "i5"), ("title", "five"), ("assignee", "p1")],
        ),
        event(
            "Filed",
            vec![("id", "i4"), ("title", "four"), ("assignee", "p2")],
        ),
        // And empty: everything Bo has, then everything Ada has.
        event("Closed", vec![("id", "i3")]),
        event("Closed", vec![("id", "i4")]),
        event("Closed", vec![("id", "i5")]),
    ];

    let mut compared = subject.agrees("the empty log");
    for (i, e) in log.into_iter().enumerate() {
        subject.fold(e);
        compared += subject.agrees(&format!("event {}", i + 1));
    }
    assert!(compared >= 28, "only {compared} pages were compared");

    // Both piles empty, so a green run above is not a run over a log that never went down.
    let page = subject
        .runtime
        .view(&subject.state, ACTORS[0])
        .expect("recompute")
        .render();
    assert!(
        page.contains("Ada — 0 open") && page.contains("Bo — 0 open"),
        "the log this test is written around no longer empties both piles:\n{page}"
    );
}

/// A group's **ends** are maintained, and the four events a generated log reaches only by luck are
/// written out.
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's other two
/// aggregates. `corpus/36-auction.beck` shows the lowest and the highest bid on each lot, and
/// [`beck_core::plan::Op::GroupBy`] answers both from one multiset per group. The oracle is the same
/// one every other test here uses — the recomputed page, byte for byte — and what this adds is a log
/// in which each of the four ways an extreme can be wrong actually happens:
///
///   * the **standing minimum is withdrawn**, so the answer has to be promoted from the tree rather
///     than left where it was;
///   * the **standing maximum is withdrawn**, which is the same failure read from the other end and
///     is the half an implementation that only maintains `min` gets wrong;
///   * a **tie is broken by half**: two bids of the same amount, one withdrawn. A tree that held
///     values rather than a multiset of them drops the answer here and shows the wrong end;
///   * the **last bid on a lot goes**, so the group empties and the page must say so again —
///     an empty group is a missing entry rather than an entry holding nothing.
///
/// The closing assertion is on the end state, because a green run over a log that never went down
/// says nothing: one lot back to no bids at all, and one holding a single bid that arrived after
/// both of its ends had been taken away.
#[test]
fn a_maintained_extreme_per_group_survives_the_events_that_take_it_down() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/36-auction.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let mut subject = Subject::new("corpus/36-auction.beck", compile("36-auction.beck", &src));

    let event = |variant: &str, fields: Vec<(&str, Value)>| {
        Value::data(
            Arc::from("Event"),
            Some(Arc::from(variant)),
            beck_core::core::Fields::from_iter(fields.into_iter().map(|(k, v)| (Arc::from(k), v))),
        )
    };
    let opened = |id: &str, title: &str| {
        event(
            "Opened",
            vec![("id", Value::str_(id)), ("title", Value::str_(title))],
        )
    };
    let offered = |id: &str, lot: &str, amount: i64| {
        event(
            "Offered",
            vec![
                ("id", Value::str_(id)),
                ("lot", Value::str_(lot)),
                ("amount", Value::Int(amount)),
            ],
        )
    };
    let withdrawn = |id: &str| event("Withdrawn", vec![("id", Value::str_(id))]);

    let log = vec![
        opened("l1", "a lamp"),
        opened("l2", "a rug"),
        offered("b1", "l1", 40),
        offered("b2", "l1", 12),
        offered("b3", "l1", 95),
        // Between the two ends: the group moves and neither answer does.
        offered("b4", "l1", 63),
        offered("b5", "l2", 7),
        // Down at the bottom, then at the top.
        withdrawn("b2"),
        withdrawn("b3"),
        // A tie, then half of it — the multiplicity is what keeps 40 the answer.
        offered("b6", "l1", 40),
        withdrawn("b6"),
        // And empty: everything on the lamp, then the rug's one bid, then one bid back.
        withdrawn("b1"),
        withdrawn("b4"),
        withdrawn("b5"),
        offered("b7", "l1", 21),
    ];

    let mut compared = subject.agrees("the empty log");
    for (i, e) in log.into_iter().enumerate() {
        subject.fold(e);
        compared += subject.agrees(&format!("event {}", i + 1));
    }
    assert!(compared >= 30, "only {compared} pages were compared");

    let page = subject
        .runtime
        .view(&subject.state, ACTORS[0])
        .expect("recompute")
        .render();
    assert!(
        page.contains("a lamp: 21 / 21") && page.contains("a rug: no bids / no bids"),
        "the log this test is written around no longer empties a group and refills it:\n{page}"
    );
}

/// **An event that moves a group without moving its ends re-renders nothing.**
///
/// This is the property that separates an aggregate from a `filter_list` somebody measures, and it
/// is about the operator's *output* rather than its cost:
/// [`beck_core::plan::Op::GroupBy`] publishes a change only when the answer moved, so a bid between
/// the standing low and the standing high stops at that operator and nothing below it runs — not the
/// join, not the loop, not the page.
///
/// Measured as a difference between two events on the same program rather than against a constant,
/// because two of the plan's recomputes read the accumulator and re-evaluate whatever the event was:
/// what this asserts is that the *page* is among them for one event and not for the other.
#[test]
fn a_bid_between_the_ends_does_not_re_render_the_page() {
    use beck_core::plan::Plan;

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/36-auction.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let placed = compile("36-auction.beck", &src);
    let backend = beck_eval::backend(&placed);
    let plan = Arc::new(Plan::compile(&placed));
    let prepared = Arc::new(Prepared::new(plan, backend.as_ref()).expect("the plan prepares"));
    let runtime = Runtime::new(placed, backend).expect("the program prepares");

    let event = |variant: &str, fields: Vec<(&str, Value)>| {
        Value::data(
            Arc::from("Event"),
            Some(Arc::from(variant)),
            beck_core::core::Fields::from_iter(fields.into_iter().map(|(k, v)| (Arc::from(k), v))),
        )
    };

    // One lot with two bids, so there is a low and a high to sit between.
    let cost_of_next = |amount: i64| -> beck_core::engine::Work {
        let mut engine = Engine::new(prepared.clone());
        let session = runtime.session(ACTORS[0]);
        let here = beck_core::edge::presence_of(ACTORS[0]);
        let mut state = runtime.initial_state().expect("an initial accumulator");
        let mut seq = 0u64;
        let fold = |state: &Value, body: Value, seq: u64| {
            let env = Envelope {
                seq,
                at: Instant(seq as i64),
                actor: ACTORS[0].to_string(),
                body: body.clone(),
            };
            runtime.fold(state, &env, body).expect("folds")
        };
        for body in [
            event(
                "Opened",
                vec![("id", Value::str_("l1")), ("title", Value::str_("a lamp"))],
            ),
            event(
                "Offered",
                vec![
                    ("id", Value::str_("b1")),
                    ("lot", Value::str_("l1")),
                    ("amount", Value::Int(20)),
                ],
            ),
            event(
                "Offered",
                vec![
                    ("id", Value::str_("b2")),
                    ("lot", Value::str_("l1")),
                    ("amount", Value::Int(80)),
                ],
            ),
        ] {
            seq += 1;
            state = fold(&state, body, seq);
        }
        engine.render(&state, &session, &here).expect("renders");
        seq += 1;
        state = fold(
            &state,
            event(
                "Offered",
                vec![
                    ("id", Value::str_("b3")),
                    ("lot", Value::str_("l1")),
                    ("amount", Value::Int(amount)),
                ],
            ),
            seq,
        );
        engine.render(&state, &session, &here).expect("renders");
        engine.work()
    };

    let between = cost_of_next(50);
    let below = cost_of_next(5);
    println!(
        "a bid between the ends: {} recomputed, {} touched; one below the low: {} recomputed, {} \
         touched",
        between.recomputed, between.touched, below.recomputed, below.touched
    );
    assert!(
        between.recomputed < below.recomputed,
        "a bid that moved neither end cost {} recomputes and one that moved the low cost {} — so \
         the aggregate published a change for an answer that did not move, and every subscriber \
         reassembled a page identical to the one it had",
        between.recomputed,
        below.recomputed
    );
    assert_eq!(
        between.touched, 1,
        "a bid between the ends touched {} arrangement entries; one is the `map_values` diff of \
         the accumulator's own map, and anything more is an operator below the aggregate reacting \
         to a change that should not have been published",
        between.touched
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

/// A group's **total** is maintained, and the events a generated log reaches only by luck are
/// written out.
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's last
/// aggregate. `corpus/37-ledger.beck` shows every account's balance and
/// [`beck_core::plan::Op::GroupBy`] keeps one running total per group, so what can go wrong is not
/// what goes wrong for an extreme: a total is never *promoted* from anywhere, it is added to and
/// taken back from, and the failures are all failures of arithmetic bookkeeping.
///
///   * a posting is **voided**, so its amount has to come back out of a number nothing else
///     remembers it went into;
///   * a **credit** is posted and then voided, so the subtraction has to be a subtraction of a
///     negative rather than of a magnitude;
///   * two postings of the **same amount**, one voided — where an operator holding a set of
///     contributions rather than a total would take out both, or neither;
///   * **the last posting on an account goes**, so the group empties and the balance has to be `0`
///     rather than the missing entry an extreme reads as `None`;
///   * a posting **arrives after empty**, so a group that was removed is rebuilt from nothing.
///
/// The closing assertion is on the end state, because a green run over a log that only ever went up
/// says nothing: one account emptied and refilled, one holding a credit and a debit that do not
/// cancel.
#[test]
fn a_maintained_total_survives_the_events_that_take_it_back_down() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/37-ledger.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let mut subject = Subject::new("corpus/37-ledger.beck", compile("37-ledger.beck", &src));

    let event = |variant: &str, fields: Vec<(&str, Value)>| {
        Value::data(
            Arc::from("Event"),
            Some(Arc::from(variant)),
            beck_core::core::Fields::from_iter(fields.into_iter().map(|(k, v)| (Arc::from(k), v))),
        )
    };
    let opened = |id: &str, name: &str| {
        event(
            "Opened",
            vec![("id", Value::str_(id)), ("name", Value::str_(name))],
        )
    };
    let posted = |id: &str, account: &str, amount: i64| {
        event(
            "Posted",
            vec![
                ("id", Value::str_(id)),
                ("account", Value::str_(account)),
                ("amount", Value::Int(amount)),
            ],
        )
    };
    let voided = |id: &str| event("Voided", vec![("id", Value::str_(id))]);

    let log = vec![
        opened("a1", "cash"),
        opened("a2", "stock"),
        posted("p1", "a1", 250),
        posted("p2", "a1", 125),
        // A credit, then the debit that does not quite cancel it.
        posted("p3", "a1", -90),
        posted("p4", "a2", 40),
        // Out again, from the middle of the pile and from the credit.
        voided("p2"),
        voided("p3"),
        // Two of the same amount, then half of the pair — a total is not a set of contributions.
        posted("p5", "a1", 250),
        voided("p5"),
        // And empty: everything on cash, then stock's one posting, then one back.
        voided("p1"),
        voided("p4"),
        posted("p6", "a1", -60),
        posted("p7", "a1", 275),
    ];

    let mut compared = subject.agrees("the empty log");
    for (i, e) in log.into_iter().enumerate() {
        subject.fold(e);
        compared += subject.agrees(&format!("event {}", i + 1));
    }
    assert!(compared >= 28, "only {compared} pages were compared");

    let page = subject
        .runtime
        .view(&subject.state, ACTORS[0])
        .expect("recompute")
        .render();
    assert!(
        page.contains("cash: 215p") && page.contains("stock: 0p"),
        "the log this test is written around no longer empties a group and refills it:\n{page}"
    );
}

/// **The maintained plan and the recompute agree about whether the program failed**, which is the
/// half of [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's
/// decision that an operator could get wrong without ever printing a wrong number.
///
/// `list_sum` raises when its *answer* does not fit an `Int` and not when the way there does, so a
/// running total kept in a wider accumulator is the same function as the recompute — the first
/// assertion is that pair of postings, whose sum is an ordinary `Int` and whose partial total is
/// not. A plan that maintained the balance with the language's own `+` would raise on the second
/// posting and disagree with a recompute that never fails.
///
/// The second assertion is the other edge, and it is why [`beck_core::plan::Op::GroupBy`] publishes
/// an unrepresentable total rather than raising one. The engine maintains **every** group; the
/// recompute only ever sums the groups the loop reaches. So a ghost account — postings whose
/// account was never opened, which the page therefore never asks about — must not fail a render
/// that recompute completes. Only when the same total is asked for does either of them raise, and
/// then both do.
///
/// The log is written directly rather than proposed, because `validate` rejects a posting to an
/// account that does not exist and the point of the case is a group with no reader.
#[test]
fn a_total_outside_int_fails_where_it_is_asked_for_and_nowhere_else() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/37-ledger.beck");
    let src = std::fs::read_to_string(&path).expect("the corpus program is readable");
    let mut subject = Subject::new("corpus/37-ledger.beck", compile("37-ledger.beck", &src));

    let event = |variant: &str, fields: Vec<(&str, Value)>| {
        Value::data(
            Arc::from("Event"),
            Some(Arc::from(variant)),
            beck_core::core::Fields::from_iter(fields.into_iter().map(|(k, v)| (Arc::from(k), v))),
        )
    };
    let posted = |id: &str, account: &str, amount: i64| {
        event(
            "Posted",
            vec![
                ("id", Value::str_(id)),
                ("account", Value::str_(account)),
                ("amount", Value::Int(amount)),
            ],
        )
    };

    // Recompute and maintained, side by side, as answers that may be failures.
    fn both(subject: &mut Subject) -> (Result<String, String>, Result<String, String>) {
        let actor = ACTORS[0];
        let recomputed = subject
            .runtime
            .view(&subject.state, actor)
            .map(|v| v.render())
            .map_err(|e| format!("{e:#}"));
        let state = subject.state.clone();
        let session = subject.runtime.session(actor);
        let here = beck_core::edge::presence_of(actor);
        let aware = subject
            .runtime
            .contribution(actor)
            .expect("an awareness roster");
        let maintained = subject.engines[0]
            .render_all(&state, &session, &here, &aware)
            .map(|got| page("corpus/37-ledger.beck", got))
            .map_err(|e| e.to_string());
        (recomputed, maintained)
    }

    subject.fold(event(
        "Opened",
        vec![("id", Value::str_("a1")), ("name", Value::str_("cash"))],
    ));
    // A total that leaves `Int` on the way and comes back inside it.
    subject.fold(posted("p1", "a1", i64::MAX));
    subject.fold(posted("p2", "a1", i64::MAX));
    subject.fold(posted("p3", "a1", -i64::MAX));
    let (recomputed, maintained) = both(&mut subject);
    let expected = format!("cash: {}p", i64::MAX);
    assert!(
        recomputed.as_deref().is_ok_and(|p| p.contains(&expected)),
        "the recompute did not answer a sum whose partial total is not an `Int`: {recomputed:?}"
    );
    assert_eq!(
        maintained, recomputed,
        "the maintained balance is not the recomputed one where the two are both answers"
    );

    // A group nobody asks about, whose total no `Int` holds. The page must not notice.
    subject.fold(posted("g1", "ghost", i64::MAX));
    subject.fold(posted("g2", "ghost", i64::MAX));
    let (recomputed, maintained) = both(&mut subject);
    assert!(
        recomputed.is_ok(),
        "the recompute failed on a group it never sums, so this case is not the one described"
    );
    assert_eq!(
        maintained, recomputed,
        "a group with no reader failed the maintained render and not the recomputed one — \
         `Op::GroupBy` is raising at maintenance time rather than publishing (docs/99 §99.9 item 6)"
    );

    // And the same total, asked for. Both raise, with the same words.
    subject.fold(posted("p4", "a1", i64::MAX));
    let (recomputed, maintained) = both(&mut subject);
    // Compared on the words rather than on the whole string, because the two call paths wrap
    // differently — the runtime puts "rendering the view" in front of what the backend raised and
    // the engine hands its error back bare. What the aggregate owes is that both fail and that both
    // fail *as `list_sum`*, which is what this asserts.
    for (which, got) in [("recompute", &recomputed), ("maintained", &maintained)] {
        assert!(
            got.as_ref()
                .is_err_and(|e| e.contains("`list_sum` overflowed")),
            "a balance no `Int` holds did not raise in the {which}: {got:?}"
        );
    }
}
