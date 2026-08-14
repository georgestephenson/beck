//! What the front end costs, per phase and per program, printed and never gated.
//!
//! Run with `cargo test --release --test measure_compile -- --nocapture`.
//!
//! [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7 lists "keystroke→diagnostic
//! latency, incremental build time, clean build time" among the budgets every merge answers to,
//! and [`docs/25-benchmarks-and-expressiveness.md`](../../../../docs/25-benchmarks-and-expressiveness.md)
//! §25.9 schedules the compile-speed budgets for Phase 3. This is the measuring half. The gate is
//! [`compile_speed.rs`](compile_speed.rs), and it asserts a *shape* rather than a rate for §13.7's
//! own reason: a timing threshold on a shared runner cannot be held honestly, and a gate that
//! flakes gets deleted.
//!
//! # What the numbers are
//!
//! Every phase is timed **in-process** and separately, against the same source:
//!
//! | | what it is |
//! |---|---|
//! | **parse** | `beck_syntax::parse_file` — lex and parse to a [`Node`](beck_syntax) tree |
//! | **expand** | `beck_macro::expand_module` — hygienic macro expansion |
//! | **check** | `beck_core::check_module` — types, effect rows, exhaustiveness |
//! | **place** | `place::solve` + `apply` + `check_placement` — the tier assignment |
//! | **secure** | `secure::check_security` — §3.5's flow properties |
//!
//! In-process rather than through the binary, because the question is which phase costs what and
//! a subprocess measurement answers it with process startup mixed in. `measure_awfy.rs` measures
//! the binary, which is the other question — what a person waits for.
//!
//! Nothing here is compared to another language, another compiler, or a previous run. A number is
//! comparable to another number from the same table on the same machine and to nothing else.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use beck_core::{check_module, place, secure};
use beck_diag::{Diagnostics, SourceMap};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .to_path_buf()
}

/// Every `.beck` file in a directory, sorted so a table is stable across runs.
fn beck_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("beck"))
        .collect();
    out.sort();
    out
}

/// One pass of the whole front end, with each phase timed separately.
///
/// Diagnostics are collected and discarded: a file that does not compile still parses and checks,
/// and this measures the work rather than the verdict.
#[derive(Default, Clone, Copy)]
struct Phases {
    parse: Duration,
    expand: Duration,
    check: Duration,
    place: Duration,
    secure: Duration,
}

impl Phases {
    fn total(&self) -> Duration {
        self.parse + self.expand + self.check + self.place + self.secure
    }
}

fn time_phases(name: &str, src: &str) -> Phases {
    let mut map = SourceMap::new();
    let file = map.add(name, src);
    let mut diags = Diagnostics::new();
    let mut t = Phases::default();

    let started = Instant::now();
    let parsed = beck_syntax::parse_file(file, name, src, &mut diags);
    t.parse = started.elapsed();

    let started = Instant::now();
    let expanded = beck_macro::expand_module(&parsed, &mut diags);
    t.expand = started.elapsed();

    let started = Instant::now();
    let mut program = check_module(&expanded, &mut diags);
    t.check = started.elapsed();

    let started = Instant::now();
    let solution = place::solve(&program, None);
    place::apply(&mut program, &solution);
    place::check_placement(&program, &mut diags);
    t.place = started.elapsed();

    let started = Instant::now();
    secure::check_security(&program, &mut diags);
    t.secure = started.elapsed();

    t
}

/// The median of five passes, on the stack the front end declares it needs.
///
/// `beck_diag::depth::on_the_front_end_stack` is what the CLI dispatches onto, so measuring
/// anywhere else measures a configuration nobody runs — and libtest's own thread is small enough
/// that a few thousand sequential local bindings overflow it (`docs/64` §64.4).
fn median_of_five(name: &str, src: &str) -> Phases {
    beck_diag::depth::on_the_front_end_stack(|| median_of_five_here(name, src))
}

