//! `beck explain incremental` — which views a plan could maintain, and why the rest could not.
//!
//! `docs/03-type-and-effect-system.md` §3.8 asks for this command by name and
//! `docs/20-phase-2-report.md` §20.5 recorded it as unbuilt. It was the analysis with nothing
//! behind it; there is now an engine (`docs/23-incremental-views-report.md`), and the obligation
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

/// `beck explain cost` over a program, which is `explain incremental`'s sibling about the same
/// plan: that one says whether a view *can* be maintained, this one says what maintaining it costs.
fn cost(p: &Placed) -> String {
    cost_with(p, beck_core::plan::Relate::Recognise)
}

fn cost_with(p: &Placed, relate: beck_core::plan::Relate) -> String {
    beck_core::plan::cost_report(&beck_core::plan::Plan::compile_with(p, relate))
}

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

/// The tally counts every line the report itself printed as `O(n)` per event.
///
/// It did not. The summary counted operators whose cost mentions `n entries copied` and the
/// capture line — the one that says a per-element function captured a plan node — was written
/// *after* the count, so a program whose loop captured the accumulator was told "1 of 29" when two
/// operators cost `O(n)`. The headline was wrong on exactly the programs where the cost matters,
/// and wrong in the reassuring direction (`docs/99` §99.3).
///
/// So this reads both numbers **out of the printed text** rather than from the plan. A test that
/// recomputed the count from the plan would agree with a report that printed something else, which
/// is the shape of the defect rather than a check on it.
#[test]
fn the_tally_counts_every_line_the_report_prints() {
    // `27-review.beck` is the program §99.3 quotes, **with the join switched off**, which is the
    // plan that finding was made against. Its loop is an equi-join and the compiler now reads it as
    // one, so with the recognition on there is no captured accumulator left to count — the defect
    // this test was written for was in the *report*, not in the plan, and it is still the report
    // that has to be held to what it prints. `Relate::Refuse` is how a program with two reasons for
    // costing `O(n)` stays reachable now that the corpus has one fewer.
    let text = cost_with(&corpus("27-review.beck"), beck_core::plan::Relate::Refuse);
    let printed = text
        .lines()
        .filter(|l| l.contains("n entries copied") || l.contains("n applications on every event"))
        .count();
    assert!(
        printed >= 2,
        "this program should show both reasons:\n{text}"
    );

    let headline = text
        .lines()
        .find(|l| l.contains("operators cost O(n) per event"))
        .unwrap_or_else(|| panic!("no tally at all:\n{text}"));
    let counted: usize = headline
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("the tally is not a number: {headline:?}"));
    assert_eq!(
        counted, printed,
        "the report printed {printed} operators costing O(n) per event and said {counted}:\n{text}"
    );

    // And both reasons are named, because they are not the same defect and do not have the same
    // fix: one is the arrangement's representation, the other is a program that left the algebra.
    assert!(
        text.contains("an arrangement is a keyed collection"),
        "{text}"
    );
    assert!(text.contains("captured the state"), "{text}");
}

