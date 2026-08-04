//! Multi-module compilation: check against signatures, then link.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../../docs/03-type-and-effect-system.md) §3.6's
//! consequence, spelled out: "modules compile against signatures (true separate compilation,
//! parallel builds); body edits don't invalidate downstream modules".
//!
//! # Two passes, and the difference between them is the whole point
//!
//! **Checking** a module needs the *interfaces* of what it imports and nothing else. Its types
//! unify against imported types, its rows widen with imported rows, and its placement is solved
//! within the module — because §3.6 makes placement part of the published signature, so an imported
//! name's tier is a given rather than a variable. That is why editing a body downstream is free:
//! there is nothing in the interface for it to change.
//!
//! **Linking** needs the bodies, because a program that runs has to have code in it. This is the
//! `.mli`/`.ml` division, and it is worth being explicit that the two halves have different inputs:
//! an interface is enough to *compile* against, and never enough to *run*.
//!
//! # What the link step does and does not do
//!
//! It merges checked modules into one [`crate::check::Program`] and slices that. Every imported
//! definition arrives with its placement already decided and marked as such, so the root module's
//! solve cannot move it — a downstream edit re-placing an upstream function would be exactly the
//! failure §3.6 exists to prevent.
//!
//! It does **not** namespace: two modules that define the same name are an error rather than a
//! shadowing rule, because Phase 2 has no qualified references to disambiguate with. Named, rather
//! than discovered.
//!
//! # Where a module comes from
//!
//! A [`Loader`] answers for the program being compiled — for the CLI, the directory the root module
//! lives in. What it cannot answer for, [`crate::stdlib`] does: the standard library's Beck half is
//! carried in the compiler, so `import bignum` resolves from any directory
//! ([`10`](../../../../../docs/10-decisions.md) D23,
//! [`adr/0018`](../../../../../docs/adr/0018-the-standard-library-is-carried-in-the-compiler.md)).
//!
//! The order is loader first, library second, and it is not arbitrary: a project must be able to
//! keep the name of a module it already has when the standard library grows one, and a library
//! being *worked on* — `lib/decimal.beck` importing `bignum` — must get the file beside it rather
//! than the copy the compiler was built with.

use std::collections::{BTreeMap, BTreeSet};

use beck_diag::{Diagnostic, Diagnostics, Span};

use crate::check::{check_module_with, Mode, Program};
use crate::iface::Interface;
use crate::place;
use crate::split::Placed;

/// One module's sources: its implementation, and its published interface if one is checked in.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// The `.beck` file. Required to link; optional to check against.
    pub module: Option<String>,
    /// The `.becki` file, if it is checked in. When present it is what downstream modules see —
    /// which is the point: reviewing the contract is reviewing this file.
    pub interface: Option<String>,
    /// Where the module text came from, if it was a file. Two things depend on it and both are
    /// visible to a user: which surface the text is in — `.sx` selects the S-expression reader
    /// (§2.2) — and what a diagnostic calls the file. Defaults to `<name>.beck`.
    pub path: Option<String>,
}

/// Where modules come from.
pub trait Loader {
    fn load(&self, name: &str) -> Option<Sources>;
}

impl<F: Fn(&str) -> Option<Sources>> Loader for F {
    fn load(&self, name: &str) -> Option<Sources> {
        self(name)
    }
}

/// A module, checked and placed, with its published interface.
pub struct Checked {
    pub program: Program,
    pub interface: Interface,
}

/// Check one module against its imports' interfaces, and solve its placement.
pub fn check_one(
    name: &str,
    src: &str,
    imports: &[(String, Interface)],
    lock: Option<&place::Lock>,
    diags: &mut Diagnostics,
) -> Checked {
    let mut map = beck_diag::SourceMap::new();
    let file = map.add(name, src);
    check_one_in(file, name, src, imports, lock, diags)
}

