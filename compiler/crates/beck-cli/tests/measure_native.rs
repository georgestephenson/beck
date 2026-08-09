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

/// The benchmark programs.
///
/// Two of the four are lifted out of files somebody else wrote: `escapes` is
/// `awfy/mandelbrot.beck`'s inner loop verbatim and `xor_from` is its hand-written exclusive-or,
/// both of which the Are We Fast Yet port needs and neither of which was written for this. The
/// other two are the classics a compiler is expected to be good at.
const SRC: &str = r#"
## Are We Fast Yet's mandelbrot, escape-time for one point — the file's inner loop, unchanged.
def escapes(zrzr: Float, zi: Float, zizi: Float, cr: Float, ci: Float, z: Int) -> Int:
    if z >= 50:
        return 0
    zr = zrzr - zizi + cr
    next_zi = 2.0 * zr * zi + ci
    next_zrzr = zr * zr
    next_zizi = next_zi * next_zi
    if next_zrzr + next_zizi > 4.0:
        return 1
    return escapes(next_zrzr, next_zi, next_zizi, cr, ci, z + 1)

## …driven over a square of the plane, so one call is a whole image rather than one pixel. The
## driver is scalar because the original's is not: `awfy/mandelbrot.beck` packs its bits into a
## record, and a record does not compile here.
def image(size: Int) -> Int:
    return rows(0, size, 0)

def rows(y: Int, size: Int, acc: Int) -> Int:
    if y >= size:
        return acc
    ci = 2.0 * float(y) / float(size) - 1.0
    return rows(y + 1, size, columns(0, size, ci, acc))

def columns(x: Int, size: Int, ci: Float, acc: Int) -> Int:
    if x >= size:
        return acc
    cr = 2.0 * float(x) / float(size) - 1.5
    return columns(x + 1, size, ci, acc + escapes(0.0, 0.0, 0.0, cr, ci, 0))

## Are We Fast Yet's exclusive-or, written as arithmetic because Beck has no bitwise operators
## (`docs/53` §53.5) — so it is eight recursive steps where another language has one instruction.
def xor_from(a: Int, b: Int, weight: Int, acc: Int) -> Int:
    if a == 0 and b == 0:
        return acc
    return xor_from(a / 2, b / 2, weight * 2, acc + weight * ((a % 2 + b % 2) % 2))

def xor_sweep(n: Int, acc: Int) -> Int:
    if n <= 0:
        return acc
    return xor_sweep(n - 1, xor_from(n % 256, (n * 7) % 256, 1, 0) + acc)

## Tree recursion: no accumulator, no tail call, one frame per node.
def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

## A tail-recursive loop, which is the shape a `while` has in this language.
def sum_to(n: Int, acc: Int) -> Int:
    if n <= 0:
        return acc
    return sum_to(n - 1, acc + n)

## Nothing at all, so a call to it is the round trip and none of the computation.
def nothing(n: Int) -> Int:
    return n
"#;

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
/// `clang -O2`.
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
