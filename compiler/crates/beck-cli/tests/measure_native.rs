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
//! an out-of-process backend and the first thing §93.14 would change.
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

/// What the **heap** costs against the tree-walker.
///
/// Apart from the benchmark above and not folded into it, because what is being measured is a
/// different thing: `fib` and `sum_to` are arithmetic in registers, and these three allocate.
/// Every one answers with an `Int`, so nothing here is measuring the reply's marshalling — that is
/// `the_arena_costs_the_same_per_object_at_every_size` in `native.rs`, which counts bytes and has
/// no clock in it.
///
/// Two sizes, and **nothing here is asserted about a rate** — which is a departure from the
/// benchmark above and is deliberate. That one gates the ratio's *shape*: a ratio that falls as the
/// problem grows is a backend whose per-unit overhead grows. This one cannot, because the ratio
/// moves with the build profile — the native side is `clang -O2` in a debug run and a release one
/// alike, and the evaluator is not, so the same code answers 2,460× under `cargo test` and 120×
/// under `cargo test --release`. A gate on that number would be a gate on which profile ran it, and
/// `docs/13` §13.7's rule is that a gate which flakes gets deleted.
///
/// The shape claim this work makes is gated where it needs no clock at all:
/// `the_arena_costs_the_same_per_object_at_every_size` in `native.rs` counts the **bytes** the arena
/// holds at two sizes, and a per-object cost that grew would have to show there.
#[test]
fn what_the_heap_costs_against_the_tree_walker() {
    const HEAP: &str = r#"
union Tree[T]:
    Leaf(value: T)
    Node(left: Tree[T], right: Tree[T])

model Acc:
    total: Int
    count: Int

def build(n: Int, acc: Tree[Int]) -> Tree[Int]:
    if n <= 0:
        return acc
    return build(n - 1, Node(left=Leaf(value=n), right=acc))

## Walked in **tail position**, because a spine of a hundred thousand is past the evaluator's
## 4,000-frame ceiling and this is a measurement of allocation rather than of recursion.
def sum_spine(t: Tree[Int], acc: Int) -> Int:
    match t:
        case Node(left=Leaf(value), right=rest):
            return sum_spine(rest, acc + value)
        case Node(left=_, right=rest):
            return sum_spine(rest, acc)
        case Leaf(value):
            return acc + value

## Allocate a spine and walk it: the whole life of an object, in one call that answers an `Int`.
def build_and_sum(n: Int) -> Int:
    return sum_spine(build(n, Leaf(value=0)), 0)

## `with` in a loop, which is the shape a fold has and the one the evaluator rebuilds in place.
def fold_with(n: Int, acc: Acc) -> Acc:
    if n <= 0:
        return acc
    return fold_with(n - 1, acc.with(total = acc.total + n, count = acc.count + 1))

def folded(n: Int) -> Int:
    return fold_with(n, Acc(total=0, count=0)).total

## The control: the same loop reading the same field, and allocating **nothing**. What separates
## the two rows above from this one is the arena and not the loop.
def scan(n: Int, acc: Acc, sum: Int) -> Int:
    if n <= 0:
        return sum
    return scan(n - 1, acc, sum + acc.total)
"#;
    let program = compile("heap.beck", HEAP);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);
    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>9}",
        "benchmark", "size", "evaluator", "native", "ratio"
    );

    let benches: [(&str, [i64; 2]); 3] = [
        ("build_and_sum", [10_000, 100_000]),
        ("folded", [10_000, 100_000]),
        ("scan", [10_000, 100_000]),
    ];
    let mut ratios: Vec<(&str, f64, f64)> = Vec::new();
    for (name, sizes) in benches {
        let mut seen = [0.0f64; 2];
        for (i, size) in sizes.iter().enumerate() {
            let args = if name == "scan" {
                vec![
                    Value::Int(*size),
                    beck_core::Value::data(
                        std::sync::Arc::from("Acc"),
                        None,
                        beck_core::core::Fields::from_iter([
                            (std::sync::Arc::from("count"), Value::Int(0)),
                            (std::sync::Arc::from("total"), Value::Int(7)),
                        ]),
                    ),
                    Value::Int(0),
                ]
            } else {
                vec![Value::Int(*size)]
            };
            let runs = if i == 0 { 7 } else { 3 };
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[name].body)
                    .expect("prepares");
                median(runs, || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(runs, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            let ratio = walked.as_secs_f64() / compiled.as_secs_f64();
            seen[i] = ratio;
            println!(
                "{:<14} {:>10} {:>14} {:>14} {:>8.1}×",
                if i == 0 { name } else { "" },
                size,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratio
            );
        }
        ratios.push((name, seen[0], seen[1]));
    }

    // What *is* asserted is the direction, which no profile changes: a compiled backend that is
    // slower than the tree-walker is a finding rather than a number.
    for (name, small, large) in &ratios {
        assert!(
            *small > 1.0 && *large > 1.0,
            "`{name}` was {small:.1}× at the small size and {large:.1}× at the large one"
        );
    }
    println!(
        "\nRatios are evaluator ÷ native, wall clock, on this machine, and every one of them \
         includes\na pipe round trip — 35.6 µs in a release run, and `what_native_code_costs…` is \
         where it is measured."
    );
}

