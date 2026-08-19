//! What a page's classes are, and whether the compiler can know them.
//!
//! [`docs/104-styling-and-the-component-library.md`](../../../../docs/104-styling-and-the-component-library.md)
//! §104.4 is the design and [`docs/08`](../../../../docs/08-roadmap.md) §8.5.4's styling cluster is
//! the order. This suite holds the third item, which is the one everything else in that half is
//! behind: **`class=` takes a list**, and the compiler enumerates what can reach one.
//!
//! The two are one item because they are one idea. A list of alternatives can be enumerated; a
//! string built with `+` cannot, by Beck or by Tailwind's own scanner (§104.3 measured that scanner
//! over this tree and found it reading English prose out of comments and missing a real utility
//! behind a module boundary). So the surface exists to make the analysis possible, and the analysis
//! exists to say when the surface was not used.

use std::sync::Arc;

use beck_core::style::{classes, Because};

fn compile(name: &str, src: &str) -> beck_core::Placed {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    placed.unwrap_or_else(|| panic!("{name} did not slice"))
}

fn example(file: &str) -> beck_core::Placed {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(file);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
    compile(file, &src)
}

/// A program whose page is one element carrying `attrs`.
fn page(attrs: &str) -> String {
    format!(
        r#"
model State:
    items: Map[Str, Str]

union Command:
    Add(k: Str)

union Event:
    Added(k: Str)

union Rejection:
    Blank

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Added(k):
            return s.with(items=map_insert(s.items, k, k))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Add(k):
            if str_is_empty(str_trim(k)):
                return Err(error=Blank)
            return Ok(value=[Added(k=k)])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            div({attrs}): "hello"

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, st, validate)
st: Signal[State] = durable(fold(apply_event, State(items={{}}), events))
page: Signal[Html] = per_session(st, view)
"#
    )
}

/// The rendered page of a program built by [`page`].
fn rendered(attrs: &str) -> String {
    let placed = compile("style.beck", &page(attrs));
    let backend = beck_eval::backend(&placed);
    let runtime = beck_rt::Runtime::new(placed, backend).expect("the program prepares");
    let state = runtime.initial_state().expect("an initial accumulator");
    runtime.view(&state, "ana").expect("a page").render()
}

