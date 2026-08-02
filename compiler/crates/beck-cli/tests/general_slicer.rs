//! The general slicer: topologies the old splitter could not slice, and one it sliced wrongly.
//!
//! `docs/19-phase-1-report.md` §19.9 assigned a general slicer to Phase 2;
//! `docs/20-phase-2-report.md` §20.5 recorded that Phase 2 did not deliver it;
//! `docs/22-phase-3-report.md` §22.6 said it was "debt with two phases' names on it" and "should
//! be the next thing built". `docs/23-general-slicer-report.md` is what building it found.
//!
//! Every test here is a *program*, compiled through the same `beck_core::compile_str` the CLI
//! uses, because the claim is about programs and not about a data structure. Three groups:
//!
//! 1. Topologies that are now accepted, with an assertion about what they were sliced *into* —
//!    "it compiled" is the claim a mis-slicing compiler also passes.
//! 2. The defect: two durable folds, accepted by the old splitter and sliced with both folds
//!    reading one accumulator. It is pinned as a program whose *behaviour* is asserted, so it
//!    cannot regress into compiling-but-wrong again.
//! 3. The refusals that remain, each of which is about meaning rather than about the slicer's
//!    reach, and each of which names itself.

use beck_core::core::CoreKind;
use beck_core::signal::Op;
use beck_core::{Placed, Tier};

/// Compile a program, requiring it to slice.
fn ok(name: &str, src: &str) -> Placed {
    let (placed, d, map) = beck_core::compile_str(name, src);
    assert!(!d.has_errors(), "{name}:\n{}", d.render(&map));
    placed.unwrap_or_else(|| panic!("{name} did not slice"))
}

/// Compile a program, requiring it to be refused — and to say which rule refused it.
fn refused(name: &str, src: &str) -> Vec<&'static str> {
    let (placed, d, _) = beck_core::compile_str(name, src);
    assert!(placed.is_none(), "{name} must not produce roles");
    assert!(d.has_errors(), "{name} must say why it was refused");
    let codes: Vec<&'static str> = d.iter().map(|x| x.code).collect();
    assert!(
        codes.iter().all(|c| !c.is_empty()),
        "{name}: a refusal carries a code"
    );
    codes
}

/// Everything a small application needs except its signal declarations, so that a test is the
/// topology and nothing else.
const PRELUDE: &str = r#"
model Count:
    n: Int

model Names:
    seen: Map[Str, Str]

model Summary:
    total: Int

union Command:
    Bump
    Name(who: Str)

union Event:
    Bumped
    Named(who: Str)

union Rejection:
    Blank

def apply_count(s: Count, env: Envelope[Event]) -> Count:
    match env.body:
        case Bumped:
            return s.with(n=s.n + 1)
        case Named(who):
            return s

def apply_names(s: Names, env: Envelope[Event]) -> Names:
    match env.body:
        case Bumped:
            return s
        case Named(who):
            return s.with(seen=map_insert(s.seen, who, who))

def validate(s: Count, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Bump:
            return Ok(value=[Bumped])
        case Name(who):
            if str_is_empty(str_trim(who)):
                return Err(error=Blank)
            return Ok(value=[Named(who=who)])

def summarise(c: Count) -> Summary:
    return Summary(total=c.n)

def restate(s: Summary) -> Summary:
    return s

def show(s: Summary, session: Session) -> Html:
    return ui:
        main:
            p: (str(s.total) + " bumps")

def show_both(c: Count, n: Names) -> Html:
    return ui:
        main:
            p: (str(c.n) + " bumps")
            p: (str(map_len(n.seen)) + " names")
"#;

fn program(signals: &str) -> String {
    format!("{PRELUDE}\n{signals}\n")
}

// ---------------------------------------------------------------------------------------------
// 1. Topologies the old splitter could not slice
// ---------------------------------------------------------------------------------------------

#[test]
fn a_chain_of_derived_signals_slices_to_any_depth() {
    // The old `Inliner` handled four combinators and any depth of them, so this is the case that
    // already worked; it is here so that a regression in the general path is caught by the easy
    // program rather than only by the hard one.
    let p = ok(
        "chain.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             a: Signal[Summary] = signal_map(count, summarise)\n\
             b: Signal[Summary] = signal_map(a, restate)\n\
             c: Signal[Summary] = signal_map(b, restate)\n\
             page: Signal[Html] = per_session(c, show)",
        ),
    );
    assert_eq!(p.roles.state_name.as_ref(), "count");
    for n in ["a", "b", "c"] {
        assert!(
            p.roles.inlined.iter().any(|i| i.as_ref() == n),
            "`{n}` should have been sliced into the view: {:?}",
            p.roles.inlined
        );
    }
    assert!(p.roles.shared.is_empty(), "each is read once");
}

