//! `parse(print(parse(src)))` — over every Beck program in the tree.
//!
//! `beck-syntax/src/print.rs` opens by naming this property and the file that asserts it:
//!
//! > Round-tripping is lossless *modulo formatting*: `parse(print(parse(src)))` is structurally
//! > equal to `parse(src)`, which is the property `tests/roundtrip.rs` asserts over the corpus.
//!
//! That file did not exist. What asserted the property was three hand-written snippets inside the
//! printer's own `#[cfg(test)]` module, and a shape none of the three used could print as something
//! that is not surface syntax without anything noticing — which is exactly what had happened:
//! `decoded = try: base64_decode(…)` printed as `decoded = try(base64_decode(…))`, in a checked-in
//! standard-library file, so `beck fmt lib/crypto.beck` emitted a program that does not compile
//! (`docs/80` §80.6).
//!
//! So the corpus this asserts over is not the corpus but **the tree**: every `.beck` file under
//! `compiler/`, plus the examples. A form that cannot print itself is now a failing test the day
//! somebody writes it, rather than the day somebody formats a file.
//!
//! Three properties, and the second is the one that would have caught it on its own:
//!
//! 1. printing is **idempotent** — formatting a formatted file changes nothing;
//! 2. the printed text **re-parses**, with no diagnostics;
//! 3. it re-parses to a **structurally equal tree** — same forms, same order, spans aside.
//!
//! Both run on `beck_diag::depth::on_the_front_end_stack`, because the printer recurses over the
//! same user-chosen structure the reader does and this is a harness rather than the CLI. The first
//! version of this file did not, and `awfy/havlak.beck` overflowed a test thread's default stack
//! before it got to assert anything — which is `docs/adr/0012`'s declared-stack rule finding its
//! way to the one caller of `beck-syntax` that is not `beck-cli`.

use std::path::{Path, PathBuf};

use beck_diag::{Diagnostics, SourceMap};
use beck_syntax::{node::Node, parser, print};

/// Every `.beck` file in the tree, in path order.
///
/// Rooted at the workspace rather than at one directory, because the point is that no *shape* a
/// program in this repository uses can print as something that is not surface syntax — and the
/// shapes are spread across the standard library, two SICP chapters, two benchmark suites and the
/// corpus.
fn every_program() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the compiler directory is checked in");
    let mut out = Vec::new();
    for dir in [
        "corpus",
        "corpus/project",
        "examples",
        "lib",
        "sicp",
        "sicp/refusals",
        "awfy",
        "clbg",
    ] {
        let path = root.join(dir);
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue; // `sicp/refusals/` is empty, and an empty directory may not exist
        };
        for e in entries.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "beck") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn parse(name: &str, src: &str) -> (Node, String) {
    let mut map = SourceMap::new();
    let file = map.add(name, src);
    let mut d = Diagnostics::new();
    let node = parser::parse_module(file, "t", src, &mut d);
    let rendered = if d.has_errors() {
        d.render(&map)
    } else {
        String::new()
    };
    (node, rendered)
}

#[test]
fn every_program_in_the_tree_prints_back_as_itself() {
    beck_diag::depth::on_the_front_end_stack(prints_back_as_itself)
}

fn prints_back_as_itself() {
    let programs = every_program();
    assert!(
        programs.len() > 60,
        "the tree has more Beck programs than this: {}",
        programs.len()
    );

    let mut bad: Vec<String> = Vec::new();
    for path in programs {
        let name = path
            .strip_prefix(path.parent().and_then(|p| p.parent()).unwrap_or(&path))
            .unwrap_or(&path)
            .display()
            .to_string();
        let src = std::fs::read_to_string(&path).expect("readable");

        let (node, errs) = parse(&name, &src);
        assert!(errs.is_empty(), "{name} does not parse:\n{errs}");

        let once = print::to_python(&node);
        let (again, errs) = parse(&name, &once);
        if !errs.is_empty() {
            bad.push(format!("{name}: printed text does not parse:\n{errs}"));
            continue;
        }
        if !node.structurally_eq(&again) {
            bad.push(format!("{name}: printing changed the program"));
            continue;
        }
        let twice = print::to_python(&again);
        if once != twice {
            bad.push(format!("{name}: `beck fmt` is not idempotent"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} programs:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// §2.2's other half: the S-expression surface reads back to the same tree too.
#[test]
fn every_program_in_the_tree_round_trips_through_the_s_expression_surface() {
    beck_diag::depth::on_the_front_end_stack(round_trips_through_the_canonical_form)
}

fn round_trips_through_the_canonical_form() {
    for path in every_program() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("readable");
        let (node, errs) = parse(&name, &src);
        assert!(errs.is_empty(), "{name} does not parse:\n{errs}");

        let printed = print::to_sexpr_pretty(&node);
        let mut map = SourceMap::new();
        let file = map.add(&name, &printed);
        let mut d = Diagnostics::new();
        let read = beck_syntax::sexpr::read_all(file, &printed, &mut d);
        assert!(
            !d.has_errors(),
            "{name}: the canonical form does not read back:\n{}",
            d.render(&map)
        );
        assert_eq!(
            read.len(),
            1,
            "{name}: a module prints as one form, not {}",
            read.len()
        );
        assert!(
            node.structurally_eq(&read[0]),
            "{name}: the canonical form is a different tree"
        );
    }
}
