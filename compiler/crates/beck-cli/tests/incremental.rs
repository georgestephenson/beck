//! `beck explain incremental` — which views a plan could maintain, and why the rest could not.
//!
//! `docs/03-type-and-effect-system.md` §3.8 asks for this command by name and
//! `docs/20-phase-2-report.md` §20.5 recorded it as unbuilt. It was the analysis with nothing
//! behind it; there is now an engine (`docs/24-incremental-views-report.md`), and the obligation
//! this harness enforces has not changed: the first line must be true of *this program*, because a
//! command called `explain incremental` that let a reader believe their view was being maintained
//! when it is not would be worse than no command.
//!
//! The three verdicts are asserted on programs that produce them, rather than on the corpus alone.
//! Every corpus program's views happen to be relational — lists, maps, counts and `ui:` — so a
//! harness that only read the corpus would pass with an analysis that answered "incremental" to
//! everything.

use beck_core::incremental::{assess, report, verdicts, Verdict, RULES};
use beck_core::Placed;

fn ok(name: &str, src: &str) -> Placed {
    let (placed, d, map) = beck_core::compile_str(name, src);
    assert!(!d.has_errors(), "{name}:\n{}", d.render(&map));
    placed.unwrap_or_else(|| panic!("{name} did not slice"))
}

fn corpus(file: &str) -> Placed {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(file);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
    ok(file, &src)
}

/// A program whose view shape is the thing under test.
fn with_view(defs: &str, view_signal: &str) -> String {
    format!(
        r#"
model State:
    items: Map[Str, Int]

model Pick:
    label: Str

union Command:
    Add(k: Str)

union Event:
    Added(k: Str)

union Rejection:
    Blank

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Added(k):
            return s.with(items=map_insert(s.items, k, 1))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Add(k):
            if str_is_empty(str_trim(k)):
                return Err(error=Blank)
            return Ok(value=[Added(k=k)])

{defs}

def show(p: Pick, session: Session) -> Html:
    return ui:
        main:
            p: p.label

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, items, validate)
items: Signal[State] = durable(fold(apply_event, State(items={{}}), events))
{view_signal}
page: Signal[Html] = per_session(chosen, show)
"#
    )
}

#[test]
fn the_report_says_what_is_true_of_this_program_before_it_says_anything_else() {
    // The sketch's view holds a collection, so operators are maintained and the first line says so.
    let p = corpus("../examples/todo.beck");
    let text = report(&p, None);
    let first = text.lines().next().unwrap_or_default();
    assert!(
        first.contains("maintained by delta"),
        "the first line has to be the honest one: {first:?}"
    );
    // …and the same line says what is still not incremental, in the same breath.
    assert!(
        text.contains("assembled in full every time"),
        "the headline hides the O(n) that remains: {text}"
    );

    // A program whose view holds no collection has nothing maintained, and the first line has to
    // say *that* rather than repeat the feature. This is the assertion that would have failed if
    // the report had been written about the engine instead of about the program.
    let p = corpus("22-shared.beck");
    let text = report(&p, None);
    let first = text.lines().next().unwrap_or_default();
    assert!(
        first.contains("Nothing in this view is maintained"),
        "{first:?}"
    );
}

#[test]
fn a_relational_view_could_be_maintained_by_delta() {
    // §3.8's own example: "`remaining` updates by ±1 per event, never by recount". The sketch's
    // view sorts, filters, counts and renders, and every one of those has a delta rule.
    let p = corpus("../examples/todo.beck");
    let v = verdicts(&p);
    assert_eq!(v.get("page"), Some(&Verdict::Incremental), "{v:?}");

    let a = assess(&p);
    let page = a.iter().find(|a| a.label.as_ref() == "page").expect("page");
    let names: Vec<&str> = page.ops.iter().map(|(p, _)| p.name()).collect();
    for want in [
        "sort_by",
        "filter_list",
        "list_len",
        "map_values",
        "html_el",
    ] {
        assert!(names.contains(&want), "expected `{want}` in {names:?}");
    }
    // And every operation is reported with the rule it would be maintained by, not just its name.
    assert!(page.ops.iter().all(|(_, r)| !r.is_empty()));
}

#[test]
fn a_match_on_the_input_is_recompute_and_the_report_says_why() {
    // A delta can move the scrutinee between arms, which changes which computation runs. Treating
    // that as maintainable would be the analysis lying about the interesting case.
    let src = with_view(
        r#"def pick(s: State) -> Pick:
    match map_get(s.items, "a"):
        case Some(value):
            return Pick(label="has a")
        case None:
            return Pick(label="no a")"#,
        "chosen: Signal[Pick] = signal_map(items, pick)",
    );
    let p = ok("match.beck", &src);
    let v = verdicts(&p);
    let Some(Verdict::Recompute { because }) = v.get("chosen") else {
        panic!("expected a recompute verdict: {v:?}");
    };
    assert!(because.contains("match"), "{because}");
    // …and the rest of the program is unaffected: one blocked view does not blanket the plan.
    assert_eq!(v.get("page"), Some(&Verdict::Incremental));
}

