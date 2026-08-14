//! Query fusion: the rewritten plan against the plan it was rewritten from.
//!
//! [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.3 asks for the pass and
//! [`docs/23-incremental-views-report.md`](../../../../docs/23-incremental-views-report.md) is what building
//! it found. `incremental_engine.rs` already compares the maintained view with the recomputed one,
//! and since [`beck_core::plan::Plan::compile`] fuses, that harness covers this pass — which is
//! exactly why this file exists separately. Three things it cannot say:
//!
//! * **which plan was wrong.** "Maintained ≠ recomputed" names the engine; this compares the fused
//!   plan against the *unfused* one on the same events, so a failure names the rewrite;
//! * **whether a rule ever fired.** A pass that matched nothing would leave every differential
//!   green and prove nothing about any rule, so each rule is asserted to fire on a named program;
//! * **whether a refusal still refuses.** The two conditions that stop a rewrite —
//!   an arrangement two operators read, and a fusion that would cross §5.3's session cut — are
//!   *pessimisations* rather than errors when they are dropped, so no differential can see them.
//!   They are asserted on programs built to make each one bite.
//!
//! That last group is [`84`](../../../../docs/84-a-quota-is-only-as-good-as-its-actor-report.md)
//! §84.5's rule applied while the answer is still fresh: what would have to be true for this file
//! to go red? Delete the `consumers > 1` check and `an_arrangement_two_operators_read_is_not_fused`
//! fails; delete the session-cut check and `fusion_does_not_move_shared_work_per_subscriber` fails;
//! get the keys wrong in any rule and the differential fails on the first program that renders a
//! list.

use std::collections::BTreeSet;
use std::sync::Arc;

use beck_core::engine::{Engine, Prepared};
use beck_core::fuse::fuse;
use beck_core::gen::{arbitrary, Rng};
use beck_core::plan::{Op, Plan};
use beck_core::{Placed, Ty, Value};
use beck_rt::{Envelope, Instant, Runtime};

mod support;

const ACTORS: &[&str] = &["ana", "bo"];

fn compile(name: &str, src: &str) -> Placed {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    placed.unwrap_or_else(|| panic!("{name} did not slice"))
}

/// The programs written for the rules the corpus does not reach, and the rule each makes fire.
///
/// They are part of the differential's subject list as well as this test's, because "the rule
/// fired" and "the rule was sound" are different claims and only the first is about the plan.
const RULE_PROGRAMS: &[(&str, &str, &str)] = &[
    (
        "count-over-map.beck",
        "def render(s: State, session: Session) -> Html:\n\
        \x20   return ui:\n\
        \x20       main:\n\
        \x20           p: (str(list_len(map_list(map_values(s.posts), lambda p: p.text))) + \" posts\")",
        "a count over a cardinality-preserving operator",
    ),
    (
        "map-over-map.beck",
        "def render(s: State, session: Session) -> Html:\n\
        \x20   shouted = map_list(map_list(map_values(s.posts), lambda p: p.text), lambda t: t + \"!\")\n\
        \x20   return ui:\n\
        \x20       main:\n\
        \x20           ul:\n\
        \x20               for t in shouted:\n\
        \x20                   li: t",
        "map_list over map_list",
    ),
    (
        "filter-over-filter.beck",
        "def render(s: State, session: Session) -> Html:\n\
        \x20   long = filter_list(filter_list(map_values(s.posts), lambda p: not str_is_empty(p.text)), lambda p: str_len(p.text) > 3)\n\
        \x20   return ui:\n\
        \x20       main:\n\
        \x20           p: (str(list_len(long)) + \" long\")",
        "filter_list over filter_list",
    ),
];

