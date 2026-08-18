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