#[test]
fn a_view_with_an_effect_fails_the_precondition_before_its_shape_is_looked_at() {
    // §3.8: a view whose row is empty is a pure function of the signal. One whose row is not is
    // re-evaluated when the effect says so, and no delta rule applies.
    let src = with_view(
        r#"def legacy_label(k: Str) -> Str uses external.read(legacy):
    return k

def pick(s: State) -> Pick:
    return Pick(label=legacy_label(str(map_len(s.items))))"#,
        "chosen: Signal[Pick] = signal_map(items, pick)",
    );
    let p = ok("effectful.beck", &src);
    let v = verdicts(&p);
    let Some(Verdict::Effectful { effects }) = v.get("chosen") else {
        panic!("expected an effectful verdict: {v:?}");
    };
    assert!(
        effects.iter().any(|e| e.name().contains("external.read")),
        "{effects:?}"
    );
    let text = report(&p, Some("chosen"));
    assert!(text.contains("§3.8's precondition"), "{text}");
}

#[test]
fn an_ambient_effect_does_not_block_a_view() {
    // §3.2's `log` and `metrics` force nothing — [`20`](docs/20-phase-2-report.md) §20.4 item 3 —
    // so a view that logs is still a pure function of its input as far as maintenance goes.
    let src = with_view(
        r#"def audited(what: Str) -> Str uses log, metrics:
    return what

def pick(s: State) -> Pick:
    return Pick(label=audited(str(map_len(s.items))))"#,
        "chosen: Signal[Pick] = signal_map(items, pick)",
    );
    let p = ok("ambient.beck", &src);
    assert_eq!(verdicts(&p).get("chosen"), Some(&Verdict::Incremental));
}

#[test]
fn the_plan_names_the_shared_prefix_and_the_per_session_cut() {
    // [`05`](docs/05-tier-lowering.md) §5.3: "one shared dataflow whose final per-session operators
    // run per subscriber". Both halves of that sentence are now things the compiler can point at.
    let p = corpus("22-shared.beck");
    let a = assess(&p);
    let shared: Vec<&str> = a
        .iter()
        .filter(|x| x.shared)
        .map(|x| x.label.as_ref())
        .collect();
    assert_eq!(shared, vec!["tally"], "{a:#?}");

    let fanout: Vec<&str> = a
        .iter()
        .filter(|x| x.per_session)
        .map(|x| x.label.as_ref())
        .collect();
    assert_eq!(fanout, vec!["page"], "the cut is at `per_session`: {a:#?}");
    // `tally` is *above* the cut, so one arrangement serves every session — which is the whole
    // difference between this and a thousand plans.
    assert!(
        !a.iter()
            .find(|x| x.label.as_ref() == "tally")
            .expect("tally")
            .per_session
    );

    let text = report(&p, None);
    assert!(text.contains("shared arrangement: tally"), "{text}");
    assert!(text.contains("per subscriber:"), "{text}");
}

#[test]
fn a_broadcast_program_has_no_per_session_operators() {
    let p = corpus("21-two-folds.beck");
    assert!(assess(&p).iter().all(|a| !a.per_session));
    assert!(report(&p, None).contains("broadcasts one view to every connection"));
}

#[test]
fn the_fold_and_the_chokepoint_are_not_views() {
    // §3.8's question is about views. A fold is not maintained by delta — it is what produces the
    // deltas — and answering "incremental?" about `merge_clients()` would be filling the report
    // with rows nobody asked for.
    let p = corpus("../examples/todo.beck");
    let labels: Vec<String> = assess(&p).iter().map(|a| a.label.to_string()).collect();
    assert_eq!(labels, vec!["page"], "{labels:?}");
}

#[test]
fn asking_about_a_name_that_is_not_a_view_lists_the_ones_that_are() {
    let p = corpus("22-shared.beck");
    let text = report(&p, Some("ballot"));
    assert!(text.contains("is not a view"), "{text}");
    assert!(text.contains("tally"), "{text}");
}

#[test]
fn every_rule_in_the_table_names_an_operation_and_a_maintenance_strategy() {
    // The table is stated rather than measured — the same discipline `cost.rs` holds itself to —
    // so the thing to check is that no entry is a name with no rule beside it.
    assert!(RULES.len() > 20, "the table is not a stub");
    for (op, rule) in RULES {
        assert!(!rule.is_empty(), "`{}` has no rule", op.name());
        assert!(
            rule.len() > 8,
            "`{}`'s rule says nothing: {rule:?}",
            op.name()
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for (op, _) in RULES {
        assert!(seen.insert(op.name()), "`{}` is listed twice", op.name());
    }
}

#[test]
fn every_corpus_program_is_assessable_and_none_of_them_is_a_mystery() {
    // Not "every view is incremental" — that is a property of these programs, not of the analysis.
    // What is asserted is that no view comes back with an empty explanation.
    for entry in
        std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
            .expect("the corpus is readable")
    {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "beck") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("readable");
        let (placed, d, map) = beck_core::compile_str(&name, &src);
        assert!(!d.has_errors(), "{name}:\n{}", d.render(&map));
        let placed = placed.expect("slices");
        for a in assess(&placed) {
            match &a.verdict {
                Verdict::Incremental => assert!(
                    !a.ops.is_empty(),
                    "{name}: `{}` is incremental and applies nothing",
                    a.label
                ),
                Verdict::Recompute { because } => assert!(!because.is_empty()),
                Verdict::Effectful { effects } => assert!(!effects.is_empty()),
            }
        }
        let text = report(&placed, None);
        assert!(
            text.contains("maintained by delta") || text.contains("Nothing in this view"),
            "{name}: the report does not lead with what is true of this program:\n{text}"
        );
        assert!(
            text.contains("the operators the view compiles to"),
            "{name}"
        );
    }
}