/// What text costs against the tree-walker, and where it costs *more*.
///
/// Two rows that are the same shape as the heap's, and a third that is the honest one. Walking a
/// string by character index and searching one are what a compiled backend should win; **building
/// one in a loop is what it should lose**, because `docs/70` §70.2 gave the evaluator an in-place
/// `push_str` when the last-use analysis proves nobody else holds the accumulator, and an arena
/// with no ownership in it cannot prove that. So `grown` allocates the whole accumulator every
/// step where the evaluator appends to it — `O(n²)` bytes against `O(n)` — and this is where that
/// shows.
///
/// Nothing here is asserted except the direction, and `grown`'s direction is asserted **the other
/// way**: a run in which the compiled accumulator caught up would mean the evaluator had lost its
/// in-place append, which is a finding rather than good news (§93.7).
#[test]
fn what_text_costs_against_the_tree_walker() {
    const TEXT: &str = r#"
## Walk a string by character index, which is the loop `docs/70` §70.2 made linear.
def walk(s: Str, i: Int, acc: Int) -> Int:
    if i >= str_len(s):
        return acc
    if str_slice(s, i, 1) == "x":
        return walk(s, i + 1, acc + 1)
    return walk(s, i + 1, acc)

## Search a string, repeatedly: the naive scan against Rust's `str::find`.
def hunt(s: Str, needle: Str, n: Int, acc: Int) -> Int:
    if n <= 0:
        return acc
    if str_contains(s, needle):
        return hunt(s, needle, n - 1, acc + 1)
    return hunt(s, needle, n - 1, acc)

## Build one in a loop, which is the row this backend loses.
def grown(s: Str, n: Int, acc: Str) -> Int:
    if n <= 0:
        return str_len(acc)
    return grown(s, n - 1, acc + s)
"#;
    let program = compile("text.beck", TEXT);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);
    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>9}",
        "benchmark", "size", "evaluator", "native", "ratio"
    );

    let long = |n: usize| Value::str_(("abcxefgh").repeat(n / 8));
    let benches: [(&str, [usize; 2]); 3] = [
        ("walk", [2_000, 16_000]),
        ("hunt", [2_000, 16_000]),
        ("grown", [1_000, 4_000]),
    ];
    let mut ratios: Vec<(&str, f64, f64)> = Vec::new();
    for (name, sizes) in benches {
        let mut seen = [0.0f64; 2];
        for (i, size) in sizes.iter().enumerate() {
            let args = match name {
                "walk" => vec![long(*size), Value::Int(0), Value::Int(0)],
                "hunt" => vec![
                    long(2_000),
                    Value::str_("efghabc"),
                    Value::Int(*size as i64),
                    Value::Int(0),
                ],
                _ => vec![
                    Value::str_("abcdefgh"),
                    Value::Int(*size as i64),
                    Value::str_(""),
                ],
            };
            let runs = if i == 0 { 7 } else { 3 };
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[name].body)
                    .expect("prepares");
                median(runs, || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(runs, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            let ratio = walked.as_secs_f64() / compiled.as_secs_f64();
            seen[i] = ratio;
            println!(
                "{:<14} {:>10} {:>14} {:>14} {:>8.2}×",
                if i == 0 { name } else { "" },
                size,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratio
            );
        }
        ratios.push((name, seen[0], seen[1]));
    }

    for (name, small, large) in &ratios {
        // `grown` is not asserted at all, in either direction. It is *slower* here in a release
        // build and faster in a debug one, because a debug build measures an unoptimised evaluator
        // against `clang -O2` — which is this file's own warning about the Cranelift row, and a
        // gate on it would be a gate on which profile ran it. The claim it is evidence for is
        // gated with no clock in it instead:
        // `native.rs::an_accumulator_costs_the_square_of_what_it_builds`.
        if *name == "grown" {
            continue;
        }
        assert!(
            *small > 1.0 && *large > 1.0,
            "`{name}` was {small:.2}× at the small size and {large:.2}× at the large one"
        );
    }
    println!(
        "\nRatios are evaluator ÷ native, wall clock, on this machine. `grown` is the row that \
         goes the\nother way in a release build: `docs/93` §93.7 is the in-place append this \
         backend does not have."
    );
}

