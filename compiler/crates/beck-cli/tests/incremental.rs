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
        .find(|l| l.contains("not read as a relational operator"))
        .unwrap_or_else(|| panic!("the loop pays for a lookup and does not say why:\n{text}"));
    assert!(
        line.contains("not an equi-join on the left row"),
        "the reason names the wrong condition: {line:?}"
    );
    // Beside the cost it explains, rather than somewhere else in the report: a reason a reader has
    // to go looking for is a reason they will not find.
    let (cause, cost_line) = (
        text.lines()
            .position(|l| l.contains("not read as a relational operator")),
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
        .find(|l| l.contains("not read as a relational operator"))
        .unwrap_or_else(|| panic!("the loop pays for a filter and does not say why:\n{text}"));
    assert!(
        line.contains("no key to arrange the collection by"),
        "the reason names the wrong condition: {line:?}"
    );
}

/// **One end of a group is a third question over the same recognition**, and its right side is not
/// an index over the rows at all.
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's other two
/// aggregates. `list_min` and `list_max` over the same `filter_list` — bare, or over a `map_list` of
/// it — compile to [`Op::GroupBy`], which holds one entry per group; so the join above it probes a
/// point rather than a range and there is **no `arrange_by` in the plan at all**. That last half is
/// the one worth asserting: an implementation that built the index and then took the first row of
/// each range would answer correctly, cost the group, and pass every test that only reads the page.
#[test]
fn one_end_of_a_group_is_a_group_by_and_not_an_index_over_its_rows() {
    use beck_core::plan::{Agg, Matching, Op, Plan};

    let shape = |name: &str, body: &str| -> (bool, Option<Agg>, Option<Matching>) {
        let plan = Plan::compile(&ok(name, &capturing(body)));
        (
            plan.nodes
                .iter()
                .any(|n| matches!(n.op, Op::ArrangeBy { .. })),
            plan.nodes.iter().find_map(|n| match n.op {
                Op::GroupBy { agg, .. } => Some(agg),
                _ => None,
            }),
            plan.nodes.iter().find_map(|n| match n.op {
                Op::Join { matched, .. } => Some(matched),
                _ => None,
            }),
        )
    };

    // The smallest row of the group, with no projection: what each row contributes is the row.
    assert_eq!(
        shape(
            "smallest.beck",
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: str(unwrap_or(list_min(filter_list(map_values(s.items), \
                                                                              lambda v: str(v) == \
                                                                              k)), 0)))\n",
        ),
        (false, Some(Agg::Min), Some(Matching::Unique)),
        "the smallest of a group is one entry per group, probed like a `map_get` — not a range \
         over an index of the rows"
    );

    // The largest of a *projection* of the group, which is how the question is usually asked, and
    // the half §99.9 item 6 expected to be the expensive one.
    assert_eq!(
        shape(
            "largest.beck",
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: str(unwrap_or(list_max(map_list(\
                                                  filter_list(map_values(s.items), \
                                                              lambda v: str(v) == k), \
                                                  lambda v: v + 1)), 0)))\n",
        ),
        (false, Some(Agg::Max), Some(Matching::Unique)),
        "the largest of a projection of a group is the same operator reading its own tree from the \
         other end, and costs what the smallest costs"
    );

    // A projection that reads the **loop's** element is not a function of the group, so there is no
    // aggregate to maintain — and the filter under it is still the group the program would
    // otherwise re-scan. The recogniser falls back to the site inside the one that failed rather
    // than refusing the whole body, which is the difference between paying for an index and paying
    // for the collection.
    assert_eq!(
        shape(
            "unmaintainable.beck",
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: str(unwrap_or(list_min(map_list(\
                                                  filter_list(map_values(s.items), \
                                                              lambda v: str(v) == k), \
                                                  lambda v: v + str_len(k))), 0)))\n",
        ),
        (true, None, Some(Matching::Group)),
        "a projection the aggregate cannot maintain took the index down with it — the group is \
         still a group, and refusing the whole body leaves the loop at O(n) per event"
    );
}

