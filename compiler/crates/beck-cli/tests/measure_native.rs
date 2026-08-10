//! What the native backend costs, and what it saves.
//!
//! Run with `cargo test --release --test measure_native -- --nocapture`.
//!
//! [`docs/08-roadmap.md`](../../../../docs/08-roadmap.md) §8.4 held every comparative number back
//! until there was a second backend: "the interpreter-vs-Cranelift-vs-LLVM differential and the
//! first honest compute number arrive together, and not before". This is that file. Nothing here
//! compares Beck to another language — the harnesses in `measure_awfy.rs` and `measure_clbg.rs`
//! are where that belongs, and they measure the tree-walker.
//!
//! # The two things measured, and why both
//!
//! A call through this backend is a pipe round trip plus a computation, and reporting one number
//! would hide which is which. So each benchmark is run at **two sizes** — `AGENTS.md`'s rule, and
//! the only way to tell a constant from a slope:
//!
//! * **the ratio at each size**, evaluator over native, which is the speedup;
//! * **the round trip**, measured on its own by calling a definition that does nothing.
//!
//! A ratio that grows with size is the round trip being amortised away, and it is reported rather
//! than smoothed: at a small size the round trip is most of the cost, which is a real property of
//! an out-of-process backend and the first thing §93.7 would change.
//!
//! # What is gated, and what is only printed
//!
//! Nothing here asserts a rate. [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7: a
//! timing gate on a shared runner cannot be held honestly. What *is* asserted is a **shape** —
//! that the ratio does not fall as the problem grows — because that is the claim a compiling
//! backend makes and it is false for any implementation whose per-unit overhead grows. It is the
//! pattern `scaling.rs` and `docs/64` use, for their reason.

use std::sync::Arc;
use std::time::{Duration, Instant};

use beck_core::{Program, Value};
use beck_llvm::Artifact;

fn require_llvm() -> bool {
    std::env::var("BECK_REQUIRE_LLVM").is_ok_and(|v| v == "1")
}

fn compile(name: &str, src: &str) -> Arc<Program> {
    let (placed, diags, map) = beck_core::compile_or_library_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    Arc::new(placed.expect("slices").program)
}

/// The benchmark programs, read from the file every other language's port is a port of.
///
/// `xlang/bench.beck` rather than a string here, because `measure_xlang.rs` times the same four
/// definitions against C, Rust, Node, Python and Ruby: two harnesses over one program, so a change
/// to the benchmark cannot make them disagree about what was measured.
const SRC: &str = include_str!("../../../xlang/bench.beck");

/// The median of `runs` timings of `f`, because one wall-clock reading of anything is mostly noise.
fn median(runs: usize, mut f: impl FnMut()) -> Duration {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed()
        })
        .collect();
    times.sort();
    times[times.len() / 2]
}

struct Bench {
    name: &'static str,
    /// The two sizes, and the argument tuple each produces.
    sizes: [i64; 2],
    args: fn(i64) -> Vec<Value>,
    /// How many times to repeat at each size. Small problems need more repetitions to be readable.
    runs: [usize; 2],
}

fn benches() -> Vec<Bench> {
    vec![
        Bench {
            name: "fib",
            sizes: [24, 30],
            args: |n| vec![Value::Int(n)],
            runs: [7, 3],
        },
        Bench {
            name: "sum_to",
            sizes: [100_000, 1_000_000],
            args: |n| vec![Value::Int(n), Value::Int(0)],
            runs: [7, 3],
        },
        Bench {
            name: "image",
            sizes: [24, 96],
            args: |n| vec![Value::Int(n)],
            runs: [7, 3],
        },
        Bench {
            name: "xor_sweep",
            sizes: [2_000, 20_000],
            args: |n| vec![Value::Int(n), Value::Int(0)],
            runs: [7, 3],
        },
    ]
}