/// The same, against a caller's source map so that diagnostics point at the right file.
pub fn check_one_in(
    file: beck_diag::FileId,
    name: &str,
    src: &str,
    imports: &[(String, Interface)],
    lock: Option<&place::Lock>,
    diags: &mut Diagnostics,
) -> Checked {
    let parsed = beck_syntax::parse_file(file, name, src, diags);
    let expanded = beck_macro::expand_module(&parsed, diags);
    let mut program = check_module_with(&expanded, Mode::Module, imports, diags);
    let solution = place::solve(&program, lock);
    place::apply(&mut program, &solution);
    place::check_placement(&program, diags);
    // Only the per-module half here. Whether a capability has a holder is a question about the
    // linked program, and a module that holds `cap.session` while the wiring lives elsewhere is
    // the *correct* factoring, not a violation.
    crate::secure::check_boundaries(&program, diags);
    let interface = Interface::of(&program);
    Checked { program, interface }
}

/// The modules a source file imports, in source order.
pub fn imports_of(file: beck_diag::FileId, name: &str, src: &str) -> Vec<String> {
    let mut diags = Diagnostics::new();
    let parsed = beck_syntax::parse_file(file, name, src, &mut diags);
    parsed
        .args
        .iter()
        .skip(1)
        .filter(|n| n.is_form(beck_syntax::sym::IMPORT))
        .filter_map(|n| n.args.first().and_then(|a| a.as_var()))
        .map(|s| s.as_str().to_string())
        .collect()
}

/// A checked, linked project, before it is sliced.
///
/// Separate from [`compile_project`] because publishing an interface and typechecking a library are
/// things a module can do without being an application. Only slicing needs a merge point, a durable
/// fold and a page — and a policy module that has none of those is not broken, it is a policy
/// module.
pub struct Project {
    pub program: Program,
    /// The root module's placement, for `beck explain place`.
    pub solution: place::Solution,
    /// The root module's published contract.
    pub interface: Interface,
}

