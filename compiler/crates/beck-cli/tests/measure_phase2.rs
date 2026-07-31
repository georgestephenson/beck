//! Measurements quoted in `docs/20-phase-2-report.md`.
//!
//! Run with `cargo test --release --test measure_phase2 -- --nocapture`. Printed, never
//! thresholded: §13.7's rule that a shared runner cannot hold a timing gate honestly, and "a gate
//! that flakes gets deleted". The one number that *is* a gate lives in `scaling.rs`.

use std::path::{Path, PathBuf};
use std::time::Instant;

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .canonicalize()
        .expect("checked in")
}

fn programs() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(corpus())
        .expect("readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().to_string(),
                std::fs::read_to_string(&p).expect("readable"),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn what_the_front_end_costs_and_what_the_solver_costs_of_it() {
    // The question the Phase 2 report has to answer honestly: placement inference is new work on
    // every compile, so how much of a compile is it?
    println!(
        "\n{:<24} {:>10} {:>10} {:>8} {:>7} {:>7}",
        "program", "check µs", "solve µs", "solve %", "nodes", "≥2 tiers"
    );
    let (mut total_check, mut total_solve) = (0u128, 0u128);
    for (name, src) in programs() {
        // Warm: the first compile in a process pays for lazily-built statics.
        let _ = beck_core::check_str(&name, &src);

        let mut check = u128::MAX;
        let mut solve = u128::MAX;
        let mut nodes = 0;
        let mut choices = 0;
        for _ in 0..20 {
            let t = Instant::now();
            let (program, _, _) = beck_core::check_str(&name, &src);
            check = check.min(t.elapsed().as_micros());

            let t = Instant::now();
            let solution = beck_core::place::solve(&program, None);
            solve = solve.min(t.elapsed().as_micros());
            nodes = solution.tiers.len();
            // Not "free": a pinned node also has more than one legal tier. This is how many
            // placements the *effect rows alone* leave open, which is the size of the question the
            // cost model is answering.
            choices = solution
                .explanations
                .iter()
                .filter(|e| {
                    e.candidates
                        .iter()
                        .filter(|(_, c)| *c < beck_core::cost::FORBIDDEN)
                        .count()
                        > 1
                })
                .count();
        }
        total_check += check;
        total_solve += solve;
        println!(
            "{name:<24} {check:>10} {solve:>10} {:>7.1}% {nodes:>7} {choices:>8}",
            100.0 * solve as f64 / (check + solve).max(1) as f64
        );
    }
    println!(
        "\n{:<24} {total_check:>10} {total_solve:>10} {:>7.1}%",
        "total",
        100.0 * total_solve as f64 / (total_check + total_solve).max(1) as f64
    );
}

#[test]
fn where_the_corpus_ends_up() {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut defs = 0;
    for (name, src) in programs() {
        let (program, d, map) = beck_core::check_str(&name, &src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        for (_, t) in beck_core::place::solve(&program, None).tiers {
            *counts.entry(t.name()).or_default() += 1;
            defs += 1;
        }
    }
    println!("\n{defs} placed things across the corpus:");
    for (tier, n) in &counts {
        println!(
            "  {tier:<8} {n:>4}  {:>5.1}%",
            100.0 * *n as f64 / defs as f64
        );
    }
    println!(
        "\n`any` is the interesting number: unplaced means pure, so that is the share of the\n\
         corpus that compiles to every tier that needs it rather than to one."
    );
}

#[test]
fn what_an_incremental_rebuild_costs_in_a_three_module_project() {
    // §3.6's firewall, as a number rather than as an assertion: how much of a project a one-line
    // body edit re-checks, against how much a signature change does.
    use beck_db::{Compiler, Database};
    use std::sync::Arc;

    let dir = corpus().join("project");
    let read = |n: &str| std::fs::read_to_string(dir.join(format!("{n}.beck"))).expect("readable");

    let mut db = Database::new();
    for m in ["domain", "policy", "app"] {
        db.set(m, &read(m));
    }
    let t = Instant::now();
    let _ = db.checked(Arc::from("app"));
    let _ = db.checked(Arc::from("policy"));
    let cold = t.elapsed();
    let cold_modules = beck_db::take_rechecked().len();

    // A body edit in the deepest module.
    let edited = read("domain").replace(
        "        case Toggled(id):\n            return toggled(s, id)",
        "        case Toggled(id):\n            return toggled(s, id)\n",
    );
    assert_ne!(edited, read("domain"), "the edit must apply");
    db.set("domain", &edited);
    let t = Instant::now();
    let _ = db.checked(Arc::from("app"));
    let _ = db.checked(Arc::from("policy"));
    let warm = t.elapsed();
    let warm_modules = beck_db::take_rechecked().len();

    println!(
        "\nthree-module project\n  \
         cold          {cold:?} over {cold_modules} module(s)\n  \
         after a body edit  {warm:?} over {warm_modules} module(s)\n\n\
         The second number is the firewall: the edited module is re-checked and its interface comes\n\
         out identical, so nothing above it has anything to redo."
    );
    assert_eq!(warm_modules, 1);
}
