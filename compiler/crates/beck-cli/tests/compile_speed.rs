//! The compile-speed budget: a *shape* rather than a rate.
//!
//! [`docs/25-benchmarks-and-expressiveness.md`](../../../../docs/25-benchmarks-and-expressiveness.md)
//! §25.9 schedules compile-speed budgets for Phase 3, and
//! [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7 lists them among the numbers every
//! merge answers to. This file is the gate; [`measure_compile.rs`](measure_compile.rs) is the
//! table.
//!
//! # Why it asserts a shape
//!
//! §13.7's own rule — "a gate that flakes gets deleted" — rules out a wall-clock threshold on a
//! shared runner. What survives is what [`scaling.rs`](scaling.rs) already does for the fold:
//! assert that **cost per declaration does not grow with the number of declarations**. That is the
//! regression a compile-speed budget is actually for, because a constant factor is a nuisance and
//! an exponent is a wall.
//!
//! It caught one on the day it was written ([`docs/64`](../../../../docs/64-compile-speed-report.md)
//! §64.2): placement's explanation loop re-summed the whole program three times per definition, so
//! the front end was quadratic in a module's width. 12,800 definitions took 6.2 s and now take
//! 0.58 s.
//!
//! # The bound
//!
//! **4.0× per-declaration growth across a 16× increase in declarations**, and both numbers were
//! chosen from measurements rather than from a round-number instinct:
//!
//! | | measured |
//! |---|---|
//! | the defect §64.2 removed | **×6.00** — caught, with half again to spare |
//! | width, fixed | ×1.17 |
//! | width with an edge per definition, fixed | ×2.62 — the residual §64.3 records as *not* fixed |
//! | depth | ×1.04 |
//!
//! The range is 16× rather than `scaling.rs`'s 8× because 8× did not leave enough room: over that
//! range the same defect measures ×3.11 against a bound of 3.0, which is a gate that catches the
//! bug it was written for and might not catch the next one. Widening the range separates the
//! signal from the bound instead of tightening the bound against the noise.
//!
//! **One test, not three.** The three axes are measured in sequence inside a single `#[test]`,
//! because libtest runs tests in a binary concurrently and three of these hammering the CPU at
//! once measures the contention rather than the compiler. The first draft did exactly that and
//! reported ×2.13 on an axis that measures ×1.4 alone, which is the flake §13.7 warns about,
//! caught before it was committed rather than after.

use std::time::{Duration, Instant};

use beck_core::{check_module, place, secure};
use beck_diag::{Diagnostics, SourceMap};

/// The whole front end over one source, timed. The same sequence `beck_core::compile` runs.
fn front_end(name: &str, src: &str) -> Duration {
    let mut map = SourceMap::new();
    let file = map.add(name, src);
    let mut diags = Diagnostics::new();

    let started = Instant::now();
    let parsed = beck_syntax::parse_file(file, name, src, &mut diags);
    let expanded = beck_macro::expand_module(&parsed, &mut diags);
    let mut program = check_module(&expanded, &mut diags);
    let solution = place::solve(&program, None);
    place::apply(&mut program, &solution);
    place::check_placement(&program, &mut diags);
    secure::check_security(&program, &mut diags);
    let elapsed = started.elapsed();

    assert!(
        !diags.has_errors(),
        "the generated program has to compile, or this measures error recovery:\n{}",
        diags.render(&map)
    );
    elapsed
}

/// The best of five, not the median: the *floor* is the least noisy statistic a shared runner
/// offers, because interference can only ever add time.
fn best_of_five(name: &str, src: &str) -> Duration {
    beck_diag::depth::on_the_front_end_stack(|| {
        (0..5)
            .map(|_| front_end(name, src))
            .min()
            .expect("five runs")
    })
}

/// Cost per declaration at `small` and at `large`, and the ratio between them.
fn growth(gen: fn(usize) -> String, small: usize, large: usize, what: &str) -> f64 {
    let per = |n: usize| best_of_five("budget.beck", &gen(n)).as_secs_f64() / n as f64;
    let (a, b) = (per(small), per(large));
    let ratio = b / a;
    println!(
        "{what}: {small} → {large} declarations, {:.2} → {:.2} µs each — ×{ratio:.2}",
        a * 1e6,
        b * 1e6
    );
    ratio
}

