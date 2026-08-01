//! Types, `Core`, placement and splitting — stages 4 through 8 of §4.1's pipeline.
//!
//! ```text
//!  4 Resolve    modules, imports, name binding, hygiene scopes
//!  5 Typecheck  HM → typed AST
//!  6 Lower      desugar to CORE
//!  7 PLACE      ◀── the product.  every Core node carries a tier
//!  8 Split      partition Core per tier; SYNTHESISE boundaries
//! ```
//!
//! Stages 4–6 are one pass ([`check`]) because §4.2 permits three IRs and no more: a separate
//! resolved-but-untyped tree would be a fourth. Stages 7 and 8 are [`place`] and [`split`], which
//! §4.1 calls out as "novel … where the engineering budget goes".

pub mod backend;
pub mod check;
pub mod compat;
pub mod core;
pub mod cost;
pub mod gen;
pub mod graph;
pub mod html;
pub mod iface;
pub mod place;
pub mod pmap;
pub mod prelude;
pub mod project;
pub mod repr;
pub mod row;
pub mod secure;
pub mod signal;
pub mod split;
pub mod testing;
pub mod ty;

pub use backend::{Backend, Callable, ExecError};
pub use check::{check_module, Def, Program, SignalDecl};
pub use compat::{compare, is_breaking, Change};
pub use core::{digest, Const, Core, CoreKind, Env, Prim, Value, VarId};
pub use graph::{DepGraph, EdgeKind, GraphBuilder, GraphNode, NodeId, NodeKind};
pub use html::Html;
pub use iface::Interface;
pub use place::{Key, Lock, Method, Solution};
pub use pmap::PMap;
pub use project::{compile_project, Sources};
pub use row::{Ambient, Effect, Row};
pub use secure::{sendable, storable, NotSendable};
pub use signal::{Cut, Graph as SignalGraph, Op as SignalOp, SigId};
pub use split::{Placed, Roles, StateRole};
pub use testing::{Clause, Expectation, TestDef};
pub use ty::{Tier, Ty, TyDecl};

use beck_diag::{Diagnostics, FileId, SourceMap};

/// The whole front end: parse, expand, check, place, split.
///
/// One function, so `beck check`, `beck run`, `beck build` and the test harnesses cannot drift
/// apart — §4.6's "one binary serves `beck build`, `beck check`, `beck lsp` and `beck explain`;
/// there is no separate language server implementation to drift."
pub fn compile(file: FileId, name: &str, src: &str, diags: &mut Diagnostics) -> Option<Placed> {
    compile_with(file, name, src, None, diags)
}

/// The same, against a previously solved placement — §3.4's stability guardrail.
pub fn compile_with(
    file: FileId,
    name: &str,
    src: &str,
    lock: Option<&Lock>,
    diags: &mut Diagnostics,
) -> Option<Placed> {
    let parsed = beck_syntax::parse_file(file, name, src, diags);
    let expanded = beck_macro::expand_module(&parsed, diags);
    let mut program = check_module(&expanded, diags);
    // Stage 7: solve first, then verify. Verification runs over the *solved* tiers as well as the
    // written ones, so an annotation and an inference are held to one standard.
    let solution = place::solve(&program, lock);
    place::apply(&mut program, &solution);
    place::check_placement(&program, diags);
    secure::check_security(&program, diags);
    if diags.has_errors() {
        return None;
    }
    let mut placed = split::split(program, diags)?;
    placed.placement = solution;
    Some(placed)
}

/// Parse, expand and check one source string, stopping before placement.
///
/// The shape a test that is interested in *inference* wants: a program whose rows are known even
/// when its placement is the thing under test.
pub fn check_str(name: &str, src: &str) -> (check::Program, Diagnostics, SourceMap) {
    let mut map = SourceMap::new();
    let file = map.add(name, src);
    let mut diags = Diagnostics::new();
    let parsed = beck_syntax::parse_file(file, name, src, &mut diags);
    let expanded = beck_macro::expand_module(&parsed, &mut diags);
    let program = check_module(&expanded, &mut diags);
    (program, diags, map)
}

/// Compile one source string against a fresh source map. The shape every test uses.
pub fn compile_str(name: &str, src: &str) -> (Option<Placed>, Diagnostics, SourceMap) {
    let mut map = SourceMap::new();
    let file = map.add(name, src);
    let mut diags = Diagnostics::new();
    let placed = compile(file, name, src, &mut diags);
    (placed, diags, map)
}
