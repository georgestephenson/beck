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

/// One statement of a `ui` block as a single `Html` expression.
fn node_expr(n: &Node, diags: &mut Diagnostics) -> Node {
    let span = n.span();
    if let Some(tag) = element_tag(n) {
        let mut attrs = Vec::new();
        for a in &n.args {
            if a.is_form(sym::KW_ARG) && a.args.len() == 2 {
                if a.args[0].as_var().map(|s| s.as_str()) == Some("do") {
                    continue;
                }
                attrs.push(attr_expr(a, &tag, diags));
            }
        }
        let children = match block_of(n) {
            Some(block) => children_expr(&block.args, diags, span),
            None => Node::form(sym::LIST, vec![], span),
        };
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
    Node::form("html_attr", vec![str_lit(attr_name, span), value], span)
}

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