/// Check and link a project, stopping before the slicer.
pub fn check_project(
    root: &str,
    loader: &dyn Loader,
    lock: Option<&place::Lock>,
    map: &mut beck_diag::SourceMap,
    diags: &mut Diagnostics,
) -> Option<Project> {
    let mut order: Vec<String> = Vec::new();
    let mut visiting: Vec<String> = Vec::new();
    let mut sources: BTreeMap<String, (Sources, beck_diag::FileId)> = BTreeMap::new();
    // Which of them came from the compiler rather than from the caller's directory, because a
    // standard-library module's tests are not the program's — see where this is read, below.
    let mut from_library: BTreeSet<String> = BTreeSet::new();

    // Depth-first over imports, deepest first, so a module is checked only once everything it
    // depends on has an interface.
    #[allow(clippy::too_many_arguments)]
    fn visit(
        name: &str,
        loader: &dyn Loader,
        map: &mut beck_diag::SourceMap,
        sources: &mut BTreeMap<String, (Sources, beck_diag::FileId)>,
        from_library: &mut BTreeSet<String>,
        order: &mut Vec<String>,
        visiting: &mut Vec<String>,
        diags: &mut Diagnostics,
    ) {
        if order.iter().any(|n| n == name) {
            return;
        }
        if visiting.iter().any(|n| n == name) {
            diags.push(
                Diagnostic::error(
                    "B0602",
                    format!("module `{name}` imports itself, directly or through a cycle"),
                    Span::NONE,
                )
                .with_note(format!("the cycle is {} → {name}", visiting.join(" → ")))
                .with_note(
                    "a module's interface is derived from its body, so a cycle would mean each \
                     module needed the other's contract before either had one",
                ),
            );
            return;
        }
        // The caller's directory first, the standard library second — the module doc says why the
        // order is that way round rather than the other.
        let loaded = loader.load(name).or_else(|| {
            crate::stdlib::sources(name).inspect(|_| {
                from_library.insert(name.to_string());
            })
        });
        let Some(src) = loaded else {
            diags.push(
                Diagnostic::error("B0603", format!("cannot find module `{name}`"), Span::NONE)
                    .with_note(format!(
                        "looked for `{name}.becki` and `{name}.beck` beside the root module, and \
                         for a standard-library module called `{name}`"
                    )),
            );
            return;
        };
        let text = src
            .module
            .clone()
            .or_else(|| src.interface.clone())
            .unwrap_or_default();
        let display = src.path.clone().unwrap_or_else(|| format!("{name}.beck"));
        let file = map.add(display.clone(), text.clone());
        visiting.push(name.to_string());
        for dep in imports_of(file, &display, &text) {
            visit(
                &dep,
                loader,
                map,
                sources,
                from_library,
                order,
                visiting,
                diags,
            );
        }
        visiting.pop();
        sources.insert(name.to_string(), (src, file));
        order.push(name.to_string());
    }

    visit(
        root,
        loader,
        map,
        &mut sources,
        &mut from_library,
        &mut order,
        &mut visiting,
        diags,
    );
    if diags.has_errors() {
        return None;
    }

    let mut interfaces: BTreeMap<String, Interface> = BTreeMap::new();
    let mut checked: Vec<Checked> = Vec::new();

    for name in &order {
        let Some((src, file)) = sources.get(name) else {
            continue;
        };
        let display = src.path.clone().unwrap_or_else(|| format!("{name}.beck"));
        let deps: Vec<(String, Interface)> = {
            let text = src.module.clone().or_else(|| src.interface.clone());
            imports_of(*file, &display, text.as_deref().unwrap_or(""))
                .into_iter()
                .filter_map(|d| interfaces.get(&d).map(|i| (d, i.clone())))
                .collect()
        };

        // The published interface, if one is checked in, is what downstream sees — not what this
        // module happens to compile to today. That is the difference between a contract and a
        // description, and it is the reason `beck iface` writes a file rather than a cache entry.
        if let Some(text) = &src.interface {
            let published = Interface::parse(name, text, map, diags);
            interfaces.insert(name.clone(), published);
        }

        let Some(module_src) = &src.module else {
            // Interface only: it can be checked against, but there is no code to link.
            if name == root {
                diags.push(
                    Diagnostic::error(
                        "B0604",
                        format!("`{name}` has an interface but no implementation"),
                        Span::NONE,
                    )
                    .with_note("an interface is enough to compile against and never enough to run"),
                );
            }
            continue;
        };

        let mut one = check_one_in(*file, &display, module_src, &deps, lock, diags);
        // A standard-library module's `test` blocks are the *compiler's* tests, not this program's.
        // They are still checked — a library that stopped compiling its own tests would be broken —
        // and they are dropped before the link, so `beck test` on a program that imports `bignum`
        // reports the program's tests and not two hundred of ours. `beck-cli/tests/stdlib.rs` is
        // where they run (§21.2's rule that a program's behaviour is asserted in the program still
        // holds; the program asserting them is the library file itself).
        if from_library.contains(name) {
            one.program.tests.clear();
        }
        // Where both exist, the checked-in interface is the contract and the module must meet it.
        if let Some(published) = interfaces.get(name) {
            if published.digest() != one.interface.digest() {
                diags.push(
                    Diagnostic::error(
                        "B0605",
                        format!("`{name}` does not match its published interface"),
                        Span::NONE,
                    )
                    .with_note(format!(
                        "`{name}.becki` says {} and the module compiles to {}",
                        published.digest(),
                        one.interface.digest()
                    ))
                    .with_fix("regenerate it with `beck iface`, and review the diff"),
                );
            }
        } else {
            interfaces.insert(name.clone(), one.interface.clone());
        }
        checked.push(one);
    }

    if diags.has_errors() {
        return None;
    }

    let interface = interfaces.get(root).cloned().unwrap_or_default();
    let merged = link(root, checked, diags)?;
    // Now the whole program exists, so the whole-program questions can be asked.
    crate::secure::check_capabilities(&merged, diags);
    if diags.has_errors() {
        return None;
    }
    // Every placement was decided by the module that owns it and is pinned by the link; solving
    // over the merged program is how those decisions are collected for `beck explain place`.
    let solution = place::solve(&merged, lock);
    Some(Project {
        program: merged,
        solution,
        interface,
    })
}

