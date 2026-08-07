//! The front end's recursion bound, end to end: the file that used to abort the process.
//!
//! [`docs/42-security-assurance.md`](../../../../docs/42-security-assurance.md) §42.2 measured it
//! as a defect and §42.11 states the gate in two halves — "a nesting one past the ceiling is a
//! *diagnostic*; and the declared stack holds the ceiling. The pair `beck-eval` already has."
//! `beck-syntax` and `beck-core` hold the second half against their own recursions with a measured
//! bytes-per-level figure; this file holds the first, through the binary a stranger would run, and
//! checks that the one thread the CLI dispatches onto is big enough for both consumers of it.
//!
//! It runs the binary rather than calling the library on purpose. The failure it exists to prevent
//! is a **process abort** — no unwind, no diagnostic, nothing a test inside the process can catch —
//! so the observation has to be made from outside one.

use std::process::Command;

/// §42.2's reproduction, verbatim: 3,785 levels of parentheses, the depth that aborted a debug
/// build. The ceiling is far below it now, so the interesting part is the *shape* of the refusal.
const DEEP: usize = 3_785;

fn check(src: &str, name: &str) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!("beck-front-end-bound-{name}"));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let path = dir.join("deep.beck");
    std::fs::write(&path, src).expect("the file is written");

    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("the compiler runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // `status.code()` is `None` when a process died on a signal, which is exactly the outcome
    // being ruled out — SIGABRT from a stack overflow, or SIGSEGV without one.
    let exited = out.status.code().is_some();
    assert!(
        exited,
        "`beck check` died on a signal rather than exiting: {:?}\n{text}",
        out.status
    );
    (out.status.success(), text)
}

#[test]
fn the_file_that_aborted_the_compiler_is_now_refused_with_a_span() {
    let src = format!(
        "def f() -> Int:\n    return {}1{}\n",
        "(".repeat(DEEP),
        ")".repeat(DEEP)
    );
    let (ok, text) = check(&src, "parens");
    assert!(!ok, "a program past the ceiling must be refused:\n{text}");
    assert!(
        text.contains("B0121"),
        "and refused by the nesting bound, not by something incidental:\n{text}"
    );
    assert!(
        text.contains("deep.beck"),
        "with a span pointing into the file:\n{text}"
    );
}

/// The S-expression surface is the same front end and the same ceiling. It is a separate test
/// because it is a separate recursion — `docs/42` §42.2 quotes the Scriban advisory for exactly
/// this reason: a bound at one production is not a bound.
#[test]
fn the_canonical_surface_is_bounded_too() {
    let dir = std::env::temp_dir().join("beck-front-end-bound-sexpr");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let path = dir.join("deep.sx");
    let src = format!("(module deep {}1{})\n", "(".repeat(DEEP), ")".repeat(DEEP));
    std::fs::write(&path, &src).expect("the file is written");

    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("the compiler runs");
    assert!(
        out.status.code().is_some(),
        "`beck check` died on a signal rather than exiting: {:?}",
        out.status
    );
    let text =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("B0121"), "{text}");
}

/// A program a person would actually write is unaffected — stated as a test because a bound that
/// refuses real programs is a worse defect than the one it fixed.
#[test]
fn the_corpus_is_nowhere_near_the_ceiling() {
    let deepest = beck_diag::depth::MAX_NESTING;
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus"))
        .expect("the corpus is where the harnesses expect it")
    {
        let path = entry.expect("a corpus entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("beck") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("a corpus program reads");
        let (_, d, map) = beck_core::compile_str(path.to_string_lossy().as_ref(), &src);
        assert!(
            !d.iter().any(|x| x.code == "B0121" || x.code == "B0390"),
            "{} is within {deepest} levels of nesting:\n{}",
            path.display(),
            d.render(&map)
        );
    }
}

/// The axis `MAX_NESTING` does **not** measure, and the counter that now does.
///
/// A block of sequential local bindings is flat: `v0 = …` then `v1 = …` is not one inside the
/// other, so a body 25,000 bindings long is at nesting level 2 and the nesting ceiling never sees
/// it. The front end still recurses once per binding — a block is a chain in `Core`, whatever it
/// looks like in source.
///
/// Until `docs/85` what bounded this axis was the **declared stack**, which is the property
/// [`adr/0007`](../../../../docs/adr/0007-evaluator-stack-is-declared-not-discovered.md) says a
/// ceiling must not have: `docs/64` §64.4 measured a debug build aborting at 12,000 bindings and a
/// release build at 100,000, with no diagnostic, so *which programs compile depended on how the
/// compiler was built*. `MAX_BLOCK` is the counted answer, and this test is the pair the nesting
/// axis has had since [`adr/0012`](../../../../docs/adr/0012-the-front-end-counts-its-own-recursion.md):
/// under the ceiling compiles, over it is a diagnostic, and the number does not move with the
/// profile.
///
/// Through the binary, for this file's reason: the failure being ruled out is a process abort,
/// which nothing inside the process can catch.
///
/// [`42`]: ../../../../docs/42-security-assurance.md
#[test]
fn a_flat_block_is_bounded_by_a_counted_ceiling_rather_than_by_the_stack() {
    let body = |n: usize| {
        let mut src = String::from("def deep(x: Int) -> Int:\n    v0 = x + 1\n");
        for i in 1..n {
            src.push_str(&format!("    v{i} = v{} + {i}\n", i - 1));
        }
        src.push_str(&format!("    return v{}\n", n - 1));
        src
    };
    let ceiling = beck_diag::depth::MAX_BLOCK as usize;

    // Under it: an ordinary, if long, function.
    let (ok, text) = check(&body(ceiling - 16), "flat-block-under");
    assert!(ok, "a body under the ceiling must compile:\n{text}");

    // Over it: a diagnostic with a span, in *this* profile and in the other one — which is the
    // whole point, and what an abort could never be.
    let (ok, text) = check(&body(ceiling * 8), "flat-block-over");
    assert!(!ok, "a body eight times the ceiling must be refused");
    assert!(
        text.contains("B0389"),
        "and refused by the block counter, with a span:\n{text}"
    );
    assert!(
        !text.contains("B0121"),
        "not by the nesting ceiling, because this is not nesting:\n{text}"
    );
}

/// The two declarations that share one thread.
///
/// `beck-cli` dispatches every command inside `beck_eval::on_the_evaluator_stack`, so the front
/// end runs on the stack the *evaluator* declared. That is fine and is the cheap arrangement — but
/// it is only fine while the front end's own declaration fits inside it, and nothing said so until
/// this line did. It is a `const` assertion rather than a test because both sides are constants:
/// this fails the build, which is a strictly better time to hear about it than the test run.
const _: () = assert!(
    beck_diag::depth::STACK_BYTES <= beck_eval::STACK_BYTES,
    "the front end declares a stack larger than the thread `beck-cli` dispatches onto"
);