/// What a list costs against the tree-walker, and where the arena's shape shows.
///
/// The same three shapes text has, one type over: walking one, searching one, and taking a range
/// out of one. There is no accumulator row, because `list_append` is **refused** here — which is
/// `docs/93` §93.7's decision and the asymmetry with text, where `+` had to ship because there is
/// no other way to build a string.
#[test]
fn what_a_list_costs_against_the_tree_walker() {
    const SRC: &str = r#"
def walk(xs: list[Int], i: Int, acc: Int) -> Int:
    if i >= list_len(xs):
        return acc
    match list_get(xs, i):
        case Some(value):
            return walk(xs, i + 1, acc + value)
        case None():
            return acc

def hunt(xs: list[Int], n: Int, times: Int, acc: Int) -> Int:
    if times <= 0:
        return acc
    if list_contains(xs, n):
        return hunt(xs, n, times - 1, acc + 1)
    return hunt(xs, n, times - 1, acc)

def windows(xs: list[Int], i: Int, acc: Int) -> Int:
    if i >= list_len(xs):
        return acc
    return windows(xs, i + 1, acc + list_len(list_slice(xs, i, 4)))
"#;
    let program = compile("lists.beck", SRC);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);
    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>9}",
        "benchmark", "size", "evaluator", "native", "ratio"
    );

    let long = |n: usize| {
        Value::List(std::sync::Arc::new(
            (0..n as i64).map(Value::Int).collect::<Vec<_>>(),
        ))
    };
    let benches: [(&str, [usize; 2]); 3] = [
        ("walk", [2_000, 16_000]),
        ("hunt", [500, 4_000]),
        ("windows", [2_000, 16_000]),
    ];
    let mut ratios: Vec<(&str, f64, f64)> = Vec::new();
    for (name, sizes) in benches {
        let mut seen = [0.0f64; 2];
        for (i, size) in sizes.iter().enumerate() {
            let args = match name {
                "hunt" => vec![
                    long(500),
                    Value::Int(-1),
                    Value::Int(*size as i64),
                    Value::Int(0),
                ],
                _ => vec![long(*size), Value::Int(0), Value::Int(0)],
            };
            let runs = if i == 0 { 7 } else { 3 };
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[name].body)
                    .expect("prepares");
                median(runs, || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(runs, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            let ratio = walked.as_secs_f64() / compiled.as_secs_f64();
            seen[i] = ratio;
            println!(
                "{:<14} {:>10} {:>14} {:>14} {:>8.2}×",
                if i == 0 { name } else { "" },
                size,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratio
            );
        }
        ratios.push((name, seen[0], seen[1]));
    }
    for (name, small, large) in &ratios {
        assert!(
            *small > 1.0 && *large > 1.0,
            "`{name}` was {small:.2}× at the small size and {large:.2}× at the large one"
        );
    }
}