/// A capture says how often what it captured *moves*, because that is the whole of what it costs.
///
/// The line used to name the captured node and stop. A captured constant never moves, a captured
/// session moves when somebody navigates, and a captured state moves on every event — the same
/// sentence for all three, leaving a reader to trace inputs back to `#0` by hand. One of the real
/// cases in the corpus is two hops away (`docs/99` §99.3).
///
/// Three programs differing in that one respect, and the assertion is that the three lines differ.
/// A fix that classified nothing would print three identical lines and still pass a test that only
/// looked at one of them, which is why all three are here and why they are compared to each other
/// as well as to what they should say.
#[test]
fn a_capture_says_how_often_what_it_captured_moves() {
    let captured_line = |rows: &str| -> String {
        let text = cost(&ok("captures.beck", &capturing(rows)));
        text.lines()
            .find(|l| l.contains("its function captured"))
            .unwrap_or_else(|| panic!("nothing captured anything:\n{text}"))
            .trim()
            .to_string()
    };

    // Captured: a list that is closed, so the plan evaluates it once and it never moves again.
    let never = captured_line(
        "def palette() -> list[Str]:\n    \
             return [\"a\", \"b\", \"c\"]\n\
         \ndef rows(s: State, session: Session) -> list[Str]:\n    \
             ps = palette()\n    \
             return map_list(map_keys(s.items), lambda k: k + str(list_len(ps)))\n",
    );

    // Captured: the session. It moves while a subscription is open, but not with the log — which
    // is `examples/todo.beck`'s own shape, and the reason this is not simply an expensive case.
    let per_subscription = captured_line(
        "def rows(s: State, session: Session) -> list[Str]:\n    \
             return filter_list(map_keys(s.items), lambda k: k != session.actor)\n",
    );

    // Captured: the state, which is the expensive one and the reason this report exists.
    let per_event = captured_line(
        "def rows(s: State, session: Session) -> list[Str]:\n    \
             return map_list(map_keys(s.items), \
                             lambda k: k + str(list_len(map_keys(s.items))))\n",
    );

    assert!(
        never.contains("never moves") && never.contains("no cost per event"),
        "a captured constant costs nothing and should say so: {never:?}"
    );
    assert!(
        per_subscription.contains("when the session moves")
            && per_subscription.contains("not per event"),
        "a captured session is not a per-event cost: {per_subscription:?}"
    );
    assert!(
        per_event.contains("on every event") && per_event.contains("downstream of the state"),
        "a captured state is the expensive one: {per_event:?}"
    );
    assert_ne!(never, per_subscription);
    assert_ne!(per_subscription, per_event);
    assert_ne!(never, per_event);
}

/// A loop that looks something up and is **not** read as a join says which condition failed, and
/// says it after fusion.
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.6's rule for the shape
/// inference cannot see — "compile it the slow way and *say so*". The reason is recorded on the
/// `map_list` the decomposition built; every `ui:` loop then fuses that `map_list` into the
/// `flatten` above it, and the survivor kept its own empty field. So the explanation existed and the
/// one shape it exists for was the one shape that never printed it, on both programs in the tree
/// that have it. The assertion is therefore on the **fused** plan, which is what
/// `beck explain cost` prints.
#[test]
fn a_loop_that_is_not_read_as_a_join_says_why_after_fusion() {
    // The key reads the session as well as the element, so it is not an equi-join on the left row:
    // there is one lookup per (element, session) pair and no single index answers it.
    let text = cost(&ok(
        "not-a-join.beck",
        &capturing(
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: k + str(unwrap_or(map_get(s.items, \
                                                                     k + session.actor), 0)))\n",
        ),
    ));

    let line = text
        .lines()
        .find(|l| l.contains("not read as a join"))
        .unwrap_or_else(|| panic!("the loop pays for a lookup and does not say why:\n{text}"));
    assert!(
        line.contains("not an equi-join on the left row"),
        "the reason names the wrong condition: {line:?}"
    );
    // Beside the cost it explains, rather than somewhere else in the report: a reason a reader has
    // to go looking for is a reason they will not find.
    let (cause, cost_line) = (
        text.lines().position(|l| l.contains("not read as a join")),
        text.lines().position(|l| l.contains("on every event")),
    );
    assert_eq!(
        cause,
        cost_line.map(|n| n + 1),
        "the reason is not under the line it explains:\n{text}"
    );
}

