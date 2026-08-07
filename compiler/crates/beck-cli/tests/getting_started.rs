//! `docs/86-getting-started.md`, compiled.
//!
//! [`08`](../../../../docs/08-roadmap.md) §8.5.4 names documentation as the one remaining reason
//! Phase 3's exit criterion cannot be attempted. A guide is the answer to that, and **a guide whose
//! examples do not compile is worse than no guide** — a newcomer who types what it says and gets a
//! diagnostic learns that the documentation lies, which is a slower and more expensive lesson than
//! finding nothing at all.
//!
//! So the guide is gated the way `docs/reference/` is: this file reads the markdown, extracts every
//! ` ```beck ` block and every `beck <command>` shown, and checks them. The discipline is
//! `docs/34`'s — a document derived from or checked against the compiler cannot drift from it in
//! silence.
//!
//! Two conventions the guide has to keep, both asserted below:
//!
//! * a ` ```beck ` block is a **complete module**, checked with the real front end. A fragment goes
//!   in a ` ```text ` block, which nothing here reads;
//! * a command shown in a ` ```text ` block starting `$ beck ` names a real subcommand.

use std::path::{Path, PathBuf};

fn guide() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/86-getting-started.md")
        .canonicalize()
        .expect("the guide is checked in")
}

/// Every fenced block with the given language tag, in document order.
fn blocks(src: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in src.lines() {
        match &mut current {
            Some(buf) => {
                if line.trim_start().starts_with("```") {
                    out.push(std::mem::take(buf));
                    current = None;
                } else {
                    buf.push_str(line);
                    buf.push('\n');
                }
            }
            None => {
                if line.trim_end() == format!("```{tag}") {
                    current = Some(String::new());
                }
            }
        }
    }
    out
}

/// Every Beck program in the guide compiles, with the real front end.
#[test]
fn every_program_in_the_guide_compiles() {
    let src = std::fs::read_to_string(guide()).expect("readable");
    let programs = blocks(&src, "beck");
    assert!(
        programs.len() >= 3,
        "the guide should build up more than {} programs",
        programs.len()
    );

    for (i, program) in programs.iter().enumerate() {
        let (_, d, map) = beck_diag::depth::on_the_front_end_stack(|| {
            beck_core::compile_or_library_str("shelf.beck", program)
        });
        assert!(
            !d.has_errors(),
            "program {} of {} in docs/86 does not compile:\n{}\n---\n{program}",
            i + 1,
            programs.len(),
            d.render(&map)
        );
    }
}

/// …and the tests each program declares pass, which is a stronger claim than "it compiles".
///
/// The guide shows their output. A reader who runs `beck test` and sees a failure where the guide
/// printed `ok` has been told something untrue, and the shape of that lie — a passing compile and a
/// failing assertion — is exactly what a compile-only gate would miss.
#[test]
fn every_program_in_the_guide_passes_its_own_tests() {
    let src = std::fs::read_to_string(guide()).expect("readable");
    for (i, program) in blocks(&src, "beck").iter().enumerate() {
        let (placed, d, map) = beck_core::compile_or_library_str("shelf.beck", program);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let placed = placed.expect("it compiles");
        let backend = beck_eval::backend(&placed);
        let report = beck_rt::testing::run(&placed, backend, &beck_rt::testing::Options::default());
        for case in &report.cases {
            assert!(
                case.outcome.is_pass(),
                "in program {} of docs/86, `{}` fails: {:?}",
                i + 1,
                case.name,
                case.outcome
            );
        }
    }
}

/// Every `beck <command>` the guide shows is a subcommand the binary has.
///
/// The cheapest kind of documentation rot: a guide that names a command somebody renamed. Checked
/// against the CLI's own help rather than against a list here, because a second list is a second
/// thing to keep true.
#[test]
fn every_command_the_guide_shows_exists() {
    let src = std::fs::read_to_string(guide()).expect("readable");
    let shown: Vec<String> = blocks(&src, "text")
        .iter()
        .flat_map(|b| b.lines().map(|l| l.to_string()).collect::<Vec<_>>())
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("$ beck ")?;
            let word = rest.split_whitespace().next()?;
            // `--version` and friends are flags, not subcommands.
            (!word.starts_with('-')).then(|| word.to_string())
        })
        .collect();
    assert!(
        shown.len() >= 4,
        "the guide should show more commands than {shown:?}"
    );

    let help = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
        .arg("--help")
        .output()
        .expect("the binary this test run built");
    let help = String::from_utf8_lossy(&help.stdout).to_string();
    for command in shown {
        assert!(
            help.lines()
                .any(|l| l.trim_start().starts_with(&format!("{command} "))),
            "docs/86 shows `beck {command}`, which the binary's own help does not list"
        );
    }
}
