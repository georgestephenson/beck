//! The seam between `Core` and something that can execute it.
//!
//! # Why this exists
//!
//! [`docs/05-tier-lowering.md`] §5.2 says the `Core → Target` seam is what lets a backend slot in
//! later, and [`docs/04-compiler-architecture.md`] §4.8 names a differential test *between backends*
//! as a thing the project will need. Neither is possible while the host calls a particular
//! evaluator by name.
//!
//! Until this module existed, `beck-rt` constructed `Interp` directly in four places. That is not a
//! narrow Phase 1 — it is a Phase 1 whose successor is a refactor rather than an addition. The trait
//! below is the whole interface a host needs, so a native backend is a new crate that implements it
//! and a line that chooses it, and the two can be run against each other on the same program.
//!
//! # The shape, and why it is this small
//!
//! A host needs exactly two things from an executor: turn a closed `Core` expression into a value
//! (the fold's initial state), and turn one denoting a function into something callable
//! (`validate`, the fold, the view). Everything else — environments, closures, fuel — is a detail
//! of *how* a backend executes, and a tree-walker and a JIT do not agree on any of it.
//!
//! So [`Backend::function`] returns a [`Callable`] rather than a backend-specific handle. There is
//! no `call(handle, args)` method to downcast through, and no `Value::Closure` in the interface —
//! that variant is the tree-walker's representation and a compiled backend would not produce one.

use std::sync::Arc;

use beck_diag::Span;

use crate::core::{Core, Value};

/// A failure while executing `Core`.
///
/// Carries a span because a language server has to survive evaluating half-written code, and
/// because "folding at seq 41 failed" is not an answer without a location.
#[derive(Clone, Debug)]
pub struct ExecError {
    pub message: String,
    pub span: Span,
}

impl ExecError {
    pub fn new(message: impl Into<String>, span: Span) -> ExecError {
        ExecError {
            message: message.into(),
            span,
        }
    }
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExecError {}

/// A function a host can call, however the backend made it.
///
/// `'static` and `Send + Sync` because the runtime calls the fold from a sequencer task and the
/// view from a connection task, and a backend that cannot survive that is not a backend for this
/// runtime.
pub type Callable = Arc<dyn Fn(Vec<Value>) -> Result<Value, ExecError> + Send + Sync>;

/// A way to execute `Core`.
pub trait Backend: Send + Sync {
    /// What to call this in a diagnostic or on a dashboard. Two backends running differentially
    /// need to be distinguishable in the report that says they disagreed.
    fn name(&self) -> &'static str;

    /// Reduce a closed expression to a value — the fold's initial accumulator.
    fn constant(&self, code: &Core) -> Result<Value, ExecError>;

    /// Prepare an expression denoting a function for calling.
    ///
    /// Called once per role at startup, so a backend that compiles is free to do the expensive
    /// thing here rather than on every event.
    fn function(&self, code: &Core) -> Result<Callable, ExecError>;
}
