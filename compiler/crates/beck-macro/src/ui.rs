//! The `ui:` macro — a typed DOM tree from an indented block.
//!
//! [`docs/02-syntax.md`](../../../../../docs/02-syntax.md) §2.9 settles this: "**a `ui:` macro
//! producing a typed DOM tree** vs. JSX-like literal syntax. The macro keeps the surface small and
//! is implementable by users for other targets (terminal UI, native). Its output is the Hiccup
//! lineage the original sketch used — `[:main [:h1 "todos"] ...]` maps 1:1 onto the `ui:` block's
//! `Node` tree, so the sketch's pages *are* these pages."
//!
//! So this:
//!
//! ```text
//! ui:
//!     main:
//!         h1: "todos"
//!         ul:
//!             for t in todos:
//!                 li(key=t.id, class=done_class(t)):
//!                     span(on_click=Toggle(id=t.id)): t.text
//! ```
//!
//! is `(main (h1 "todos") (ul ...))` in the sketch's notation, and lowers to calls on five runtime
//! primitives — `html_el`, `html_text`, `html_attr`, `html_on`, `html_key` — plus `map_list` and
//! `concat_lists` for the loops. Nothing here is DOM mutation: §4.2 requires UI trees to stay
//! symbolic so the same value can be server-side rendered, diffed, or (Phase 3) compiled for the
//! client.
//!
//! Handlers become *declarative attributes*: `on_click=Toggle(id=t.id)` carries a serialised
//! command constructor, so §5.1's "no user JavaScript runs in Mode A at all" holds by construction.

use std::collections::BTreeSet;

use beck_diag::{Diagnostic, Diagnostics, Span};
use beck_syntax::{sym, Node};

use crate::str_lit;
use crate::vocabulary;

/// Expand `(ui (kw do (quote (do …))))` into an `Html`-valued expression.
pub fn expand_ui(call: &Node, diags: &mut Diagnostics) -> Node {
    let span = call.span();
    let Some(block) = block_of(call) else {
        diags.push(
            Diagnostic::error("B0210", "`ui` needs an indented block", span)
                .with_primary_label("write `ui:` followed by an element"),
        );
        return unit(span);
    };

    let roots: Vec<&Node> = block.args.iter().collect();
    match roots.len() {
        1 => node_expr(roots[0], diags),
        0 => {
            diags.push(
                Diagnostic::error("B0211", "`ui` block is empty", span)
                    .with_primary_label("a view must produce exactly one root element"),
            );
            unit(span)
        }
        _ => {
            diags.push(
                Diagnostic::error(
                    "B0212",
                    "`ui` block has more than one root",
                    roots[1].span(),
                )
                .with_primary_label("a second root element")
                .with_note("an Html value is a single tree; wrap these in one element")
                .with_fix("put them inside a `div:` or `main:` block"),
            );
            node_expr(roots[0], diags)
        }
    }
}

/// The `do=quote(...)` argument the block rule attached, if there is one.
fn block_of(call: &Node) -> Option<&Node> {
    let last = call.args.last()?;
    if !last.is_form(sym::KW_ARG) || last.args.len() != 2 {
        return None;
    }
    if last.args[0].as_var().map(|s| s.as_str()) != Some("do") {
        return None;
    }
    let quoted = &last.args[1];
    if !quoted.is_form(sym::QUOTE) || quoted.args.len() != 1 {
        return None;
    }
    Some(&quoted.args[0])
}

fn unit(span: Span) -> Node {
    Node::sym("unit", span)
}

/// The attribute that turns one of [`accessibility`]'s three checks off, with a reason.
///
/// [`docs/12`](../../../../../docs/12-standards-and-conformance.md) §12.4 asked for
/// `@a11y(exempt, reason=…)`. It is an *attribute* instead because a `ui:` block's statements are
/// element calls rather than declarations, so an annotation there would be new syntax in the
/// parser for one escape hatch — and the tree already carries keyword arguments. It is stripped
/// rather than emitted: the page must not carry it to a browser.
const EXEMPT: &str = "a11y_exempt";

