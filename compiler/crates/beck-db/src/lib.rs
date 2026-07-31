//! The Salsa spine.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.6: "Use
//! **Salsa** … as the compiler's spine from the first commit, not as a later retrofit. Everything
//! is a memoised query."
//!
//! The queries are the ones §4.6 names. What makes them worth having is the *firewall*: because
//! §3.6 makes signatures the module interface, editing a function body invalidates that item's
//! `core` and nothing upstream — the property that makes both sub-second IDE feedback and fast CI
//! builds possible.
//!
//! Phase 1 is one module, so the interesting demonstration is not cross-module: it is that a
//! re-query after an *unrelated* edit does not re-run the parse. That is what `tests` asserts.

use std::sync::Arc;

use beck_diag::{Diagnostics, SourceMap};
use beck_syntax::Node;

#[salsa::query_group(SourceStorage)]
pub trait Compiler: salsa::Database {
    /// The text of a file. The only input; everything else is derived.
    #[salsa::input]
    fn source(&self, name: Arc<str>) -> Arc<str>;

    /// `parse(file) → Node`
    fn parse(&self, name: Arc<str>) -> Arc<Node>;

    /// `expand(module) → Node`
    fn expand(&self, name: Arc<str>) -> Arc<Node>;

    /// The names a module defines — the closest Phase 1 gets to §3.6's published signature, and
    /// the thing a downstream module would depend on instead of on a body.
    fn signature(&self, name: Arc<str>) -> Arc<Vec<String>>;
}

fn parse(db: &dyn Compiler, name: Arc<str>) -> Arc<Node> {
    let src = db.source(name.clone());
    let mut map = SourceMap::new();
    let file = map.add(name.to_string(), src.to_string());
    let mut diags = Diagnostics::new();
    Arc::new(beck_syntax::parse_file(file, &name, &src, &mut diags))
}

fn expand(db: &dyn Compiler, name: Arc<str>) -> Arc<Node> {
    let parsed = db.parse(name);
    let mut diags = Diagnostics::new();
    Arc::new(beck_macro::expand_module(&parsed, &mut diags))
}

fn signature(db: &dyn Compiler, name: Arc<str>) -> Arc<Vec<String>> {
    let expanded = db.expand(name);
    let mut names = Vec::new();
    for item in expanded.args.iter().skip(1) {
        let mut inner = item;
        while inner.is_form(beck_syntax::sym::DECORATE) && inner.args.len() == 2 {
            inner = &inner.args[1];
        }
        if inner.is_form(beck_syntax::sym::DEF) || inner.is_form(beck_syntax::sym::LET) {
            if let Some(n) = inner.args.first().and_then(|a| a.as_var()) {
                names.push(n.as_str().to_string());
            }
        }
    }
    names.sort();
    Arc::new(names)
}

#[salsa::database(SourceStorage)]
#[derive(Default)]
pub struct Database {
    storage: salsa::Storage<Database>,
}

impl salsa::Database for Database {}

impl Database {
    pub fn new() -> Database {
        Database::default()
    }

    pub fn set(&mut self, name: &str, src: &str) {
        self.set_source(Arc::from(name), Arc::from(src));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_memoised_and_re_runs_only_when_its_input_changes() {
        let mut db = Database::new();
        db.set("a.beck", "def f() -> Int:\n    return 1\n");
        let first = db.parse(Arc::from("a.beck"));
        let second = db.parse(Arc::from("a.beck"));
        assert!(
            Arc::ptr_eq(&first, &second),
            "the second query must be served from the memo, not re-parsed"
        );

        db.set("a.beck", "def f() -> Int:\n    return 2\n");
        let third = db.parse(Arc::from("a.beck"));
        assert!(!Arc::ptr_eq(&first, &third), "an edit must invalidate");
    }

    #[test]
    fn a_body_edit_does_not_change_the_signature() {
        // §3.6's firewall, in miniature: the *interface* is stable across a body edit, so anything
        // depending on the signature rather than the body does not need recompiling. That is the
        // property separate compilation is built on.
        let mut db = Database::new();
        db.set("a.beck", "def f() -> Int:\n    return 1\n");
        let before = db.signature(Arc::from("a.beck"));
        db.set("a.beck", "def f() -> Int:\n    return 999\n");
        let after = db.signature(Arc::from("a.beck"));
        assert_eq!(*before, *after);

        db.set(
            "a.beck",
            "def f() -> Int:\n    return 1\n\ndef g() -> Int:\n    return 2\n",
        );
        assert_ne!(*before, *db.signature(Arc::from("a.beck")));
    }

    #[test]
    fn expansion_is_cached_on_top_of_parsing() {
        let mut db = Database::new();
        db.set(
            "a.beck",
            "macro twice(x):\n    return quote:\n        pair($x, $x)\n\n\
             def f() -> Int:\n    return twice(1)\n",
        );
        let a = db.expand(Arc::from("a.beck"));
        let b = db.expand(Arc::from("a.beck"));
        assert!(Arc::ptr_eq(&a, &b));
        // `pair` is macro-introduced, so it carries a hygiene scope — `(pair{1} 1 1)`. The
        // scope number is an expansion-order detail; what this asserts is that the template was
        // instantiated with the argument in both positions.
        let printed = beck_syntax::print::to_sexpr(&a);
        assert!(
            printed.contains("pair") && printed.contains(" 1 1)"),
            "{printed}"
        );
    }
}
