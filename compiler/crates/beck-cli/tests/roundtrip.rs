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
//! (`docs/80` §80.11).
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
use beck_syntax::{node::Node, print};

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

/// Parse the way `beck fmt` does, which is the point.
///
/// Not `parser::parse_module`: comments are attached by a pass *after* parsing
/// (`beck_syntax::doc`), so a harness that called the parser directly would assert idempotence
/// over trees with no comments in them and pass however badly the formatter treated them. Every
/// property in this file is about what `beck fmt` writes, so it goes through what `beck fmt` calls.
fn parse(name: &str, src: &str) -> (Node, String) {
    let mut map = SourceMap::new();
    let file = map.add(name, src);
    let mut d = Diagnostics::new();
    let node = beck_syntax::parse_file(file, name, src, &mut d);
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

/// **No comment is deleted**, over every program in the tree.
///
/// Idempotence above is necessary and not sufficient: a formatter that dropped every comment would
/// be perfectly idempotent from the second run onwards. This is the property
/// `DEFECTS.md::fmt-comments` was about — the lexer skipped `#` lines, so `beck fmt` deleted every
/// one of them, which is why `textDocument/formatting` was deliberately not offered.
///
/// The comments are extracted here rather than asked of `beck_syntax::doc`, and that is the point:
/// a test that used the collector would agree with it about which lines are comments however wrong
/// it was. This walks the text.
#[test]
fn formatting_keeps_every_comment() {
    beck_diag::depth::on_the_front_end_stack(keeps_every_comment)
}

fn keeps_every_comment() {
    let mut checked = 0usize;
    let mut bad: Vec<String> = Vec::new();
    for path in every_program() {
        let name = path.display().to_string();
        let src = std::fs::read_to_string(&path).expect("readable");
        let (node, errs) = parse(&name, &src);
        assert!(errs.is_empty(), "{name} does not parse:\n{errs}");
        let out = print::to_python(&node);

        let printed = comments_in(&out);
        for want in comments_in(&src) {
            checked += 1;
            if !printed.contains(&want) {
                bad.push(format!("{name}: `beck fmt` deleted {want:?}"));
            }
        }
    }
    assert!(checked > 200, "only {checked} comments were checked");
    assert!(bad.is_empty(), "{} lost:\n{}", bad.len(), bad.join("\n"));
    println!("{checked} comments across the tree, every one of them still there afterwards");
}

/// Every ordinary comment in a source text, as its text with the `#` and padding trimmed.
///
/// A `#` inside a string literal is not a comment; Beck strings cannot span lines, so a line at a
/// time is exact. A `##` is documentation and is `Meta::doc`'s business, not this one.
fn comments_in(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let bytes = line.as_bytes();
        let (mut in_string, mut i) = (false, 0usize);
        while i < bytes.len() {
            match bytes[i] {
                b'\\' if in_string => i += 1,
                b'"' => in_string = !in_string,
                b'#' if !in_string => {
                    let text = line[i..].trim_end();
                    if !text.starts_with("##") {
                        out.push(text.to_string());
                    }
                    break;
                }
                _ => {}
            }
            i += 1;
        }
    }
    out
}

/// A comment in every position the grammar allows, and the file comes back byte for byte.
///
/// The gate `DEFECTS.md::fmt-comments` asked for. `formatting_keeps_every_comment` says nothing is
/// deleted; this says each one comes back *where it was written*, which is the difference between
/// a formatter somebody will run on save and one they will run once.
///
/// The fixture is already in canonical form, so this is byte-identity rather than
/// round-trip-to-a-fixed-point: `beck fmt` normalises parentheses and blank lines between items,
/// and a fixture that needed normalising would be asserting those instead.
#[test]
fn a_comment_in_every_position_survives_formatting() {
    const EVERY_POSITION: &str = r#"# A file header, which is a comment block with
# more than one line in it.

# A second block, after a blank line.
model Point:
    # Above a field.
    x: Int
    y: Int  # and at the end of one

union Shape:
    # Above a variant.
    Dot
    Box(w: Int, h: Int)  # and at the end of one

# Above a definition.
def area(s: Shape) -> Int:  # and at the end of its first line
    # Above a statement, inside a block.
    match s:
        # Above an arm.
        case Dot:
            return 0
        case Box(w, h):
            return (w * h)  # at the end of a statement
    # At the end of a body, with nothing after it.

# An ordinary comment above a documented definition.
## The documentation sits immediately above what it documents.
def twice(n: Int) -> Int:
    return (n * 2)

# The last thing in the file.
"#;

    let (node, errs) = parse("every.beck", EVERY_POSITION);
    assert!(errs.is_empty(), "the fixture does not parse:\n{errs}");
    let out = print::to_python(&node);
    assert_eq!(out, EVERY_POSITION, "`beck fmt` moved or dropped a comment");
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