#[test]
fn what_native_code_costs_against_the_tree_walker() {
    let Some(artifact) =
        Artifact::build_within(&compile("bench.beck", SRC), Duration::from_secs(300))
            .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let program = compile("bench.beck", SRC);
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);

    // The round trip on its own: a definition that computes nothing, so what is left is the write,
    // the read, and the two context switches between them.
    let trip = median(2001, || {
        artifact.call("nothing", &[Value::Int(1)]).expect("answers");
    });
    println!("a call that computes nothing: {trip:?} — this is the pipe, on every call below\n");

    println!(
        "{:<12} {:>10} {:>14} {:>14} {:>9}",
        "benchmark", "size", "evaluator", "native", "ratio"
    );
    let mut ratios: Vec<(&str, f64, f64)> = Vec::new();
    for bench in benches() {
        let mut seen = [0.0f64; 2];
        for (i, size) in bench.sizes.iter().enumerate() {
            let args = (bench.args)(*size);
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[bench.name].body)
                    .expect("prepares");
                median(bench.runs[i], || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(bench.runs[i], || {
                artifact
                    .call(bench.name, &args)
                    .expect("the native backend answers");
            });
            let ratio = walked.as_secs_f64() / compiled.as_secs_f64();
            seen[i] = ratio;
            println!(
                "{:<12} {:>10} {:>14} {:>14} {:>8.1}×",
                if i == 0 { bench.name } else { "" },
                size,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratio
            );
        }
        ratios.push((bench.name, seen[0], seen[1]));
    }

    // The shape, and the only thing asserted. A backend whose per-unit overhead grows with the
    // problem would show a ratio that *falls* as the size rises; a compiling one shows a ratio that
    // holds or rises, because its fixed cost — the round trip — is amortised. The bound is
    // deliberately loose for `scaling.rs`'s reason: a gate that flakes gets deleted.
    for (name, small, large) in &ratios {
        assert!(
            large > &(small * 0.5),
            "`{name}` was {small:.1}× faster at the small size and only {large:.1}× at the large \
             one — the advantage is shrinking as the problem grows, which is the shape a compiling \
             backend must not have"
        );
    }
    println!(
        "\nRatios are evaluator ÷ native, wall clock, on this machine. They include the round \
         trip,\nso the small size understates the compiled code and the large one is closer to it."
    );
}

/// What compiling costs, which is the other half of a dual-backend argument.
///
/// §5.2 buys Cranelift for `beck dev` because LLVM's codegen step is slow, and that claim is about
/// a number nobody here has measured. This measures ours: emitting the module, and handing it to
/// `clang -O2`. [`the_two_code_generators_against_each_other`] is the comparison.
#[test]
fn what_compiling_costs() {
    let Some(toolchain) = beck_llvm::Toolchain::find() else {
        assert!(!require_llvm(), "BECK_REQUIRE_LLVM=1 and no `clang`");
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let program = compile("bench.beck", SRC);

    let emit = median(21, || {
        let m = beck_llvm::module(&program);
        assert!(!m.functions.is_empty());
    });
    let module = beck_llvm::module(&program);
    let whole = median(5, || {
        beck_llvm::Artifact::build_bounded(&program, toolchain.clone(), None, None)
            .expect("builds");
    });
    println!(
        "{} definitions, {} lines of IR\n  emitting the module: {emit:?}\n  \
         and assembling it with `clang -O2`, linking, and starting the worker: {whole:?}",
        module.functions.len(),
        module.ir.lines().count()
    );
}

/// §7.3's reason for having two code generators, measured.
///
/// > **Cranelift** … ~40% faster whole-compile and ~10× faster codegen step than LLVM; makes
/// > `beck dev` hot reload feel instant.
///
/// What is compared is **program to executable** on both paths, because that is what a developer
/// waits for: Cranelift emits an object and a linker turns it into a program; LLVM writes text and
/// `clang -O2` does the rest. Neither number is asserted — `docs/13` §13.7 — and both are printed
/// with the size of the program they are about, because a ratio without one is not a measurement.
///
/// It is in a release-only file for a reason this project has been caught by before: Cranelift is
/// a **crate**, so a debug build of this workspace measures our unoptimised build of it against a
/// distribution's optimised `clang`, and that comparison runs the *other* way. `cranelift.rs` says
/// so and asserts nothing about time.
#[test]
fn the_two_code_generators_against_each_other() {
    let Some(linker) = beck_clif::Linker::find() else {
        assert!(!require_llvm(), "BECK_REQUIRE_LLVM=1 and no linker");
        println!("skipped: no linker. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let Some(toolchain) = beck_llvm::Toolchain::find() else {
        assert!(!require_llvm(), "BECK_REQUIRE_LLVM=1 and no `clang`");
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };

    println!(
        "\nProgram to executable, by code generator\n{:>12}  {:>14} {:>14} {:>8}",
        "definitions", "cranelift", "llvm + clang", "×"
    );
    // Two sizes, per `AGENTS.md`: one number cannot tell a fixed cost from a per-definition one.
    for count in [50usize, 400] {
        let mut src = String::new();
        for i in 0..count {
            src.push_str(&format!(
                "def f{i}(a: Int, b: Int) -> Int:\n    return (a * b) + (a - b) + {i}\n\n"
            ));
        }
        let program = compile("wide.beck", &src);
        let clif = median(5, || {
            beck_clif::Artifact::build_bounded(&program, linker.clone(), None, None)
                .expect("cranelift builds");
        });
        let llvm = median(5, || {
            beck_llvm::Artifact::build_bounded(&program, toolchain.clone(), None, None)
                .expect("llvm builds");
        });
        println!(
            "{count:>12}  {:>13.1?} {:>13.1?} {:>8.1}",
            clif,
            llvm,
            llvm.as_secs_f64() / clif.as_secs_f64().max(f64::MIN_POSITIVE)
        );
    }
    println!(
        "  Both numbers include starting the worker process, because that is what a caller waits\n  \
         for. Neither is asserted (docs/13 §13.7)."
    );
}