/// One statement of a `ui` block as a single `Html` expression.
fn node_expr(n: &Node, diags: &mut Diagnostics) -> Node {
    let span = n.span();
    if let Some(tag) = element_tag(n) {
        let mut attrs = Vec::new();
        // The names as HTML spells them, for the checks below. Collected here rather than derived
        // from `attrs` afterwards, because by then each one is an `html_attr` form and the question
        // "did somebody write `alt`" would be asked of generated code.
        let mut written: BTreeSet<String> = BTreeSet::new();
        let mut exempt = false;
        for a in &n.args {
            if a.is_form(sym::KW_ARG) && a.args.len() == 2 {
                let name = a.args[0].as_var().map(|s| s.as_str().to_string());
                match name.as_deref() {
                    Some("do") => continue,
                    Some(EXEMPT) => {
                        exempt = true;
                        continue;
                    }
                    Some(name) => {
                        written.insert(name.replace('_', "-"));
                    }
                    None => {}
                }
                attrs.push(attr_expr(a, &tag, diags));
            }
        }
        let children = match block_of(n) {
            Some(block) => children_expr(&block.args, diags, span),
            None => Node::form(sym::LIST, vec![], span),
        };
        if !exempt {
            let has_children = block_of(n).is_some_and(|b| !b.args.is_empty());
            accessibility(&tag, &written, has_children, span, diags);
        }
        return Node::form(
            "html_el",
            vec![
                str_lit(&tag, span),
                Node::form(sym::LIST, attrs, span),
                children,
            ],
            span,
        );
    }
    // Anything that is not an element is a text node.
    Node::form("html_text", vec![n.clone()], span)
}

/// [`docs/12`](../../../../../docs/12-standards-and-conformance.md) §12.4's first three checks, over
/// the tree `ui:` already builds.
///
/// The design claim §12.4 keeps is that a typed tree makes accessibility *checkable at compile
/// time* in a way a template language cannot match — and it stayed a claim, because "checkable is
/// not checked". These are the three it names: an `img` with no alt text, a `button` with no
/// accessible name, and a form control with no label. Each is an error rather than a warning,
/// because a warning on a page nobody can use is a page nobody can use.
///
/// Which element needs what is [`vocabulary::NAMING`] rather than three tag names written here,
/// which is why the vocabulary was scheduled in front of these: a tree that accepted `on_keydown`
/// and `cls=` in silence could not honestly carry an accessibility claim, and a check that matched
/// a misspelled tag would never fire and no test over correct programs could notice.
///
/// # What each one can see, and the one it cannot
///
/// A compile-time check knows the *shape* of the tree and not the values in it, so each is written
/// to fire on an absence rather than on a value: `alt=""` is HTML's own spelling for a decorative
/// image and is accepted, and a `button` with any child at all is assumed to name itself, because
/// whether an expression renders to empty text is not a question this stage can answer.
///
/// The label check has a real hole and it is stated rather than hidden. A control is named by
/// `aria-label`, `aria-labelledby` or `title` — or by a `<label for=…>` elsewhere, which this
/// cannot see, because a `ui:` block composes out of functions and the label may be in another one.
/// So an `id` is accepted as evidence that such a label exists. What that leaves is the case worth
/// having: a control whose only human-readable text is a **`placeholder`**, which is the commonest
/// real failure of WCAG 3.3.2 and is exactly what four programs in this tree were doing.
///
/// What none of them can see is a *user's helper that shares an element's name*. Inside a `ui:`
/// block a lowercase call with keyword arguments is indistinguishable from an element
/// ([`crate::vocabulary`] says why), so `def input(…)` called by name is checked as an `input`.
/// That is the same limit `B0218` already has, and it moves when `ui:` becomes a user-written typed
/// macro rather than a compiler-provided one (D22).
fn accessibility(
    tag: &str,
    written: &BTreeSet<String>,
    has_children: bool,
    span: Span,
    diags: &mut Diagnostics,
) {
    let labelled = || vocabulary::LABELLING.iter().any(|n| written.contains(*n));
    match vocabulary::naming(tag) {
        None => {}
        Some(vocabulary::Naming::Alt) if !written.contains("alt") => {
            diags.push(
                Diagnostic::error("B0219", "this image has no alt text", span)
                    .with_primary_label("a screen reader announces the file name, or nothing")
                    .with_note(
                        "`alt=\"\"` is the right answer for an image that carries no meaning — it \
                         is HTML's own way of saying so, and it is accepted here",
                    )
                    .with_fix(format!(
                        "add `alt=\"…\"`, or `{EXEMPT}=\"…\"` with a reason"
                    )),
            );
        }
        Some(vocabulary::Naming::TextOrLabel) if !has_children && !labelled() => {
            diags.push(
                Diagnostic::error("B0220", "this button has no accessible name", span)
                    .with_primary_label("nothing announces what it does")
                    .with_note(
                        "a button is named by its own text, or by `aria_label=` when it has none \
                         — an icon button is the usual case",
                    )
                    .with_fix(format!(
                        "give it text, `aria_label=\"…\"`, or `{EXEMPT}=\"…\"` with a reason"
                    )),
            );
        }
        Some(vocabulary::Naming::Label) if !labelled() && !written.contains("id") => {
            diags.push(
                Diagnostic::error("B0221", format!("this `{tag}` has no label"), span)
                    .with_primary_label(if written.contains("placeholder") {
                        "a placeholder is not a label — it disappears as soon as somebody types"
                    } else {
                        "nothing announces what this control is for"
                    })
                    .with_note(
                        "`id=` is accepted as evidence of a `label(for=…)` elsewhere, because a \
                         `ui:` block composes out of functions and this check sees one at a time",
                    )
                    .with_fix(format!(
                        "add `aria_label=\"…\"`, an `id=` with a `label(for=…)`, or \
                         `{EXEMPT}=\"…\"` with a reason"
                    )),
            );
        }
        Some(_) => {}
    }
}

