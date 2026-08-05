//! What the Benchmarks Game costs on the tree-walker, printed and never gated.
//!
//! Run with `cargo test --release --test measure_clbg -- --nocapture`.
//!
//! [`docs/25-benchmarks-and-expressiveness.md`](../../../../docs/25-benchmarks-and-expressiveness.md)
//! §25.3 measured the evaluator at about 33× CPython and drew the sequencing rule this suite obeys:
//! **adopt the harness now, publish the numbers now, and make them unflattering**, because a
//! benchmark suite acquired after the thing it measures gets good has no history and therefore no
//! regression-detecting power. §25.9 adds the other half — no *comparative* claim until there is a
//! second backend — so nothing here is compared to anything, and least of all to the Game's own
//! table. That table is what §25.2 calls "widely quoted and widely misused"; entering a number
//! from a placeholder interpreter into it would be the misuse.
//!
//! # What the numbers are
//!
//! Two wall-clock times per benchmark, both of the shipped binary on a whole file:
//!
//! | | what it is |
//! |---|---|
//! | **check** | `beck check <file>` — parse, expand, check, infer effects, place. No evaluation |
//! | **test** | `beck test <file>` — the same front end, then the file's `test` blocks run |
//!
//! The difference is therefore *the benchmark plus the test harness around it*, not the benchmark
//! alone, and it is reported as a difference rather than as a benchmark time for that reason.
//!
//! Two further reasons a number here is not a benchmark time, both specific to this directory and
//! both larger than the harness overhead they sit beside:
//!
//! - **A file runs its imports' tests too.** `revcomp.beck` imports `fasta.beck`, so `beck test`
//!   on it runs fasta's three tests before revcomp's own.
//! - **A `test` block has no local bindings** (`B0705` — §21.2 admits no fixture), so an assertion
//!   about five properties of one 10 KB output computes that output five times. `revcomp` and
//!   `knucleotide` both do.
//!
//! A number here is comparable to another number from the same run on the same machine, and to
//! nothing else. It is not thresholded:
//! [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7 — a timing gate on a shared runner
//! cannot be held honestly, and a gate that flakes gets deleted.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn clbg_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .join("clbg")
}

fn benchmarks() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(clbg_dir())
        .expect("readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("beck"))
        .collect();
    out.sort();
    out
}

/// How many times each command is run: five in release, one in debug.
///
/// The reproducible form of this suite is `cargo test --release`, and the debug run happens only
/// because `cargo test --workspace` runs everything. A debug timing is not a number anybody may
/// read, and `pidigits` is slow enough in that build to be worth several minutes of nothing; the
/// run is still worth doing once, because it exercises the binary on every benchmark.
const RUNS: usize = if cfg!(debug_assertions) { 1 } else { 5 };

/// The median of [`RUNS`] runs, because a single wall-clock reading of a process is mostly noise.
fn median(args: &[&str]) -> Duration {
    let mut runs: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let started = Instant::now();
            let out = Command::new(env!("CARGO_BIN_EXE_beck"))
                .args(args)
                .output()
                .expect("the compiler is built");
            let elapsed = started.elapsed();
            assert!(out.status.success(), "{args:?} failed");
            elapsed
        })
        .collect();
    runs.sort();
    runs[RUNS / 2]
}

#[test]
fn what_the_benchmarks_game_costs_on_the_tree_walker() {
    println!(
        "\n{:<16} {:>10} {:>10} {:>12}",
        "benchmark", "check ms", "test ms", "difference"
    );
    println!("{}", "-".repeat(52));
    for path in benchmarks() {
        let file = path.to_string_lossy().to_string();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let check = median(&["check", &file]);
        let test = median(&["test", &file]);
        println!(
            "{:<16} {:>10.1} {:>10.1} {:>12.1}",
            name,
            check.as_secs_f64() * 1000.0,
            test.as_secs_f64() * 1000.0,
            (test.as_secs_f64() - check.as_secs_f64()) * 1000.0,
        );
    }
    println!(
        "\nMedian of {RUNS}, {} build, at the sizes the Game publishes an expected output for —\n\
         which are its *format-checking* sizes and not the ones it measures at. No comparative\n\
         claim: docs/25 §25.9 holds those until there is a second backend for them to be about.\n\
         `pidigits` measures lib/bignum.beck rather than a host's big integer, which is the\n\
         thing to know before reading its row against anything.\n",
        if cfg!(debug_assertions) {
            "debug — read nothing off this table, the reproducible form is --release"
        } else {
            "release"
        }
    );
}
