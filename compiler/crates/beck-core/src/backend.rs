//! The seam between `Core` and something that can execute it.
//!
//! # Why this exists
//!
//! `docs/05-tier-lowering.md` §5.2 says the `Core → Target` seam is what lets a backend slot in
//! later, and `docs/04-compiler-architecture.md` §4.8 names a differential test *between backends*
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

/// Something that answers a call *instead of* the definition it names.
///
/// This exists for exactly one caller, and the reason it is on the seam rather than inside a
/// backend is `docs/21-tests-in-beck-and-proof.md` §21.3: "**A mock is not a stand-in for an
/// object. It is a value for an effect.**" A stub is therefore not a program transformation the
/// compiler can do once — the *complete list* of what got stubbed has to be reportable per test,
/// with the arguments each stubbed call was passed, because §21.3 rule 4 makes verification a query
/// over what happened rather than an expectation set in advance.
///
/// A backend that cannot offer this says so by returning `None` from [`Backend::intercepting`], and
/// the harness reports that stubs are unavailable rather than running the test and lying about it.
pub trait Interceptor: Send + Sync {
    /// Called before a top-level definition named `name` is applied to `args`. Returning `Some`
    /// replaces the call; returning `None` runs the real body.
    fn intercept(&self, name: &str, args: &[Value]) -> Option<Value>;
}

/// A backend's running count of what it has executed, if it keeps one.
///
/// # Why the seam carries this at all
///
/// [`crate::engine::Work`] counts what the *engine* does — functions applied, arrangement entries
/// moved, pointwise operators re-evaluated — and one application is one application whatever that
/// application goes on to do. When a plan's per-element function is a whole page, the counters say
/// three and the clock says tenfold, and the failure is silent and flatters the plan that hides the
/// most: every shape gate over an opaque operator was blind to exactly the pessimisation an opaque
/// operator can hide.
///
/// The count has to come from whatever executed the code, so it comes through here. It is a
/// **count**, not a duration, for the reason every other number a gate asserts on is:
/// [`docs/13`](../../../../../docs/13-testing.md) §13.7 says a shared runner cannot hold a timing
/// gate honestly.
///
/// # What a step is, and what it is not
///
/// Deliberately unspecified across backends. The tree-walker's is its own evaluation budget — a
/// node, plus a charge per element for a primitive whose work is proportional to a length the
/// caller chose — so it is *comparable between two runs of the same backend* and means nothing
/// between two backends. A gate that reads it is asking "did this plan do more work than that
/// one", never "how long did it take".
pub trait Steps: Send + Sync {
    /// Steps this backend has executed since it was created, monotonically.
    fn taken(&self) -> u64;
}

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

    /// The same program, executed with an [`Interceptor`] consulted at every call of a top-level
    /// definition. `None` — the default — means this backend cannot do it.
    ///
    /// Defaulted rather than required because it is not part of *executing a program*: a backend
    /// that only ever runs an application in production has no reason to carry it, and the seam
    /// should not grow a method every host must implement to serve one command.
    fn intercepting(&self, _by: Arc<dyn Interceptor>) -> Option<Arc<dyn Backend>> {
        None
    }

    /// How much host stack a thread must have before it calls into this backend.
    ///
    /// Zero — the default — means "whatever the caller has", which is the honest answer for a
    /// backend that compiles to a machine-code loop and never nests host frames on the program's
    /// recursion. A tree-walker does nest, and needs to say so: `docs/27` §27.2 records what
    /// leaving it unsaid cost, which was a `SIGSEGV` where a diagnostic belonged.
    ///
    /// It is part of the seam rather than of one crate because the *runtime* is what spawns
    /// threads and the runtime may not name a backend crate (`docs/19` §19.9). Asking the backend
    /// it was handed is how it finds out without one.
    fn stack_bytes(&self) -> usize {
        0
    }

    /// This backend's step counter, if it keeps one — see [`Steps`].
    ///
    /// `None` — the default — is the honest answer for a backend that compiles to machine code and
    /// has nothing to count without instrumenting what it emitted. A caller that needs the number
    /// says so by refusing rather than by reading a zero as "no work", which is the failure this
    /// exists to end.
    fn steps(&self) -> Option<Arc<dyn Steps>> {
        None
    }
}