/// A statement is an element when its head is a symbol that is not one of the control forms.
fn element_tag(n: &Node) -> Option<String> {
    if !n.applied {
        return None;
    }
    let head = n.head_name()?;
    if matches!(
        head,
        sym::FOR
            | sym::IF
            | sym::LET
            | sym::VAR
            | sym::DO
            | sym::MATCH
            | sym::RETURN
            | sym::LIST
            | sym::RECORD
            | sym::MAP
            | sym::CALL
            | sym::DOT
            | sym::QUOTE
    ) {
        return None;
    }
    // An element's arguments are all keyword arguments — `li(key=…)`. A call with positional
    // arguments is an ordinary function call producing text or Html, e.g. `row(t)`.
    if !n.args.iter().all(|a| a.is_form(sym::KW_ARG)) {
        return None;
    }
    if !head
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    Some(head.to_string())
}

/// One `name=value` on an element, and the two places a name is held to a vocabulary.
///
/// The check is here rather than beside the table because what it needs is a *span*: a name that
/// does not exist is a diagnostic pointing at where somebody wrote it, and everything the reader
/// needs — what it might have meant, and what the alternative is — is in the message rather than
/// in a rule they have to go and read. [`crate::vocabulary`] is where the names live and why.
fn attr_expr(kw: &Node, tag: &str, diags: &mut Diagnostics) -> Node {
    let span = kw.span();
    let name = kw.args[0]
        .as_var()
        .map(|s| s.as_str().to_string())
        .unwrap_or_default();
    let value = kw.args[1].clone();

    if name == "key" {
        return Node::form("html_key", vec![value], span);
    }
    // `on_click=Toggle(id=…)` — a handler is a declarative attribute carrying a command.
    if let Some(event) = name.strip_prefix("on_") {
        if !vocabulary::is_event(event) {
            let known = vocabulary::EVENTS
                .iter()
                .map(|(e, what)| format!("`on_{e}` ({what})"))
                .collect::<Vec<_>>()
                .join(", ");
            let mut d = Diagnostic::error(
                "B0217",
                format!("`{name}` is not an event the client listens for"),
                span,
            )
            .with_primary_label("this would be an attribute wired to nothing")
            .with_note(format!("the client interprets {known}"));
            if let Some(near) = vocabulary::event_suggestion(event) {
                d = d.with_fix(format!("did you mean `on_{near}`?"));
            }
            diags.push(d);
            return Node::form("html_on", vec![str_lit(event, span), value], span);
        }
        return Node::form("html_on", vec![str_lit(event, span), value], span);
    }
    // `data_b_k` reads better in Python than `data-b-k`, and hyphens are what HTML wants.
    let attr_name = name.replace('_', "-");
    if !vocabulary::is_attribute(&attr_name) {
        let mut d = Diagnostic::error(
            "B0218",
            format!("`{attr_name}` is not an HTML attribute"),
            span,
        )
        .with_primary_label(format!(
            "`{tag}` would carry this to the browser, which ignores it"
        ))
        .with_note(
            "an attribute of your own is spelled `data_…`, which is HTML's own extension point \
             and reaches the page as `data-…`",
        );
        if let Some(near) = vocabulary::suggestion(&attr_name) {
            d = d.with_fix(format!("did you mean `{}`?", near.replace('-', "_")));
        }
        diags.push(d);
    }
    // A list where HTML wants a space-separated value, joined here rather than at the seam.
    //
    // `class=["btn", "primary" if hot else "plain"]` is the shape [`docs/104`] §104.4 asks programs
    // to write, and the reason is not taste: the alternative is `"btn " + …`, and a concatenation
    // is as invisible to a compiler that wants to enumerate the classes a page can carry as it is
    // to Tailwind's scanner. A list of alternatives can be enumerated; a string built at run time
    // cannot, and [`beck_core::style`] is what says which of the two a program wrote.
    //
    // It is done in the **lowering** rather than in `html_attr`, so every backend agrees by
    // construction: what reaches the checker is one `str_join` and there is nothing for an emitter
    // to know about ([`docs/19`] §19.9's seam, honoured by not touching it).
    //
    // A list whose every element is a **literal** is joined here and now: a string decided at
    // compile time should not be assembled at run time, on every element of every page.
    //
    // It used to be more than that, and the reason it is not is worth keeping. A list with a
    // *name* in it stays a `str_join`, and [`beck_core::incremental`] used to block on that name
    // and report the whole view as a recompute — so this fold was the difference between a page
    // the report called maintained and one it did not. It was never the difference between two
    // plans: the join sits inside the per-element function of a maintained `map_list`, applied to
    // what moved and nothing else. That analysis now asks what the join is applied *to*, which is
    // the question, and the shape §104.4's item 4 recommends costs nothing either way.
    let value = match value.head_name() {
        Some(head) if head == sym::LIST && SPACE_SEPARATED.contains(&attr_name.as_str()) => {
            match value
                .args
                .iter()
                .map(Node::as_str_lit)
                .collect::<Option<Vec<&str>>>()
            {
                Some(tokens) => str_lit(tokens.join(" "), span),
                None => Node::form("str_join", vec![value, str_lit(" ", span)], span),
            }
        }
        _ => value,
    };
    Node::form("html_attr", vec![str_lit(attr_name, span), value], span)
}