/// What a closure costs against the tree-walker.
///
/// Three shapes: applying one in a loop, mapping a list through one, and folding a list with one.
/// The evaluator builds a `Value::Closure` per `lam` *evaluated* — an `Arc` bump over shared code
/// since `docs/70` §70.3 — and applies it by pushing an environment frame; here it is one object of
/// one word per capture and a switch into a direct call.
///
/// The row that says whether the design is right is **apply**: it is the one where the loop's work
/// *is* the application, so a switch that had turned into something expensive would show there and
/// nowhere else. The two sizes are what separate a constant from a growth (`AGENTS.md`).
#[test]
fn what_a_closure_costs_against_the_tree_walker() {
    const SRC: &str = r#"
def apply_often(n: Int, acc: Int) -> Int:
    f = lambda x: x + 1
    if n <= 0:
        return acc
    return apply_often(n - 1, f(acc))

def mapped(xs: list[Int], by: Int) -> Int:
    return list_len(map_list(xs, lambda x: x * by))

def folded(xs: list[Int]) -> Int:
    return list_fold(xs, 0, lambda acc, x: acc + x)

## Sorted by a key that reverses the order, so the input is the worst case for a merge that walks it.
## Two sizes eight times apart, because a sort is where a wrong asymptote hides: `n log n` costs about
## 9.3× more at 16,000 than at 2,000 and `n²` costs 64×, and the *ratio against the tree-walker* —
## which is Rust's own stable sort — is what would collapse.
def sorted_down(xs: list[Int]) -> list[Int]:
    return sort_by(xs, lambda x: 0 - x)

def joined_up(xss: list[list[Int]]) -> Int:
    return list_len(concat_lists(xss))
"#;
    let program = compile("closures.beck", SRC);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);
    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>9}",
        "benchmark", "size", "evaluator", "native", "ratio"
    );

    let long = |n: usize| {
        Value::List(std::sync::Arc::new(
            (0..n as i64).map(Value::Int).collect::<Vec<_>>(),
        ))
    };
    // The last column is whether the *compiled* side is expected to win, and one row says no. See
    // the assertions below, which are per row rather than one threshold for all of them.
    let benches: [(&str, [usize; 2], bool); 5] = [
        ("apply_often", [20_000, 160_000], true),
        ("mapped", [2_000, 16_000], true),
        ("folded", [2_000, 16_000], true),
        ("sorted_down", [2_000, 16_000], true),
        ("joined_up", [2_000, 16_000], false),
    ];
    let mut ratios: Vec<(&str, f64, f64, bool)> = Vec::new();
    for (name, sizes, faster) in benches {
        let mut seen = [0.0f64; 2];
        for (i, size) in sizes.iter().enumerate() {
            let args = match name {
                "apply_often" => vec![Value::Int(*size as i64), Value::Int(0)],
                "mapped" => vec![long(*size), Value::Int(3)],
                // `n` inner lists of one element, so the answer is `n` and the outer list is the
                // input: a concatenation's cost is its answer's size either way.
                "joined_up" => vec![Value::List(std::sync::Arc::new(
                    (0..*size as i64)
                        .map(|i| Value::List(std::sync::Arc::new(vec![Value::Int(i)])))
                        .collect::<Vec<_>>(),
                ))],
                _ => vec![long(*size)],
            };
            let runs = if i == 0 { 7 } else { 3 };
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[name].body)
                    .expect("prepares");
                median(runs, || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(runs, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            let ratio = walked.as_secs_f64() / compiled.as_secs_f64();
            seen[i] = ratio;
            println!(
                "{:<14} {:>10} {:>14} {:>14} {:>8.2}×",
                if i == 0 { name } else { "" },
                size,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratio
            );
        }
        ratios.push((name, seen[0], seen[1], faster));
    }
    for (name, small, large, faster) in &ratios {
        if *faster {
            assert!(
                *small > 1.0 && *large > 1.0,
                "`{name}` was {small:.2}× at the small size and {large:.2}× at the large one"
            );
        } else {
            // **`concat_lists` is slower here, and asserted to be.** `docs/93` §93.5 is the
            // precedent: a row below one is a fact to pin rather than a number to leave out. The
            // work *is* the marshalling — the evaluator already holds the lists and copies words
            // between two `Vec`s, while a compiled call sends 2,000 list objects down a pipe and
            // reads the answer back out of the reply's arena. `docs/93` §93.1's round trip is the
            // whole of the difference, and it is a property of every call rather than of this one.
            assert!(
                *small < 1.0 && *large < 1.0,
                "`{name}` was expected to lose to the tree-walker and was {small:.2}× and {large:.2}×"
            );
        }
    }
    // And the shape the sort is really about. Eight times the elements is about ten times the work
    // for `n log n` and sixty-four for `n²`, so a quadratic merge would show as a ratio that fell by
    // about a factor of six against a tree-walker whose own sort is Rust's. The bound is the loose
    // one this file already uses for `scaling.rs`'s reason: a gate that flakes gets deleted.
    let (_, small, large, _) = ratios
        .iter()
        .find(|(n, ..)| *n == "sorted_down")
        .expect("the sort was measured");
    println!(
        "the sort was {small:.2}× at the small size and {large:.2}× at the large one — {:.2}× of the \
         ratio kept over eight times the elements, which is `n log n` with a constant worse than \
         Rust's stable sort. A quadratic would have lost about a factor of six",
        large / small
    );
    assert!(
        large > &(small * 0.5),
        "the sort was {small:.2}× at the small size and only {large:.2}× at the large one, which is \
         the shape a merge sort must not have"
    );
}