/// A loop whose body **filters** another collection is a join when the predicate is an equality, it
/// answers with whatever the body asked the group for, and it says which half was missing when the
/// predicate is not one.
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.6's second shape and
/// §99.9 item 6's first aggregate, which are the same recognition reading two questions.
/// `filter_list(xs, lambda y: g(y) == k(x))` is a many-to-one equi-join over an `arrange_by`; the
/// same expression under a `list_len` is that join asked **how many**, and the difference is whether
/// a group is built at all. A predicate that is not an equality between one function of the row and
/// one of the element is not an index probe either way, and the honest outcome is the slow
/// compilation with the reason attached. All three are asserted, because a rule that refused
/// *everything* would pass the last on its own, and one that counted everything would pass the
/// first.
#[test]
fn a_loop_that_filters_by_an_equality_is_a_join_answering_what_its_body_asked() {
    use beck_core::plan::{Matching, Op, Plan};

    let answers = |name: &str, body: &str| -> (bool, Option<Matching>) {
        let plan = Plan::compile(&ok(name, &capturing(body)));
        (
            plan.nodes
                .iter()
                .any(|n| matches!(n.op, Op::ArrangeBy { .. })),
            plan.nodes.iter().find_map(|n| match n.op {
                Op::Join { matched, .. } => Some(matched),
                _ => None,
            }),
        )
    };

    // The rows: what the filter evaluated to, so the group is built.
    assert_eq!(
        answers(
            "grouped.beck",
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: str(unwrap_or(list_get(filter_list(map_values(s.items), \
                                                                              lambda v: str(v) == \
                                                                              k), 0), 0)))\n",
        ),
        (true, Some(Matching::Group)),
        "a loop that filters another collection by an equality is a join over an index it needs \
         built, answering with the group its body reads"
    );

    // The count: the same filter, measured rather than read, so no group is built (§99.9 item 6).
    assert_eq!(
        answers(
            "counted.beck",
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: str(list_len(filter_list(map_values(s.items), \
                                                                    lambda v: str(v) == k))))\n",
        ),
        (true, Some(Matching::Count)),
        "the same shape under a `list_len` is the group's size, which the join answers from a tally"
    );

    // The same loop with `<` where the `==` was. There is no key to arrange by, so there is no
    // index — and the reason has to name *that* rather than one of the conditions the equality
    // shape shares with a `map_get`.
    let text = cost(&ok(
        "not-an-equality.beck",
        &capturing(
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: str(list_len(filter_list(map_values(s.items), \
                                                                    lambda v: str(v) < k))))\n",
        ),
    ));
    let line = text
        .lines()
        .find(|l| l.contains("not read as a join"))
        .unwrap_or_else(|| panic!("the loop pays for a filter and does not say why:\n{text}"));
    assert!(
        line.contains("no key to arrange the collection by"),
        "the reason names the wrong condition: {line:?}"
    );
}

/// **Several lookups in one body are several joins**, chained — not a refusal.
///
/// `corpus/33-awareness.beck` renders a person's whereabouts *and* their note, so its loop looks up
/// in two collections. Refusing that shape would leave the capture in place and the whole collection
/// reconsidered per event, which is the cost the operator exists to remove — and a row showing two
/// related things is an ordinary page rather than a corner.
///
/// One of the two is a lookup into the **awareness roster**, which
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.5 decision 3 expected
/// would have to be refused, because a roster moves when `seq` does not. It does not have to be:
/// the index and the left side advance together inside one subscriber's engine, and everything
/// downstream of the roster was already per-subscriber. The plan shape is asserted here and the
/// *answers* are the differential's, which drives the shared dataflow as well as a standalone
/// engine.
#[test]
fn a_loop_that_looks_up_twice_becomes_two_joins_and_captures_nothing() {
    use beck_core::plan::{Op, Plan, Relate};

    let placed = corpus("33-awareness.beck");
    let plan = Plan::compile(&placed);
    let joins = plan
        .nodes
        .iter()
        .filter(|n| matches!(n.op, Op::Join { .. }))
        .count();
    assert_eq!(
        joins,
        2,
        "its loop looks up in two collections, so it is two joins: {:?}",
        plan.nodes.iter().map(|n| n.op.name()).collect::<Vec<_>>()
    );

    // The point of the chain rather than a count of it: nothing is reapplied per event any more.
    let text = cost(&placed);
    assert!(
        !text.contains("downstream of the state"),
        "a capture that moves per event survived the rewrite:\n{text}"
    );
    // And it was there to remove — otherwise this passes on a program that never had the shape.
    let before = beck_core::plan::cost_report(&Plan::compile_with(&placed, Relate::Refuse));
    assert!(
        before.contains("downstream of the state"),
        "33-awareness.beck no longer has the shape this gate is about:\n{before}"
    );
}

/// A program whose page is a list built by `rows`, so that what `rows`'s lambda captures is the
/// only thing that differs between the three above.
fn capturing(rows: &str) -> String {
    format!(
        r#"
model State:
    items: Map[Str, Int]

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

{rows}

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            ul:
                for r in rows(s, session):
                    li: r

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, items, validate)
items: Signal[State] = durable(fold(apply_event, State(items={{}}), events))
page: Signal[Html] = per_session(items, view)
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
                // A vertex that applies nothing at all is its own verdict rather than an
                // `Incremental` with an empty explanation — which is what this gate caught the
                // first time a corpus program had one (`docs/48` §48.9).
                Verdict::Trivial => {}
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
