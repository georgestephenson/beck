//! The Beck front end: two surfaces, one AST.
//!
//! ```text
//!    surface/py.beck ──┐                                        ┌── printer.py    ──▶ .beck
//!                      ├──▶  Reader  ──▶  Node (canonical AST) ─┤
//!  surface/sx.beck  ───┘        ▲                               └── printer.sexpr ──▶ .sx
//!                               │
//!                     both readers produce identical Node trees
//! ```
//!
//! ([`docs/02-syntax.md`](../../../../docs/02-syntax.md) §2.2.) The equivalence in that last line is
//! not a slogan here: `print::tests::the_two_surfaces_are_the_same_language` reads the same
//! definition through both readers and asserts the trees are structurally equal.

pub mod doc;
pub mod lexer;
pub mod node;
pub mod parser;
pub mod print;
pub mod security;
pub mod sexpr;

pub use node::{sym, Head, Lit, Meta, Node, Scope, ScopeSet, Symbol};

use beck_diag::{Diagnostics, FileId};

/// Read a source file in whichever surface its extension names.
///
/// `.beck` is the Python surface and the default; `.sx` is the canonical S-expression surface,
/// which stays "documented and supported (it is invaluable for macro debugging, for the spec, and
/// for generated code)" (§2.2).
/// A file name, as the identifier a module is called.
///
/// A module name is printed into `(module <name> …)` and has to read back as a symbol, so it cannot
/// simply be the file name: `01-counter.beck` would print a form the reader rejects, and
/// `parse(print(parse(src))) == parse(src)` — the round-trip property §4.8 asks for — would fail on
/// any file whose name is not already an identifier. Found by naming a corpus in file order.
pub fn module_ident(name: &str) -> String {
    let stem = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_end_matches(".becki")
        .trim_end_matches(".beck")
        .trim_end_matches(".sx");
    let mut out = String::with_capacity(stem.len() + 1);
    for (i, c) in stem.chars().enumerate() {
        match c {
            'a'..='z' | 'A'..='Z' | '_' => out.push(c),
            '0'..='9' if i > 0 => out.push(c),
            '0'..='9' => {
                out.push('m');
                out.push(c);
            }
            _ => out.push('_'),
        }
    }
    if out.is_empty() {
        out.push_str("main");
    }
    out
}

pub fn parse_file(file: FileId, name: &str, src: &str, diags: &mut Diagnostics) -> Node {
    let mut parsed = parse_forms(file, name, src, diags);
    // Doc comments are attached after parsing rather than lexed (see [`doc`]), so this is the one
    // place both surfaces pass through and the one place the attachment has to happen.
    doc::attach(&mut parsed, &doc::collect(src, doc::marker_for(name)));
    parsed
}

fn parse_forms(file: FileId, name: &str, src: &str, diags: &mut Diagnostics) -> Node {
    let module_name = module_ident(name);

    // Before either surface reads a byte: what a source file is allowed to contain at all.
    // One place for both notations — see [`security`].
    security::scan(file, src, diags);

    if name.ends_with(".sx") {
        let forms = sexpr::read_all(file, src, diags);
        // A file that already *is* a module is that module. `beck fmt --surface sexpr` prints
        // `(module todo …)`, and wrapping that in a second module made the printer's own output
        // unreadable by the checker — so `parse(print(parse(src)))` failed on the one surface §2.2
        // calls canonical. Found by round-tripping the corpus; it predates Phase 2.
        if forms.len() == 1 && forms[0].is_form(sym::MODULE) {
            return forms.into_iter().next().expect("length checked");
        }
        let mut items = vec![Node::sym(&module_name, beck_diag::Span::NONE)];
        items.extend(forms);
        Node::form(sym::MODULE, items, beck_diag::Span::new(file, 0..src.len()))
    } else {
        parser::parse_module(file, &module_name, src, diags)
    }
}

#[cfg(test)]
mod module_name_tests {
    use super::module_ident;

    #[test]
    fn a_printed_module_reads_back_as_that_module_rather_than_as_a_nested_one() {
        use beck_diag::{Diagnostics, SourceMap};
        let src = "def f() -> Int:\n    return 1\n";
        let mut map = SourceMap::new();
        let file = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let parsed = super::parse_file(file, "t.beck", src, &mut d);

        let printed = super::print::to_sexpr_pretty(&parsed);
        let file2 = map.add("t.sx", printed.clone());
        let reread = super::parse_file(file2, "t.sx", &printed, &mut d);
        assert!(!d.has_errors());
        assert_eq!(
            super::print::to_sexpr(&parsed),
            super::print::to_sexpr(&reread),
            "printed:\n{printed}"
        );
    }

    #[test]
    fn a_file_name_becomes_an_identifier_a_reader_can_read_back() {
        assert_eq!(module_ident("todo.beck"), "todo");
        assert_eq!(module_ident("src/orders.beck"), "orders");
        assert_eq!(module_ident("a/b/c.sx"), "c");
        assert_eq!(module_ident("orders.becki"), "orders");
        // The ones that motivated this: a leading digit is not an identifier, and a hyphen is an
        // operator.
        assert_eq!(module_ident("01-counter.beck"), "m01_counter");
        assert_eq!(module_ident("my app.beck"), "my_app");
        assert_eq!(module_ident(""), "main");
    }
}