#[test]
fn a_signal_read_twice_is_computed_once() {
    // §5.3's shared prefix, at compile time. The old splitter inlined per use, so `a` appeared
    // twice in the view and nothing recorded that they were one computation.
    let p = ok(
        "shared.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             a: Signal[Summary] = signal_map(count, summarise)\n\
             b: Signal[Summary] = signal_map(a, restate)\n\
             page: Signal[Html] = per_session(a, show)",
        ),
    );
    // `a` is read by `b` and by `page`.
    assert_eq!(
        p.roles
            .shared
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        vec!["a"],
    );
    // And the slice really is a binding rather than two copies: the view's body opens with a
    // `let`, and `summarise` is applied exactly once inside it.
    let CoreKind::Lam { body, .. } = &p.roles.view.kind else {
        panic!("the view is a lambda");
    };
    assert!(
        matches!(body.kind, CoreKind::Let { .. }),
        "a shared signal is bound, not inlined: {:?}",
        body.kind
    );
    assert_eq!(
        count_calls(&p.roles.view, "summarise"),
        1,
        "`summarise` is one computation however many readers it has"
    );
}

#[test]
fn a_fold_nested_inside_the_view_needs_no_name() {
    // `durable(fold(...))` written where a signal is expected, rather than declared and referred
    // to. The graph gives it a vertex either way; the old splitter searched for a *declaration*
    // whose expression was `durable`, so an anonymous one was a program it could not find.
    let p = ok(
        "anon.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             page: Signal[Html] = per_session(signal_map(count, summarise), show)",
        ),
    );
    // The inner `signal_map` is a vertex with no written name.
    let g = &p.graph;
    let anon: Vec<&str> = g
        .nodes
        .iter()
        .filter(|n| n.name.is_none())
        .map(|n| n.label.as_ref())
        .collect();
    assert!(
        anon.contains(&"page·signal_map"),
        "an unnamed operation is still a vertex: {anon:?}"
    );
    assert!(matches!(g.node(g.states()[0]).op, Op::Durable));
}

#[test]
fn an_alias_names_a_signal_without_adding_a_computation() {
    let p = ok(
        "alias.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             mirror: Signal[Count] = count\n\
             page: Signal[Html] = per_session(signal_map(mirror, summarise), show)",
        ),
    );
    assert_eq!(p.roles.state_name.as_ref(), "count");
    assert!(matches!(
        p.graph.node(p.graph.by_name["mirror"]).op,
        Op::Alias
    ));
}

#[test]
fn every_tier_crossing_is_enumerated_with_an_id() {
    // §4.3: "Every signal edge that crosses tiers becomes a subscription … resumable by
    // (subscription id, last seq)." The old `beck explain flow` printed one hard-coded sentence
    // claiming there was exactly one crossing, which was true of the todo sketch and of nothing
    // else.
    let src = include_str!("../../../examples/todo.beck");
    let p = ok("examples/todo.beck", src);
    assert!(
        p.graph.cuts.len() >= 2,
        "the sketch crosses tiers more than once: {:?}",
        p.graph.cuts.len()
    );
    for c in &p.graph.cuts {
        assert_ne!(p.graph.node(c.from).tier, p.graph.node(c.to).tier);
        assert_eq!(c.id.len(), 16, "a subscription is keyed by a content id");
    }
    // Content-derived, so two crossings carrying different types have different ids.
    let ids: std::collections::BTreeSet<&str> =
        p.graph.cuts.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.len(), p.graph.cuts.len(), "ids collide");

    // …and the whole report is derived from the graph rather than from a template.
    let report = beck_core::split::flow_report(&p);
    assert!(report.contains("todos·fold"), "{report}");
    assert!(report.contains("tier crossings"), "{report}");
}

