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

pub mod check;
pub mod core;
pub mod eval;
pub mod graph;
pub mod html;
pub mod place;
pub mod pmap;
pub mod prelude;
pub mod split;
pub mod ty;

pub use check::{check_module, Def, Program, SignalDecl};
pub use core::{Const, Core, CoreKind, Env, Prim, Value, VarId};
pub use eval::{digest, EvalError, Host, Interp};
pub use graph::{DepGraph, EdgeKind, GraphBuilder, GraphNode, NodeId, NodeKind};
pub use html::Html;
pub use pmap::PMap;
pub use split::{Placed, Roles};
pub use ty::{Effect, Tier, Ty, TyDecl};

use beck_diag::{Diagnostics, FileId, SourceMap};

/// The whole front end: parse, expand, check, place, split.
///
/// One function, so `beck check`, `beck run`, `beck build` and the test harnesses cannot drift
/// apart — §4.6's "one binary serves `beck build`, `beck check`, `beck lsp` and `beck explain`;
/// there is no separate language server implementation to drift."
pub fn compile(file: FileId, name: &str, src: &str, diags: &mut Diagnostics) -> Option<Placed> {
    let parsed = beck_syntax::parse_file(file, name, src, diags);
    let expanded = beck_macro::expand_module(&parsed, diags);
    let program = check_module(&expanded, diags);
    place::check_placement(&program, diags);
    if diags.has_errors() {
        return None;
    }
    split::split(program, diags)
}

/// Compile one source string against a fresh source map. The shape every test uses.
pub fn compile_str(name: &str, src: &str) -> (Option<Placed>, Diagnostics, SourceMap) {
    let mut map = SourceMap::new();
    let file = map.add(name, src);
    let mut diags = Diagnostics::new();
    let placed = compile(file, name, src, &mut diags);
    (placed, diags, map)
}