/// The attributes whose value HTML defines as a space-separated list of tokens.
///
/// Not "every attribute", because most take one value and joining a list into one of those would
/// turn a mistake into a plausible string. These are the ones where a list is what the attribute
/// *means*: `class` and `rel` by the HTML specification, and the two ARIA relationships whose value
/// is a list of ids.
const SPACE_SEPARATED: &[&str] = &["class", "rel", "aria-labelledby", "aria-describedby"];

/// The children of an element: a `list[Html]` built from the block's statements.
///
/// Each statement contributes a *list* so that `for` loops splice rather than nest, and the parts
/// are concatenated. A block with one plain child is emitted as a literal list, with no
/// concatenation call at all.
fn children_expr(stmts: &[Node], diags: &mut Diagnostics, span: Span) -> Node {
    let mut parts: Vec<Node> = Vec::new();
    let mut literal: Vec<Node> = Vec::new();

    for s in stmts {
        if s.is_form(sym::FOR) && s.args.len() == 3 {
            if !literal.is_empty() {
                parts.push(Node::form(sym::LIST, std::mem::take(&mut literal), span));
            }
            let var = s.args[0].clone();
            let seq = s.args[1].clone();
            let body = children_expr(&s.args[2].args, diags, s.span());
            let lambda = Node::form(
                sym::FN,
                vec![
                    Node::form(sym::PARAMS, vec![var], s.span()),
                    Node::form(sym::DO, vec![body], s.span()),
                ],
                s.span(),
            );
            parts.push(Node::form(
                "concat_lists",
                vec![Node::form("map_list", vec![seq, lambda], s.span())],
                s.span(),
            ));
            continue;
        }

        if s.is_form(sym::IF) && s.args.len() >= 2 && s.args[1].is_form(sym::DO) {
            if !literal.is_empty() {
                parts.push(Node::form(sym::LIST, std::mem::take(&mut literal), span));
            }
            let cond = s.args[0].clone();
            let then = children_expr(&s.args[1].args, diags, s.span());
            let alt = match s.args.get(2) {
                Some(a) => children_expr(&a.args, diags, s.span()),
                None => Node::form(sym::LIST, vec![], s.span()),
            };
            parts.push(Node::form(sym::IF, vec![cond, then, alt], s.span()));
            continue;
        }

        literal.push(node_expr(s, diags));
    }

    if !literal.is_empty() {
        parts.push(Node::form(sym::LIST, literal, span));
    }
    match parts.len() {
        0 => Node::form(sym::LIST, vec![], span),
        1 => parts.pop().expect("checked non-empty"),
        _ => Node::form(
            "concat_lists",
            vec![Node::form(sym::LIST, parts, span)],
            span,
        ),
    }
}

