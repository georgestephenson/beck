//! A test about the tests, and it exists because of what it found.
//!
//! [`docs/08-roadmap.md`](../../../../docs/08-roadmap.md) §8.3 makes the harnesses "the project's
//! conscience" and says "every phase ships a demo that runs". Phase 2 discovered that the workflow
//! enforcing that was **invalid YAML from the day it was written** — a step name beginning with a
//! backtick, which YAML reserves and refuses to start a plain scalar with. GitHub Actions rejects
//! the whole file, so every gate in it had been silently absent, and a real defect (the
//! S-expression surface not round-tripping through `beck check`) survived a phase because of it.
//!
//! [`docs/19-phase-1-report.md`](../../../../docs/19-phase-1-report.md) §19.4 item 10 names this
//! exact pattern about *artefacts*: "an artefact nobody has executed is a design document". A CI
//! workflow is an artefact. This is the check that would have caught it, and it is deliberately
//! narrow — it looks for the two characters YAML reserves and nothing else, because a
//! half-understood linter that fires on valid files gets deleted.

use std::path::{Path, PathBuf};

fn workflows() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.github/workflows");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .collect();
    out.sort();
    out
}

#[test]
fn there_are_workflows_to_check() {
    assert!(
        !workflows().is_empty(),
        "no workflow files found — this test would otherwise pass by looking at nothing"
    );
}

#[test]
fn no_workflow_starts_a_value_with_a_character_yaml_reserves() {
    // `@` and a backtick are *reserved indicators*: YAML 1.2 §7.3.3 forbids a plain scalar from
    // beginning with either, and a parser rejects the document rather than the line. Both read
    // perfectly naturally in a step name — "`beck fmt` round-trips" — which is why this happened
    // and why it happened silently.
    for path in workflows() {
        let text = std::fs::read_to_string(&path).expect("readable");
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            let Some(colon) = trimmed.find(": ") else {
                continue;
            };
            // Only `key: value` lines, and only where the key is a plain word — this is not a YAML
            // parser and should not pretend to be one.
            let key = trimmed[..colon].trim_start_matches("- ");
            if !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                continue;
            }
            let value = trimmed[colon + 2..].trim_start();
            let Some(first) = value.chars().next() else {
                continue;
            };
            assert!(
                first != '`' && first != '@',
                "{}:{}: a value may not begin with `{first}` — YAML reserves it, and the whole \
                 workflow is rejected rather than this line. Quote it.\n  {line}",
                path.display(),
                n + 1
            );
        }
    }
}

#[test]
fn the_compiler_workflow_still_runs_the_harnesses_it_exists_for() {
    // §8.3 names three things as gates. A workflow that quietly stopped running one of them would
    // be the same failure in a different form, so the gates are asserted by name.
    let path = workflows()
        .into_iter()
        .find(|p| p.file_name().is_some_and(|n| n == "compiler.yml"))
        .expect("the compiler workflow is checked in");
    let text = std::fs::read_to_string(path).expect("readable");
    for required in [
        "cargo test --workspace",  // the differential and replay harnesses
        "cargo clippy",            // no warnings
        "cargo fmt",               // one format
        "beck check examples/todo.beck", // the demo that runs
        "corpus/",                 // Phase 2's exit criterion
        "--wire-compat",           // §4.3's rolling-deploy gate
        "--assert-place",          // §3.4's assertability guardrail
    ] {
        assert!(
            text.contains(required),
            "the workflow no longer runs `{required}`"
        );
    }
}
