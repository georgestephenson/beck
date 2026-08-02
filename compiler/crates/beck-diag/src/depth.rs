//! The front end's nesting ceiling: a count, and the host stack that count implies.
//!
//! The front end recurses over structure the user chose — nested parentheses, nested blocks, a
//! macro that expands into more of itself, a type inside a type. Every one of those is a host
//! frame, and until this module existed nothing counted them:
//! [`docs/42-security-assurance.md`](../../../../docs/42-security-assurance.md) §42.2 measured an
//! ~7.6 KB file that aborted `beck check` in a debug build with no span and nothing catchable.
//!
//! The bound is a **count**, for the reason
//! [`docs/adr/0007`](../../../../docs/adr/0007-evaluator-stack-is-declared-not-discovered.md) gave
//! for the evaluator's: a stack-headroom budget accepts a program in a release build and refuses it
//! in a debug one, and a diagnostic that depends on the profile is not a diagnostic.
//! [`docs/adr/0012`](../../../../docs/adr/0012-the-front-end-counts-its-own-recursion.md) makes
//! that argument for the front end.
//!
//! It lives in this crate because three crates share it — `beck-syntax` reads, `beck-macro`
//! expands, `beck-core` checks — and a ceiling with three definitions is three ceilings.

/// How deep the front end will follow user-chosen structure before it refuses.
///
/// The number is chosen to be far above anything a person writes and far below anything the stack
/// notices. SICP's deepest expression is under 20 levels and the corpus's is 11; the parser spends
/// about 18 KiB per level in an unoptimised build, which `the_ceiling_fits_the_declared_stack`
/// measures rather than assumes, so this ceiling costs under 5 MiB — inside the 8 MiB a main
/// thread ordinarily has, and a small fraction of the [`STACK_BYTES`] declared below. A ceiling
/// nobody legitimately reaches is the point: this bound exists to turn an abort into a message,
/// not to have an opinion about style.
pub const MAX_NESTING: u32 = 256;

/// The host stack the front end needs to reach [`MAX_NESTING`] on every one of its recursions.
///
/// Declared rather than discovered, and held to the ceiling by a test in each crate that recurses
/// (`beck-syntax`, `beck-core`) which *measures* bytes per level and fails if the declaration has
/// stopped covering it — the pair `beck-eval` has had since `docs/31` §31.3, for the same reason
/// and against the same failure.
///
/// It is deliberately the same 64 MiB `beck_eval::STACK_BYTES` declares, because the two are
/// consumers of *one* thread: `beck-cli` compiles and evaluates on the stack it dispatches onto,
/// and `the_front_end_fits_the_stack_the_cli_gives_it` is what keeps the two numbers honest about
/// each other. They are not summed: a compilation has finished reading before it begins running.
pub const STACK_BYTES: usize = 64 * 1024 * 1024;

/// Run `f` on a thread that has [`STACK_BYTES`], and give back what it returned.
///
/// The counterpart of `beck_eval::on_the_evaluator_stack`, and the answer to "who guarantees the
/// count is reachable" for the front end. A caller already inside `on_the_evaluator_stack` — which
/// is every path through `beck-cli` — needs neither, because the two declare the same number.
pub fn on_the_front_end_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(STACK_BYTES)
            .name("beck-front-end".into())
            .spawn_scoped(scope, f)
            .expect("a thread for the front end")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// A recursion counter, held by whatever is recursing.
///
/// The discipline is [`enter`](Nesting::enter) at the recursion site and [`leave`](Nesting::leave)
/// on the way out — at the *site*, not at one grammar rule that seemed to be where nesting comes
/// from. That is the Scriban lesson (GHSA-p6q4-fgr8-vx4p, §42.2): a limit added at the one
/// production somebody thought of was bypassed through a different one.
#[derive(Debug)]
pub struct Nesting {
    depth: u32,
    limit: u32,
    reported: bool,
}

