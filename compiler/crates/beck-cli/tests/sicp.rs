//! The SICP suite's first chapter, and the walls the rest of it hits.
//!
//! [`docs/25-benchmarks-and-expressiveness.md`] proposes SICP as Beck's expressiveness benchmark,
//! on the grounds that the project's own premise ([`docs/01`] §1.1) is that Beck is SICP's three
//! moves made into a language. A proposal is not evidence, so this harness holds the report's
//! claims to the standard `AGENTS.md` sets: every number in §25.6 is produced by something here.
//!
//! Two halves, and the second is the point:
//!
//! * [`sicp/ch1.beck`](../../../sicp/ch1.beck) is the part of chapter 1 that runs today, with the
//!   book's own stated answers as the oracle. It passes.
//! * [`sicp/refusals/`](../../../sicp/refusals/) is one file per wall, each the smallest program
//!   that hits it. This harness asserts each one *still* fails, and with which diagnostic — so
//!   that a wall coming down is a test that starts failing rather than a fact somebody notices.
//!
//! A refusal here is not a bug report against the compiler. Five of the six are features Beck has
//! not built yet and the roadmap names; the sixth (`higher-order.beck`) is a defect, and is
//! labelled as one.

use std::process::Command;

use beck_core::Placed;
use beck_rt::testing::Options;

fn compile(name: &str, src: &str) -> (Option<Placed>, String) {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    (placed, diags.render(&map))
}

/// The whole diagnostic text, for asserting on a code *and* on what it says.
fn errors(name: &str, src: &str) -> String {
    let (_, rendered) = compile(name, src);
    assert!(
        rendered.contains("error["),
        "`{name}` was expected to be refused, and it compiled:\n{rendered}"
    );
    rendered
}

// ---------------------------------------------------------------------------------------------
// 1. Chapter 1 runs, and the book is the oracle
// ---------------------------------------------------------------------------------------------

const CH1: &str = include_str!("../../../sicp/ch1.beck");

/// Chapter 1 needs more stack than `libtest` hands a test thread, and *that is the finding*.
///
/// §25.6 item 5: the evaluator has no proper tail calls and spends host stack per Beck-level call.
/// A debug build spends several times more of it per frame, and `libtest` runs each test on a
/// thread with ~2 MiB rather than the main thread's 8 — so `beck test sicp/ch1.beck` passes from
/// the command line, in both profiles, while the same thirteen tests abort inside the harness.
/// That combination is worth stating plainly: **the suite's own evidence for the missing-tail-call
/// gap is that collecting the evidence trips over it.**
///
/// Raising the stack here is the honest fix rather than shrinking the exercises: `count_change(100)`
/// is 292 because SICP says it is 292, and trimming the book's own answer to fit an interpreter's
/// frame size would be measuring the harness instead of the language. When tail calls land, this
/// wrapper goes away and the `RUST_MIN_STACK`-shaped workaround goes with it.
const CH1_STACK: usize = 32 * 1024 * 1024;

#[test]
fn chapter_one_passes_against_the_books_own_answers() {
    std::thread::Builder::new()
        .stack_size(CH1_STACK)
        .spawn(|| {
            let (placed, rendered) = compile("sicp/ch1.beck", CH1);
            let placed = placed.unwrap_or_else(|| panic!("chapter 1 compiles:\n{rendered}"));

            let backend = beck_eval::backend(&placed);
            let report = beck_rt::testing::run(&placed, backend, &Options::default());

            assert!(
                report.cases.len() >= 13,
                "chapter 1 is the evidence for §25.6 and has to carry the exercises it claims"
            );
            assert_eq!(
                report.failed(),
                0,
                "{}",
                beck_rt::testing::render(&report, true)
            );
            assert_eq!(
                report.skipped(),
                0,
                "nothing in chapter 1 performs an effect"
            );
        })
        .expect("a thread")
        .join()
        .expect("chapter 1 runs without exhausting a 32 MiB stack");
}

