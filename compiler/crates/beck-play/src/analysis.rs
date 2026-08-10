//! Rung A: everything the compiler derives from a source string, and nothing else.
//!
//! [`docs/17-playground.md`](../../../../../docs/17-playground.md) §17.1 lists what a visitor gets
//! with zero servers: "type checking with real diagnostics, macro expansion … **inferred placement
//! per definition**, generated dataflow/SQL plans, generated Kubernetes objects, effect signatures,
//! and `beck explain` — source on the left, *what the compiler derives* on the right."
//!
//! Every section below is one call into the compiler, and **not one line of rendering lives here**.
//! `beck explain place` and this page print the same characters because they call the same
//! function; the ones that did not — the placement table, the wire id, a type's flow — were prints
//! in `main.rs` until this crate needed them, and are now [`beck_core::place::report`],
//! [`beck_core::split::wire_report`] and [`beck_core::secure::flow_report`]. That is what
//! `playground.rs::the_playground_shows_what_the_command_line_shows` gates, and it is the whole
//! reason this file is short.

use beck_core::Placed;
use beck_diag::{Diagnostics, SourceMap};

/// One tab on the right-hand side.
pub struct Section {
    /// Stable across renders: the page keeps the selected tab when the source changes.
    pub id: &'static str,
    pub title: &'static str,
    pub text: String,
}

/// What a source string compiles to, as the page shows it.
pub struct Analysis {
    /// Rendered diagnostics — the same text `beck check` writes, spans and all.
    pub diagnostics: String,
    pub errors: usize,
    pub warnings: usize,
    /// Absent when the program did not compile. A diagnostic is then the *only* honest section:
    /// there is no placement for a program that has no types.
    pub sections: Vec<Section>,
    /// Whether this program is an application — a merge point, a page, a fold — and can therefore
    /// be run in the tab (rung B).
    pub runnable: bool,
}

const FILE: &str = "playground.beck";

/// Compile a source string and derive everything §17.1 names.
pub fn analyse(source: &str) -> Analysis {
    let mut map = SourceMap::new();
    let id = map.add(FILE, source);
    let mut diags = Diagnostics::new();

    // The surfaces first, because they survive a program that does not typecheck: a visitor whose
    // program is broken should still be able to see what the macros did to it, which is often why
    // it is broken.
    let parsed = beck_syntax::parse_file(id, FILE, source, &mut diags);
    let mut sections = Vec::new();
    if !diags.has_errors() {
        let expanded = beck_macro::expand_module(&parsed, &mut diags);
        sections.push(Section {
            id: "sexpr",
            title: "S-expressions",
            text: beck_syntax::print::to_sexpr_pretty(&expanded),
        });
        sections.push(Section {
            id: "python",
            title: "Formatted",
            text: beck_syntax::print::to_python(&parsed),
        });
    }

    let placed = beck_core::compile(id, FILE, source, &mut diags);

    // A module with no merge point is a **library**, not a mistake — and a visitor pasting three
    // definitions into an empty editor is writing one. `beck check` says "ok: … a library", and a
    // page that answered the same source with three red errors would be teaching that a Beck
    // program has to be an application.
    if placed.is_none() && is_a_library(&diags) {
        sections.splice(0..0, of_a_library(source));
        return Analysis {
            diagnostics: format!(
                "{}\nA library: no merge point, so there is nothing to run — the sections that \
                 need an application (the dataflow plan, the read model, the infrastructure) are \
                 not here. `beck iface` publishes what it offers.\n",
                diags.render(&map)
            ),
            errors: 0,
            warnings: 0,
            runnable: false,
            sections,
        };
    }

    let diagnostics = diags.render(&map);
    let errors = diags
        .iter()
        .filter(|d| d.severity == beck_diag::Severity::Error)
        .count();
    let warnings = diags.len() - errors;

    if let Some(placed) = &placed {
        sections.splice(0..0, derived(placed));
    }

    Analysis {
        diagnostics,
        errors,
        warnings,
        runnable: placed.is_some(),
        sections,
    }
}

/// Whether the only thing wrong with this module is that it is not an application.
///
/// The three codes are `beck_core::project`'s own list, so a fourth reason to refuse a slice is
/// covered here the day it is added rather than the day somebody notices the playground calling it
/// an error.
fn is_a_library(diags: &Diagnostics) -> bool {
    diags
        .iter()
        .filter(|d| d.severity == beck_diag::Severity::Error)
        .all(|d| beck_core::project::NOT_AN_APPLICATION.contains(&d.code))
}

/// What a library can still be asked: where its definitions run, and what it publishes.
///
/// Solved rather than sliced. Everything else §17.1 lists is a question about an application — a
/// plan is a plan *of* a view, and a NetworkPolicy is derived from what a deployment would do.
fn of_a_library(source: &str) -> Vec<Section> {
    let (mut program, _, _) = beck_core::check_str(FILE, source);
    let solution = beck_core::place::solve(&program, None);
    beck_core::place::apply(&mut program, &solution);
    vec![
        Section {
            id: "place",
            title: "Placement",
            text: beck_core::place::report(&solution, None).unwrap_or_else(|why| why.to_string()),
        },
        Section {
            id: "iface",
            title: "Signatures",
            text: beck_core::iface::Interface::of(&program).render(),
        },
    ]
}

/// The sections that need a compiled program, in the order the page shows them.
fn derived(placed: &Placed) -> Vec<Section> {
    let plan = beck_core::plan::Plan::compile(placed);
    let unfused = beck_core::plan::Plan::unfused(placed);
    let (fused, fusions) = beck_core::fuse::fuse(unfused);
    let infra = beck_infra::graph(placed);

    vec![
        Section {
            id: "place",
            title: "Placement",
            text: beck_core::place::report(&placed.placement, None)
                .unwrap_or_else(|why| why.to_string()),
        },
        Section {
            id: "iface",
            title: "Signatures",
            text: beck_core::iface::Interface::of(&placed.program).render(),
        },
        Section {
            id: "flow",
            title: "Signal graph",
            text: beck_core::split::flow_report(placed),
        },
        Section {
            id: "render",
            title: "Render mode",
            text: placed.render.explain(&beck_core::Bundle::of(placed)),
        },
        Section {
            id: "query",
            title: "Dataflow",
            text: format!(
                "{}{}",
                beck_core::plan::query_report(&fused),
                beck_core::fuse::report(&fusions)
            ),
        },
        Section {
            id: "incremental",
            title: "Incremental",
            text: beck_core::incremental::report(placed, None),
        },
        Section {
            id: "cost",
            title: "Cost per event",
            text: beck_core::plan::cost_report(&plan),
        },
        Section {
            id: "sql",
            title: "Read model",
            text: beck_core::read::Schema::of(placed, &plan).ddl(),
        },
        Section {
            id: "deploy",
            title: "Infrastructure",
            text: infra.explain(),
        },
        Section {
            id: "k8s",
            title: "Kubernetes",
            text: manifests(&infra, &placed.wire_id),
        },
        Section {
            id: "wire",
            title: "Wire",
            text: beck_core::split::wire_report(placed),
        },
    ]
}

/// Every generated object, in one document, the way `beck build` writes them — one file per
/// heading, so what the page shows and what the directory would contain are the same objects.
fn manifests(infra: &beck_infra::InfraGraph, wire_id: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (name, yaml) in beck_infra::k8s::render(infra, wire_id) {
        let _ = writeln!(out, "# {name}\n{yaml}");
    }
    out
}