/// **A total is the same operator probed as a value rather than as an option**, which is the one
/// place the aggregates differ downstream of the group.
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's last
/// aggregate. `list_sum` over the same `filter_list` compiles to the same [`Op::GroupBy`] the
/// extremes do — no `arrange_by`, one entry per group — and the join above it reads a missing entry
/// as `0` rather than as `None`, because a group with no rows has a sum and has no smallest
/// element. The body says so by not needing `unwrap_or` at all: `list_sum` answers with an `Int`.
#[test]
fn a_total_is_a_group_by_probed_as_a_value_rather_than_an_option() {
    use beck_core::plan::{Agg, Matching, Op, Plan};

    let plan = Plan::compile(&ok(
        "total.beck",
        &capturing(
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: str(list_sum(map_list(\
                                                  filter_list(map_values(s.items), \
                                                              lambda v: str(v) == k), \
                                                  lambda v: v + 1))))\n",
        ),
    ));
    assert!(
        !plan
            .nodes
            .iter()
            .any(|n| matches!(n.op, Op::ArrangeBy { .. })),
        "the total built an index over the group's rows, which is the collection the question was \
         avoiding"
    );
    assert_eq!(
        plan.nodes.iter().find_map(|n| match n.op {
            Op::GroupBy { agg, .. } => Some(agg),
            _ => None,
        }),
        Some(Agg::Sum),
    );
    assert_eq!(
        plan.nodes.iter().find_map(|n| match n.op {
            Op::Join { matched, .. } => Some(matched),
            _ => None,
        }),
        Some(Matching::Total),
        "a total probed as an option would make an account nobody has posted to render nothing \
         where the program says `0`"
    );
}

/// **Two lookups that index the same collection by the same key build one index.**
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.5 decision 4: an
/// index is a second arrangement and the sharing is what keeps a second one from costing a second
/// copy. The sharing key is a string, and `Core` numbers variables **per definition** — so
/// `lambda v: str(v)` and `lambda w: str(w)`, which are the same key, arrived with different
/// numbers and built two identical arrangements. An arrangement is memory per subscriber as well as
/// work per event ([`docs/23`](../../../../docs/23-incremental-views-report.md) §23.14).
///
/// The two lookups here ask *different questions* of the same index — one the group's size, one a
/// row of it — because that is what makes them two joins rather than one common subexpression the
/// front end would have folded before the plan ever saw it.
#[test]
fn two_lookups_by_the_same_key_share_one_index() {
    use beck_core::plan::{Op, Plan};

    let plan = Plan::compile(&ok(
        "twice.beck",
        &capturing(
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return map_list(map_keys(s.items), \
                                 lambda k: str(list_len(filter_list(map_values(s.items), \
                                                                    lambda v: str(v) == k))) + \
                                           str(unwrap_or(list_get(filter_list(\
                                                   map_values(s.items), \
                                                   lambda w: str(w) == k), 0), 0)))\n",
        ),
    ));
    let indexes = plan
        .nodes
        .iter()
        .filter(|n| matches!(n.op, Op::ArrangeBy { .. }))
        .count();
    let joins = plan
        .nodes
        .iter()
        .filter(|n| matches!(n.op, Op::Join { .. }))
        .count();
    assert_eq!(
        (joins, indexes),
        (2, 1),
        "two questions about one collection keyed the same way are two joins over **one** index: \
         {:?}",
        plan.nodes.iter().map(|n| n.op.name()).collect::<Vec<_>>()
    );
}

