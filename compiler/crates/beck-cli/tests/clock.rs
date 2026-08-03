//! The clock is supplied, not ambient — and this is the gate that keeps it that way.
//!
//! [`docs/14-review-findings.md`](../../../../docs/14-review-findings.md) F11 says deterministic
//! simulation cannot be retrofitted and records the constraint: virtualize the clock from the
//! first line of runtime code. [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.4
//! restates it in bold. The runtime then read `SystemTime::now()` directly for three phases
//! ([`docs/42`](../../../../docs/42-security-assurance.md) §42.4) — not because anybody disagreed,
//! but because a decision with no gate is a sentence in a document.
//!
//! §42.11 states the gate in one line: "a test that `SystemTime::now()` appears in exactly one
//! place". This is that test. It scans the workspace's own source the way `docs.rs` scans it for
//! diagnostic codes, and it is deliberately a *count*, not a list of blessed files: a second
//! reader is a failure the moment it is written, wherever it is written.

use std::path::{Path, PathBuf};

mod support;

fn compiler_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .to_path_buf()
}

/// Every `.rs` file in the workspace, tests included.
///
/// Tests are *not* excluded here, unlike in `docs.rs`. A harness that reaches for the host clock
/// is a harness whose result depends on when it ran, and this seam exists so it does not have to.
fn sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&compiler_root().join("crates"), &mut out);
    // This file names the call in its own failure messages, and a gate that fails on its own
    // prose is a gate nobody keeps.
    out.retain(|p| !p.ends_with("tests/clock.rs"));
    out.sort();
    assert!(out.len() > 20, "the source listing is wrong, not the repo");
    out
}

fn sites_of(needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    for path in sources() {
        let src = std::fs::read_to_string(&path).expect("a source file is readable");
        for (n, line) in src.lines().enumerate() {
            // The doc comment that *names* the call while explaining why it is not made is not a
            // call. Prose about a rule is how the rule stays legible.
            let code = line.split("//").next().unwrap_or("");
            if code.contains(needle) {
                out.push(format!("{}:{}", path.display(), n + 1));
            }
        }
    }
    out
}

#[test]
fn the_host_clock_is_read_in_exactly_one_place() {
    let sites = sites_of("SystemTime::now()");
    assert_eq!(
        sites.len(),
        1,
        "`SystemTime::now()` belongs to `beck_core::clock::SystemClock` and to nothing else — \
         F11's constraint is that time is supplied, and a second reader is where that stops being \
         true. Found:\n  {}",
        sites.join("\n  ")
    );
    assert!(
        sites[0].contains("beck-core/src/clock.rs"),
        "and the one place is the seam, not wherever it happens to be: {}",
        sites[0]
    );
}

/// The `SystemClock` reads it twice — milliseconds and nanoseconds — which is one *place* and two
/// lines. Stated so the count above cannot be quietly relaxed by moving a second call in beside
/// the first.
#[test]
fn the_seam_is_the_only_file_that_names_the_standard_librarys_clock() {
    let files: std::collections::BTreeSet<String> = sites_of("SystemTime")
        .into_iter()
        .map(|s| {
            s.rsplit_once(':')
                .expect("a site is file:line")
                .0
                .to_string()
        })
        .collect();
    assert_eq!(
        files.len(),
        1,
        "found {:?}",
        files.iter().collect::<Vec<_>>()
    );
}

/// An envelope's `at` comes from the configured clock and from nowhere else.
///
/// This is the half that matters for replay: an envelope is logged, and a fold over the log has to
/// see the same instant the run saw. §3.7's "the merge point is the one place time enters" is only
/// true if that entry is a value somebody handed in.
#[tokio::test]
async fn an_envelopes_instant_is_the_clock_the_app_was_given() {
    use std::sync::Arc;

    let placed = support::todo_program();
    let backend = beck_eval::backend(&placed);
    let runtime = beck_rt::Runtime::new(placed, backend).expect("the example prepares");

    let clock = Arc::new(beck_core::clock::ManualClock::at(1_700_000_000_000));
    let store: Arc<dyn beck_rt::LogStore> = Arc::new(beck_rt::MemoryLog::new());
    let app = beck_rt::App::start(
        runtime,
        store.clone(),
        beck_rt::AppConfig {
            clock: clock.clone(),
            ..Default::default()
        },
    )
    .await
    .expect("the app starts");

    app.propose(
        "c1".into(),
        "alice".into(),
        support::command("Add", &[("id", "t1"), ("text", "write it down")]),
    )
    .await
    .expect("the command is accepted");

    clock.advance(60_000);
    app.propose(
        "c2".into(),
        "alice".into(),
        support::command("Add", &[("id", "t2"), ("text", "and again")]),
    )
    .await
    .expect("the command is accepted");

    let stamped: Vec<i64> = store
        .read(0, 100)
        .await
        .expect("the log reads")
        .iter()
        .map(|e| e.at.0)
        .collect();
    assert_eq!(
        stamped,
        vec![1_700_000_000_000, 1_700_000_060_000],
        "the envelopes carry the instants the clock was set to, and no reading of the host's"
    );
}
