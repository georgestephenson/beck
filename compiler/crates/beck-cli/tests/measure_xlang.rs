//! Beck's native backend beside five other languages.
//!
//! Run with `cargo test --release --test measure_xlang -- --nocapture`.
//!
//! [`docs/25-benchmarks-and-expressiveness.md`](../../../../docs/25-benchmarks-and-expressiveness.md)
//! §25.9 rule 2 held every comparative claim back "until the second backend exists". It exists
//! ([`docs/93`](../../../../docs/93-llvm-backend-report.md)), and this is the first place in the
//! repository where a Beck number is put beside another language's.
//!
//! [`xlang/README.md`](../../../xlang/README.md) is the rules the ports are held to and the list of
//! what this does **not** measure — read that before quoting anything from here. The two lines
//! worth repeating: this is the **scalar subset**, which is the most flattering ground the native
//! backend has, and the integer semantics differ down the column.
//!
//! # What is gated, and what is only printed
//!
//! **The answers are gated. The times are not.** Every implementation must compute the same four
//! results as Beck, and that assertion is deterministic — it cannot flake on a shared runner, which
//! is what [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7 says a timing threshold
//! would do. It is also the thing that makes the table trustworthy: six independent implementations
//! agreeing on four answers is a far stronger statement about the ports than six files that look
//! alike.
//!
//! A language the machine does not have prints a skip and is left out. `clang` is the exception —
//! without it there is no Beck native backend to compare against, so the whole suite skips, and
//! `BECK_REQUIRE_LLVM=1` forbids that.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use beck_core::Value;
use beck_llvm::Artifact;

/// The four benchmarks, at the size `docs/93` §93.5 quotes.
const SIZES: [(&str, i64); 4] = [
    ("fib", 30),
    ("sum_to", 1_000_000),
    ("image", 96),
    ("xor_sweep", 20_000),
];

/// The answers, which every implementation has to agree on. Written out rather than taken from
/// whichever ran first: a table of six implementations agreeing on the wrong number is a table of
/// six implementations agreeing.
const ANSWERS: [(&str, i64); 4] = [
    ("fib", 832_040),
    ("sum_to", 500_000_500_000),
    ("image", 3_688),
    ("xor_sweep", 2_220_064),
];

/// No call may take longer than this. Generous: Ruby's `fib(30)` is the slowest thing here.
const LIMIT: Duration = Duration::from_secs(300);

/// Timings per benchmark, and the ports use the same number.
///
/// Eleven and not five: this runs on a shared, virtualised machine, and five samples of a 5 ms
/// benchmark put `fib(30)` anywhere between 6.1 ms and 8.6 ms across three runs of one binary. A
/// median is only as good as what it is a median of, and the extra samples cost seconds.
const RUNS: usize = 11;

fn xlang_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .join("xlang")
}

fn require_llvm() -> bool {
    std::env::var("BECK_REQUIRE_LLVM").is_ok_and(|v| v == "1")
}

