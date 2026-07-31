//! The Salsa spine.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.6: "Use
//! **Salsa** … as the compiler's spine from the first commit, not as a later retrofit. Everything
//! is a memoised query."
//!
//! The queries are the ones §4.6 names. What makes them worth having is the *firewall*: because
//! §3.6 makes signatures the module interface, editing a function body invalidates that module's
//! `checked` and nothing downstream — the property that makes both sub-second IDE feedback and
//! fast CI builds possible.
//!
//! Phase 1 could only demonstrate the shape, because it had one module. Phase 2 has `.becki`
//! interfaces and a project pipeline, so the firewall is now a property with a *number* attached:
//! [`tests::a_body_edit_upstream_does_not_recheck_anything_downstream`] counts how many modules a
//! one-line edit re-checks, and the answer is one.
//!
//! # How the firewall is expressed to Salsa
//!
//! `checked(m)` depends on `source(m)` and on `interface(d)` for each import `d` — never on
//! `source(d)`. So an edit to `d`'s body invalidates `interface(d)`, which is recomputed, compares
//! *equal* to what it was, and Salsa backdates it: `checked(m)` is not re-run. The equality is the
//! whole mechanism, which is why [`beck_core::Interface`] closes its rows and hashes its meaning
//! rather than its rendering.

use std::sync::Arc;

use beck_core::project::{check_one, Checked};
use beck_core::Interface;
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

    /// The modules this one imports.
    fn imports(&self, name: Arc<str>) -> Arc<Vec<Arc<str>>>;

    /// `signature(item) → Signature` — **the separate-compilation firewall** (§3.6).
    ///
    /// This is the query the whole design turns on. Everything downstream depends on it and
    /// nothing downstream depends on a body, so a body edit stops here.
    fn interface(&self, name: Arc<str>) -> Arc<Interface>;

    /// `typecheck_body(item)` and `core(item)`, as one query per module: the expensive half, and
    /// the one the firewall exists to avoid re-running.
    fn checked(&self, name: Arc<str>) -> Arc<Outcome>;
}