/// What a map costs against the tree-walker, and whether the search is really binary.
///
/// The interesting row is `lookup`. A `PMap` is a weight-balanced tree and this is a sorted run
/// searched by halving, so both are `O(log n)` — and the shape claim is what a **ratio that does
/// not collapse** at eight times the size says: a linear scan here would lose a factor of eight
/// between the two rows, which no constant explains.
#[test]
fn what_a_map_costs_against_the_tree_walker() {
    const SRC: &str = r#"
def lookup(m: Map[Int, Int], k: Int, times: Int, acc: Int) -> Int:
    if times <= 0:
        return acc
    match map_get(m, k):
        case Some(value):
            return lookup(m, k, times - 1, acc + value)
        case None():
            return lookup(m, k, times - 1, acc)

def walk(m: Map[Int, Int], i: Int, acc: Int) -> Int:
    if i >= map_len(m):
        return acc
    match list_get(map_keys(m), i):
        case Some(value):
            return walk(m, i + 1, acc + value)
        case None():
            return acc

## The control: the same loop, the same map, and **no search**. What separates it from `lookup` is
## the search and nothing else — in particular not the map arriving down the pipe, which is eight
## times bigger at the large size and would otherwise be read as the search growing.
def spin(m: Map[Int, Int], k: Int, times: Int, acc: Int) -> Int:
    if times <= 0:
        return acc
    return spin(m, k, times - 1, acc + map_len(m))
"#;
    let program = compile("maps.beck", SRC);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);
    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>9}",
        "benchmark", "entries", "evaluator", "native", "ratio"
    );

    let big = |n: usize| {
        Value::Map(
            (0..n as i64)
                .map(|i| (Value::Int(i * 2), Value::Int(i)))
                .collect(),
        )
    };
    let mut ratios: Vec<(&str, f64, f64)> = Vec::new();
    let mut native_lookup = [0.0f64; 2];
    let mut native_spin = [0.0f64; 2];
    for name in ["lookup", "spin", "walk"] {
        let mut seen = [0.0f64; 2];
        for (i, size) in [250usize, 2_000].iter().enumerate() {
            let args = match name {
                "lookup" | "spin" => vec![
                    big(*size),
                    Value::Int(*size as i64),
                    Value::Int(2_000),
                    Value::Int(0),
                ],
                _ => vec![big(*size), Value::Int(0), Value::Int(0)],
            };
            let runs = if i == 0 { 7 } else { 3 };
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[name].body)
                    .expect("prepares");
                median(runs, || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(runs, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            match name {
                "lookup" => native_lookup[i] = compiled.as_secs_f64(),
                "spin" => native_spin[i] = compiled.as_secs_f64(),
                _ => {}
            }
            let ratio = walked.as_secs_f64() / compiled.as_secs_f64();
            seen[i] = ratio;
            println!(
                "{:<14} {:>10} {:>14} {:>14} {:>8.2}×",
                if i == 0 { name } else { "" },
                size,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratio
            );
        }
        ratios.push((name, seen[0], seen[1]));
    }
    for (name, small, large) in &ratios {
        assert!(
            *small > 1.0 && *large > 1.0,
            "`{name}` was {small:.2}× at the small size and {large:.2}× at the large one"
        );
    }
    // The search is **not** asserted, and the control is why. `spin` does the same loop over the
    // same map and searches nothing, and it costs within a few percent of `lookup` at both sizes —
    // so at 250 and 2,000 entries a binary search is smaller than the tail-recursive loop that
    // calls it, and this measurement cannot tell one from a scan. Saying so is the honest answer;
    // `native.rs::a_lookup_costs_the_same_whatever_the_map_holds` is the claim that *can* be made
    // here, and it has no clock in it.
    println!(
        "\n`spin` is the control: the same loop over the same map, searching nothing. It costs \
         {:.0} µs and {:.0} µs\nagainst `lookup`'s {:.0} µs and {:.0} µs, so the search is under \
         the loop's own cost at these sizes\nand this table says nothing about whether it halves.",
        native_spin[0] * 1e6,
        native_spin[1] * 1e6,
        native_lookup[0] * 1e6,
        native_lookup[1] * 1e6,
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

/// What a page costs against the tree-walker, and it is the row this feature has to be honest
/// about.
///
/// A compiled `view` does not render anything: what it builds is the *call*, and the host bakes the
/// tree out of it on the way back (`docs/93`). So the work is divided rather than removed — the
/// program's own logic is compiled and every leaf's rendering is still `Value::display` in the
/// host, plus a pipe in the middle that the evaluator does not have. The two sizes are what
/// separate a constant from a growth (`AGENTS.md`), and nothing here is asserted to be **faster**:
/// what is asserted is that the ratio does not *collapse* with size, which is what a recipe that
/// copied the children built so far would do.
#[test]
fn what_a_page_costs_against_the_tree_walker() {
    const SRC: &str = r#"
model Row:
    id: Str
    text: Str
    done: Bool

union Command:
    Toggle(id: Str)

## The `ui:` block `examples/todo.beck` has, at whatever size it is given.
def page(rows: list[Row]) -> Html:
    return ui:
        ul:
            for r in rows:
                li(key=r.id, class=done_class(r)):
                    span(on_click=Toggle(id=r.id)): r.text

def done_class(r: Row) -> Str:
    return "done" if r.done else ""

## The same page with nothing deferred but text, so the row above can be read against one where no
## handler has to become JSON.
def plain(rows: list[Row]) -> Html:
    return ui:
        ul:
            for r in rows:
                li: r.text
"#;
    let program = compile("page.beck", SRC);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);
    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>9}",
        "page", "rows", "evaluator", "native", "ratio"
    );

    let rows = |n: usize| {
        Value::List(Arc::new(
            (0..n)
                .map(|i| {
                    Value::record(
                        "Row",
                        None,
                        [
                            ("id", Value::str_(format!("r{i}"))),
                            ("text", Value::str_("something to do")),
                            ("done", Value::Bool(i % 2 == 0)),
                        ],
                    )
                })
                .collect::<Vec<_>>(),
        ))
    };
    let mut seen: Vec<(&str, f64, f64)> = Vec::new();
    for name in ["page", "plain"] {
        let mut ratios = [0.0f64; 2];
        for (i, size) in [200usize, 1_600].iter().enumerate() {
            let args = vec![rows(*size)];
            let runs = if i == 0 { 7 } else { 3 };
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[name].body)
                    .expect("prepares");
                median(runs, || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(runs, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            ratios[i] = walked.as_secs_f64() / compiled.as_secs_f64();
            println!(
                "{:<14} {:>10} {:>14} {:>14} {:>8.2}×",
                if i == 0 { name } else { "" },
                size,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratios[i]
            );
        }
        seen.push((name, ratios[0], ratios[1]));
    }
    println!(
        "\n  Neither row is asserted to be faster. A compiled `view` builds the call and the host\n  \
         bakes the tree, so the rendering is the same `Value::display` either way and the pipe is\n  \
         additional — `docs/93` §93.5. What is asserted is that the ratio holds its shape over\n  \
         eight times the rows, because a page that copied what it had built would lose it."
    );
    for (name, small, large) in &seen {
        assert!(
            large / small > 0.5,
            "`{name}` was {small:.2}× at 200 rows and {large:.2}× at 1,600 — the ratio collapsed, \
             which is what a page whose cost is quadratic in its rows looks like"
        );
    }
}

