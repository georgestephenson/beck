//! The Computer Language Benchmarks Game, in Beck.
//!
//! [`docs/25-benchmarks-and-expressiveness.md`](../../../../docs/25-benchmarks-and-expressiveness.md)
//! §25.2 adopts the Benchmarks Game for "the language core, popularly", and §25.9 schedules the
//! harness for Phase 3 with **no compute number published** until there is a second backend for
//! one to be about. This file is the harness half of that.
//!
//! [`docs/64-compile-speed-report.md`](../../../../docs/64-compile-speed-report.md) §64.7.1 is why
//! it did not exist until now, and it is a constraint on *how* rather than a doubt about whether:
//!
//! > So the ten benchmarks could be *written* here and not one of them could be *verified*, which
//! > would produce a suite that measures Beck against numbers this repository made up.
//!
//! The suite's expected-output files are reachable now, so they are checked in under
//! `clbg/expected/` and **everything below is organised around making them the only oracle**.
//!
//! Four things are asserted, in order of what they are worth:
//!
//! 1. **Every file in `clbg/` passes its own tests**, through the binary, with no list of file
//!    names — a benchmark added to that directory is gated by being there.
//! 2. **Each port's asserted output is the published file, byte for byte.** This is the assertion
//!    the whole directory exists for. A port asserts a literal (or, for the two 10 KB outputs, a
//!    digest); this file reconstructs that literal from `clbg/expected/` and fails if the two have
//!    drifted. A constant invented here cannot survive it — which is the property §64.7.1 said an
//!    unverifiable harness could not have.
//! 3. **The suite is the suite.** Which of the ten are ported *is* a claim, and which are not is a
//!    larger one, so both lists are written down here.
//! 4. **Provenance travels with the code.** These are ports of somebody else's BSD-licensed
//!    benchmarks, and a file that stops saying so is a licensing problem rather than a style one.
//!
//! What is deliberately **not** here is a threshold. `measure_clbg.rs` prints wall-clock and
//! nothing gates on it, for [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7's reason:
//! a timing gate on a shared runner flakes, and a gate that flakes gets deleted.

use std::path::{Path, PathBuf};
use std::process::Command;

fn clbg_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .join("clbg")
}

fn beck_files() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(clbg_dir())
        .expect("the benchmark directory is where the harness expects it")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("beck"))
        .collect();
    out.sort();
    out
}

fn beck(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(args)
        .output()
        .expect("the compiler is built");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn source(stem: &str) -> String {
    std::fs::read_to_string(clbg_dir().join(format!("{stem}.beck")))
        .unwrap_or_else(|_| panic!("{stem}.beck is readable"))
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(clbg_dir().join("expected").join(name))
        .unwrap_or_else(|_| panic!("clbg/expected/{name} is readable"))
}

/// The seven benchmarks of the ten that are ported.
///
/// Written down here rather than left to the directory listing because *which* of the suite is
/// ported is the claim the report makes, and a listing cannot be wrong about a file that was never
/// added.
const PORTED: [&str; 7] = [
    "binarytrees",
    "fannkuchredux",
    "fasta",
    "knucleotide",
    "nbody",
    "revcomp",
    "spectralnorm",
];

/// The three that are not, and the reason each is not.
///
/// Kept beside the seven and asserted against the directory, because "we ported seven of ten" is
/// only honest if the missing three are named. `docs/68` §68.6 is the long form of each reason;
/// dropping a benchmark from this list without adding it to `PORTED` is what this stops.
const NOT_PORTED: [(&str, &str); 3] = [
    (
        "mandelbrot",
        "the published output is a binary PBM whose bytes are not UTF-8, and Beck's Str is",
    ),
    (
        "pidigits",
        "needs lib/bignum.beck, which no module outside lib/ can import",
    ),
    (
        "regexredux",
        "the suite requires its nine regex patterns, and Beck has no regex",
    ),
];

/// The support modules — everything in `clbg/` that is not a benchmark.
///
/// Named rather than inferred, so that a benchmark that fails to be recognised as one shows up as
/// a stray support module rather than as nothing at all.
const SUPPORT: [&str; 1] = ["format"];

/// Every file in `clbg/` runs its own tests and passes them.
///
/// The test inside each port asserts its output against the suite's published file, so this is the
/// whole correctness claim of the directory: not that the Beck runs, but that it computes what the
/// Java computes, character for character.
#[test]
fn every_benchmark_verifies_against_the_suites_own_published_output() {
    for path in beck_files() {
        let file = path.to_string_lossy().to_string();
        let (ok, text) = beck(&["test", &file]);
        assert!(ok, "`beck test {file}`:\n{text}");
        assert!(text.contains("0 failed"), "{file}:\n{text}");
    }
}

/// And each one is a **library** — no merge point, nothing to deploy.
///
/// A benchmark that had grown an application around it would be measuring the application, which
/// is the failure mode [`27`](../../../../docs/27-walls-report.md) removed for `sicp/` and this
/// directory inherits from `awfy/`.
#[test]
fn each_benchmark_is_a_library() {
    for path in beck_files() {
        let file = path.to_string_lossy().to_string();
        let (ok, text) = beck(&["check", &file]);
        assert!(ok, "`beck check {file}`:\n{text}");
        assert!(text.contains("a library"), "{file}:\n{text}");
    }
}

/// The directory is the seven ports and the support modules, and nothing else.
#[test]
fn the_ported_suite_is_seven_of_the_games_ten() {
    let mut found: Vec<String> = beck_files()
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
        .collect();
    found.sort();
    let mut expected: Vec<String> = PORTED
        .iter()
        .chain(&SUPPORT)
        .map(|s| s.to_string())
        .collect();
    expected.sort();
    assert_eq!(found, expected, "the ported suite changed shape");

    // And the three that are not ported are still not, which is the half of the claim a directory
    // listing cannot make. A file appearing for one of them without leaving this list is a report
    // that has gone stale.
    for (name, _) in NOT_PORTED {
        assert!(
            !found.contains(&name.to_string()),
            "{name} is ported now — move it to PORTED and correct docs/68 §68.6"
        );
    }
}

/// A Beck string literal for `text` — the escaping `expect … == "…"` needs.
///
/// Only the four escapes the published outputs actually contain are handled, and anything else is
/// a panic rather than a passthrough: a silently mis-escaped byte would make this whole file
/// compare two things that are both wrong.
fn as_beck_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            c => panic!("clbg/expected/ has a character this escaper does not handle: {c:?}"),
        }
    }
    out
}