// ---------------------------------------------------------------------------------------------
// 2. The defect: two durable folds
// ---------------------------------------------------------------------------------------------

#[test]
fn two_durable_folds_are_fused_rather_than_confused() {
    // The regression this whole exercise exists for. Before the general slicer this program
    // compiled — `beck check` printed `ok` — and both folds were lowered to the same accumulator,
    // so `show_both` was handed a `Count` where it expected a `Names`.
    let p = ok(
        "two.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             names: Signal[Names] = durable(fold(apply_names, Names(seen={}), events))\n\
             page: Signal[Html] = map2(show_both, count, names)",
        ),
    );
    assert!(p.roles.is_fused());
    assert_eq!(
        p.roles
            .states
            .iter()
            .map(|s| s.name.to_string())
            .collect::<Vec<_>>(),
        vec!["count", "names"],
    );
    // Each fold occupies its own field, so the two are told apart by construction.
    assert_eq!(
        p.roles.states[0].field.as_deref(),
        Some("count"),
        "each fold has its own field"
    );
    assert_eq!(p.roles.states[1].field.as_deref(), Some("names"));
    assert_eq!(
        p.roles.state_ty.con_name(),
        Some(beck_core::signal::FUSED_STATE)
    );
    // The synthetic accumulator is a compiler product and no module publishes it.
    let iface = beck_core::Interface::of(&p.program);
    assert!(
        !iface
            .types
            .iter()
            .any(|t| t.name().as_ref() == beck_core::signal::FUSED_STATE),
        "the fused accumulator is not part of any module's contract"
    );
}

#[test]
fn each_fused_fold_folds_its_own_events_and_the_view_reads_the_right_one() {
    // The behavioural half, because "it has two fields" is a claim a compiler that filled both
    // from the same fold would also pass. This runs the program.
    let src = program(
        "proposals: Stream[Proposal] = merge_clients()\n\
         events: Stream[Event] = decide(proposals, count, validate)\n\
         count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
         names: Signal[Names] = durable(fold(apply_names, Names(seen={}), events))\n\
         page: Signal[Html] = map2(show_both, count, names)\n\
         \n\
         test \"each fold keeps its own answer\":\n    \
             given [Bumped, Named(who=\"ana\"), Bumped]\n    \
             expect page contains \"2 bumps\"\n    \
             expect page contains \"1 names\"\n",
    );
    let p = ok("two-run.beck", &src);
    let backend = beck_eval::backend(&p);
    let report = beck_rt::testing::run(&p, backend, &Default::default());
    assert_eq!(report.failed(), 0, "{report:#?}");
    assert_eq!(report.passed(), 1);
}

#[test]
fn the_chokepoint_reads_the_fold_it_was_given() {
    // `decide(proposals, count, validate)` names `count`, and `validate` takes a `Count`. With a
    // fused accumulator the slicer has to project the *named* fold out of it, and picking the
    // wrong one is a defect no type in the sliced `Core` would catch.
    let p = ok(
        "chokepoint.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             names: Signal[Names] = durable(fold(apply_names, Names(seen={}), events))\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             page: Signal[Html] = map2(show_both, count, names)",
        ),
    );
    // `names` is declared first, so a slicer that took "the first fold" would project `names`.
    assert_eq!(p.roles.states[0].name.as_ref(), "names");
    let CoreKind::Lam { body, .. } = &p.roles.validate.kind else {
        panic!("a fused validate is a lambda over the accumulator");
    };
    let CoreKind::App { args, .. } = &body.kind else {
        panic!("it applies the program's own validate");
    };
    let CoreKind::Field { name, .. } = &args[0].kind else {
        panic!("…to a field of the accumulator");
    };
    assert_eq!(name.as_ref(), "count", "the field the chokepoint named");
}