#[test]
fn chapter_one_needs_an_application_wrapped_around_it_to_run_at_all() {
    // The wrapper in `ch1.beck` is not decoration: strip it and the same procedures, with the
    // same tests, cannot be run. This is the suite's first ask (§25.7 item 1), and it is asserted
    // rather than asserted-about.
    let src = include_str!("../../../sicp/refusals/library.beck");
    let (placed, rendered) = compile("library.beck", src);
    assert!(
        placed.is_none() && rendered.contains("B0500"),
        "a library has nothing to run:\n{rendered}"
    );
    // The split is the whole finding. `beck check` treats B0500 as "this is a library" and prints
    // `ok` (`main.rs`, `NOT_AN_APPLICATION`); `beck test` is built on `Placed` and has nothing to
    // drive, so it reports the same diagnostic as an error and exits non-zero.
    assert!(
        beck_core::project::NOT_AN_APPLICATION.contains(&"B0500"),
        "and the compiler already knows it is a library rather than a broken application"
    );
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sicp/refusals/library.beck"
    );
    let check = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["check", file])
        .output()
        .expect("the compiler is built");
    assert!(check.status.success(), "`beck check` accepts a library");
    let test = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["test", file])
        .output()
        .expect("the compiler is built");
    assert!(
        !test.status.success(),
        "`beck test` cannot run one — docs/22 §22.6, and §25.7 item 1"
    );
}

// ---------------------------------------------------------------------------------------------
// 2. The walls, each still standing
// ---------------------------------------------------------------------------------------------

#[test]
fn a_type_cannot_mention_itself_so_chapter_two_stops_at_the_closure_property() {
    let out = errors(
        "recursive-type.beck",
        include_str!("../../../sicp/refusals/recursive-type.beck"),
    );
    assert!(out.contains("B0310"), "{out}");
    assert!(out.contains("cannot find type `Tree`"), "{out}");
}

#[test]
fn a_user_cannot_write_a_polymorphic_definition_so_map_cannot_be_built() {
    let out = errors(
        "generic.beck",
        include_str!("../../../sicp/refusals/generic.beck"),
    );
    assert!(out.contains("B0120"), "{out}");
}

#[test]
fn there_is_no_real_arithmetic_so_newtons_method_does_not_typecheck() {
    let out = errors(
        "real.beck",
        include_str!("../../../sicp/refusals/real.beck"),
    );
    assert!(out.contains("B0320"), "{out}");
    assert!(
        out.contains("expected `Int`, found `Float`"),
        "the wall is that `+` is Int-only, not that `Float` is unknown:\n{out}"
    );
}

#[test]
fn an_if_over_two_function_values_is_refused_when_one_of_them_is_a_calls_result() {
    // Exercise 1.43. Unlike its five neighbours this is a defect: the branches' effect rows are
    // unified against each other rather than against a fresh variable, and the message renders
    // the empty row where a user is owed a sentence. When this is fixed, `repeated` goes back
    // into `ch1.beck` and this test is deleted rather than adjusted.
    let out = errors(
        "higher-order.beck",
        include_str!("../../../sicp/refusals/higher-order.beck"),
    );
    assert!(out.contains("B0320"), "{out}");
    assert!(
        out.contains("may not perform {}"),
        "§25.6 quotes this rendering as evidence that it is unactionable:\n{out}"
    );
}

#[test]
fn a_tail_call_consumes_stack_so_an_iterative_process_is_not_iterative() {
    // §1.2.1's distinction, which Beck cannot currently make. This one is run through the binary
    // rather than in-process because the failure is a `SIGABRT`, not a `Result` — which is itself
    // half of the finding.
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sicp/refusals/tail.beck");
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["test", file])
        .output()
        .expect("the compiler is built");

    assert!(
        !out.status.success(),
        "a tail call eight thousand deep is expected to die; if this passes, Beck grew proper \
         tail calls and §25.6 needs rewriting (happily)"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("overflowed its stack"),
        "and to die by exhausting the host stack specifically:\n{err}"
    );
}