/// **The assertion this directory exists for.** Each port's expected output is the suite's file.
///
/// The five small outputs are asserted in Beck as string literals, and this reconstructs each
/// literal from `clbg/expected/` and looks for it in the source. That is a stronger claim than
/// "the tests pass": the tests would also pass against a constant somebody typed from memory, and
/// `docs/64` §64.7.1 is the record of why that specific failure was worth waiting to avoid.
#[test]
fn each_ports_asserted_output_is_the_published_file_byte_for_byte() {
    let cases = [
        ("spectralnorm", "spectralnorm-output.txt"),
        ("nbody", "nbody-output.txt"),
        ("fannkuchredux", "fannkuchredux-output.txt"),
        ("binarytrees", "binarytrees-output.txt"),
        ("knucleotide", "knucleotide-output.txt"),
    ];
    for (stem, file) in cases {
        let literal = as_beck_literal(&expected(file));
        assert!(
            source(stem).contains(&format!("== \"{literal}\"")),
            "{stem}.beck does not assert clbg/expected/{file}.\n\
             The published output, as the literal the port must contain:\n  \"{literal}\""
        );
    }
}

/// The same claim for the two outputs asserted by digest rather than by literal.
///
/// `fasta` and `revcomp` each produce 10,245 characters. A literal that long asserts the same
/// thing and is no more checkable by a reader, so the port asserts `digest(…) == "<hex>"` and this
/// recomputes the hex from the published file. The oracle is still the file: nothing in the
/// directory chose the number, and this fails if anything ever does.
#[test]
fn the_two_large_outputs_are_asserted_by_the_published_files_own_digest() {
    for (stem, file) in [
        ("fasta", "fasta-output.txt"),
        ("revcomp", "revcomp-output.txt"),
    ] {
        let hex = blake3::hash(expected(file).as_bytes()).to_hex().to_string();
        assert!(
            source(stem).contains(&hex),
            "{stem}.beck does not assert the digest of clbg/expected/{file}, which is {hex}"
        );
    }
}

/// The suite's own pipeline: the input two benchmarks read is the output a third writes.
///
/// `revcomp.beck` and `knucleotide.beck` generate their input by calling `fasta.beck` rather than
/// reading a file, because Beck has no stdin and a `test` block has no file to open. That
/// substitution is only exact if the suite's published *input* files really are its published
/// fasta *output* — so this asserts it rather than the ports assuming it.
#[test]
fn the_input_the_readers_generate_is_the_input_the_game_publishes() {
    let fasta = expected("fasta-output.txt");
    for input in ["revcomp-input.txt", "knucleotide-input.txt"] {
        assert_eq!(
            expected(input),
            fasta,
            "clbg/expected/{input} is no longer fasta's published output, so generating it \
             instead of reading it is no longer the same program"
        );
    }
}