/// Slice a checked project into the roles the runtime drives.
///
/// Separate from [`check_project`] so that "this typechecks" and "this is a runnable application"
/// are two answers rather than one: a library gets the first and not the second, and that is not a
/// failure.
pub fn slice(project: Project, diags: &mut Diagnostics) -> Option<Placed> {
    let solution = project.solution;
    crate::split::split(project.program, diags).map(|mut p| {
        p.placement = solution;
        p
    })
}

/// Slice a project, or wrap it as a library if the only thing wrong with it is that it is one.
///
/// [`slice()`] answers the *application* question and a module that is not an application is still a
/// module — `beck check` has said so since Phase 2. What it could not do was give that module back
/// to a caller, so a library had no way to run its own tests
/// (`docs/22-phase-3-report.md` §22.6, `docs/25-benchmarks-and-expressiveness.md` §25.6 item 1).
///
/// The B0500/B0501/B0505 diagnostics are **dropped** on that path rather than downgraded to
/// warnings, because they are answers to a question this caller did not ask. Every other diagnostic
/// is kept and the result is `None`: a library with a type error is a broken module, not a library.
pub fn slice_or_library(project: Project, diags: &mut Diagnostics) -> Option<Placed> {
    let program = project.program.clone();
    let solution = project.solution.clone();
    let mut slicing = Diagnostics::new();
    if let Some(mut placed) = crate::split::split(project.program, &mut slicing) {
        diags.extend(slicing);
        placed.placement = solution;
        return Some(placed);
    }
    if !slicing.iter().all(|d| NOT_AN_APPLICATION.contains(&d.code)) {
        diags.extend(slicing);
        return None;
    }
    // The graph is rebuilt rather than kept from the failed slice, because `split` consumed the
    // program. A graph that cannot be built is a real error and lands in `diags`.
    let graph = crate::signal::Graph::build(&program, diags)?;
    let wire_id = format!("lib:{}", program.name);
    Some(Placed::library(program, graph, wire_id))
}

/// The diagnostics that mean "this module is a library", not "this module is wrong".
///
/// Each is the slicer reporting a missing *application* part — a merge point, a durable fold, a
/// page. A domain module has none of them by design.
pub const NOT_AN_APPLICATION: [&str; 3] = ["B0500", "B0501", "B0505"];

/// Compile a whole project: check, link, and slice.
///
/// The caller's `SourceMap` is what every module is added to, because a diagnostic about the third
/// module in a project has to be renderable by whoever asked for the first.
pub fn compile_project(
    root: &str,
    loader: &dyn Loader,
    lock: Option<&place::Lock>,
    map: &mut beck_diag::SourceMap,
    diags: &mut Diagnostics,
) -> Option<Placed> {
    let project = check_project(root, loader, lock, map, diags)?;
    slice(project, diags)
}

