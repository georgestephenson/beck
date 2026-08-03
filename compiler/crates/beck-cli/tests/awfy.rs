//! Are We Fast Yet, in Beck.
//!
//! [`docs/25-benchmarks-and-expressiveness.md`](../../../../docs/25-benchmarks-and-expressiveness.md)
//! §25.2 picks Are We Fast Yet as "the methodologically strongest choice for Beck's core", and
//! §25.9 schedules the harness for Phase 3 with **no compute number published** until there is a
//! backend for one to be about. This file is the harness half of that: the ported benchmarks run,
//! and each one agrees with the number the original suite verifies against.
//!
//! Three things are asserted, in order of what they are worth:
//!
//! 1. **Every file in `awfy/` passes its own tests**, through the binary, with no list of file
//!    names — a benchmark added to that directory is gated by being there. Each file's `test` block
//!    is the original's `verifyResult`, so this is the correctness claim.
//! 2. **The suite is the suite.** Which of Are We Fast Yet's benchmarks are ported *is* a claim, so
//!    the names are enumerated here — dropping one quietly is what this stops.
//! 3. **Provenance travels with the code.** These are ports of somebody else's MIT-licensed
//!    benchmarks, and a file that stops saying so is a licensing problem rather than a style one.
//!
//! What is deliberately **not** here is a threshold. `measure_awfy.rs` prints wall-clock and
//! nothing gates on it, for [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7's reason:
//! a timing gate on a shared runner flakes, and a gate that flakes gets deleted.

use std::path::{Path, PathBuf};
use std::process::Command;

fn awfy_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .join("awfy")
}

fn benchmarks() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(awfy_dir())
        .expect("the benchmark directory is where the harness expects it")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("beck"))
        .collect();
    out.sort();
    out
}

fn beck(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(args)
        .output()
        .expect("the compiler is built");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// The nine micro-benchmarks of Are We Fast Yet, and the numbers each one's `verifyResult` checks.
///
/// Written down here rather than left to the directory listing because *which* of the suite is
/// ported is the claim the report makes, and a listing cannot be wrong about a file that was never
/// added. The five macro-benchmarks — CD, DeltaBlue, Havlak, Json, Richards — are not ported, and
/// [`awfy/README.md`](../../../../awfy/README.md) says why rather than leaving it to be inferred.
const MICRO: [&str; 9] = [
    "bounce",
    "list",
    "mandelbrot",
    "nbody",
    "permute",
    "queens",
    "sieve",
    "storage",
    "towers",
];

/// Every benchmark runs its own tests and passes them.
///
/// The test inside each file is the original's `verifyResult` with the original's constant in it,
/// so this is the whole correctness claim of the port: it is not that the Beck runs, it is that it
/// computes what the Java computes.
#[test]
fn every_benchmark_verifies_against_the_original_suites_own_number() {
    for path in benchmarks() {
        let file = path.to_string_lossy().to_string();
        let (ok, text) = beck(&["test", &file]);
        assert!(ok, "`beck test {file}`:\n{text}");
        assert!(text.contains("0 failed"), "{file}:\n{text}");
    }
}

/// And each one is a **library** — no merge point, nothing to deploy.
///
/// A benchmark that had grown an application around it would be measuring the application, which
/// is the failure mode [`27`](../../../../docs/27-walls-report.md) removed for `sicp/` and this
/// directory inherits.
#[test]
fn each_benchmark_is_a_library() {
    for path in benchmarks() {
        let file = path.to_string_lossy().to_string();
        let (ok, text) = beck(&["check", &file]);
        assert!(ok, "`beck check {file}`:\n{text}");
        assert!(text.contains("a library"), "{file}:\n{text}");
    }
}

/// The nine are all there, and nothing else is.
#[test]
fn the_ported_suite_is_are_we_fast_yets_nine_micro_benchmarks() {
    let mut found: Vec<String> = benchmarks()
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
        .collect();
    found.sort();
    assert_eq!(found, MICRO, "the ported suite changed shape");
}

/// Each file says whose benchmark it is a port of.
///
/// These are derived from the SOM class library and the Computer Language Benchmarks Game, both
/// MIT-licensed, and [`awfy/README.md`](../../../../awfy/README.md) carries the notice. A header
/// that lost the attribution would be the one defect in this directory that no amount of green
/// tests would surface.
#[test]
fn every_benchmark_names_the_suite_it_is_a_port_of() {
    for path in benchmarks() {
        let text = std::fs::read_to_string(&path).expect("readable");
        assert!(
            text.contains("Are We Fast Yet"),
            "{} does not name the suite it is a port of",
            path.display()
        );
    }
}

/// `and` and `or` short-circuit, and this is where the fact that they did not was found.
///
/// `Queens.java` writes `if (getRowColumn(r, c)) { … if (placeQueen(c + 1)) …`, which reads as one
/// conjunction. Written as one in Beck it used to search from squares that were already attacked,
/// because both operands were evaluated before the operator was applied
/// ([`53`](../../../../docs/53-are-we-fast-yet-report.md) §53.5). It is a `if a: b else: false` in
/// the IR now, so the guard guards.
///
/// The test is here rather than beside the checker's other lowerings on purpose: it is where the
/// defect was found, and a benchmark suite is the only thing in this repository that had ever
/// exercised the difference. It asserts through the binary, on a program whose right operand
/// *cannot* be evaluated, which is the only way to observe the change from outside.
#[test]
fn and_and_or_short_circuit_so_a_guard_written_as_a_conjunction_guards() {
    let cases = [
        // The left operand decides, so the right — which divides by zero — must not run.
        ("false and risky(0)", "not (false and risky(0))"),
        ("true or risky(0)", "true or risky(0)"),
        // And the right operand still runs when the left does not decide, or `and` would be a
        // constant. Both directions, for `docs/20`'s reason: one is not a test.
        ("true and risky(1)", "true and risky(1)"),
        ("false or risky(1)", "false or risky(1)"),
    ];
    for (name, expr) in cases {
        let src = format!(
            "def risky(n: Int) -> Bool:\n    return 1 / n > 0\n\n\
             test \"{name}\":\n    expect {expr}\n"
        );
        let file = std::env::temp_dir().join("beck-awfy-shortcircuit.beck");
        std::fs::write(&file, &src).expect("a scratch file");
        let (ok, text) = beck(&["test", file.to_string_lossy().as_ref()]);
        let _ = std::fs::remove_file(&file);
        assert!(ok && text.contains("0 failed"), "{name}:\n{src}\n{text}");
    }
}

/// The directory's own README exists and says what the port changes.
///
/// The ports are not transcriptions — there is no mutable array in Beck and no bitwise operator —
/// so the rules they follow have to be written down in one place or each file will invent its own.
#[test]
fn the_directory_documents_what_the_port_changes() {
    let readme = std::fs::read_to_string(awfy_dir().join("README.md")).expect("a README");
    for expected in ["What the port changes", "Provenance", "licence"] {
        assert!(readme.contains(expected), "the README lost `{expected}`");
    }
}