/// **A filter that is a membership test is the difference; one that merely contains a membership
/// test says so.**
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7. The
/// recognition is deliberately narrower than the join's: a join is read at a *site inside* a body
/// because a loop does other things besides look up, and a filter's predicate **is** the operator,
/// so a predicate that asks a membership question and something else is not this shape.
///
/// Three cases, because a rule that recognised everything would pass the first alone and one that
/// recognised nothing would pass the last two. The middle one is the case §99.9 item 5 is explicit
/// about: a refusal that leaves a program at `O(n)` per event is the defect with a sentence
/// attached, so the sentence has to name **which** condition failed rather than the one the shape
/// happens to share with every other filter in the tree.
#[test]
fn a_predicate_that_is_a_membership_test_is_the_operator_and_one_that_contains_it_is_not() {
    use beck_core::plan::{Op, Plan};

    let restricted = |name: &str, body: &str| -> bool {
        Plan::compile(&ok(name, &capturing(body)))
            .nodes
            .iter()
            .any(|n| matches!(n.op, Op::Restrict { .. }))
    };

    assert!(
        restricted(
            "membership.beck",
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return filter_list(map_keys(s.items), lambda k: map_contains(s.items, k))\n",
        ),
        "a filter whose predicate asks a captured collection whether it holds a key is the \
         intersection by key"
    );

    // The same question with one more clause. The reason has to name *that*, because every other
    // condition this shape could fail is one it shares with an ordinary filter.
    let and_more = ok(
        "and-more.beck",
        &capturing(
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return filter_list(map_keys(s.items), \
                                    lambda k: not str_is_empty(k) and map_contains(s.items, k))\n",
        ),
    );
    assert!(
        !Plan::compile(&and_more)
            .nodes
            .iter()
            .any(|n| matches!(n.op, Op::Restrict { .. })),
        "a predicate that asks a membership question and something else was read as the \
         membership question alone, which is a different filter"
    );
    let text = cost(&and_more);
    let line = text
        .lines()
        .find(|l| l.contains("not read as a relational operator"))
        .unwrap_or_else(|| panic!("the filter pays for a capture and does not say why:\n{text}"));
    assert!(
        line.contains("asks something else as well"),
        "the reason names the wrong condition: {line:?}"
    );

    // And the probe key: a membership test whose key is not a function of the element is not an
    // index probe, so the collection would have to be scanned to answer it.
    let elsewhere = ok(
        "key-elsewhere.beck",
        &capturing(
            "def rows(s: State, session: Session) -> list[Str]:\n    \
                 return filter_list(map_keys(s.items), \
                                    lambda k: map_contains(s.items, session.actor))\n",
        ),
    );
    assert!(
        !Plan::compile(&elsewhere)
            .nodes
            .iter()
            .any(|n| matches!(n.op, Op::Restrict { .. })),
        "a probe key that reads the session rather than the element was read as an index probe"
    );
}