#[test]
fn a_single_fold_program_is_untouched_by_fusion() {
    // Everything Phase 1 and Phase 2 claimed about the sketch has to still be true, and "still
    // true" includes the accumulator being the program's own type rather than a wrapper.
    let src = include_str!("../../../examples/todo.beck");
    let p = ok("examples/todo.beck", src);
    assert!(!p.roles.is_fused());
    assert_eq!(p.roles.state_ty.con_name(), Some("State"));
    assert_eq!(p.roles.states.len(), 1);
    assert!(p.roles.states[0].field.is_none());
    // The fold role is still the function the program wrote, not a synthesised lambda.
    assert!(
        matches!(&p.roles.fold.kind, CoreKind::Global(n) if n.as_ref() == "apply_event"),
        "the sketch's fold is `apply_event` itself: {:?}",
        p.roles.fold.kind
    );
}

#[test]
fn a_filter_map_between_the_chokepoint_and_a_fold_narrows_that_fold_only() {
    let src = include_str!("../../../corpus/23-slices.beck");
    let p = ok("corpus/23-slices.beck", src);
    assert!(p.roles.is_fused());
    let backend = beck_eval::backend(&p);
    let report = beck_rt::testing::run(&p, backend, &Default::default());
    assert_eq!(report.failed(), 0, "{report:#?}");
    assert!(report.passed() >= 3);
}

// ---------------------------------------------------------------------------------------------
// 3. The refusals that remain
// ---------------------------------------------------------------------------------------------

#[test]
fn a_cycle_with_no_fold_in_it_is_refused_by_name() {
    // Why the rule exists: a fold is where a cycle bottoms out, because an accumulator is a value
    // the slicer can take as a parameter. Without one there is no first value to compute, and a
    // slicer that did not check would recurse until the stack ran out.
    let codes = refused(
        "loop.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             a: Signal[Summary] = signal_map(b, restate)\n\
             b: Signal[Summary] = signal_map(a, restate)\n\
             page: Signal[Html] = per_session(a, show)",
        ),
    );
    assert!(codes.contains(&"B0509"), "{codes:?}");
}

#[test]
fn a_view_that_reads_a_stream_is_refused_before_the_slicer_ever_sees_it() {
    // §3.7: a `Stream` is discrete occurrences and a `Signal` is a value defined at all times, and
    // the two are different types. So the *typechecker* refuses this, which is an earlier and
    // better diagnostic than the slicer's B0507 — and the reason B0507's view arm is a refusal
    // that should be unreachable rather than one that fires.
    //
    // Deliberately asserting that *something* names itself rather than which stage, by the same
    // reasoning `docs/19` §19.9 gives: the property is that no shape is quietly mis-sliced.
    let codes = refused(
        "stream-view.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             page: Signal[Html] = per_session(events, show)",
        ),
    );
    assert!(!codes.is_empty(), "{codes:?}");
}

#[test]
fn a_signal_that_is_a_computation_rather_than_a_node_is_refused_by_name() {
    // A signal is a vertex in the dataflow, and a conditional between two of them is a
    // computation. The diagnostic says where a computation goes: in a `def`, with the signal
    // naming it.
    //
    // This is one of the few shapes that reaches the graph builder at all. `Signal[T]` and
    // `Stream[T]` are ordinary types, so unification already refuses most of what B0507 and B0508
    // describe — see docs/23 §23.5.
    let codes = refused(
        "computed.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             odd: Signal[Count] = count if True else count\n\
             page: Signal[Html] = per_session(signal_map(count, summarise), show)",
        ),
    );
    assert!(
        codes.contains(&"B0507") || codes.contains(&"B0508"),
        "{codes:?}"
    );
}

