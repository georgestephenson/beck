//! The programs a differential over **views** needs, and the arguments to run them on.
//!
//! Shared for the reason [`super::clofix`] is: three backends are held to these, and a second copy
//! of the programs would be a second opinion about what the subset is.
//!
//! # What these are chosen to catch
//!
//! An `Html` in the arena is the *call* that would build the tree rather than the tree
//! (`beck_llvm::heap::Repr::Html`), so what a differential over one has to look for is not "did the
//! bytes survive" but "does the tree the host bakes out of them equal the tree the evaluator
//! baked". `Value`'s equality on an `Html` includes every structural hash, so a node assembled in a
//! different order fails even when it renders the same.
//!
//! * **An attribute whose value is empty**, which `html_el` *drops*. It is the one rule that makes
//!   the recipe and the tree different lengths, and `class=""` is what every conditional class in
//!   this repository produces on the false branch.
//! * **A key**, which is not an attribute at all: it sets the node's key and is folded into a
//!   different hash accumulator than the attributes are. A recipe that made it an attribute would
//!   render the same and diff differently.
//! * **A handler**, whose command is a record that becomes JSON. It is the case where the deferred
//!   value is an *object* rather than text, and where what renders it is `Value::to_json` — a
//!   function only the host has.
//! * **The order of the attributes**, because the hash is folded in the order they arrive and two
//!   attributes swapped is a tree that renders identically and hashes differently.
//! * **A text node of every shape a value has here** — a `Str`, an `Int`, a `Bool`, a `Float`, a
//!   record, a list — because the repr is a *datum* in this one place, and a wrong index reads the
//!   right word as the wrong thing.
//! * **A child that is already a tree**, which is spliced rather than rendered. That is what makes
//!   a view composable out of functions, and rendering it instead is escaped markup on the page
//!   (`docs/94` §94.4).
//! * **Children built by a loop**, since the list is the shape a real `ui:` block always has.
//! * **The empty element**, whose two lists are empty and whose children hash is the accumulator's
//!   seed.
//! * **A view that is a function of a record**, so the arguments cross *into* the call and the tree
//!   crosses back out.
//! * **A view inside a record**, because a field is a different decode path from a result: an
//!   object frame with a node under it.
//! * **A tree given back in**, which is the direction nothing in a program needs and the boundary
//!   has anyway: the host encodes a baked tree as a recipe whose leaves are text, and the round
//!   trip has to be exact.

#![allow(dead_code)] // each suite uses the half of this it needs

use std::sync::Arc;

use beck_core::Value;

/// Views built out of the five primitives directly, so that what is being compared is the
/// primitive rather than what `ui:` chose to lower to.
pub const VIEWS: &str = r#"
model Card:
    title: Str
    count: Int
    done: Bool
    ratio: Float

union Command:
    Toggle(id: Str)
    Clear

def blank() -> Html:
    return html_el("div", [], [])

def just_text(s: Str) -> Html:
    return html_text(s)

def a_number(n: Int) -> Html:
    return html_text(n)

def a_flag(b: Bool) -> Html:
    return html_text(b)

def a_real(f: Float) -> Html:
    return html_text(f)

def a_record(c: Card) -> Html:
    return html_text(c)

def a_list(xs: list[Int]) -> Html:
    return html_text(xs)

def titled(c: Card) -> Html:
    return html_el("h1", [html_attr("class", "title")], [html_text(c.title)])

# The empty value is dropped by `html_el`, so this element has one attribute or two.
def maybe_done(c: Card) -> Html:
    return html_el("li", [html_attr("class", done_class(c)), html_attr("id", c.title)], [])

def done_class(c: Card) -> Str:
    return "done" if c.done else ""

# Two attributes in a fixed order: swapping them renders the same and hashes differently.
def ordered(c: Card) -> Html:
    return html_el("p", [html_attr("a", c.title), html_attr("b", str(c.count))], [])

def keyed(c: Card) -> Html:
    return html_el("li", [html_key(c.title), html_attr("class", "row")], [])

def keyed_number(c: Card) -> Html:
    return html_el("li", [html_key(c.count)], [])

def handled(c: Card) -> Html:
    return html_el("button", [html_on("click", Toggle(id=c.title))], [html_text("x")])

def handled_nullary(c: Card) -> Html:
    return html_el("button", [html_on("click", Clear)], [])

# A child that is already a tree, spliced rather than rendered.
def wrapped(c: Card) -> Html:
    return html_el("div", [], [html_text(titled(c))])

def nested(c: Card) -> Html:
    return html_el("main", [], [titled(c), html_el("section", [], [handled(c)])])

def rows(xs: list[Int]) -> Html:
    return html_el("ul", [], map_list(xs, lambda n: html_el("li", [], [html_text(n)])))

def attrs_from(xs: list[Int]) -> Html:
    return html_el("div", map_list(xs, lambda n: html_attr("data-n", n)), [])