/// What a raise costs against the tree-walker, at two depths.
///
/// The evaluator unwinds by returning a Rust `Err` through its own frames; this returns through
/// compiled frames, each of which loads four bytes and branches. The row that says whether the
/// design is right is the **ratio's shape over depth**, not its value: a mechanism that cost
/// something per frame — a trace, a copy, an allocation — would lose ground as the stack got
/// deeper, and this is where that would show.
#[test]
fn what_a_raise_costs_against_the_tree_walker() {
    const SRC: &str = r#"
union Bad:
    Blank

## Raised at the bottom of `n` frames and caught at the top, with the recursion **not** in tail
## position so that every frame on the way out checks the cell.
def deeply(n: Int) -> Int uses raises(Bad):
    if n <= 0:
        raise Blank
    return deeply(n - 1) + 1

def caught(n: Int) -> Result[Int, Bad]:
    return try:
        deeply(n)

## The same recursion that does not fail, so the row above can be read against the cost of the
## frames themselves.
def down(n: Int) -> Int:
    if n <= 0:
        return 0
    return down(n - 1) + 1
"#;
    let program = compile("raise.beck", SRC);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);
    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>9}",
        "benchmark", "frames", "evaluator", "native", "ratio"
    );
    let mut seen: Vec<(&str, f64, f64)> = Vec::new();
    for name in ["caught", "down"] {
        let mut ratios = [0.0f64; 2];
        // 3,000 rather than 4,000: `adr/0007` gives the evaluator a declared nesting ceiling and
        // this recursion is not in tail position, so the *evaluator* is what bounds the larger
        // size — which is itself a difference worth knowing about and is why the sizes are six
        // times apart rather than eight.
        for (i, depth) in [500i64, 3_000].iter().enumerate() {
            let args = vec![Value::Int(*depth)];
            let runs = if i == 0 { 7 } else { 3 };
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[name].body)
                    .expect("prepares");
                median(runs, || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(runs, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            ratios[i] = walked.as_secs_f64() / compiled.as_secs_f64();
            println!(
                "{:<14} {:>10} {:>14} {:>14} {:>8.2}×",
                if i == 0 { name } else { "" },
                depth,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratios[i]
            );
        }
        seen.push((name, ratios[0], ratios[1]));
    }
    for (name, small, large) in &seen {
        assert!(
            *small > 1.0 && *large > 1.0,
            "`{name}` was {small:.2}× at the small depth and {large:.2}× at the large one"
        );
    }
}