fn on_the_path(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// One row: what ran it, and the median milliseconds and answer per benchmark.
struct Row {
    label: String,
    /// The integer arithmetic this row actually performs, because the column is not like for like.
    integers: &'static str,
    times: Vec<(String, f64, i64)>,
}

/// Run a port and read back its `name<TAB>milliseconds<TAB>answer` lines.
fn run(label: &str, integers: &'static str, mut cmd: Command) -> Option<Row> {
    for (_, n) in SIZES {
        cmd.arg(n.to_string());
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        println!(
            "skipped {label}: it exited {} — {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut times = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 3 {
            continue;
        }
        times.push((
            cols[0].to_string(),
            cols[1].trim().parse().ok()?,
            cols[2].trim().parse().ok()?,
        ));
    }
    (times.len() == SIZES.len()).then_some(Row {
        label: label.to_string(),
        integers,
        times,
    })
}

/// Compile one of the ports into a temporary directory.
fn build(compiler: &str, args: &[&str], out: &Path, source: &Path) -> bool {
    Command::new(compiler)
        .args(args)
        .arg("-o")
        .arg(out)
        .arg(source)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn median(runs: usize, mut f: impl FnMut() -> Value) -> (f64, i64) {
    let mut times = Vec::with_capacity(runs);
    let mut answer = Value::Unit;
    for _ in 0..runs {
        let started = Instant::now();
        answer = f();
        times.push(started.elapsed().as_secs_f64() * 1e3);
    }
    times.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a duration"));
    let n = match answer {
        Value::Int(i) => i,
        other => panic!("a benchmark answered {}", other.display()),
    };
    (times[runs / 2], n)
}

#[test]
fn beck_beside_five_other_languages() {
    let dir = xlang_dir();
    let src = std::fs::read_to_string(dir.join("bench.beck")).expect("xlang/bench.beck");
    let (placed, diags, map) = beck_core::compile_or_library_str("xlang/bench.beck", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let program = Arc::new(placed.expect("slices").program);

    let Some(artifact) = Artifact::build_within(&program, LIMIT).expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain, so there is no native backend to compare against.");
        return;
    };

    // Somewhere to put the compiled ports. Not `target/`: nothing here is a build artefact of the
    // workspace, and leaving binaries in the source tree is how a `.gitignore` entry gets written.
    let work = std::env::temp_dir().join(format!("beck-xlang-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("a working directory");

    let mut rows = Vec::new();

    // ---- Beck, native. The round trip is inside every number, and printed on its own below.
    let trip = {
        let mut ts: Vec<f64> = (0..2001)
            .map(|_| {
                let s = Instant::now();
                artifact.call("nothing", &[Value::Int(1)]).expect("answers");
                s.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        ts.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
        ts[ts.len() / 2]
    };
    let mut native = Vec::new();
    for (name, n) in SIZES {
        let args: Vec<Value> = match name {
            "sum_to" | "xor_sweep" => vec![Value::Int(n), Value::Int(0)],
            _ => vec![Value::Int(n)],
        };
        let (ms, answer) = median(RUNS, || artifact.call(name, &args).expect("native answers"));
        native.push((name.to_string(), ms, answer));
    }
    rows.push(Row {
        label: "Beck, native".into(),
        integers: "checked",
        times: native,
    });

    // ---- Beck, the tree-walker, so the table carries its own baseline.
    let evaluator = beck_eval::backend_for(program.clone());
    let walked: Vec<(String, f64, i64)> = beck_eval::on_the_evaluator_stack(|| {
        SIZES
            .iter()
            .map(|(name, n)| {
                let f = evaluator
                    .function(&program.defs[*name].body)
                    .expect("prepares");
                let args: Vec<Value> = match *name {
                    "sum_to" | "xor_sweep" => vec![Value::Int(*n), Value::Int(0)],
                    _ => vec![Value::Int(*n)],
                };
                let (ms, answer) = median(3, || f(args.clone()).expect("the evaluator answers"));
                (name.to_string(), ms, answer)
            })
            .collect()
    });
    rows.push(Row {
        label: "Beck, evaluator".into(),
        integers: "checked",
        times: walked,
    });

    // ---- C, twice: wrapping, and with Beck's overflow semantics.
    let clang = artifact.toolchain().clang.clone();
    let c = dir.join("bench.c");
    for (label, integers, flags, exe) in [
        ("C, -O2", "wrapping", vec!["-O2"], work.join("bench_c")),
        (
            "C, -O2, checked",
            "checked",
            vec!["-O2", "-DCHECKED=1"],
            work.join("bench_c_checked"),
        ),
    ] {
        if build(&clang.to_string_lossy(), &flags, &exe, &c) {
            rows.extend(run(label, integers, Command::new(&exe)));
        } else {
            println!("skipped {label}: it did not build");
        }
    }

    // ---- Rust, with `checked_*`: Beck's semantics in a safe language.
    let rs = work.join("bench_rs");
    if on_the_path("rustc") && build("rustc", &["-O"], &rs, &dir.join("bench.rs")) {
        rows.extend(run("Rust, -O", "checked", Command::new(&rs)));
    } else {
        println!("skipped Rust: no `rustc`, or it did not build the port");
    }

    // ---- The three that need no build step.
    for (label, integers, exe, script) in [
        ("Node", "f64", "node", "bench.js"),
        ("Python", "bignum", "python3", "bench.py"),
        ("Ruby", "bignum", "ruby", "bench.rb"),
    ] {
        if !on_the_path(exe) {
            println!("skipped {label}: no `{exe}` on the path");
            continue;
        }
        let mut cmd = Command::new(exe);
        cmd.arg(dir.join(script));
        rows.extend(run(label, integers, cmd));
    }

    // -- the gate ---------------------------------------------------------------------------
    //
    // Six implementations, four answers, no disagreement. This is the assertion; everything below
    // is printed.
    for row in &rows {
        for (name, _, got) in &row.times {
            let want = ANSWERS
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, a)| *a)
                .unwrap_or_else(|| panic!("no expected answer for `{name}`"));
            assert_eq!(
                *got, want,
                "{} computed {got} for `{name}` where every other implementation computes {want}",
                row.label
            );
        }
    }

    // -- the table --------------------------------------------------------------------------
    println!("\n{}", artifact.toolchain().version);
    println!(
        "a Beck call that computes nothing: {trip:.4} ms — the pipe, inside every Beck row below\n"
    );
    print!("{:<18}{:>10}", "implementation", "integers");
    for (name, n) in SIZES {
        print!("{:>18}", format!("{name}({n})"));
    }
    println!();
    for row in &rows {
        print!("{:<18}{:>10}", row.label, row.integers);
        for (name, _) in SIZES {
            let ms = row
                .times
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, ms, _)| *ms)
                .unwrap_or(f64::NAN);
            print!("{:>18}", format!("{ms:.3} ms"));
        }
        println!();
    }
    println!(
        "\n{} implementations, all agreeing on every answer. Wall clock, medians, this machine \
         only.\nRead xlang/README.md before quoting any of it: this is the scalar subset, and the \
         integers\ncolumn is why the rows are not all comparing the same thing.",
        rows.len()
    );

    let _ = std::fs::remove_dir_all(&work);
}

/// Is a gap on the mandelbrot the code generation, or the semantics?
///
/// Four spellings of one loop, all compiled by the same `clang -O2`, so the only variable is what
/// the language demands. `xlang/README.md` says what each is; the row that matters is `key`, which
/// is what `beck-llvm` emits today — if Beck's `image` number lands on it, the code generation is
/// at parity and what is left is the price of `docs/32` §32.2's structural equality on reals.
#[test]
fn what_the_mandelbrot_gap_is_made_of() {
    let Some(toolchain) = beck_llvm::Toolchain::find() else {
        assert!(!require_llvm(), "BECK_REQUIRE_LLVM=1 and no `clang`");
        println!("skipped: no LLVM toolchain.");
        return;
    };
    let work = std::env::temp_dir().join(format!("beck-xlang-ev-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("a working directory");
    let exe = work.join("escapes_variants");
    assert!(
        build(
            &toolchain.clang.to_string_lossy(),
            &["-O2"],
            &exe,
            &xlang_dir().join("escapes_variants.c")
        ),
        "the diagnostic did not build"
    );
    let out = Command::new(&exe).arg("96").output().expect("it runs");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("one mandelbrot at 96x96, C -O2, only the semantics changing:\n{text}");
    for line in text.lines() {
        assert!(
            line.contains("3688"),
            "every variant has to compute the same image: {line}"
        );
    }
    let _ = std::fs::remove_dir_all(&work);
}