/// Programs written for an *operator* the rest of the tree does not reach, rather than for a rule.
///
/// `flatten` is here because fusion took its only shape away: a `for` loop is a `map_list` under a
/// `flatten` and now compiles to one `flat_map`, so what remains of `flatten` is a collection of
/// lists that came from somewhere else. An operator with an engine implementation and no program
/// is a hole in the differential, and `every_operator_the_engine_implements_is_exercised` is what
/// keeps it from opening again.
const OPERATOR_PROGRAMS: &[(&str, &str)] = &[
    (
        "flatten-over-filter.beck",
        "def render(s: State, session: Session) -> Html:\n\
        \x20   parts = map_list(map_values(s.posts), lambda p: [p.id, p.text])\n\
        \x20   kept = filter_list(parts, lambda l: not list_is_empty(l))\n\
        \x20   return ui:\n\
        \x20       main:\n\
        \x20           ul:\n\
        \x20               for t in concat_lists(kept):\n\
        \x20                   li: t",
    ),
    // `list_is_empty` is written twice in the corpus and reaches the plan neither time: both are
    // inside an `if`, and an `if` is one opaque operator. So the engine's emptiness arm had never
    // run against recompute — a hole this gate found rather than one fusion made.
    (
        "emptiness.beck",
        "def render(s: State, session: Session) -> Html:\n\
        \x20   return ui:\n\
        \x20       main:\n\
        \x20           p: (\"empty \" + str(list_is_empty(map_list(map_values(s.posts), lambda p: p.text))))",
    ),
];

fn corpus() -> Vec<(String, Placed)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the corpus directory is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    paths.sort();
    let mut out = vec![("examples/todo.beck".to_string(), support::todo_program())];
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("a readable corpus program");
        let placed = compile(&name, &src);
        out.push((name, placed));
    }
    for (name, view, _) in RULE_PROGRAMS {
        out.push((name.to_string(), program(name, view)));
    }
    for (name, view) in OPERATOR_PROGRAMS {
        out.push((name.to_string(), program(name, view)));
    }
    out
}

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

fn page(v: Value) -> String {
    match v {
        Value::Html(h) => h.render(),
        other => other.display(),
    }
}

/// The names of the operators a plan is made of.
fn ops(plan: &Plan) -> Vec<&'static str> {
    plan.nodes.iter().map(|n| n.op.name()).collect()
}

// ---------------------------------------------------------------------------------------------
// 1. The rewrite is the identity, on every program in the tree
// ---------------------------------------------------------------------------------------------

/// Three plans over one log: the fused one, the unfused one, and the recompute that is the whole
/// engine's oracle.
///
/// Recompute is here rather than left to `incremental_engine.rs` because that harness runs the
/// corpus and this one also runs the programs written for the rules and operators the corpus does
/// not reach — and a fixture compared only against the unfused plan would agree with it about a
/// shared mistake. `list_is_empty` is exactly that case: both plans reach the same engine arm.
#[test]
fn the_fused_plan_the_unfused_plan_and_recompute_all_agree() {
    let all = corpus();
    assert!(
        all.len() >= 24,
        "only {} programs were exercised; the corpus is the measurement",
        all.len()
    );
    let (mut programs, mut compared) = (0usize, 0usize);
    for (name, placed) in all {
        let backend = beck_eval::backend(&placed);
        let unfused = Arc::new(
            Prepared::new(Arc::new(Plan::unfused(&placed)), backend.as_ref())
                .expect("the unfused plan prepares"),
        );
        let fused = Arc::new(
            Prepared::new(Arc::new(fuse(Plan::unfused(&placed)).0), backend.as_ref())
                .expect("the fused plan prepares"),
        );
        let log = log_for(&placed, &name, 30);
        let runtime = Runtime::new(placed, backend).expect("the program prepares");
        let mut state = runtime.initial_state().expect("an initial accumulator");
        // One engine per subscriber per plan, kept warm across the whole log: a rewrite that got
        // the *deltas* right and the arrangements wrong shows up only in an engine that has been
        // running, which is why nothing here renders from cold.
        let mut before: Vec<Engine> = ACTORS
            .iter()
            .map(|_| Engine::new(unfused.clone()))
            .collect();
        let mut after: Vec<Engine> = ACTORS.iter().map(|_| Engine::new(fused.clone())).collect();
        for (i, event) in std::iter::once(None)
            .chain(log.into_iter().map(Some))
            .enumerate()
        {
            if let Some(event) = event {
                let env = Envelope {
                    seq: i as u64,
                    at: Instant(i as i64),
                    actor: ACTORS[i % ACTORS.len()].to_string(),
                    body: event.clone(),
                };
                state = runtime
                    .fold(&state, &env, event)
                    .unwrap_or_else(|e| panic!("{name}: folding at seq {i}: {e}"));
            }
            for (k, actor) in ACTORS.iter().enumerate() {
                let session = runtime.session(actor);
                let here = beck_core::edge::presence_of(actor);
                let want = page(
                    before[k]
                        .render(&state, &session, &here)
                        .unwrap_or_else(|e| panic!("{name}: unfused engine: {e}")),
                );
                let got = page(
                    after[k]
                        .render(&state, &session, &here)
                        .unwrap_or_else(|e| panic!("{name}: fused engine: {e}")),
                );
                assert_eq!(
                    got, want,
                    "{name} at event {i}, subscriber `{actor}`: fusion changed the page"
                );
                let oracle = runtime
                    .view(&state, actor)
                    .unwrap_or_else(|e| panic!("{name}: recompute: {e}"))
                    .render();
                assert_eq!(
                    got, oracle,
                    "{name} at event {i}, subscriber `{actor}`: the fused plan is not the \
                     recomputed view"
                );
                compared += 1;
            }
        }
        programs += 1;
    }
    assert!(
        compared >= 2_000,
        "only {compared} pages were compared over {programs} programs"
    );
}

