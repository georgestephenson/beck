//! What Are We Fast Yet costs on the tree-walker, printed and never gated.
//!
//! Run with `cargo test --release --test measure_awfy -- --nocapture`.
//!
//! [`docs/25-benchmarks-and-expressiveness.md`](../../../../docs/25-benchmarks-and-expressiveness.md)
//! §25.3 measured the evaluator at about 33× CPython and drew the sequencing rule this suite obeys:
//! **adopt the harness now, publish the numbers now, and make them unflattering**, because a
//! benchmark suite acquired after the thing it measures gets good has no history and therefore no
//! regression-detecting power. §25.9 adds the other half — no *comparative* claim until there is a
//! second backend — so nothing here is compared to anything.
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
//! alone, and it is reported as a difference rather than as a benchmark time for that reason. A
//! number here is comparable to another number from the same run on the same machine and to
//! nothing else.
//!
//! It is not thresholded. [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7: a timing
//! gate on a shared runner cannot be held honestly, and a gate that flakes gets deleted.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn awfy_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .join("awfy")
}

fn benchmarks() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(awfy_dir())
        .expect("readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("beck"))
        .collect();
    out.sort();
    out
}

/// The median of five runs, because a single wall-clock reading of a process is mostly noise.
fn median_of_five(args: &[&str]) -> Duration {
    let mut runs: Vec<Duration> = (0..5)
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
    runs[2]
}

#[test]
fn what_are_we_fast_yet_costs_on_the_tree_walker() {
    println!(
        "\n{:<14} {:>10} {:>10} {:>12}",
        "benchmark", "check ms", "test ms", "difference"
    );
    println!("{}", "-".repeat(50));
    for path in benchmarks() {
        let file = path.to_string_lossy().to_string();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let check = median_of_five(&["check", &file]);
        let test = median_of_five(&["test", &file]);
        println!(
            "{:<14} {:>10.1} {:>10.1} {:>12.1}",
            name,
            check.as_secs_f64() * 1000.0,
            test.as_secs_f64() * 1000.0,
            (test.as_secs_f64() - check.as_secs_f64()) * 1000.0,
        );
    }
    println!(
        "\nMedian of five, release build. The difference is the benchmark plus the test harness,\n\
         not the benchmark alone. No comparative claim: docs/25 §25.9 holds those until there is a\n\
         second backend for them to be about.\n"
    );
}