fn median_of_five_here(name: &str, src: &str) -> Phases {
    let mut runs: Vec<Phases> = (0..5).map(|_| time_phases(name, src)).collect();
    let pick = |f: fn(&Phases) -> Duration| {
        let mut v: Vec<Duration> = runs.iter().map(f).collect();
        v.sort();
        v[2]
    };
    let out = Phases {
        parse: pick(|p| p.parse),
        expand: pick(|p| p.expand),
        check: pick(|p| p.check),
        place: pick(|p| p.place),
        secure: pick(|p| p.secure),
    };
    runs.clear();
    out
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

/// Every checked-in Beck program, per phase.
///
/// The corpus, the SICP chapters, the Are We Fast Yet ports, the standard library and the
/// examples — which between them are the largest and the most various Beck source that exists.
#[test]
fn the_front_end_over_every_program_in_the_tree() {
    let root = root();
    let mut files = Vec::new();
    for dir in ["corpus", "sicp", "awfy", "lib", "examples"] {
        let path = root.join(dir);
        if path.is_dir() {
            for f in beck_files(&path) {
                files.push((dir, f));
            }
        }
    }
    assert!(
        files.len() >= 50,
        "the tree has more Beck in it than this found: {} files",
        files.len()
    );

    println!();
    println!("front end, per phase, median of five in-process passes (ms)");
    println!(
        "{:<28} {:>6} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "program", "lines", "parse", "expand", "check", "place", "secure", "total"
    );

    let (mut lines_all, mut total_all) = (0usize, Duration::ZERO);
    let mut slowest: Vec<(Duration, String, usize)> = Vec::new();
    for (dir, path) in &files {
        let src = std::fs::read_to_string(path).expect("a checked-in file is readable");
        let name = format!(
            "{dir}/{}",
            path.file_name().expect("a file").to_string_lossy()
        );
        let lines = src.lines().count();
        let t = median_of_five(&name, &src);
        lines_all += lines;
        total_all += t.total();
        slowest.push((t.total(), name.clone(), lines));
        println!(
            "{:<28} {:>6} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2}",
            name,
            lines,
            ms(t.parse),
            ms(t.expand),
            ms(t.check),
            ms(t.place),
            ms(t.secure),
            ms(t.total())
        );
    }

    slowest.sort_by(|a, b| b.0.cmp(&a.0));
    println!();
    println!(
        "{} programs, {lines_all} lines, {:.1} ms of front end — {:.0} lines/s",
        files.len(),
        ms(total_all),
        lines_all as f64 / total_all.as_secs_f64()
    );
    println!("slowest three:");
    for (d, name, lines) in slowest.iter().take(3) {
        println!(
            "  {name:<26} {lines:>5} lines  {:>8.2} ms  ({:.0} lines/s)",
            ms(*d),
            *lines as f64 / d.as_secs_f64()
        );
    }
}

/// §13.7's "keystroke→diagnostic latency", measured as the work an editor would repeat.
///
/// An LSP re-runs parse, expand and check on each edit and does **not** re-run placement or the
/// security pass to draw a squiggle, so that prefix is what this reports — separately from the
/// whole front end, because the two answer different questions and only the first is on the path
/// between a keystroke and a diagnostic.
#[test]
fn the_prefix_an_editor_reruns_per_keystroke() {
    let root = root();
    let mut rows: Vec<(String, usize, Duration, Duration)> = Vec::new();
    for dir in ["corpus", "sicp", "awfy", "lib"] {
        for path in beck_files(&root.join(dir)) {
            let src = std::fs::read_to_string(&path).expect("readable");
            let name = format!(
                "{dir}/{}",
                path.file_name().expect("a file").to_string_lossy()
            );
            let lines = src.lines().count();
            let t = median_of_five(&name, &src);
            rows.push((name, lines, t.parse + t.expand + t.check, t.total()));
        }
    }
    rows.sort_by(|a, b| b.2.cmp(&a.2));

    println!();
    println!("keystroke→diagnostic: parse + expand + check, the prefix an editor reruns");
    let worst = rows.first().expect("the tree is not empty");
    println!(
        "  worst of {} programs: {} at {} lines — {:.2} ms ({:.2} ms for the whole front end)",
        rows.len(),
        worst.0,
        worst.1,
        ms(worst.2),
        ms(worst.3)
    );
    let median = &rows[rows.len() / 2];
    println!(
        "  median:                {} at {} lines — {:.2} ms",
        median.0,
        median.1,
        ms(median.2)
    );
}

/// A module `n` top-level definitions wide, and one `n` bindings deep.
///
/// The two axes a module grows along, and they are separate questions: a checker that re-walks its
/// environment per binding is quadratic in *depth*, and one that re-resolves every declaration per
/// declaration is quadratic in *width*.
fn wide(n: usize) -> String {
    (0..n)
        .map(|i| format!("def f{i}(x: Int) -> Int:\n    return x + {i}\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn deep(n: usize) -> String {
    let mut out = String::from("def deep(x: Int) -> Int:\n    v0 = x + 1\n");
    for i in 1..n {
        out.push_str(&format!("    v{i} = v{} + {i}\n", i - 1));
    }
    out.push_str(&format!("    return v{}\n", n - 1));
    out
}

/// The third axis: `n` definitions with an edge each, so the dependency graph grows too.
fn chained(n: usize) -> String {
    let mut out = String::from("def g0(x: Int) -> Int:\n    return x + 1\n");
    for i in 1..n {
        out.push_str(&format!(
            "\ndef g{i}(x: Int) -> Int:\n    return g{}(x) + {i}\n",
            i - 1
        ));
    }
    out
}

/// How each phase scales along each axis — the table `compile_speed.rs` gates one column of.
///
/// Printed as **cost per definition** rather than as a total, because that is the number whose
/// *shape* is the finding: a flat column is linear, and a column that doubles when `n` doubles is
/// quadratic.
#[test]
fn how_each_phase_scales_in_width_and_in_depth() {
    for (axis, gen) in [
        ("width — top-level definitions", wide as fn(usize) -> String),
        ("width — with one edge per definition", chained),
        (
            "depth — local bindings in one body",
            deep as fn(usize) -> String,
        ),
    ] {
        println!();
        println!("{axis}: µs per declaration, median of five");
        println!(
            "{:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
            "n", "parse", "expand", "check", "place", "secure", "total"
        );
        let mut prev: Option<(usize, f64)> = None;
        for n in [200usize, 400, 800, 1600, 3200] {
            let src = gen(n);
            let t = median_of_five("scale.beck", &src);
            let per = |d: Duration| d.as_secs_f64() * 1e6 / n as f64;
            println!(
                "{n:>7} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>9.2}",
                per(t.parse),
                per(t.expand),
                per(t.check),
                per(t.place),
                per(t.secure),
                per(t.total())
            );
            if let Some((pn, pt)) = prev {
                let growth = per(t.total()) / pt;
                println!(
                    "        {pn} → {n}: per-declaration cost ×{growth:.2} \
                     (1.0 is linear, 2.0 is quadratic)"
                );
            }
            prev = Some((n, per(t.total())));
        }
    }
}

/// What the two answers that *change the file* cost, at two sizes.
///
/// Both are `O(file)` and neither is on the keystroke path: hints are asked for once per viewport
/// and a rename once per rename. What is worth measuring is the **constant**, because a rename
/// re-analyses the text it proposes to write — [`Editor::rename`](beck_core::editor::Editor::rename)
/// declines an edit that would not compile, and that promise costs one more analysis of the file.
///
/// Two sizes rather than one, per [`AGENTS.md`](../../../../AGENTS.md): one measurement cannot tell
/// a linear cost from a quadratic one. Median of five, like every other number in this file, and on
/// a stack big enough for the front end — a cold first call parses the standard library too, and
/// reporting that as the cost of a keystroke would overstate it by an order of magnitude.
#[test]
fn what_an_inlay_hint_and_a_rename_cost() {
    use beck_core::editor::Editor;

    let root = root();
    println!();
    println!("the editor's two editing answers, in-process, median of five");
    for path in [
        root.join("corpus/01-counter.beck"),
        root.join("awfy/cd.beck"),
    ] {
        let src = std::fs::read_to_string(&path).expect("readable");
        let name = path.display().to_string();
        let lines = src.lines().count();

        let median = |mut runs: Vec<Duration>| {
            runs.sort();
            runs[runs.len() / 2]
        };
        let timed = |f: &dyn Fn()| {
            median(
                (0..5)
                    .map(|_| {
                        let started = Instant::now();
                        f();
                        started.elapsed()
                    })
                    .collect(),
            )
        };

        beck_diag::depth::on_the_front_end_stack(|| {
            // Once before anything is timed: the first analysis in a process parses the standard
            // library, and that cost is not part of any of the three questions below.
            let editor = Editor::of(&name, &src);
            let analysis = timed(&|| {
                Editor::of(&name, &src);
            });
            let hints = editor.hints().len();
            let hinting = timed(&|| {
                editor.hints();
            });

            // The first name that renames, so the number is a rename that happened rather than a
            // refusal that returned early.
            let names: Vec<String> = editor.symbols().map(|(n, _)| n.to_string()).collect();
            let mut renaming = Duration::default();
            let mut renamed = "none";
            for symbol in &names {
                let (start, end) = editor.symbol(symbol).and_then(|s| s.span).expect("a span");
                let caret = start
                    + src[start as usize..end as usize]
                        .find(symbol.as_str())
                        .expect("a declaration writes its own name") as u32;
                if editor.rename(caret, "renamed_by_the_measurement").is_err() {
                    continue;
                }
                renaming = timed(&|| {
                    editor.rename(caret, "renamed_by_the_measurement").ok();
                });
                renamed = symbol;
                break;
            }

            println!(
                "  {:<20} {lines:>4} lines  analyse {:>6.2} ms  hints {:>6.3} ms ({hints})  \
                 rename {:>6.2} ms (`{renamed}`)",
                path.file_name().expect("a file").to_string_lossy(),
                ms(analysis),
                ms(hinting),
                ms(renaming),
            );
        });
    }
}