// ---------------------------------------------------------------------------------------------
// 2. Every rule fires somewhere
// ---------------------------------------------------------------------------------------------

/// Everything a small application needs except its view and its signals.
const PRELUDE: &str = r#"
model Post:
    id: Str
    text: Str

model State:
    posts: Map[Str, Post]

union Command:
    Publish(id: Str, text: Str)

union Event:
    Published(id: Str, text: Str)

union Rejection:
    Blank

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Published(id, text):
            return s.with(posts=map_insert(s.posts, id, Post(id=id, text=text)))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Publish(id, text):
            if str_is_empty(str_trim(text)):
                return Err(error=Blank)
            return Ok(value=[Published(id=id, text=text)])
"#;

const SIGNALS: &str = r#"
proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, posts, validate)
posts: Signal[State] = durable(fold(apply_event, State(posts={}), events))
page: Signal[Html] = per_session(posts, render)
"#;

fn program(name: &str, view: &str) -> Placed {
    compile(name, &format!("{PRELUDE}\n{view}\n{SIGNALS}"))
}

/// Every rule in the pass, and a program in the tree that makes it fire.
///
/// A rule nothing exercises is a rule the differential above says nothing about, so this is the
/// list that stops one being added and forgotten.
#[test]
fn every_rule_fires_on_a_program_somebody_can_open() {
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for (name, placed) in corpus() {
        if RULE_PROGRAMS.iter().any(|(n, _, _)| *n == name) {
            continue;
        }
        for f in fuse(Plan::unfused(&placed)).1.fired {
            seen.insert(f.rule);
        }
    }
    // The corpus is where the two common ones live: every `ui:` loop is a `flatten` over a
    // `map_list`, and `17-derived.beck` writes `concat_lists([map_values(…)])`.
    for wanted in ["flatten over map_list", "concat_lists of one list"] {
        assert!(
            seen.contains(wanted),
            "no corpus program exercises `{wanted}`"
        );
    }

    // The other three have a program written for them, which is the honest way round: a rule with
    // no program is a rule with no evidence.
    for (name, view, rule) in RULE_PROGRAMS {
        let placed = program(name, view);
        let fired: Vec<&str> = fuse(Plan::unfused(&placed))
            .1
            .fired
            .iter()
            .map(|f| f.rule)
            .collect();
        assert!(
            fired.contains(rule),
            "{name} did not exercise `{rule}`; it fired {fired:?}"
        );
        seen.insert(rule);
    }

    // The strong form, and the reason this test is worth its length: not "the rules I remembered
    // fire" but "every rule the pass has fires". A rule added without a program to exercise it
    // fails here rather than sitting in the module looking like coverage.
    let all: BTreeSet<&str> = beck_core::fuse::RULES.iter().copied().collect();
    assert_eq!(
        seen,
        all,
        "these rules have no program that reaches them: {:?}",
        all.difference(&seen).collect::<Vec<_>>()
    );
}

