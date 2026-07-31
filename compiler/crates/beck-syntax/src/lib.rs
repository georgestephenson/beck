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

pub mod lexer;
pub mod node;
pub mod parser;
pub mod print;
pub mod sexpr;

pub use node::{sym, Head, Lit, Meta, Node, Scope, ScopeSet, Symbol};

use beck_diag::{Diagnostics, FileId};

/// Read a source file in whichever surface its extension names.
///
/// `.beck` is the Python surface and the default; `.sx` is the canonical S-expression surface,
/// which stays "documented and supported (it is invaluable for macro debugging, for the spec, and
/// for generated code)" (§2.2).
pub fn parse_file(file: FileId, name: &str, src: &str, diags: &mut Diagnostics) -> Node {
    let module_name = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .trim_end_matches(".beck")
        .trim_end_matches(".sx")
        .to_string();

    if name.ends_with(".sx") {
        let forms = sexpr::read_all(file, src, diags);
        let mut items = vec![Node::sym(&module_name, beck_diag::Span::NONE)];
        items.extend(forms);
        Node::form(sym::MODULE, items, beck_diag::Span::new(file, 0..src.len()))
    } else {
        parser::parse_module(file, &module_name, src, diags)
    }
}