#[test]
fn two_pages_are_refused_and_told_it_is_routing() {
    // The limit is the runtime's, not the slicer's, and the diagnostic says so: the slicer slices
    // both, and choosing between them is a router — a Phase 3 client bullet that is not built.
    let codes = refused(
        "two-pages.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             names: Signal[Names] = durable(fold(apply_names, Names(seen={}), events))\n\
             page: Signal[Html] = map2(show_both, count, names)\n\
             admin: Signal[Html] = map2(show_both, count, names)",
        ),
    );
    assert!(codes.contains(&"B0510"), "{codes:?}");
}

#[test]
fn a_fold_that_is_not_durable_is_refused_by_name() {
    let codes = refused(
        "transient.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             shadow: Signal[Count] = fold(apply_count, Count(n=0), events)\n\
             page: Signal[Html] = per_session(signal_map(shadow, summarise), show)",
        ),
    );
    assert!(codes.contains(&"B0513"), "{codes:?}");
}

#[test]
fn a_second_chokepoint_is_refused_because_authority_is_one_place() {
    let codes = refused(
        "two-decides.beck",
        &program(
            "proposals: Stream[Proposal] = merge_clients()\n\
             events: Stream[Event] = decide(proposals, count, validate)\n\
             again: Stream[Event] = decide(proposals, count, validate)\n\
             count: Signal[Count] = durable(fold(apply_count, Count(n=0), events))\n\
             other: Signal[Count] = durable(fold(apply_count, Count(n=0), again))\n\
             page: Signal[Html] = per_session(signal_map(count, summarise), show)",
        ),
    );
    assert!(codes.contains(&"B0511"), "{codes:?}");
}

#[test]
fn the_placement_of_a_general_graph_still_keeps_the_log_off_the_browser() {
    // §3.5 as a property of the *solution*: whatever the graph's shape, no vertex carrying a
    // durable accumulator is placed on the client.
    for file in [
        "../../../corpus/21-two-folds.beck",
        "../../../corpus/22-shared.beck",
        "../../../corpus/23-slices.beck",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join(file);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
        let p = ok(file, &src);
        for s in p.graph.states() {
            assert_ne!(
                p.graph.node(s).tier,
                Tier::Client,
                "{file}: `{}` holds the log",
                p.graph.label(s)
            );
        }
    }
}

/// How many times a named global is applied inside an expression.
fn count_calls(c: &beck_core::Core, name: &str) -> usize {
    let mut n = 0;
    walk(c, &mut |k| {
        if let CoreKind::App { func, .. } = &k.kind {
            if matches!(&func.kind, CoreKind::Global(g) if g.as_ref() == name) {
                n += 1;
            }
        }
    });
    n
}

fn walk(c: &beck_core::Core, f: &mut impl FnMut(&beck_core::Core)) {
    f(c);
    match &c.kind {
        CoreKind::Lam { body, .. } => walk(body, f),
        CoreKind::App { func, args } => {
            walk(func, f);
            args.iter().for_each(|a| walk(a, f));
        }
        CoreKind::Prim { args, .. } => args.iter().for_each(|a| walk(a, f)),
        CoreKind::Let { value, body, .. } => {
            walk(value, f);
            walk(body, f);
        }
        CoreKind::If { cond, then, alt } => {
            walk(cond, f);
            walk(then, f);
            walk(alt, f);
        }
        CoreKind::Match { scrutinee, arms } => {
            walk(scrutinee, f);
            arms.iter().for_each(|a| walk(&a.body, f));
        }
        CoreKind::Make { fields, .. } => fields.iter().for_each(|(_, v)| walk(v, f)),
        CoreKind::Field { base, .. } => walk(base, f),
        CoreKind::With { base, fields } => {
            walk(base, f);
            fields.iter().for_each(|(_, v)| walk(v, f));
        }
        CoreKind::ListLit(items) => items.iter().for_each(|i| walk(i, f)),
        CoreKind::MapLit(pairs) => pairs.iter().for_each(|(k, v)| {
            walk(k, f);
            walk(v, f);
        }),
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
    }
}