/// Every operator the engine implements, and a program that compiles to it.
///
/// The sibling of the rule test above, and it exists because this pass *closed* a hole rather than
/// only opening one: `flatten` was reached by every `ui:` loop in the tree until fusion turned that
/// pair into `flat_map`, and nothing would have said so. An operator the differential never runs is
/// an engine arm nobody checks against recompute.
#[test]
fn every_operator_the_engine_implements_is_exercised() {
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for (_, placed) in corpus() {
        for name in ops(&Plan::compile(&placed)) {
            seen.insert(name);
        }
    }
    let all: BTreeSet<&str> = beck_core::plan::OPERATORS.iter().copied().collect();
    assert_eq!(
        seen,
        all,
        "no program in the tree compiles to these operators: {:?}",
        all.difference(&seen).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------------------------
// 3. The refusals, which no differential can see
// ---------------------------------------------------------------------------------------------

#[test]
fn an_arrangement_two_operators_read_is_not_fused() {
    // `corpus/24-feed.beck` sorts once and reads the result three times — the loop, the count and
    // the emptiness check. Fusing the sort into the count would sort again for every reader, which
    // is docs/23's shared prefix undone, and the plan would still render the right page. Only a
    // test that looks at the plan can say so.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/24-feed.beck"),
    )
    .expect("the corpus program is readable");
    let placed = compile("24-feed.beck", &src);
    let (plan, fusions) = fuse(Plan::unfused(&placed));

    let sorts: Vec<usize> = (0..plan.nodes.len())
        .filter(|&i| matches!(plan.nodes[i].op, Op::SortBy { .. }))
        .collect();
    assert_eq!(sorts.len(), 1, "the feed sorts once: {:?}", ops(&plan));
    assert!(
        plan.nodes[sorts[0]].consumers >= 3,
        "the sorted collection is read by {} operators, so this program no longer makes the point",
        plan.nodes[sorts[0]].consumers
    );
    let refusal = fusions
        .refused
        .iter()
        .find(|r| r.kept == sorts[0])
        .unwrap_or_else(|| panic!("nothing was refused over the shared sort: {:?}", ops(&plan)));
    assert!(
        refusal.why.contains("read by"),
        "the refusal does not name the reason: {}",
        refusal.why
    );
}

#[test]
fn fusion_does_not_move_shared_work_per_subscriber() {
    // The shape §5.3 is about: a computation that does *not* read the session, feeding one that
    // does. Fusing them is a smaller plan and a slower program — the shared operator would run
    // once per subscriber instead of once per event — and the page would be identical either way.
    let placed = program(
        "across-the-cut.beck",
        "def render(s: State, session: Session) -> Html:\n\
        \x20   texts = map_list(map_values(s.posts), lambda p: p.text)\n\
        \x20   mine = map_list(texts, lambda t: t + \" for \" + session.actor)\n\
        \x20   return ui:\n\
        \x20       main:\n\
        \x20           ul:\n\
        \x20               for t in mine:\n\
        \x20                   li: t",
    );
    let (plan, fusions) = fuse(Plan::unfused(&placed));
    let shared_map = (0..plan.nodes.len())
        .find(|&i| matches!(plan.nodes[i].op, Op::MapList { .. }) && !plan.nodes[i].per_session)
        .unwrap_or_else(|| {
            panic!(
                "the shared half of the view was fused away: {:?}",
                ops(&plan)
            )
        });
    assert_eq!(
        plan.nodes[shared_map].consumers, 1,
        "nothing else reads it, so only the session cut can be what refused the fusion"
    );
    let refusal = fusions
        .refused
        .iter()
        .find(|r| r.kept == shared_map)
        .expect("the fusion across the cut was not recorded as refused");
    assert!(
        refusal.why.contains("per subscriber"),
        "the refusal does not name the cut: {}",
        refusal.why
    );
}

// ---------------------------------------------------------------------------------------------
// 4. What it is worth, counted rather than timed
// ---------------------------------------------------------------------------------------------

/// One engine's arrangement entries and per-event work, at `n` posts plus one more.
fn cost(placed: &Placed, plan: Plan, n: usize) -> (u64, u64) {
    let backend = beck_eval::backend(placed);
    let prepared =
        Arc::new(Prepared::new(Arc::new(plan), backend.as_ref()).expect("the plan prepares"));
    let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
    let mut engine = Engine::new(prepared);
    let session = runtime.session("ana");
    let here = beck_core::edge::presence_of("ana");
    let mut state = runtime.initial_state().expect("an initial accumulator");
    let fold = |state: &Value, i: usize| -> Value {
        let mut fields = beck_core::core::Fields::new();
        fields.insert(Arc::from("id"), Value::str_(format!("p{i:04}")));
        fields.insert(Arc::from("text"), Value::str_(format!("post {i}")));
        let event = Value::data(Arc::from("Event"), Some(Arc::from("Published")), fields);
        let env = Envelope {
            seq: i as u64 + 1,
            at: Instant(i as i64 + 1),
            actor: "ana".to_string(),
            body: event.clone(),
        };
        runtime.fold(state, &env, event).expect("folding")
    };
    for i in 0..n {
        state = fold(&state, i);
    }
    engine.render(&state, &session, &here).expect("warm");
    let next = fold(&state, n);
    engine.render(&next, &session, &here).expect("step");
    (engine.arranged(), engine.work().total())
}

#[test]
fn the_arrangement_the_pair_built_is_never_built() {
    // The claim, counted: fusing `flatten` over `map_list` removes an arrangement whose size is
    // the collection's, so what a subscriber holds falls by one entry per row — at every size,
    // which is what makes it a shape rather than a constant.
    let placed = program(
        "loop.beck",
        "def render(s: State, session: Session) -> Html:\n\
        \x20   return ui:\n\
        \x20       main:\n\
        \x20           ul:\n\
        \x20               for p in map_values(s.posts):\n\
        \x20                   li: p.text",
    );
    for n in [50usize, 200] {
        let (held_before, work_before) = cost(&placed, Plan::unfused(&placed), n);
        let (held_after, work_after) = cost(&placed, Plan::compile(&placed), n);
        assert_eq!(
            held_before - held_after,
            n as u64 + 1,
            "at {n} rows the fused plan should hold one entry fewer per row"
        );
        assert!(
            work_after < work_before,
            "at {n} rows fusion did not reduce the per-event work: {work_after} against \
             {work_before}"
        );
    }
}

#[test]
fn a_count_over_a_map_never_applies_the_map() {
    // The other kind of saving, and the larger one where it applies: `list_len(map_list(xs, f))`
    // asks how many, and how many is a question about `xs`. The fused plan applies `f` to nothing
    // at all — asserted as a count of applications, so it needs no clock.
    let placed = program(
        "count-over-map.beck",
        "def render(s: State, session: Session) -> Html:\n\
        \x20   return ui:\n\
        \x20       main:\n\
        \x20           p: (str(list_len(map_list(map_values(s.posts), lambda p: p.text))) + \" posts\")",
    );
    let plan = Plan::compile(&placed);
    assert!(
        !ops(&plan).contains(&"map_list"),
        "the map survived the count: {:?}",
        ops(&plan)
    );
    for n in [50usize, 200] {
        let (held_before, _) = cost(&placed, Plan::unfused(&placed), n);
        let (held_after, _) = cost(&placed, Plan::compile(&placed), n);
        assert_eq!(
            held_before - held_after,
            n as u64 + 1,
            "at {n} rows the mapped arrangement should be gone entirely"
        );
    }
}
