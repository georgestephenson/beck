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

/// **A gate written `! cmd` does not fail the step it is in**, unless it happens to be the last
/// line of one.
///
/// `bash -e` "shall not exit" when the command that failed "is part of a `!` expression" — POSIX's
/// own words — so `! grep -q 'DELETE' grants.yaml` followed by anything at all is a comment with a
/// process behind it. **Nine of the ten in `compiler.yml` were dead**, including the one asserting
/// that a deliberately-false Beck test fails the build, and the tenth was live only because it was
/// the last line of its block. They had been green since they were written.
///
/// The workflow already knew the shape and had not generalised it: the deep-recursion step says
/// "an exit status of 134 or 139 … is exactly the thing a `! cmd` gate would have accepted, so the
/// status is checked rather than only the failure". That is
/// [`docs/82`](../../../../docs/82-the-edge-report.md) §82.10 in one file — somebody saw the gap
/// where it bit them and wrote the fix for that instance.
///
/// So the rule is the form rather than the instance: **no `run:` line may begin with `!`**, and
/// what replaces it is `if cmd; then echo 'why'; exit 1; fi`, which aborts wherever it sits and
/// says what went wrong when it does. The exception the rule does not need is the last-line case —
/// admitting it would make the check positional, and a step that grows a line afterwards would
/// silently lose its gate, which is how this happened.
#[test]
fn no_workflow_asserts_with_a_negation_that_cannot_fail() {
    let mut found: Vec<String> = Vec::new();
    for path in workflows() {
        let text = std::fs::read_to_string(&path).expect("a workflow is readable");
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("! ") {
                found.push(format!(
                    "{}:{}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    n + 1,
                    trimmed.trim_end()
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these assertions are negations, and `bash -e` does not exit on one unless it is the last \
         line of its step — so they pass whatever happens. Write `if cmd; then echo 'why'; exit 1; \
         fi` instead:\n  {}",
        found.join("\n  ")
    );
}

#[test]
fn every_published_page_is_built_with_a_link_back_to_the_repository() {
    // The site links back to the repository through `beck doc --repo`, and `docs.rs` asserts that
    // the flag reaches every page. Neither says anything about a page the *workflow* builds without
    // it, which is the way this actually goes wrong: a page kind is added to `publish`, the flag is
    // forgotten, and one page on the site is a dead end. So the gap tested here is the workflow's,
    // not the compiler's.
    //
    // `--check` is exempt because it writes nothing: it is the markdown drift gate, and markdown
    // has no page shell to put a link in.
    let path = workflows()
        .into_iter()
        .find(|p| p.file_name().is_some_and(|n| n == "docs.yml"))
        .expect("the docs workflow is checked in");
    let text = std::fs::read_to_string(path).expect("readable");

    // Shell continuations first: a `beck doc` invocation is spread over as many lines as it needs,
    // and `--repo` is usually on a later one.
    let joined = text.replace("\\\n", " ");
    let mut naked = Vec::new();
    for line in joined.lines() {
        if !line.contains("beck doc ") || line.trim_start().starts_with('#') {
            continue;
        }
        if !line.contains("--check") && !line.contains("--repo") {
            naked.push(line.trim().to_string());
        }
    }
    assert!(
        naked.is_empty(),
        "the docs workflow publishes a page with no link back to the repository — pass \
         `--repo \"${{GITHUB_SERVER_URL}}/${{GITHUB_REPOSITORY}}\"`:\n  {}",
        naked.join("\n  ")
    );
    // And the property above is worth nothing if it looked at no commands at all.
    assert!(
        joined.matches("beck doc ").count() >= 4,
        "only {} `beck doc` invocations found in the docs workflow — the check above passed by \
         looking at nothing",
        joined.matches("beck doc ").count()
    );
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
        "cargo test --workspace",        // the differential and replay harnesses
        "cargo clippy",                  // no warnings
        "cargo fmt",                     // one format
        "beck check examples/todo.beck", // the demo that runs
        "corpus/",                       // Phase 2's exit criterion
        "--wire-compat",                 // §4.3's rolling-deploy gate
        "--assert-place",                // §3.4's assertability guardrail
    ] {
        assert!(
            text.contains(required),
            "the workflow no longer runs `{required}`"
        );
    }
}