// ---------------------------------------------------------------------------------------------
// 4. The fused accumulator through the whole runtime
// ---------------------------------------------------------------------------------------------

/// A fused program driven by the real ingress, so the synthetic accumulator meets the log.
#[tokio::test]
async fn a_fused_accumulator_replays_bit_for_bit() {
    // §3.7's whole correctness argument is "replaying the log reproduces the state, bit for bit",
    // and the fused accumulator is a value the slicer invented rather than one the program wrote.
    // If `value_to_repr` could not encode it — `docs/19` §19.9 records a lossy branch that once
    // could not — this is where it would show, at write time rather than at read time.
    use beck_rt::{replay_from_genesis, replay_to, App, AppConfig, LogStore, MemoryLog};

    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/21-two-folds.beck"),
    )
    .expect("the corpus program is checked in");
    let placed = ok("corpus/21-two-folds.beck", &src);
    assert!(placed.roles.is_fused());

    let store = std::sync::Arc::new(MemoryLog::new());
    let runtime = beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed))
        .expect("the fused program prepares");
    let app = App::start(runtime, store.clone(), AppConfig::default())
        .await
        .expect("app starts");

    for (i, who) in ["ana", "bo", "cai", "ana"].iter().enumerate() {
        let _ = app
            .propose(format!("arrive-{i}"), who.to_string(), arrive(who))
            .await;
    }
    let live = beck_core::digest(&app.state().await).to_vec();

    let runtime = beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed)).expect("prep");
    let head = store.head().await.expect("head");
    assert!(head > 0, "nothing was recorded");
    let (a, _) = replay_to(&runtime, store.as_ref(), head)
        .await
        .expect("replay");
    let (b, _) = replay_from_genesis(&runtime, store.as_ref())
        .await
        .expect("replay from genesis");
    assert_eq!(beck_core::digest(&a), beck_core::digest(&b));
    assert_eq!(beck_core::digest(&a).to_vec(), live);

    // And both fields are really in there: three arrivals, one of them repeated.
    let roster = a.field("roster").expect("the roster field").clone();
    let tally = a.field("tally").expect("the tally field").clone();
    assert_eq!(tally.field("joins").and_then(|v| v.as_int()), Some(4));
    assert!(roster.field("here").is_some(), "the roster field is a map");
}

fn arrive(who: &str) -> beck_core::Value {
    beck_core::Value::Data {
        ty: std::sync::Arc::from("Command"),
        variant: Some(std::sync::Arc::from("Arrive")),
        fields: std::sync::Arc::new(std::collections::BTreeMap::from([(
            std::sync::Arc::from("who"),
            beck_core::Value::str_(who),
        )])),
    }
}

#[test]
fn a_test_block_sees_the_fused_accumulator_by_the_program_s_own_names() {
    // The accumulator is a type the compiler made, and `state` inside a `test` block is typed
    // against it. Getting this wrong is quiet: the checker would say `state` is the *first* fold's
    // type, `expect state.joins == 1` would typecheck, and the failure would arrive at run time as
    // a missing field.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/21-two-folds.beck"),
    )
    .expect("checked in");
    let p = ok("corpus/21-two-folds.beck", &src);
    let backend = beck_eval::backend(&p);
    let report = beck_rt::testing::run(&p, backend, &Default::default());
    assert_eq!(report.failed(), 0, "{report:#?}");

    // …and reaching for a field of the *wrong* fold is a compile error naming the accumulator,
    // rather than a test that fails for a reason three stages from the cause.
    let wrong = src.replace("expect state.tally.joins == 2", "expect state.joins == 2");
    let (placed, d, _) = beck_core::compile_str("wrong.beck", &wrong);
    assert!(placed.is_none());
    assert!(
        d.iter()
            .any(|x| x.code == "B0350" && x.message.contains(beck_core::signal::FUSED_STATE)),
        "{:?}",
        d.iter().map(|x| x.message.clone()).collect::<Vec<_>>()
    );
}