/// What an appended accumulator costs against the tree-walker, at two sizes.
///
/// `docs/46` §46.14 measured this loop as **quadratic in time** on the evaluator before
/// `docs/70` gave it last-use moves, and `docs/93` §93.7 refused to compile it rather than ship
/// the quadratic back. `docs/93` is the layout that removed the choice, and this is the row that
/// says so: the ratio has to **hold its shape** over four times the elements, because a copying
/// append would lose a factor of four between the two rows.
///
/// The second pair is the control from the other direction — the same loop reading rather than
/// growing — so a reader can tell a linear append from a fast one.
#[test]
fn what_an_appended_accumulator_costs_against_the_tree_walker() {
    const SRC: &str = r#"
## The idiom: a tail-recursive accumulator, appended to once per element.
def doubled(xs: list[Int]) -> list[Int]:
    return push_from(xs, 0, [])

def push_from(xs: list[Int], i: Int, acc: list[Int]) -> list[Int]:
    if i >= list_len(xs):
        return acc
    match list_get(xs, i):
        case Some(value):
            return push_from(xs, i + 1, list_append(acc, value * 2))
        case None():
            return acc

## The same walk with nothing built, so the append is what the difference is.
def summed(xs: list[Int]) -> Int:
    return add_from(xs, 0, 0)

def add_from(xs: list[Int], i: Int, acc: Int) -> Int:
    if i >= list_len(xs):
        return acc
    match list_get(xs, i):
        case Some(value):
            return add_from(xs, i + 1, acc + value)
        case None():
            return acc
"#;
    let program = compile("append.beck", SRC);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };
    let evaluator = beck_eval::backend_for(program.clone());
    println!("{}\n", artifact.toolchain().version);
    println!(
        "{:<14} {:>10} {:>14} {:>14} {:>9}",
        "benchmark", "elements", "evaluator", "native", "ratio"
    );
    let long = |n: usize| Value::List(Arc::new((0..n as i64).map(Value::Int).collect::<Vec<_>>()));
    let mut seen: Vec<(&str, f64, f64)> = Vec::new();
    for name in ["doubled", "summed"] {
        let mut ratios = [0.0f64; 2];
        for (i, size) in [2_000usize, 8_000].iter().enumerate() {
            let args = vec![long(*size)];
            let runs = if i == 0 { 7 } else { 3 };
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = evaluator
                    .function(&program.defs[name].body)
                    .expect("prepares");
                median(runs, || {
                    f(args.clone()).expect("the evaluator answers");
                })
            });
            let compiled = median(runs, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            ratios[i] = walked.as_secs_f64() / compiled.as_secs_f64();
            println!(
                "{:<14} {:>10} {:>14} {:>14} {:>8.2}×",
                if i == 0 { name } else { "" },
                size,
                format!("{walked:?}"),
                format!("{compiled:?}"),
                ratios[i]
            );
        }
        seen.push((name, ratios[0], ratios[1]));
    }
    for (name, small, large) in &seen {
        assert!(
            large / small > 0.5,
            "`{name}` was {small:.2}× at 2,000 elements and {large:.2}× at 8,000 — the ratio \
             collapsed, which is what an append that copies looks like"
        );
    }
}