/// **A difference and the intersection beside it are one index and no rows.**
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7, and the two
/// halves of the claim that let a `filter_list` be rewritten at all when a `map_list` is what
/// becomes a join.
///
/// The first is §99.5 decision 4 again: `corpus/38-backorders.beck` asks the stock two opposite
/// questions and they are answered from **one** `map_values`, by the hash-consing that was already
/// there.
///
/// The second is the operator's reason for existing. A [`beck_core::plan::Op::Join`] emits a *row*,
/// so a `filter_list` rewritten into one would need a projection underneath it to give its
/// consumers back the element they read — and that projection is an operator per element, per
/// event, undoing what the rewrite just did. [`beck_core::plan::Op::Restrict`] emits the left
/// element, so the plan gains **no** loop it did not already have: the assertion is that the
/// recognised plan has exactly the per-element operators the refused one has, which is what
/// "no representational change at all" means when it is counted rather than said.
#[test]
fn a_difference_and_the_intersection_beside_it_are_one_index_and_no_rows() {
    use beck_core::plan::{Op, Plan, Presence, Relate};

    let placed = corpus("38-backorders.beck");
    let plan = Plan::compile(&placed);

    let restricts: Vec<(usize, Presence)> = plan
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| match n.op {
            Op::Restrict { keep, .. } => Some((i, keep)),
            _ => None,
        })
        .collect();
    assert_eq!(
        restricts.iter().map(|&(_, k)| k).collect::<Vec<_>>(),
        vec![Presence::In, Presence::NotIn],
        "the program asks the stock whether it holds a key and whether it does not: {:?}",
        plan.nodes.iter().map(|n| n.op.name()).collect::<Vec<_>>()
    );
    let indexes: Vec<usize> = restricts
        .iter()
        .map(|&(i, _)| plan.nodes[i].inputs[1])
        .collect();
    assert_eq!(
        indexes[0],
        indexes[1],
        "the two halves of the partition built two indexes over one collection: {:?}",
        plan.nodes.iter().map(|n| n.op.name()).collect::<Vec<_>>()
    );
    assert!(
        matches!(plan.nodes[indexes[0]].op, Op::MapValues),
        "the index is not the `map_values` arrangement the collection already had"
    );
    for &(i, _) in &restricts {
        let captures = plan.nodes[i].op.funs()[0].captures.len();
        assert_eq!(
            captures, 0,
            "the probe key captured {captures} operators, so it is not a function of the element \
             alone and the operator is not `O(δ)`"
        );
    }

    // The half that says the operator is not a join wearing a different name.
    let loops = |relate: Relate| -> usize {
        Plan::compile_with(&placed, relate)
            .nodes
            .iter()
            .filter(|n| matches!(n.op, Op::MapList { .. } | Op::FlatMap { .. }))
            .count()
    };
    assert_eq!(
        loops(Relate::Recognise),
        loops(Relate::Refuse),
        "recognising the difference added a per-element operator the program did not write, which \
         is the projection a join would need to give a filter's consumers back their element: {:?}",
        plan.nodes.iter().map(|n| n.op.name()).collect::<Vec<_>>()
    );
}

