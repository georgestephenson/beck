//! Expansion is bounded by what it **produces**, and this is the gate in both directions.
//!
//! [`docs/14`](../../../../docs/14-review-findings.md) F17 asked for it and
//! [`docs/42`](../../../../docs/42-security-assurance.md) §42.2 is why it matters here rather than
//! in the abstract: the playground compiles **a stranger's source in a browser tab**
//! ([`docs/98`](../../../../docs/98-playground-report.md)), so every limit the front end has is a
//! limit on what a visitor can do to the tab. Two of them existed —
//! [`adr/0012`](../../../../docs/adr/0012-the-front-end-counts-its-own-recursion.md)'s structural
//! nesting count and the expander's own re-expansion depth — and both bound how *deep* expansion
//! goes. Neither bounded how much it makes.
//!
//! That is [`docs/85`](../../../../docs/85-what-the-generator-found-report.md) §85.7's pattern for
//! the fourth time: *a limit added at the one production somebody thought of is bypassed through a
//! different one*. A macro that doubles its output at each of a few levels is shallow, terminates,
//! and is enormous.
//!
//! # Why the sizes here are what they are
//!
//! [`docs/85`](../../../../docs/85-what-the-generator-found-report.md) §85.1 is unflattering about a
//! generator calibrated against the limits you built rather than against the failures you had, so
//! these are calibrated against the *failure*: `refuses` doubles until the budget is gone, and
//! `allows` is the whole repository, which is the only honest statement of what a legitimate program
//! expands to.

use std::path::PathBuf;

/// The doubling macro, `n` deep. Six lines of source, `2^n` copies of the leaf.
fn bomb(n: usize) -> String {
    // `x + x` rather than a call, so that the program is *clean* as well as enormous: a macro that
    // names a top-level function trips hygiene here, and a fixture with an unrelated error in it
    // could not be used for the other half of this gate.
    let mut src = String::from("macro pair(x):\n    return quote:\n        ($x + $x)\n\n");
    src.push_str("def go() -> Int:\n    return ");
    let mut expr = String::from("1");
    for _ in 0..n {
        expr = format!("pair({expr})");
    }
    src.push_str(&expr);
    src.push('\n');
    src
}

/// The diagnostics a program produces, as codes.
///
/// `compile_or_library_str` rather than `compile_str`, because these fixtures are one definition
/// and no merge point — a library, which is what `B0500` says and is not what is being tested.
fn codes(name: &str, src: &str) -> Vec<String> {
    let (_, diags, _) = beck_core::compile_or_library_str(name, src);
    diags.iter().map(|d| d.code.to_string()).collect()
}

/// A macro that doubles its output is refused — **and the message is the one that says why**.
///
/// Twenty-four nestings: sixteen million nodes if nothing counted, and it is refused after a hundred
/// thousand of them, because the accounting stops when the budget does. A test that proved the point
/// by exhausting memory would not be a test.
#[test]
fn a_doubling_macro_is_refused() {
    let codes = codes("bomb.beck", &bomb(24));
    assert!(
        codes.iter().any(|c| c == "B0214"),
        "a doubling macro 24 deep should be refused by the expansion budget, and the diagnostics \
         were {codes:?}"
    );
    // Not the depth counters: both are satisfied by this program, which is the whole point of the
    // budget existing separately from them.
    assert!(
        !codes.iter().any(|c| c == "B0201" || c == "B0213"),
        "the budget should be what refuses this, not a depth limit: {codes:?}"
    );
    // Once, not once per call that would have expanded afterwards.
    assert_eq!(
        codes.iter().filter(|c| *c == "B0214").count(),
        1,
        "the budget should report once: {codes:?}"
    );
}

/// …and the same macro at a depth a person would write compiles.
///
/// The other direction, and the one that makes the number a *judgement* rather than a wall: eight
/// nestings is 256 copies of the leaf, which is more than any program in this repository generates
/// and is nowhere near the budget.
#[test]
fn a_macro_a_person_would_write_still_compiles() {
    let codes = codes("small.beck", &bomb(8));
    assert!(
        codes.is_empty(),
        "eight nestings is 256 copies and should compile: {codes:?}"
    );
}

/// Every program in the tree still expands, which is the control the assertion above needs.
///
/// A budget that refused a bomb by refusing everything would pass the test above. This is the
/// statement that it does not, over the corpus, both benchmark suites, both SICP chapters, the
/// examples and the standard library — and it is also where the number came from: the largest total
/// expansion any of them performs is 138 nodes.
#[test]
fn every_program_in_the_tree_still_expands() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .to_path_buf();
    beck_diag::depth::on_the_front_end_stack(|| check(&root));
}

/// The walk, on the stack the front end declares — the printer and the checker both recurse over a
/// program's shape, and every entry point in this workspace dispatches onto it.
fn check(root: &std::path::Path) {
    let mut seen = 0;
    for dir in ["corpus", "awfy", "clbg", "sicp", "examples", "lib"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "beck") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable");
            let name = path.display().to_string();
            let (_, diags, map) = beck_core::compile_or_library_str(&name, &src);
            assert!(
                !diags.iter().any(|d| d.code == "B0214"),
                "{name} hit the expansion budget:\n{}",
                diags.render(&map)
            );
            seen += 1;
        }
    }
    assert!(seen > 40, "only {seen} programs were checked");
}