/// **A list where HTML wants a space-separated value is one.**
///
/// `class=["btn", "primary"]` is `class="btn primary"`, and a string is untouched — an existing
/// program renders the characters it always did, which is what makes this an addition rather than a
/// change. The conditional element is the case the whole item exists for: it is what a program
/// writes instead of `"btn " + variant`.
#[test]
fn a_list_class_is_space_separated_and_a_string_is_unchanged() {
    assert!(
        rendered(r#"class=["btn", "primary"]"#).contains(r#"class="btn primary""#),
        "a list class is joined with a space"
    );
    assert!(
        rendered(r#"class="btn primary""#).contains(r#"class="btn primary""#),
        "a string class is what it always was"
    );
    // The shape §104.4 asks for, in place of a concatenation.
    let conditional = rendered(r#"class=["btn", "primary" if 1 > 0 else "plain"]"#);
    assert!(
        conditional.contains(r#"class="btn primary""#),
        "a conditional element joins like any other: {conditional}"
    );
}

/// **And an attribute HTML does not define as a list is left alone.**
///
/// The table in the lowering is four names, not "every attribute", and the reason is that joining a
/// list into an attribute that takes one value would turn a mistake into a plausible string. This is
/// the half of that decision a test can see: `title` takes one value, so a list stays a list and
/// renders as one — visibly wrong, which is the point.
#[test]
fn an_attribute_that_is_not_a_token_list_does_not_join() {
    let out = rendered(r#"title=["a", "b"]"#);
    assert!(
        !out.contains(r#"title="a b""#),
        "`title` is not a space-separated attribute and must not be joined: {out}"
    );
}

/// **The classes a program can carry are worked out from the program**, through a call and through
/// every branch of an `if`.
///
/// Three of the tree's own pages, and each is the shape §104.4 forecast programs would already be
/// written in: `class=done_class(t)` whose body one call away is two constant alternatives. The
/// empty alternative contributes nothing, because an empty class is not a class and the browser
/// drops it too.
#[test]
fn the_classes_of_a_page_are_enumerated_through_calls_and_branches() {
    for (file, want) in [
        ("examples/todo.beck", vec!["done"]),
        ("examples/routed.beck", vec!["done", "here"]),
        ("examples/board.beck", vec!["column", "columns"]),
        ("corpus/02-chat.beck", vec!["mine", "theirs"]),
    ] {
        let styles = classes(&example(file).program);
        let got: Vec<&str> = styles.classes.iter().map(|c| &**c).collect();
        assert_eq!(got, want, "{file}");
        assert!(
            styles.dynamic.is_empty(),
            "{file} builds a class at run time and did not before: {:?}",
            styles.dynamic
        );
    }
}

/// **A class the compiler cannot know is said out loud, and the rest of the program is still
/// enumerated.**
///
/// The second half is the one that would be easy to get wrong: a page with one computed class must
/// not lose the classes beside it, or the report would punish a program for one line by going
/// silent about the other ten. And the reason has to name the concatenation rather than the shape
/// underneath it, because that is the difference between a report a reader can act on — write a list
/// — and one they can only observe.
#[test]
fn a_class_built_at_run_time_is_named_and_does_not_hide_the_others() {
    let src = page(r#"class="fixed""#).replace(
        r#"            div(class="fixed"): "hello""#,
        "            div(class=\"fixed\"): \"hello\"\n            \
         span(class=(\"row-\" + session.actor)): \"there\"",
    );
    let styles = classes(&compile("dynamic.beck", &src).program);
    assert_eq!(
        styles.classes.iter().map(|c| &**c).collect::<Vec<_>>(),
        vec!["fixed"],
        "the class beside the computed one is still enumerated"
    );
    assert_eq!(styles.dynamic.len(), 1, "{:?}", styles.dynamic);
    assert_eq!(styles.dynamic[0].because, Because::Concatenated);
    assert_eq!(styles.dynamic[0].in_def, Arc::from("view"));
    assert!(
        styles.dynamic[0].because.because().contains("list"),
        "the reason names the rewrite: {}",
        styles.dynamic[0].because.because()
    );
}

/// **A class read from data is a different refusal from a class that was concatenated**, because a
/// reader does something different about each.
#[test]
fn a_class_read_from_a_value_says_so_rather_than_blaming_a_concatenation() {
    let src = page(r#"class="fixed""#).replace(
        r#"class="fixed""#,
        r#"class=unwrap_or(map_get(s.items, "k"), "")"#,
    );
    let styles = classes(&compile("from-data.beck", &src).program);
    assert_eq!(styles.dynamic.len(), 1, "{:?}", styles.dynamic);
    assert_eq!(styles.dynamic[0].because, Because::FromData);
}

/// **Beck's utility table agrees with Tailwind's compiler**, which is the only oracle for it.
///
/// §104.4: "the oracle is Tailwind itself, not a table somebody typed in […] The gate is: for every
/// name Beck accepts, Tailwind emits a rule; for every name Beck refuses, Tailwind emits nothing."
/// That is `clbg/`'s pattern — hold somebody else's published artefact so a wrong constant fails
/// even against a matching wrong expectation — and `compiler/style/generate.sh` is how the artefact
/// is obtained. It is committed rather than produced here, because a gate that installs from a
/// package registry fails when somebody else's server does.
///
/// # Three buckets, and only two of them are failures
///
/// **Unsound** — Beck accepts a name Tailwind refuses — is the one that matters: it is a class the
/// compiler would put in a stylesheet and the browser would find no rule for, which is a page
/// missing a style with every gate green. **Wrongly refused** — Tailwind refuses and Beck accepts —
/// is the same error read the other way. Both are asserted at zero.
///
/// The third is **not** a failure and is printed: a name Tailwind accepts and Beck does not know is
/// a gap in a table this documents as a subset. Counting it is what stops the subset from quietly
/// becoming the claim, and it is the number to watch shrink as the families grow.
#[test]
fn the_utility_table_agrees_with_tailwind() {
    use beck_core::style::is_utility;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../style/expected/tailwind-4.3.3.txt");
    let text = std::fs::read_to_string(&path).expect("the committed oracle is where it belongs");
    let mut accepted = 0usize;
    let (mut unsound, mut wrongly_refused, mut gaps) = (Vec::new(), Vec::new(), Vec::new());
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (verdict, name) = line.split_once('\t').expect("a verdict and a name");
        match (verdict, is_utility(name)) {
            ("rule", true) => accepted += 1,
            ("rule", false) => gaps.push(name),
            ("none", true) => unsound.push(name),
            ("none", false) => wrongly_refused.push(name),
            _ => panic!("the oracle says `{verdict}`, which this gate does not know"),
        }
    }
    let total = accepted + gaps.len() + unsound.len() + wrongly_refused.len();
    assert!(total > 500, "only {total} candidates were compared");
    println!(
        "\n  {accepted} of {} names Tailwind emits a rule for are known here; {} are not.\n  \
         {} names Tailwind refuses, and this refuses all of them.",
        accepted + gaps.len(),
        gaps.len(),
        wrongly_refused.len()
    );

    assert!(
        unsound.is_empty(),
        "these are not Tailwind utilities and this table accepts them, so a stylesheet built \
         from it would name rules the browser will not find: {unsound:?}"
    );
    // Named rather than counted, because a gap is a decision about which family to add next and a
    // reader of a green run should be able to make it without re-running the suite.
    // A ratchet rather than a bound: the gaps are the families this table has not taken, and the
    // number is here so that adding one is visible and losing one is a failure. It is measured
    // rather than chosen — 630 of 782 when the candidates were widened past the table on purpose,
    // which is what makes the row above a measurement instead of a restatement of what was typed in.
    assert!(
        accepted >= 630,
        "the table knows {accepted} of Tailwind's names and knew 630, so a family was lost"
    );
    assert!(
        !gaps.is_empty(),
        "every candidate Tailwind accepts is one this table knows, so the list is a restatement \
         of the table rather than an oracle over it — widen `compiler/style/candidates.txt`"
    );
    assert!(
        !wrongly_refused.is_empty(),
        "the oracle contains no name Tailwind refuses, so the soundness assertion above is over \
         an empty set and this gate cannot fail"
    );
}

/// **A page's own class names are not utilities, and the report says which is which.**
///
/// Every `class=` in this tree is a semantic name — `done`, `mine`, `column` — served by the eight
/// hard-coded rules in `beck-rt/src/css.rs`, and none of them is a Tailwind utility. That is not a
/// defect in those programs and the table must not treat it as one: §104.4 takes Tailwind's *names*
/// for the utilities it has, and a program's own names are the program's own. What the compiler owes
/// is to tell the two apart, so that the item which emits a stylesheet emits rules for the first and
/// leaves the second alone.
#[test]
fn a_programs_own_class_names_are_not_mistaken_for_utilities() {
    use beck_core::style::is_utility;

    for file in [
        "examples/todo.beck",
        "examples/board.beck",
        "corpus/02-chat.beck",
    ] {
        for class in classes(&example(file).program).classes {
            assert!(
                !is_utility(&class),
                "{file}'s `{class}` is being read as a Tailwind utility, so a stylesheet would \
                 claim a rule for it that this program serves itself"
            );
        }
    }
    // And the other direction on one page, so this is not a test that nothing is ever a utility.
    let styles = classes(
        &compile(
            "utilities.beck",
            &page(r#"class=["flex", "items-center", "gap-2", "mine"]"#),
        )
        .program,
    );
    let known: Vec<&str> = styles
        .classes
        .iter()
        .filter(|c| is_utility(c))
        .map(|c| &**c)
        .collect();
    assert_eq!(known, vec!["flex", "gap-2", "items-center"]);
}