/// A module `n` top-level definitions wide.
fn wide(n: usize) -> String {
    (0..n)
        .map(|i| format!("def f{i}(x: Int) -> Int:\n    return x + {i}\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One definition with `n` sequential local bindings, each reading the last.
fn deep(n: usize) -> String {
    let mut out = String::from("def deep(x: Int) -> Int:\n    v0 = x + 1\n");
    for i in 1..n {
        out.push_str(&format!("    v{i} = v{} + {i}\n", i - 1));
    }
    out.push_str(&format!("    return v{}\n", n - 1));
    out
}

/// A module that calls what it declares, so the dependency graph grows with it.
///
/// `wide` has no edges at all, and placement's cost is a function of the *graph* rather than of the
/// declaration count — so a program whose declarations never mention each other would not have
/// caught §64.2 and does not guard against its return. Each definition calls the one before it.
fn chained(n: usize) -> String {
    let mut out = String::from("def g0(x: Int) -> Int:\n    return x + 1\n");
    for i in 1..n {
        out.push_str(&format!(
            "\ndef g{i}(x: Int) -> Int:\n    return g{}(x) + {i}\n",
            i - 1
        ));
    }
    out
}

/// The bound, and the reasoning is in the module docs.
const MAX_GROWTH: f64 = 4.0;

#[test]
fn the_front_end_cost_per_declaration_does_not_grow_with_a_module() {
    // Three axes, in one test and in sequence. Each is a different quadratic: re-resolving every
    // declaration per declaration is the first, summing the whole dependency graph per node is the
    // second (docs/64 §64.2, which this caught), and re-walking the enclosing scope per binding is
    // the third. A program that grows along one is flat along the others, so none of the three is
    // a duplicate.
    // Each axis names its own pair of sizes, and only the **ratio** between them matters: this
    // gate asserts a shape — cost per declaration must not grow with the declaration count — so
    // sixteen times as many is the measurement whatever the absolute numbers are (docs/64 §64.7).
    //
    // `depth` runs smaller than the other two for a reason that is not about speed. A flat body is
    // bounded at `beck_diag::depth::MAX_BLOCK` statements (`docs/85`), because the checker recurses
    // once per statement and an unbounded body aborted the process — so 6,400 bindings in one
    // function is now a program the front end refuses, and a gate that measured it would be
    // measuring error recovery. The ceiling is a safety property and cannot move to suit a
    // benchmark; the ratio is arbitrary and can.
    for (gen, axis, small, large, note) in [
        (
            wide as fn(usize) -> String,
            "width",
            400,
            6_400,
            "the front end has gone superlinear in a module's width",
        ),
        (
            chained,
            "width, with one edge per definition",
            400,
            6_400,
            "placement is summing the whole graph per node again (docs/64 §64.2)",
        ),
        (
            deep,
            "depth",
            100,
            1_600,
            "something in the front end re-walks the enclosing scope per binding",
        ),
    ] {
        let ratio = growth(gen, small, large, axis);
        assert!(
            ratio < MAX_GROWTH,
            "along `{axis}`, sixteen times the declarations cost ×{ratio:.2} more *each* — that \
             is an exponent rather than a constant factor: {note}"
        );
    }

    // Last, and that is not arbitrary either. Anything that runs *before* the three axes warms the
    // allocator, and a warmed `small` measurement against a cold `large` one is a ratio that has
    // moved without the compiler changing: this measured ×4.36 against a bound of 4.0 when it ran
    // first, and ×2.73 — the residual the module docs record — when it does not run at all.
    hints_do_not_grow_per_definition();
}

/// A module of `n` definitions that each perform something and each need placing.
///
/// `wide` is the wrong shape for the editor's question: its definitions are pure, so they are
/// unplaced and their rows are empty, and an inlay hint has nothing to say about any of them. Each
/// of these reads the process environment, so every one carries an inferred row *and* a tier the
/// solver had to choose — which is a hint apiece, and the work this gate is about.
fn hinted(n: usize) -> String {
    (0..n)
        .map(|i| format!("def h{i}() -> secret[Str]:\n    return secret_env(\"K{i}\")\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Inlay hints cost the same per definition however many there are.
///
/// The shape this guards is the one the first version of
/// [`Editor::hints`](beck_core::editor::Editor::hints) had and
/// [`docs/65`](../../../../docs/65-the-editor-report.md) §65.7 records: finding the colon
/// that ends a signature by filtering the file's whole token stream, once per definition, which is
/// `definitions × tokens` and reads as a small constant at the sizes anybody tests by hand. It is
/// the same defect [`docs/64`](../../../../docs/64-compile-speed-report.md) §64.2 found in
/// placement, in a different file and for the same reason: a per-item pass that walks the module.
///
/// A function rather than a second `#[test]`, for this file's own reason: the two would run
/// concurrently and measure the contention. Written as one first, it pushed the axis above from
/// ×2.73 to ×4.18 and failed a gate it has nothing to do with — which is the flake §13.7 warns
/// about, caught the same way the first draft of that test caught it.
fn hints_do_not_grow_per_definition() {
    let per = |n: usize| {
        let src = hinted(n);
        beck_diag::depth::on_the_front_end_stack(|| {
            let editor = beck_core::editor::Editor::of("hints.beck", &src);
            assert!(
                !editor.diagnostics().has_errors(),
                "the generated program has to compile, or this measures error recovery"
            );
            let hints = (0..5)
                .map(|_| {
                    let started = Instant::now();
                    let count = editor.hints().len();
                    (started.elapsed(), count)
                })
                .min()
                .expect("five runs");
            assert_eq!(
                hints.1,
                2 * n,
                "every definition should carry both hints, or this measures the wrong thing"
            );
            hints.0.as_secs_f64() / n as f64
        })
    };

    let (small, large) = (200usize, 3_200usize);
    let (a, b) = (per(small), per(large));
    let ratio = b / a;
    println!(
        "inlay hints: {small} → {large} definitions, {:.2} → {:.2} µs each — ×{ratio:.2}",
        a * 1e6,
        b * 1e6
    );
    assert!(
        ratio < MAX_GROWTH,
        "sixteen times the definitions cost ×{ratio:.2} more to hint *each* — hinting is walking \
         the module once per definition again"
    );
}