/// What checking a module produced, reduced to what a caller can compare.
///
/// The `Program` itself is not stored: it holds `Core` trees whose variable numbering is an
/// implementation detail, and a query result that changes when nothing meaningful did would defeat
/// the backdating this module exists to demonstrate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub interface: Interface,
    pub diagnostics: Vec<(&'static str, String)>,
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

fn imports(db: &dyn Compiler, name: Arc<str>) -> Arc<Vec<Arc<str>>> {
    let expanded = db.expand(name);
    Arc::new(
        expanded
            .args
            .iter()
            .skip(1)
            .filter(|n| n.is_form(beck_syntax::sym::IMPORT))
            .filter_map(|n| n.args.first().and_then(|a| a.as_var()))
            .map(|s| Arc::from(s.as_str()))
            .collect(),
    )
}

fn interface(db: &dyn Compiler, name: Arc<str>) -> Arc<Interface> {
    Arc::new(db.checked(name).interface.clone())
}

fn checked(db: &dyn Compiler, name: Arc<str>) -> Arc<Outcome> {
    // The dependency that matters: each import's *interface*, never its source.
    let deps: Vec<(String, Interface)> = db
        .imports(name.clone())
        .iter()
        .map(|d| (d.to_string(), (*db.interface(d.clone())).clone()))
        .collect();

    let src = db.source(name.clone());
    let mut diags = Diagnostics::new();
    observe(&name);
    let Checked { interface, .. } = check_one(&name, &src, &deps, None, &mut diags);
    Arc::new(Outcome {
        interface,
        diagnostics: diags.iter().map(|d| (d.code, d.message.clone())).collect(),
    })
}

// ---------------------------------------------------------------------------------------------

thread_local! {
    /// Which modules `checked` actually ran for. A memoised query is only interesting for what it
    /// *does not* do, and nothing observable distinguishes a cache hit from a recomputation — so
    /// the recomputation says so.
    static OBSERVED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn observe(name: &str) {
    OBSERVED.with(|o| o.borrow_mut().push(name.to_string()));
}

/// Take and clear the record of which modules were re-checked.
pub fn take_rechecked() -> Vec<String> {
    OBSERVED.with(|o| std::mem::take(&mut *o.borrow_mut()))
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
    fn a_body_edit_does_not_change_the_interface() {
        // §3.6's firewall, in miniature: the *interface* is stable across a body edit, so anything
        // depending on the signature rather than the body does not need recompiling.
        let mut db = Database::new();
        db.set("a", "def f() -> Int:\n    return 1\n");
        let before = db.interface(Arc::from("a"));
        db.set("a", "def f() -> Int:\n    return 999\n");
        let after = db.interface(Arc::from("a"));
        assert_eq!(before.digest(), after.digest());

        db.set(
            "a",
            "def f() -> Int:\n    return 1\n\ndef g() -> Int:\n    return 2\n",
        );
        assert_ne!(before.digest(), db.interface(Arc::from("a")).digest());
    }

    /// A three-module project: a leaf, something over it, and the app.
    ///
    /// Only `domain` is ever re-`set`: an input assignment bumps Salsa's revision whether or not
    /// the value changed, so re-setting an untouched file would be the test staging the very edit
    /// it claims did not happen.
    fn domain(body: &str) -> String {
        format!(
            "model Todo:\n    text: Str\n    done: Bool\n\n\
             def label(t: Todo) -> Str:\n    return {body}\n"
        )
    }

    fn project(db: &mut Database) {
        db.set("domain", &domain("t.text"));
        db.set(
            "policy",
            "import domain\n\n\
             def allowed(t: Todo) -> Bool:\n    return not t.done\n",
        );
        db.set(
            "app",
            "import domain\nimport policy\n\n\
             def summary(t: Todo) -> Str:\n    return label(t)\n",
        );
    }

    #[test]
    fn a_body_edit_upstream_does_not_recheck_anything_downstream() {
        // The Phase 2 exit criterion, as a count: "a 3-module project rebuilds incrementally
        // without recompiling dependencies whose signatures didn't change."
        let mut db = Database::new();
        project(&mut db);
        // Prime the whole project.
        let _ = db.checked(Arc::from("app"));
        let _ = db.checked(Arc::from("policy"));
        let mut primed = take_rechecked();
        primed.sort();
        primed.dedup();
        assert_eq!(primed, ["app", "domain", "policy"]);

        // Change a body in the deepest module. Its interface does not move, so nothing above it
        // has anything to redo.
        db.set("domain", &domain("t.text + \"!\""));
        let _ = db.checked(Arc::from("app"));
        let _ = db.checked(Arc::from("policy"));
        let mut after = take_rechecked();
        after.sort();
        after.dedup();
        assert_eq!(
            after,
            ["domain"],
            "only the edited module may be re-checked; `policy` and `app` depend on its interface, \
             which did not change"
        );
    }

    #[test]
    fn a_signature_change_upstream_does_recheck_downstream() {
        // The other half, and the one that makes the first mean something: a firewall that never
        // lets anything through is a wall.
        let mut db = Database::new();
        project(&mut db);
        let _ = db.checked(Arc::from("app"));
        let _ = db.checked(Arc::from("policy"));
        let _ = take_rechecked();

        // Widen `label`'s effect row. That *is* the published contract (§3.6).
        db.set(
            "domain",
            "model Todo:\n    text: Str\n    done: Bool\n\n\
             def label(t: Todo) -> Str uses net.out(telemetry.example.com):\n    return t.text\n",
        );
        let _ = db.checked(Arc::from("app"));
        let _ = db.checked(Arc::from("policy"));
        let mut after = take_rechecked();
        after.sort();
        after.dedup();
        assert!(
            after.contains(&"domain".to_string()) && after.contains(&"app".to_string()),
            "`app` calls `label`, so an effect widening must reach it: {after:?}"
        );
        // And the bound, stated rather than discovered: `policy` never calls `label`, but it does
        // import `domain`, whose *interface* changed — so it is re-checked too. Phase 2's firewall
        // is per **module**, not per item. §4.6's query list says `signature(item)`, and narrowing
        // it from a module to an item is the next increment: it costs nothing here, where a module
        // is one file, and it is what a 50 kLOC project would need.
        assert!(
            after.contains(&"policy".to_string()),
            "the firewall is per module, so an importer is re-checked when the interface moves: \
             {after:?}"
        );
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