/// Merge checked modules into one program.
fn link(root: &str, modules: Vec<Checked>, diags: &mut Diagnostics) -> Option<Program> {
    let mut out: Option<Program> = None;
    let mut seen: BTreeSet<std::sync::Arc<str>> = BTreeSet::new();

    for Checked { mut program, .. } in modules {
        // An imported definition's placement is part of its published signature (§3.6), so at link
        // time it is a given. Marking it annotated is how the root's solve is told not to move it.
        for def in program.defs.values_mut() {
            def.tier_is_annotated = true;
        }
        for s in program.signals.iter_mut() {
            s.tier_is_annotated = true;
        }

        let Some(acc) = out.as_mut() else {
            seen.extend(program.defs.keys().cloned());
            out = Some(program);
            continue;
        };
        for (name, def) in program.defs {
            if !seen.insert(name.clone()) {
                diags.push(
                    Diagnostic::error(
                        "B0601",
                        format!("`{name}` is defined in more than one module"),
                        def.span,
                    )
                    .with_note(
                        "Phase 2 links modules into one namespace and has no qualified reference \
                         to tell two definitions apart, so a clash is an error rather than a \
                         shadowing rule",
                    ),
                );
                continue;
            }
            acc.def_order.push(name.clone());
            acc.defs.insert(name, def);
        }
        for (n, t) in program.types {
            acc.types.entry(n).or_insert(t);
        }
        acc.own_types.extend(program.own_types);
        acc.signals.extend(program.signals);
        acc.tests.extend(program.tests);
        // The doc comments too. Without this the merged program keeps only the *first* module's,
        // which is the deepest import rather than the root — so `beck doc` on a module that imports
        // another documented the wrong module's names. Invisible until a module in `lib/` imported
        // one (`docs/56` §56.5); a clash is impossible for a definition, because `B0601` above
        // already refuses two modules defining one name.
        acc.docs.extend(program.docs);
    }

    let mut merged = out?;
    merged.name = root.to_string();
    (!diags.has_errors()).then_some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::Tier;

    /// A three-module project: a domain, a policy over it, and the app that wires them.
    fn project() -> BTreeMap<String, Sources> {
        let domain = r#"
type Id = newtype[Str]

model Todo:
    id: Id
    text: Str
    done: Bool
    owner: Str

model State:
    todos: Map[Id, Todo]

union Command:
    Add(id: Id, text: Str)
    Toggle(id: Id)

union Event:
    Added(id: Id, text: Str)
    Toggled(id: Id)

union Rejection:
    BlankText
    NotOwner

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Added(id, text):
            return s.with(todos=map_insert(s.todos, id, Todo(id=id, text=text, done=False, owner=env.actor)))
        case Toggled(id):
            return s
"#;
        let policy = r#"
import domain

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Add(id, text):
            if str_is_empty(str_trim(text)):
                return Err(error=BlankText)
            return Ok(value=[Added(id=id, text=text)])
        case Toggle(id):
            return Ok(value=[Toggled(id=id)])
"#;
        let app = r#"
import domain
import policy

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            h1: "todos"
            footer: (str(map_len(s.todos)) + " todos")

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, todos, validate)
todos: Signal[State] = durable(fold(apply_event, State(todos={}), events))
page: Signal[Html] = per_session(todos, view)
"#;
        BTreeMap::from([
            (
                "domain".to_string(),
                Sources {
                    module: Some(domain.into()),
                    interface: None,
                    path: None,
                },
            ),
            (
                "policy".to_string(),
                Sources {
                    module: Some(policy.into()),
                    interface: None,
                    path: None,
                },
            ),
            (
                "app".to_string(),
                Sources {
                    module: Some(app.into()),
                    interface: None,
                    path: None,
                },
            ),
        ])
    }

    fn compile(files: &BTreeMap<String, Sources>) -> (Option<Placed>, Diagnostics) {
        let mut diags = Diagnostics::new();
        let mut map = beck_diag::SourceMap::new();
        let out = compile_project(
            "app",
            &|n: &str| files.get(n).cloned(),
            None,
            &mut map,
            &mut diags,
        );
        (out, diags)
    }

    #[test]
    fn a_three_module_project_compiles_links_and_places() {
        let files = project();
        let (placed, d) = compile(&files);
        assert!(
            !d.has_errors(),
            "{:?}",
            d.iter().map(|x| (x.code, &x.message)).collect::<Vec<_>>()
        );
        let placed = placed.expect("it links");
        // Definitions from all three modules are in the linked program…
        for name in ["apply_event", "validate", "view"] {
            assert!(placed.program.defs.contains_key(name), "missing {name}");
        }
        // …and the app's own wiring is placed as it would be alone.
        let tier = |n: &str| {
            placed
                .program
                .signals
                .iter()
                .find(|s| s.name.as_ref() == n)
                .map(|s| s.tier)
        };
        assert_eq!(tier("proposals"), Some(Tier::Server));
        assert_eq!(tier("todos"), Some(Tier::Data));
        assert_eq!(tier("page"), Some(Tier::Client));
    }

    #[test]
    fn a_body_edit_upstream_does_not_change_any_downstream_contract() {
        // §3.6's firewall, at project scale. `domain`'s body changes; its interface does not; so
        // nothing downstream has anything to recompile against.
        let files = project();
        let before = {
            let mut d = Diagnostics::new();
            check_one(
                "domain",
                files["domain"].module.as_ref().unwrap(),
                &[],
                None,
                &mut d,
            )
            .interface
        };

        let mut edited = files.clone();
        let body = files["domain"].module.as_ref().unwrap().replace(
            "case Toggled(id):\n            return s",
            "case Toggled(id):\n            return s.with(todos=s.todos)",
        );
        edited.get_mut("domain").unwrap().module = Some(body);

        let after = {
            let mut d = Diagnostics::new();
            check_one(
                "domain",
                edited["domain"].module.as_ref().unwrap(),
                &[],
                None,
                &mut d,
            )
            .interface
        };
        assert_eq!(before.digest(), after.digest());

        // …and the project still compiles, which is the other half: the firewall is only useful if
        // what it protects still works.
        let (placed, d) = compile(&edited);
        assert!(!d.has_errors());
        assert!(placed.is_some());
    }

    #[test]
    fn a_checked_in_interface_that_the_module_no_longer_meets_is_an_error() {
        // The failure mode a generated-and-committed contract exists to catch: the file says one
        // thing, the code does another, and downstream believed the file.
        let mut files = project();
        let mut d = Diagnostics::new();
        let iface = check_one(
            "domain",
            files["domain"].module.as_ref().unwrap(),
            &[],
            None,
            &mut d,
        )
        .interface;
        // Publish a contract, then widen the module's row behind it.
        files.get_mut("domain").unwrap().interface = Some(iface.render());
        let widened = files["domain"].module.as_ref().unwrap().replace(
            "def apply_event(s: State, env: Envelope[Event]) -> State:",
            "def apply_event(s: State, env: Envelope[Event]) -> State uses log:\n    return apply(s, env)\n\ndef apply(s: State, env: Envelope[Event]) -> State:",
        );
        files.get_mut("domain").unwrap().module = Some(widened);
        let (_, d) = compile(&files);
        let codes: Vec<&str> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"B0605"), "got {codes:?}");
    }

    #[test]
    fn an_import_cycle_is_reported_rather_than_looped_on() {
        let files = BTreeMap::from([
            (
                "a".to_string(),
                Sources {
                    module: Some("import b\n\ndef f() -> Int:\n    return 1\n".into()),
                    interface: None,
                    path: None,
                },
            ),
            (
                "b".to_string(),
                Sources {
                    module: Some("import a\n\ndef g() -> Int:\n    return 2\n".into()),
                    interface: None,
                    path: None,
                },
            ),
        ]);
        let mut diags = Diagnostics::new();
        let mut map = beck_diag::SourceMap::new();
        compile_project(
            "a",
            &|n: &str| files.get(n).cloned(),
            None,
            &mut map,
            &mut diags,
        );
        assert!(diags.iter().any(|d| d.code == "B0602"), "{:?}", diags.len());
    }

    #[test]
    fn a_missing_module_says_what_it_looked_for() {
        let mut diags = Diagnostics::new();
        let files: BTreeMap<String, Sources> = BTreeMap::from([(
            "a".to_string(),
            Sources {
                module: Some("import nowhere\n\ndef f() -> Int:\n    return 1\n".into()),
                interface: None,
                path: None,
            },
        )]);
        let mut map = beck_diag::SourceMap::new();
        compile_project(
            "a",
            &|n: &str| files.get(n).cloned(),
            None,
            &mut map,
            &mut diags,
        );
        assert!(diags.iter().any(|d| d.code == "B0603"));
    }

    /// D23: a module the loader has never heard of still resolves if the library has it.
    #[test]
    fn the_standard_library_resolves_with_no_file_beside_the_root() {
        let files: BTreeMap<String, Sources> = BTreeMap::from([(
            "app".to_string(),
            Sources {
                module: Some(
                    "import format\n\ndef nine(x: Float) -> Str:\n    return fixed(x, 9)\n".into(),
                ),
                interface: None,
                path: None,
            },
        )]);
        let mut diags = Diagnostics::new();
        let mut map = beck_diag::SourceMap::new();
        let project = check_project(
            "app",
            &|n: &str| files.get(n).cloned(),
            None,
            &mut map,
            &mut diags,
        );
        assert!(
            !diags.has_errors(),
            "{:?}",
            diags
                .iter()
                .map(|d| (d.code, &d.message))
                .collect::<Vec<_>>()
        );
        let project = project.expect("it links");
        assert!(project.program.defs.contains_key("fixed"));
        // And the library's own tests are the library's: they do not become this program's.
        assert!(
            project.program.tests.is_empty(),
            "{} imported test(s) from the standard library",
            project.program.tests.len()
        );
    }

    /// The loader wins, so a project keeps its own module when the library grows that name.
    #[test]
    fn a_module_beside_the_root_shadows_the_standard_library_module_of_the_same_name() {
        let files = BTreeMap::from([
            (
                "format".to_string(),
                Sources {
                    module: Some(
                        "def fixed(x: Float, places: Int) -> Str:\n    return \"mine\"\n".into(),
                    ),
                    interface: None,
                    path: None,
                },
            ),
            (
                "app".to_string(),
                Sources {
                    module: Some(
                        "import format\n\ndef nine(x: Float) -> Str:\n    return fixed(x, 9)\n"
                            .into(),
                    ),
                    interface: None,
                    path: None,
                },
            ),
        ]);
        let mut diags = Diagnostics::new();
        let mut map = beck_diag::SourceMap::new();
        let project = check_project(
            "app",
            &|n: &str| files.get(n).cloned(),
            None,
            &mut map,
            &mut diags,
        );
        // One `fixed`, not two: the library's copy was never loaded, so there is nothing to clash
        // with (`B0601`).
        assert!(
            !diags.has_errors(),
            "{:?}",
            diags
                .iter()
                .map(|d| (d.code, &d.message))
                .collect::<Vec<_>>()
        );
        assert!(project
            .expect("it links")
            .program
            .defs
            .contains_key("fixed"));
    }

    #[test]
    fn two_modules_defining_one_name_is_an_error_and_not_a_shadowing_rule() {
        let files = BTreeMap::from([
            (
                "lib".to_string(),
                Sources {
                    module: Some("def helper() -> Int:\n    return 1\n".into()),
                    interface: None,
                    path: None,
                },
            ),
            (
                "app".to_string(),
                Sources {
                    module: Some("import lib\n\ndef helper() -> Int:\n    return 2\n".into()),
                    interface: None,
                    path: None,
                },
            ),
        ]);
        let mut diags = Diagnostics::new();
        let mut map = beck_diag::SourceMap::new();
        compile_project(
            "app",
            &|n: &str| files.get(n).cloned(),
            None,
            &mut map,
            &mut diags,
        );
        assert!(
            diags.iter().any(|d| d.code == "B0601"),
            "{:?}",
            diags.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }
}