/// Scope-annotation stripper, shared by this crate's tests.
#[cfg(test)]
pub(crate) fn tests_strip(s: &str) -> String {
    tests::strip_scopes(s)
}

#[cfg(test)]
mod tests {
    use beck_diag::{Diagnostics, SourceMap};
    use beck_syntax::{parser, print};

    /// Print without hygiene annotations.
    ///
    /// The names `ui` introduces (`html_el`, `list`, `fn`, …) legitimately carry a scope — that is
    /// what stops a user function called `html_el` from capturing them — but the scope number is
    /// an expansion-order detail, so these tests assert on structure instead. Hygiene itself is
    /// asserted directly in the parent module.
    pub(super) fn strip_scopes(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' {
                let mut buf = String::new();
                while let Some(&n) = chars.peek() {
                    if n == '}' {
                        chars.next();
                        break;
                    }
                    buf.push(n);
                    chars.next();
                }
                if !buf.chars().all(|c| c.is_ascii_digit() || c == ',') {
                    out.push('{');
                    out.push_str(&buf);
                    out.push('}');
                }
                continue;
            }
            out.push(c);
        }
        out
    }

    fn ui(src: &str) -> (String, Diagnostics, SourceMap) {
        let mut map = SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let module = parser::parse_module(f, "t", src, &mut d);
        assert!(!d.has_errors(), "parse: {}", d.render(&map));
        let out = crate::expand_module(&module, &mut d);
        (strip_scopes(&print::to_sexpr(&out)), d, map)
    }

    #[test]
    fn an_element_with_text_becomes_html_primitives() {
        let (out, d, map) = ui("def v() -> Html:\n    return ui:\n        h1: \"todos\"\n");
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert!(
            out.contains(r#"(html_el "h1" (list) (list (html_text "todos")))"#),
            "{out}"
        );
    }

    #[test]
    fn attributes_keys_and_handlers_are_distinguished() {
        let (out, d, map) = ui("def v() -> Html:\n\
             \x20   return ui:\n\
             \x20       li(key=k, class=c, on_click=Toggle(id=i)):\n\
             \x20           \"x\"\n");
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert!(out.contains("(html_key k)"), "{out}");
        assert!(out.contains(r#"(html_attr "class" c)"#), "{out}");
        assert!(
            out.contains(r#"(html_on "click" (Toggle (kw id i)))"#),
            "{out}"
        );
    }

    #[test]
    fn a_for_loop_splices_children_rather_than_nesting_them() {
        let (out, d, map) = ui("def v() -> Html:\n\
             \x20   return ui:\n\
             \x20       ul:\n\
             \x20           for t in todos:\n\
             \x20               li: t.text\n");
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert!(
            out.contains("(concat_lists (map_list todos (fn (params t)"),
            "{out}"
        );
    }

    #[test]
    fn literal_children_and_loops_concatenate_in_order() {
        let (out, d, map) = ui("def v() -> Html:\n\
             \x20   return ui:\n\
             \x20       main:\n\
             \x20           h1: \"todos\"\n\
             \x20           for t in todos:\n\
             \x20               li: t.text\n\
             \x20           footer: \"end\"\n");
        assert!(!d.has_errors(), "{}", d.render(&map));
        let body = out.split(r#"(html_el "main""#).nth(1).unwrap();
        let h1 = body.find("\"h1\"").unwrap();
        let loop_at = body.find("map_list").unwrap();
        let footer = body.find("\"footer\"").unwrap();
        assert!(
            h1 < loop_at && loop_at < footer,
            "order not preserved: {out}"
        );
    }

    /// The checks accept what is correct, which is the half a suite of refusals never states.
    ///
    /// `docs/12` §12.4's three checks are errors, so every way of satisfying one has to be listed
    /// somewhere that fails when it stops working — otherwise the first program to name a control
    /// properly and be refused anyway is a user's.
    #[test]
    fn every_way_of_naming_a_control_is_accepted() {
        for element in [
            // An image that carries no meaning: HTML's own spelling, and the reason the check is
            // for the attribute rather than for a value.
            "img(src=\"/x.png\", alt=\"\")",
            "img(src=\"/x.png\", alt=\"a chart\")",
            "input(aria_label=\"name\")",
            "input(aria_labelledby=\"h\")",
            "input(title=\"name\")",
            // An `id` a `label(for=…)` somewhere else may point at — the check's stated hole.
            "input(id=\"name\")",
            "select(aria_label=\"tier\")",
            "textarea(aria_label=\"notes\")",
            // Exempted by hand, with a reason, which is the escape hatch every one of them has.
            "img(src=\"/x.png\", a11y_exempt=\"decorative, and behind aria-hidden\")",
            "input(placeholder=\"search\", a11y_exempt=\"labelled by the heading above it\")",
        ] {
            let (out, d, map) = ui(&format!(
                "def v() -> Html:\n    return ui:\n        {element}\n"
            ));
            assert!(!d.has_errors(), "`{element}`: {}", d.render(&map));
            assert!(
                !out.contains("a11y-exempt") && !out.contains("a11y_exempt"),
                "the exemption reached the page: {out}"
            );
        }
    }

    /// A button names itself with its own text, which is the ordinary case and must not be refused.
    #[test]
    fn a_button_with_text_names_itself() {
        let (out, d, map) = ui("def v() -> Html:\n\
             \x20   return ui:\n\
             \x20       button(on_click=Drop):\n\
             \x20           \"x\"\n");
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert!(out.contains(r#"(html_el "button""#), "{out}");
    }

    #[test]
    fn two_roots_are_an_error_with_a_suggestion() {
        let (_, d, _) = ui("def v() -> Html:\n\
             \x20   return ui:\n\
             \x20       h1: \"a\"\n\
             \x20       h2: \"b\"\n");
        assert!(d.iter().any(|x| x.code == "B0212" && x.fix.is_some()));
    }

    #[test]
    fn an_ordinary_call_inside_a_block_stays_a_call() {
        // `row(t)` has a positional argument, so it is a function producing Html, not a tag.
        let (out, d, map) = ui("def v() -> Html:\n\
             \x20   return ui:\n\
             \x20       ul:\n\
             \x20           row(t)\n");
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert!(out.contains("(html_text (row t))"), "{out}");
    }
}