/// **A `distinct` is a lowering, and the count above one does not visit it.**
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7's second
/// half, and the two things about it that are decisions rather than code.
///
/// The first is that there is nothing to recognise: `list_unique` **names** the operator, so this
/// takes no [`beck_core::plan::Relate`] switch where the join and the difference do — nothing is
/// being read out of a shape and no choice is being made on the program's behalf. What bought that
/// is the primitive, and the assertion is the negative one: with the recognition refused the
/// operator is still there, because it never came from a recognition.
///
/// The second is that the operator's output is an **arrangement** and not a value, so `list_len`
/// over it is [`beck_core::plan::Op::Count`] — the arrangement's size, §3.8's "±1 per event, never
/// by recount" — rather than a recompute that would have to build the list to measure it. A
/// `distinct` that published a `Value::List` would still render the right page and would cost the
/// distinct values on every event that touched the collection.
#[test]
fn a_distinct_is_an_arrangement_and_the_count_above_it_is_not_a_recompute() {
    use beck_core::plan::{Op, Plan, Relate};

    let placed = corpus("39-topics.beck");
    let plan = Plan::compile(&placed);
    let at: Vec<usize> = (0..plan.nodes.len())
        .filter(|&i| matches!(plan.nodes[i].op, Op::Distinct))
        .collect();
    assert_eq!(
        at.len(),
        1,
        "the program asks for the values in use once: {:?}",
        plan.nodes.iter().map(|n| n.op.name()).collect::<Vec<_>>()
    );
    assert!(
        plan.nodes[at[0]].op.is_arrangement(),
        "a `distinct` that published a value rather than an arrangement would cost its distinct \
         values on every event that touched the collection"
    );
    assert!(
        plan.nodes
            .iter()
            .any(|n| matches!(n.op, Op::Count) && n.inputs == vec![at[0]]),
        "the count of the values in use is not the arrangement's size: {:?}",
        plan.nodes.iter().map(|n| n.op.name()).collect::<Vec<_>>()
    );

    // The negative half: this operator does not come from the recogniser, so refusing the
    // recogniser does not remove it.
    assert!(
        Plan::compile_with(&placed, Relate::Refuse)
            .nodes
            .iter()
            .any(|n| matches!(n.op, Op::Distinct)),
        "`Relate::Refuse` removed the operator, so it is a recognition after all and owes the \
         off switch docs/08 §8.3 item 8 asks of one"
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

/// **A class written the way the styling design asks is pointwise; a join over a collection is
/// not.** One primitive, two verdicts, decided by what it is applied to.
///
/// [`docs/104`](../../../../docs/104-styling-and-the-component-library.md) §104.4 asks programs to
/// write a class as a *list of alternatives*, because a list can be enumerated and a concatenation
/// cannot. The `ui:` lowering makes that list a `str_join`, and this analysis used to block on the
/// **name** — so the shape the design document's own example recommends reported `recompute`.
///
/// It was wrong, and the plan is what says so: a program written the documented way and the same
/// program written around it compile to **byte-identical plans**. The `str_join` is inside the
/// per-element function of a maintained `map_list`, applied to what moved and nothing else, which
/// is what every "pointwise" row in [`RULES`] means. The report said a page had stopped being
/// maintained when nothing about it had changed, and the response to that report was to make the
/// sketch worse to read.
///
/// So the rule is about the *argument*: a join of a fixed list of parts is a function of those
/// parts; a join over a maintained collection reduces it to one string and has no delta rule to
/// have. Both directions are asserted here because a rule that looked at the name alone would give
/// these two the same answer, whichever answer it gave.
#[test]
fn a_join_of_a_fixed_list_is_pointwise_and_a_join_over_a_collection_is_not() {
    // The shape has to be in a program, or neither half of this has anything to be about. The
    // sketch carries §104.4's own example: `class=[…, done_class(t)]`.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/todo.beck"),
    )
    .expect("the sketch is readable");
    assert!(
        src.contains(r#"class=["flex", "gap-2", "items-baseline", done_class(t)]"#),
        "the sketch no longer writes its class as a list with a name in it, so nothing in the tree \
         is the shape docs/104 §104.4 asks for"
    );
    let p = corpus("../examples/todo.beck");
    assert_eq!(
        verdicts(&p).get("page"),
        Some(&Verdict::Incremental),
        "the shape docs/104 §104.4 recommends reports as a recompute"
    );
    // …and the rule is *stated* rather than merely not-blocking, which is what every other
    // operation in the report gets.
    let a = assess(&p);
    let page = a.iter().find(|a| a.label.as_ref() == "page").expect("page");
    let join = page
        .ops
        .iter()
        .find(|(op, _)| op.name() == "str_join")
        .unwrap_or_else(|| {
            panic!(
                "the sketch's class list did not reach the report: {:?}",
                page.ops
            )
        });
    assert!(
        join.1.contains("fixed list"),
        "the rule does not say what makes it pointwise: {:?}",
        join.1
    );

    // The other side, in the same helper so nothing but the argument differs. A join over a
    // maintained collection is a reduction, and the reason has to name *that* rather than the
    // primitive — a program told "`str_join` has no delta rule" would go looking for the wrong fix.
    let over_a_collection = ok(
        "joined.beck",
        &with_view(
            "def joined(s: State) -> Pick:\n    \
                 return Pick(label=str_join(map_list(map_keys(s.items), lambda k: k), \", \"))",
            "chosen: Signal[Pick] = signal_map(items, joined)",
        ),
    );
    let v = verdicts(&over_a_collection);
    let Some(Verdict::Recompute { because }) = v.get("chosen") else {
        panic!("a join that reduces a collection to one string is not maintainable: {v:?}");
    };
    assert!(
        because.contains("over a collection"),
        "the reason names the primitive rather than the case: {because:?}"
    );

    // And the same program with a fixed list where the collection was: everything else is
    // identical, so the verdict can only have come from the argument.
    let over_a_list = ok(
        "fixed.beck",
        &with_view(
            "def joined(s: State) -> Pick:\n    \
                 return Pick(label=str_join([\"items\", str(map_len(s.items))], \" \"))",
            "chosen: Signal[Pick] = signal_map(items, joined)",
        ),
    );
    assert_eq!(
        verdicts(&over_a_list).get("chosen"),
        Some(&Verdict::Incremental),
        "a join of a fixed list is a function of its parts"
    );
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

/// **No program in the tree reapplies a collection on every event.**
///
/// [`docs/99`](../../../../docs/99-the-data-tier-means-of-combination.md) §99.3's sweep, as a
/// standing property rather than a thing somebody re-runs. A per-element function that reads the
/// accumulator is a *different function* after every event, so §23.13's rebuild rule reapplies it
/// to every element — the nested-loop join §99 exists to remove, invisible until the collection is
/// large because nothing about it fails.
///
/// **It was a hand sweep three times and went stale in between.** The second reading found three
/// such sites; the third found a fourth that had arrived with `awareness(f)` one change after the
/// second and that nothing re-ran the sweep to catch (§99.3). That is
/// [`docs/08`](../../../../docs/08-roadmap.md) §8.5.6's third decay direction — a quoted figure
/// going stale because the tree grew under it — demonstrated one commit after the paragraph
/// describing it. A number in a document cannot notice a new program; this can.
///
/// The corpus **and** the examples, because the examples are where the sketches live and
/// `examples/board.beck` was the last site to close. A program that legitimately cannot be
/// recognised is not forbidden — §99.6's rule for that case is "compile it the slow way and say
/// so" — but it may not arrive *silently*, which is the whole difference between this gate and the
/// sweep it replaces.
#[test]
fn no_program_in_the_tree_reapplies_a_collection_per_event() {
    use beck_core::plan::Plan;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut checked = 0usize;
    let mut guilty: Vec<String> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for dir in ["corpus", "examples"] {
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(root.join(dir))
            .expect("the directory is there")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "beck"))
            .collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("a file name")
                .to_string();
            let src = std::fs::read_to_string(&path).expect("a program");
            let (placed, diags, map) = beck_core::compile_or_library_str(&name, &src);
            assert!(!diags.has_errors(), "{dir}/{name}:\n{}", diags.render(&map));
            let placed = placed.unwrap_or_else(|| panic!("{dir}/{name} did not compile"));
            // A library has no merge point, so its roles are placeholders and it has no view to
            // plan. `Placed::is_application` is the question rather than the shape of an error,
            // because a library is not a failure.
            if !placed.is_application() {
                continue;
            }
            checked += 1;
            let plan = Plan::compile(&placed);
            for id in plan.reapplied_per_event() {
                guilty.push(format!(
                    "{dir}/{name} #{id} {} — `beck explain cost {dir}/{name}` says which condition \
                     refused it",
                    plan.nodes[id].op.name()
                ));
            }
            let without = Plan::compile_with(&placed, beck_core::plan::Relate::Refuse);
            for id in without.reapplied_per_event() {
                refused.push(format!("{dir}/{name} #{id}"));
            }
        }
    }
    assert!(
        guilty.is_empty(),
        "a per-element function captured the accumulator, so its whole collection is reapplied on \
         every event (docs/99 §99.3):\n  {}\n\nEither the shape is one §99.6 should recognise and \
         does not — teach the recogniser — or it is one it cannot, in which case say so here \
         rather than letting it arrive silently.",
        guilty.join("\n  ")
    );
    // The control, because a gate that checked nothing would pass loudest of all.
    assert!(
        checked >= 40,
        "only {checked} programs had a plan to check, which is fewer than this tree has"
    );
    // And the evidence that it *can* go red, carried by the green run rather than promised by it
    // (§99.9 item 1's pattern): with the recognition switched off, the sites come back. A sweep
    // that reports "none" says nothing on its own — this is what makes the zero above a
    // difference rather than an assertion.
    assert!(
        !refused.is_empty(),
        "`Relate::Refuse` left no operator reapplying its collection per event, so this gate \
         cannot distinguish an engine that recognises these shapes from one that does not"
    );
    println!(
        "{checked} programs planned: 0 reapply a collection per event, against {} sites in {} \
         programs with the recognition switched off",
        refused.len(),
        refused
            .iter()
            .map(|s| s.split(' ').next().unwrap_or(s))
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
}