/// Each file says whose benchmark it is a port of.
///
/// These are ports of programs published under the Benchmarks Game's BSD 3-clause licence, whose
/// first condition is that redistributed source retains the copyright notice. `clbg/README.md`
/// carries the notice in full and each file names the suite and the URL; a header that lost the
/// attribution would be the one defect in this directory that no amount of green tests would
/// surface.
#[test]
fn every_benchmark_names_the_suite_it_is_a_port_of() {
    for path in beck_files() {
        let text = std::fs::read_to_string(&path).expect("readable");
        assert!(
            text.contains("The Computer Language Benchmarks Game"),
            "{} does not name the suite it is a port of",
            path.display()
        );
        assert!(
            text.contains("https://salsa.debian.org/benchmarksgame-team/benchmarksgame/"),
            "{} does not carry the suite's URL",
            path.display()
        );
    }
}

/// Every port's entry point is `<benchmark>_output`, and it is the whole of stdout.
///
/// The naming is not decoration. Beck links modules into one flat namespace with no qualified
/// reference — `B0601`, "defined in more than one module" — so two benchmarks that both called
/// their entry `benchmark` could never be imported by the same program, and `revcomp` and
/// `knucleotide` both import `fasta`. Naming each entry after its own benchmark is what makes the
/// directory composable at all, so it is gated rather than left to habit (`docs/68` §68.4).
#[test]
fn every_port_publishes_its_output_under_its_own_name() {
    for stem in PORTED {
        assert!(
            source(stem).contains(&format!("def {stem}_output(")),
            "{stem}.beck does not define `{stem}_output`"
        );
    }
}

/// The three that are not ported are still not portable, for the reasons recorded.
///
/// Each reason is a fact about the language rather than about effort, and each would stop being
/// true if the language changed — which is the point of asserting them. A regex primitive, a byte
/// string, or an import that reaches `lib/` each turns one of these red, and the change that adds
/// it is the change that should be porting the benchmark.
#[test]
fn the_three_unported_benchmarks_are_still_out_of_reach() {
    let prelude = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the workspace root")
        .join("crates/beck-core/src/prelude.rs");
    let prims = std::fs::read_to_string(prelude).expect("the prelude is readable");

    // regexredux: the suite requires its nine patterns, and there is nothing to run them with.
    assert!(
        !prims.contains("\"regex_match\"") && !prims.contains("\"regex_replace\""),
        "there is a regex primitive now — port regexredux and correct docs/68 §68.6"
    );

    // mandelbrot: the output is packed bits with NUL bytes in it, and `Str` is UTF-8.
    assert!(
        !prims.contains("\"bytes_of\"") && !prims.contains("Ty::BYTES"),
        "there is a byte string now — port mandelbrot and correct docs/68 §68.6"
    );

    // pidigits: the arbitrary-precision integer it needs is in `lib/`, and `import` resolves only
    // against the root module's own directory. This is the finding, asserted as a fact: the same
    // file that works in `lib/` fails in `clbg/`.
    let probe = clbg_dir().join("import-reaches-lib-probe.beck");
    std::fs::write(
        &probe,
        "import bignum\n\ntest \"reachable\":\n    expect true\n",
    )
    .expect("a scratch file");
    let (ok, text) = beck(&["check", probe.to_string_lossy().as_ref()]);
    let _ = std::fs::remove_file(&probe);
    assert!(
        !ok && text.contains("cannot find module `bignum`"),
        "`import bignum` resolves from clbg/ now — port pidigits and correct docs/68 §68.4:\n{text}"
    );
}

/// The directory's own README exists and says what the port changes.
///
/// The ports are not transcriptions — there is no stdin, no thread pool and no bitwise operator —
/// so the rules they follow have to be written down in one place or each file will invent its own.
#[test]
fn the_directory_documents_what_the_port_changes() {
    let readme = std::fs::read_to_string(clbg_dir().join("README.md")).expect("a README");
    for expected in [
        "What the port changes",
        "Provenance",
        "What is not here",
        "The oracle",
    ] {
        assert!(readme.contains(expected), "the README lost `{expected}`");
    }
    // The licence is BSD 3-clause and its first condition is that the notice travels. It is quoted
    // in the README rather than linked, because a link is not a retained notice.
    assert!(
        readme.contains("Isaac Gouy") && readme.contains("Redistributions of source code"),
        "the README no longer carries the Benchmarks Game's copyright notice"
    );
}