impl Default for Nesting {
    fn default() -> Nesting {
        Nesting::new()
    }
}

impl Nesting {
    pub fn new() -> Nesting {
        Nesting::with_limit(MAX_NESTING)
    }

    /// A counter with a lower ceiling, for a test that would rather not build a 256-deep input.
    pub fn with_limit(limit: u32) -> Nesting {
        Nesting {
            depth: 0,
            limit,
            reported: false,
        }
    }

    /// A counter for a sub-parse that continues this one, starting at the depth already reached.
    ///
    /// A sub-parser over a captured token run is still inside whatever brackets captured it, and a
    /// counter that started again at zero would be a way in — the same shape of bypass the Scriban
    /// advisory records.
    pub fn resumed(&self) -> Nesting {
        Nesting {
            depth: self.depth,
            limit: self.limit,
            reported: self.reported,
        }
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }

    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Descend one level. `false` means the ceiling is reached and the caller must not recurse —
    /// and must not [`leave`](Nesting::leave) either, because nothing was entered.
    #[must_use]
    pub fn enter(&mut self) -> bool {
        #[cfg(feature = "probe")]
        probe::mark();
        if self.depth >= self.limit {
            return false;
        }
        self.depth += 1;
        true
    }

    pub fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// True exactly once per compilation.
    ///
    /// One over-deep expression is refused at every level on the way out, and a reader wants the
    /// count, not one copy of it per level.
    pub fn should_report(&mut self) -> bool {
        !std::mem::replace(&mut self.reported, true)
    }

    /// The note every site prints, so the three of them say the same thing.
    pub fn note(&self) -> String {
        format!(
            "the front end follows at most {} levels of nesting; this is a fixed count rather \
             than a reading of the stack, so a program is accepted or refused identically in \
             every build",
            self.limit
        )
    }
}

/// The stack-address recorder the ceiling's adequacy is measured with.
///
/// Compiled only under the `probe` feature, which nothing but a test enables. It exists because
/// [`STACK_BYTES`] is a declaration, and a declaration nobody checks is the thing
/// [`docs/42`](../../../../docs/42-security-assurance.md) §42.2 found: 64 MiB sized for one
/// recursive consumer of the stack and already false for another.
#[cfg(feature = "probe")]
pub mod probe {
    use std::cell::Cell;

    thread_local! {
        static DEEPEST: Cell<usize> = const { Cell::new(usize::MAX) };
    }

    /// Called from [`super::Nesting::enter`]: record the deepest address the recursion has reached.
    pub fn mark() {
        let here = 0u8;
        let here = std::ptr::addr_of!(here) as usize;
        DEEPEST.with(|d| {
            if here < d.get() {
                d.set(here);
            }
        });
    }

    /// Run `f` and give back the host stack, in bytes, that the recursion inside it spent.
    ///
    /// The measurement asserts the stack grows downwards rather than assuming it, because that is
    /// a property of the platform and not of this code.
    pub fn stack_spent<T>(f: impl FnOnce() -> T) -> usize {
        let top = 0u8;
        let top = std::ptr::addr_of!(top) as usize;
        DEEPEST.with(|d| d.set(usize::MAX));
        let _ = f();
        let deepest = DEEPEST.with(|d| d.get());
        assert!(
            deepest < top,
            "the probe saw no recursion at all, or the host stack does not grow downwards"
        );
        top - deepest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_refuses_at_the_ceiling_and_recovers_on_the_way_out() {
        let mut n = Nesting::with_limit(3);
        assert!(n.enter() && n.enter() && n.enter());
        assert!(
            !n.enter(),
            "the fourth level is one past a ceiling of three"
        );
        n.leave();
        assert!(n.enter(), "and leaving makes room again");
    }

    #[test]
    fn the_refusal_is_reported_once_however_many_levels_unwind() {
        let mut n = Nesting::new();
        assert!(n.should_report());
        assert!(!n.should_report());
    }
}
