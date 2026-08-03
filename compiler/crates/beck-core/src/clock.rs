//! Time, as a thing that is supplied rather than a thing that is ambient.
//!
//! [`docs/14-review-findings.md`](../../../../../docs/14-review-findings.md) F11 records that
//! deterministic simulation cannot be retrofitted and names the constraint: virtualize clock,
//! network and disk from the first line of runtime code.
//! [`docs/13-testing.md`](../../../../../docs/13-testing.md) §13.4 restates it in bold. The runtime
//! then called `SystemTime::now()` directly for three phases anyway
//! ([`docs/42`](../../../../../docs/42-security-assurance.md) §42.4), which is what a constraint with
//! no position in an order gets you.
//!
//! This module is the cheap half of the fix, and deliberately only the cheap half. It is a
//! **seam**, not a simulator: there is no scheduler here, no virtual time, no ordering of events
//! against each other. There is a trait with one implementation that reads the host and one that a
//! caller sets, so that the retrofit F11 forbids never has to happen. `docs/42` §42.4's verdict is
//! exactly that: "adopt the injected clock now; watch DST proper".
//!
//! # What is on the seam and what is not
//!
//! **The wall clock is**: an envelope's `at`, the `now()` primitive, the milliseconds in a
//! time-ordered id, a telemetry timestamp. Those are the readings that enter data — an envelope is
//! logged and replayed, so where its `at` came from is a determinism question.
//!
//! **Elapsed time is not**, yet: `Instant::now()` survives in the places that measure how long
//! something took (`beck bench`, the append and render histograms). A duration measured for a
//! metric does not enter the log, does not reach a fold, and cannot change what a replay produces.
//! It will have to move here when DST proper arrives, and saying so is cheaper than pretending
//! this module already covers it.

use std::sync::Arc;
use std::sync::OnceLock;

/// A source of wall-clock time.
///
/// Implementations are shared across threads and cheap to call. `Debug` is required because the
/// clock rides in configuration that is printed when a process explains itself.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Milliseconds since the Unix epoch.
    fn now_millis(&self) -> i64;

    /// Nanoseconds since the Unix epoch — the unit OTLP asks for.
    ///
    /// Defaulted from [`Clock::now_millis`], so a clock somebody writes for a test has one method.
    fn now_nanos(&self) -> u64 {
        (self.now_millis().max(0) as u64).saturating_mul(1_000_000)
    }
}

/// The host's clock — **the only place in this workspace that reads it**.
///
/// `beck-cli/tests/clock.rs` asserts that, by scanning the tree for the call the way
/// `beck-cli/tests/docs.rs` scans it for diagnostic codes. A second one appearing is the failure
/// F11 describes, caught at the moment it is introduced rather than three phases later.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_millis(&self) -> i64 {
        (self.now_nanos() / 1_000_000) as i64
    }

    /// The one reading. Milliseconds are derived from it rather than taken separately, so the gate
    /// above is about a *call* and not about a file — two calls beside each other would satisfy
    /// "one place" while being exactly the habit the seam exists to end.
    fn now_nanos(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}

/// A clock whose reading is whatever the caller last set.
///
/// This is not a simulator and must not grow into one by accident: it does not advance itself, it
/// has no notion of a pending timer, and nothing schedules against it. It exists so that a test
/// can assert a program's behaviour at a stated instant, and so that the seam has a second
/// implementation — a seam with one implementation is an abstraction nobody has checked.
#[derive(Debug)]
pub struct ManualClock(std::sync::atomic::AtomicI64);

impl ManualClock {
    pub fn at(millis: i64) -> ManualClock {
        ManualClock(std::sync::atomic::AtomicI64::new(millis))
    }

    pub fn set(&self, millis: i64) {
        self.0.store(millis, std::sync::atomic::Ordering::Relaxed);
    }

    /// Move the clock forward. Panics on a negative step, because a wall clock that goes backwards
    /// is a bug in the test rather than a scenario worth supporting.
    pub fn advance(&self, millis: i64) {
        assert!(millis >= 0, "a wall clock does not run backwards");
        self.0
            .fetch_add(millis, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Clock for ManualClock {
    fn now_millis(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

static PROCESS: OnceLock<Arc<dyn Clock>> = OnceLock::new();

/// The clock for readings that have no owner to take one from.
///
/// Telemetry is the case: a metric's timestamp belongs to no application and no evaluation, and
/// threading a clock to it would mean threading one through every counter. Everything that *does*
/// have an owner — the sequencer, the evaluator's host — takes its clock as a parameter and never
/// reads this.
pub fn process_clock() -> &'static Arc<dyn Clock> {
    PROCESS.get_or_init(|| Arc::new(SystemClock))
}

/// Install the process clock. Returns `false` if one has already been read or installed.
///
/// Once, at startup, before anything reads it — which is the only discipline a `OnceLock` can
/// enforce and the reason this returns a bool rather than panicking: a test binary that runs two
/// tests in one process would otherwise abort on the second.
pub fn set_process_clock(clock: Arc<dyn Clock>) -> bool {
    PROCESS.set(clock).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_reads_what_it_was_set_to() {
        let c = ManualClock::at(1_700_000_000_000);
        assert_eq!(c.now_millis(), 1_700_000_000_000);
        c.advance(1_500);
        assert_eq!(c.now_millis(), 1_700_000_001_500);
        assert_eq!(c.now_nanos(), 1_700_000_001_500_000_000);
    }

    #[test]
    fn the_system_clock_is_after_the_date_this_was_written() {
        // Not a precision claim — an assertion that the reading is a Unix epoch in milliseconds
        // and not seconds or nanoseconds, which is the mistake this kind of helper actually makes.
        let ms = SystemClock.now_millis();
        assert!(ms > 1_750_000_000_000, "got {ms}");
        assert!(ms < 100_000_000_000_000, "got {ms}");
    }
}