def whole(c: Card, xs: list[Int]) -> Html:
    return html_el("main", [html_attr("class", done_class(c))], [titled(c), rows(xs), handled(c)])

# A view inside a record, which is a different decode path: an object frame with a node under it.
model Panel:
    body: Html
    title: Str

def panelled(c: Card) -> Panel:
    return Panel(body = titled(c), title = c.title)

# An `Attr` on its own, which is a value a definition may answer with.
def one_attr(c: Card) -> Attr:
    return html_attr("class", c.title)

def one_key(c: Card) -> Attr:
    return html_key(c.title)

def one_handler(c: Card) -> Attr:
    return html_on("click", Toggle(id=c.title))

# The direction nothing in a program needs: a tree crossing *into* a compiled call.
def again(h: Html) -> Html:
    return html_el("div", [], [h])

def beside(h: Html, c: Card) -> Html:
    return html_el("div", [], [h, titled(c)])
"#;

/// The same page written with `ui:`, which is what a program actually contains.
pub const PAGE: &str = r#"
model Todo:
    id: Str
    text: Str
    done: Bool

union Command:
    Toggle(id: Str)
    Delete(id: Str)

def page(todos: list[Todo], left: Int) -> Html:
    return ui:
        main:
            h1: "todos"
            input(placeholder="what needs doing?", on_enter=Toggle(id="$value"))
            ul:
                for t in todos:
                    li(key=t.id, class=done_class(t)):
                        span(on_click=Toggle(id=t.id)): t.text
                        button(on_click=Delete(id=t.id)): "x"
            footer: (str(left) + " remaining")

def done_class(t: Todo) -> Str:
    return "done" if t.done else ""
"#;

/// What the heap still refuses to do with a view, each with the reason a reader is given.
///
/// Two kinds. The three orderings are §93.6's, and the two `Attr` parameters are §93.6's: the
/// boundary is **directional** there, because a handler in the arena keeps its command as a value
/// and a baked tree has already turned one into a pair of strings — so a definition may answer with
/// an `Attr` and may not take one.
///
/// A row here goes red the day it starts compiling, which is the day the row should be deleted and
/// the feature written down.
pub const REFUSED: &str = r#"
model Panel:
    body: Html
    title: Str

def takes_an_attr(a: Attr) -> Html:
    return html_el("b", [a], [])

def takes_a_list_of_attrs(xs: list[Attr]) -> Html:
    return html_el("b", xs, [])

def sorted_views(xs: list[Html]) -> Bool:
    return list_contains(xs, html_text("a"))

def same_panel(a: Panel, b: Panel) -> Bool:
    return a == b

def same_view(a: Html, b: Html) -> Bool:
    return a == b
"#;

pub fn card(title: &str, count: i64, done: bool, ratio: f64) -> Value {
    Value::record(
        "Card",
        None,
        [
            ("title", Value::str_(title)),
            ("count", Value::Int(count)),
            ("done", Value::Bool(done)),
            ("ratio", Value::float(ratio)),
        ],
    )
}

/// The cards every view is built from: a false conditional class, a true one, text that needs
/// escaping, and text that is empty.
pub fn cards() -> Vec<Value> {
    vec![
        card("todos", 0, false, 0.0),
        card("todos", 1, true, 1.5),
        card("", 0, true, -0.0),
        card("<b>&</b>", -1, false, 0.1),
        card("é", i64::MAX, true, f64::INFINITY),
    ]
}

pub fn ints(xs: &[i64]) -> Value {
    Value::List(Arc::new(xs.iter().map(|n| Value::Int(*n)).collect()))
}

pub fn lists() -> Vec<Value> {
    vec![ints(&[]), ints(&[0]), ints(&[1, 2, 3]), ints(&[-1, 0, 1])]
}

pub fn todo(id: &str, text: &str, done: bool) -> Value {
    Value::record(
        "Todo",
        None,
        [
            ("id", Value::str_(id)),
            ("text", Value::str_(text)),
            ("done", Value::Bool(done)),
        ],
    )
}

/// The todo lists the `ui:` page is rendered from.
pub fn todos() -> Vec<Value> {
    vec![
        Value::List(Arc::new(vec![])),
        Value::List(Arc::new(vec![todo("a", "milk", false)])),
        Value::List(Arc::new(vec![
            todo("a", "milk", true),
            todo("b", "<eggs>", false),
        ])),
    ]
}

pub fn singles(xs: &[Value]) -> Vec<Vec<Value>> {
    xs.iter().map(|x| vec![x.clone()]).collect()
}

pub fn with(xs: &[Value], ys: &[Value]) -> Vec<Vec<Value>> {
    xs.iter()
        .flat_map(|x| ys.iter().map(move |y| vec![x.clone(), y.clone()]))
        .collect()
}