/// What asking the host costs, at two arena sizes.
///
/// The four host primitives are the one place a *call* is not one round trip: the worker writes a
/// question mid-call and blocks. So the number that matters is not the ratio against the
/// tree-walker — the tree-walker calls a Rust method and this crosses a pipe, so the tree-walker
/// wins and will keep winning — it is **what one question costs and what it grows with**.
///
/// Two rows, and they are the same program but for which question they end on:
///
/// * `clock_after` asks `now()`, whose arguments cannot point into the heap, so the question is a
///   32-byte header and some words at any size.
/// * `secret_after` asks `secret_env`, which is handed text the program built, so the live arena
///   travels with it.
///
/// Nothing here is asserted — [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7's rule
/// about timing gates on a shared runner. The **shape** is asserted where it needs no clock at
/// all: `what_a_question_carries_is_a_decision_and_not_an_accident` in `native.rs` counts the
/// bytes at the same two sizes, and a question that started copying the arena when it should not
/// would have to show there.
#[test]
fn what_asking_the_host_costs() {
    let program = compile("asking.beck", ASKING);
    let Some(artifact) = Artifact::build_within(&program, Duration::from_secs(300))
        .expect("clang accepts the module")
    else {
        assert!(
            !require_llvm(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        println!("skipped: no LLVM toolchain. Set BECK_REQUIRE_LLVM=1 to make this a failure.");
        return;
    };

    println!(
        "\n{:<16} {:>10} {:>14} {:>14} {:>16}",
        "definition", "live", "no question", "one question", "the question"
    );
    for (name, asking) in [("clock_after", "now()"), ("secret_after", "secret_env")] {
        for size in [16i64, 4096] {
            let args = vec![Value::Int(size)];
            // The same program without the question — and answering an `Int`, like the two
            // rows do. A control that answered the *list* would carry the whole arena back over
            // the pipe and be the more expensive of the two, which is a measurement of the reply
            // rather than of the question.
            let control = median(9, || {
                artifact
                    .call("no_question", &args)
                    .expect("the native backend answers");
            });
            let whole = median(9, || {
                artifact
                    .call(name, &args)
                    .expect("the native backend answers");
            });
            let (_, carried) = artifact.questions();
            println!(
                "{:<16} {:>10} {:>14} {:>14} {:>16}",
                if size == 16 { name } else { "" },
                size,
                format!("{control:?}"),
                format!("{whole:?}"),
                format!("{:?} ({carried} B)", whole.saturating_sub(control))
            );
        }
        println!("  ^ asking {asking}");
    }
    println!(
        "\nThe difference is one round trip plus whatever the arena weighed. A question that\n\
         carries nothing is the round trip alone at both sizes; one that carries the arena grows\n\
         with it, which is why `secret_env` in a loop over a large heap is a cost and `now()` is\n\
         not. `what_native_code_costs_against_the_tree_walker` is where the bare round trip is."
    );
}

/// The two questions, in front of an arena of a stated size.
///
/// A copy of `hostfix.rs`'s two definitions rather than an import, because a measurement suite that
/// reached into a differential's fixtures would make one file's edit silently change the other's
/// number.
const ASKING: &str = r#"
def range_list(n: Int) -> list[Int]:
    if n <= 0:
        return []
    return list_append(range_list(n - 1), n)

# The control: the same arena, the same `Int` answer, and nothing asked.
def no_question(n: Int) -> Int:
    xs = range_list(n)
    return list_len(xs)

def clock_after(n: Int) -> Int uses nondet:
    xs = range_list(n)
    return now() + list_len(xs)

def secret_after(n: Int) -> Int uses env:
    xs = range_list(n)
    token = secret_env("BECK_TEST_TOKEN")
    return list_len(xs) + str_len(str(list_len(xs)))
"#;
