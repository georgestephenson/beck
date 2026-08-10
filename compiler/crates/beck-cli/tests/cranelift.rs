//! The Cranelift backend, against the tree-walker **and** against LLVM.
//!
//! [`docs/07-dependencies.md`](../../../../docs/07-dependencies.md) §7.3 chooses two code
//! generators and says what each is for; [`93`](../../../../docs/93-llvm-backend-report.md) built
//! the release one and listed the development one as unbuilt. This is the suite for the second,
//! and it is a **three-way** differential rather than a second two-way one: the same call goes to
//! the evaluator, to LLVM and to Cranelift, and all three have to answer the same thing.
//!
//! # Why three matters more than two
//!
//! [`93`](../../../../docs/93-llvm-backend-report.md) §93.7's finding is that a differential
//! compares what somebody thought to write down, and its worst case is a boundary that normalises
//! on both sides. Two independent emitters make a different kind of mistake available to
//! detection: an agreement between the tree-walker and *both* of them is evidence about the
//! semantics, and a disagreement between the two compilers is evidence about one of them even when
//! the evaluator is not consulted. The first bug this backend had was exactly that shape — a
//! signed comparison of an unsigned order key, which is `-1.0 < 0.0` answering `false` — and it
//! was found by the smallest program in the suite.
//!
//! # The programs are not this file's
//!
//! They are [`support::scalar`]'s, shared with [`native.rs`](native.rs), because a second copy of
//! them would be a second opinion about what the scalar subset is.
//!
//! # Skipping
//!
//! Cranelift is a crate, so this needs no LLVM — but it needs a **linker**, because an object file
//! is not a program. With none, these tests print why they skipped and pass; `BECK_REQUIRE_LLVM=1`
//! forbids the skip, the same switch the other native suite honours, because a machine that has
//! `clang` has a linker.

use std::sync::Arc;
use std::time::Duration;

use beck_clif::Artifact as ClifArtifact;
use beck_core::backend::Backend;
use beck_core::{Program, Value};
use beck_llvm::Artifact as LlvmArtifact;

mod support;
use support::scalar::{
    float_pairs, floats, ints, pairs, render, singles, ARITHMETIC, CONTROL, REALS, RECURSION,
    REFUSED,
};

/// One call may not take longer than this. Nothing here should come close; it is the difference
/// between a red test and a hung suite.
const LIMIT: Duration = Duration::from_secs(30);

fn require_native() -> bool {
    std::env::var("BECK_REQUIRE_LLVM").is_ok_and(|v| v == "1")
}

/// The linker, or a printed skip.
macro_rules! linker {
    () => {
        match beck_clif::Linker::find() {
            Some(l) => l,
            None => {
                assert!(
                    !require_native(),
                    "BECK_REQUIRE_LLVM=1 and there is no linker on the path"
                );
                println!(
                    "skipped: no linker — no `cc`, `clang` or `gcc` on the path, and BECK_LINKER \
                     does not name a working one. Set BECK_REQUIRE_LLVM=1 to make this a failure."
                );
                return;
            }
        }
    };
}

fn compile(name: &str, src: &str) -> Arc<Program> {
    let (placed, diags, map) = beck_core::compile_or_library_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    Arc::new(
        placed
            .unwrap_or_else(|| panic!("{name} did not slice"))
            .program,
    )
}

/// What one backend answered: a value, or the message it failed with.
///
/// The span is deliberately not compared, for `native.rs`'s reason: both carry one and they name
/// the same file, but one is the `Core` node being walked and the other is what an emitter
/// recorded for a trapping instruction.
type Outcome = Result<Value, String>;

fn outcome(r: Result<Value, beck_core::ExecError>) -> Outcome {
    r.map_err(|e| e.message)
}

/// Every backend this machine can offer, over one program.
struct All {
    program: Arc<Program>,
    evaluator: Arc<dyn Backend>,
    clif: ClifArtifact,
    /// `None` on a machine with a linker and no `clang`, which is a real machine: the two-way
    /// differential still runs there and says so.
    llvm: Option<LlvmArtifact>,
}

impl All {
    fn over(name: &str, src: &str) -> All {
        let program = compile(name, src);
        let linker = beck_clif::Linker::find().expect("checked by the caller");
        let clif = ClifArtifact::build_bounded(&program, linker, None, Some(LIMIT))
            .expect("cranelift compiles the module");
        let llvm = beck_llvm::Toolchain::find().map(|t| {
            LlvmArtifact::build_bounded(&program, t, None, Some(LIMIT))
                .expect("clang accepts the module")
        });
        All {
            evaluator: beck_eval::backend_for(program.clone()),
            program,
            clif,
            llvm,
        }
    }

    fn compiled(&self) -> Vec<String> {
        self.clif
            .module()
            .functions
            .iter()
            .map(|f| f.name.to_string())
            .collect()
    }

    fn refusal(&self, name: &str) -> Option<&str> {
        self.clif
            .module()
            .refusals
            .iter()
            .find(|r| &*r.name == name)
            .map(|r| r.reason.as_str())
    }

    /// Assert every backend agrees on every tuple, and answer how many were compared.
    fn agree(&self, name: &str, tuples: &[Vec<Value>]) -> usize {
        assert!(
            self.compiled().iter().any(|n| n == name),
            "`{name}` did not compile through Cranelift, so this compares the evaluator with itself"
        );
        let def = self
            .program
            .defs
            .get(name)
            .unwrap_or_else(|| panic!("no definition `{name}`"));
        for args in tuples {
            // The tree-walker spends host frames on recursion that is not in tail position and
            // says how much stack that needs; every entry point in the workspace honours it.
            let walked = beck_eval::on_the_evaluator_stack(|| {
                let f = self.evaluator.function(&def.body).expect("prepares");
                outcome(f(args.to_vec()))
            });
            let compiled = outcome(self.clif.call(name, args));
            assert_eq!(
                walked,
                compiled,
                "`{name}` disagreed on {}: the evaluator said {walked:?} and Cranelift said \
                 {compiled:?}",
                render(args)
            );
            if let Some(llvm) = &self.llvm {
                let other = outcome(llvm.call(name, args));
                assert_eq!(
                    other,
                    compiled,
                    "`{name}` disagreed on {}: LLVM said {other:?} and Cranelift said {compiled:?}",
                    render(args)
                );
            }
        }
        tuples.len()
    }
}

// -------------------------------------------------------------------------------------------
// The differential
// -------------------------------------------------------------------------------------------

#[test]
fn the_three_backends_agree_on_integer_arithmetic() {
    linker!();
    let all = All::over("arith.beck", ARITHMETIC);
    let xs = ints(0x5EED, 26);
    let two = pairs(&xs);
    let one = singles(&xs);
    let mut n = 0;
    for f in ["plus", "minus", "times", "over", "modulo", "chained"] {
        n += all.agree(f, &two);
    }
    for f in ["compares", "orders", "logic"] {
        n += all.agree(f, &two);
    }
    for f in ["negated", "absolute"] {
        n += all.agree(f, &one);
    }
    println!("{n} calls agreed on integer arithmetic");
}

#[test]
fn the_three_backends_agree_on_reals() {
    linker!();
    let all = All::over("reals.beck", REALS);
    let xs = floats(0xF10A7, 22);
    let two = float_pairs(&xs);
    let one: Vec<Vec<Value>> = xs.iter().map(|x| vec![Value::float(*x)]).collect();
    let mut n = 0;
    for f in [
        "rplus",
        "rminus",
        "rtimes",
        "rover",
        "rless",
        "requal",
        "rorder",
        "reciprocal_of_product",
        "product_is_zero",
        "product_order",
        "zero_through_sqrt",
        "signed_zero",
    ] {
        n += all.agree(f, &two);
    }
    for f in ["rnegated", "rabs", "rsqrt", "rsin", "rcos", "truncated"] {
        n += all.agree(f, &one);
    }
    n += all.agree("widened", &singles(&ints(0xB17, 20)));
    println!("{n} calls agreed on reals");
}

/// The three places a real is normalised, each with a program that makes the difference
/// observable — the same three [`93`](../../../../docs/93-llvm-backend-report.md) §93.2 found the
/// hard way, asserted here against a second emitter that had to make the same three decisions.
#[test]
fn a_negative_zero_and_a_nan_mean_here_what_they_mean_in_the_evaluator() {
    linker!();
    let all = All::over("reals.beck", REALS);
    let inf = f64::INFINITY;
    // `0.0 * inf` is the *indefinite* NaN on x86-64, whose sign bit is set — the one that sorts
    // below every number under the order key where `f64::NAN` sorts above every one.
    all.agree(
        "product_order",
        &[vec![Value::float(0.0), Value::float(inf)]],
    );
    all.agree(
        "product_is_zero",
        &[vec![Value::float(0.0), Value::float(-1.0)]],
    );
    all.agree(
        "reciprocal_of_product",
        &[vec![Value::float(0.0), Value::float(-1.0)]],
    );
    all.agree(
        "zero_through_sqrt",
        &[vec![Value::float(2.0), Value::float(-3.0)]],
    );
    all.agree(
        "signed_zero",
        &[vec![Value::float(0.0), Value::float(-1.0)]],
    );
}

#[test]
fn the_three_backends_agree_on_control_flow() {
    linker!();
    let all = All::over("control.beck", CONTROL);
    let xs = ints(0xC0FFEE, 24);
    let one = singles(&xs);
    let two = pairs(&xs);
    let mut n = 0;
    for f in ["classify", "shadowing", "guard_falls_through"] {
        n += all.agree(f, &one);
    }
    n += all.agree("nested", &two);
    n += all.agree(
        "truthy",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    println!("{n} calls agreed on control flow");
}

#[test]
fn the_three_backends_agree_on_recursion() {
    linker!();
    let all = All::over("rec.beck", RECURSION);
    let small: Vec<Vec<Value>> = (0..20).map(|n| vec![Value::Int(n)]).collect();
    let mut n = 0;
    n += all.agree("fib", &small);
    n += all.agree("even", &small);
    n += all.agree("odd", &small);
    n += all.agree("gcd", &pairs(&ints(0x9CD, 18)));
    let loops: Vec<Vec<Value>> = [0i64, 1, 10, 1_000, 100_000]
        .iter()
        .map(|n| vec![Value::Int(*n), Value::Int(0)])
        .collect();
    n += all.agree("sum_to", &loops);
    n += all.agree("drain", &loops);
    // Ackermann is bounded by its arguments and grows fast: `(2, 3)` is 3,000 calls and `(3, 3)`
    // is 61 — small on purpose, per this suite's rule about chosen arguments.
    let ack: Vec<Vec<Value>> = (0..=2)
        .flat_map(|m| (0..=3).map(move |k| vec![Value::Int(m), Value::Int(k)]))
        .collect();
    n += all.agree("ackermann", &ack);
    println!("{n} calls agreed on recursion");
}

/// A tail call is a jump here too, and Cranelift's verifier is what says so: `return_call` is
/// refused outright if the frame cannot be discarded, which is the same guarantee `musttail` gives
/// the other backend and what `docs/31`'s property rests on.
#[test]
fn a_tail_call_costs_nothing_and_has_no_ceiling() {
    linker!();
    let all = All::over("rec.beck", RECURSION);
    // Sixty million tail calls, which is a stack overflow in any implementation that spends a
    // frame on one. The evaluator is not asked: `docs/62`'s fuel budget is what bounds it, and
    // this is a claim about compiled code.
    let deep = all
        .clif
        .call("sum_to", &[Value::Int(60_000_000), Value::Int(0)])
        .expect("sixty million tail calls");
    assert_eq!(deep, Value::Int(1_800_000_030_000_000));
    // …and one to a definition of a different arity, which is the case a C-convention `musttail`
    // cannot express.
    let other = all
        .clif
        .call("drain", &[Value::Int(1_000_000), Value::Int(0)])
        .expect("a million tail calls into a different arity");
    assert_eq!(other, Value::Int(2_000_000));
}

/// Every trap the emitter can store, with the message the evaluator would have printed.
#[test]
fn a_trap_carries_the_evaluators_own_message_and_a_span() {
    linker!();
    let all = All::over("arith.beck", ARITHMETIC);
    for (name, args) in [
        ("plus", [i64::MAX, 1]),
        ("minus", [i64::MIN, 1]),
        ("times", [i64::MAX, 2]),
        ("over", [1, 0]),
        ("modulo", [1, 0]),
        ("over", [i64::MIN, -1]),
    ] {
        let call = vec![Value::Int(args[0]), Value::Int(args[1])];
        let walked = {
            let def = &all.program.defs[name];
            beck_eval::on_the_evaluator_stack(|| {
                let f = all.evaluator.function(&def.body).expect("prepares");
                outcome(f(call.clone()))
            })
        };
        let compiled = all.clif.call(name, &call);
        let message = compiled.as_ref().expect_err("a trap").message.clone();
        assert_eq!(
            walked,
            Err(message.clone()),
            "`{name}` on {args:?}: the two do not fail the same way"
        );
        assert_ne!(
            compiled.expect_err("a trap").span,
            beck_diag::Span::NONE,
            "`{name}` on {args:?}: a trap has to name where it happened"
        );
    }
    // A `negate` and an `abs` of `i64::MIN`, which are the other two integer traps.
    for name in ["negated", "absolute"] {
        let e = all
            .clif
            .call(name, &[Value::Int(i64::MIN)])
            .expect_err("a trap");
        assert!(e.message.contains("overflow"), "{name}: {}", e.message);
    }
}

// -------------------------------------------------------------------------------------------
// The two emitters, held to one subset
// -------------------------------------------------------------------------------------------

/// The claim that makes two emitters worth having: they compile the same programs.
///
/// Not the same *reasons* — a refusal's wording is each emitter's own — but the same **set**, over
/// every program in this suite and every program in the corpus. A subset that drifted would make
/// `--backend` a choice about what the language is rather than about how long a build takes.
#[test]
fn the_two_emitters_accept_and_refuse_the_same_definitions() {
    linker!();
    if beck_llvm::Toolchain::find().is_none() {
        assert!(
            !require_native(),
            "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
        );
        // The *emitters* are compared, and `beck_llvm::emit::module` needs no toolchain — but a
        // machine with no `clang` cannot run the other half of this suite either, so saying so
        // once here is what keeps a half-run from looking like a full one.
        println!("note: no `clang`, so nothing below this line ran against a linked LLVM artefact");
    }
    let mut programs: Vec<(String, String)> = [
        ("arith.beck", ARITHMETIC),
        ("reals.beck", REALS),
        ("control.beck", CONTROL),
        ("rec.beck", RECURSION),
        ("refused.beck", REFUSED),
    ]
    .iter()
    .map(|(n, s)| (n.to_string(), s.to_string()))
    .collect();
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&corpus)
        .expect("the corpus is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        programs.push((name, std::fs::read_to_string(&path).expect("readable")));
    }

    let mut compiled_total = 0;
    for (name, src) in &programs {
        let program = compile(name, src);
        let clif = beck_clif::emit::module(&program).expect("cranelift has a target here");
        let llvm = beck_llvm::emit::module(&program);

        let mut ours: Vec<&str> = clif.functions.iter().map(|f| &*f.name).collect();
        let mut theirs: Vec<&str> = llvm.functions.iter().map(|f| &*f.name).collect();
        ours.sort_unstable();
        theirs.sort_unstable();
        assert_eq!(
            ours, theirs,
            "{name}: the two emitters compile different definitions"
        );

        let mut ours: Vec<&str> = clif.refusals.iter().map(|r| &*r.name).collect();
        let mut theirs: Vec<&str> = llvm.refusals.iter().map(|r| &*r.name).collect();
        ours.sort_unstable();
        theirs.sort_unstable();
        assert_eq!(
            ours, theirs,
            "{name}: the two emitters refuse different definitions"
        );

        // …and every compiled definition has the same index, so a dispatch table built by one and
        // read by the other would still be right. Nothing does that, and it is the cheapest way to
        // check that the *order* is the program's rather than each emitter's.
        for (a, b) in clif.functions.iter().zip(&llvm.functions) {
            assert_eq!(a.name, b.name, "{name}: dispatch order differs");
            assert_eq!(a.index, b.index, "{name}: dispatch index differs");
        }
        compiled_total += clif.functions.len();
    }
    println!(
        "{} programs, {compiled_total} definitions, one subset",
        programs.len()
    );
}

/// A refusal is by name and with a reason, and a definition that *calls* a refused one is refused
/// in turn — the fixed point, in the second emitter.
#[test]
fn what_cannot_be_compiled_is_refused_by_name_and_with_a_reason() {
    linker!();
    let all = All::over("refused.beck", REFUSED);
    for name in [
        "takes_a_record",
        "builds_a_record",
        "takes_a_list",
        "builds_a_string",
        "matches_a_union",
        "is_generic",
        "reads_the_clock",
        "calls_something_refused",
    ] {
        let reason = all
            .refusal(name)
            .unwrap_or_else(|| panic!("`{name}` must be refused, and it was not"));
        assert!(!reason.is_empty(), "`{name}` is refused without saying why");
    }
    assert_eq!(
        all.compiled(),
        vec!["scalar_and_fine".to_string()],
        "the one scalar definition is the one that compiles"
    );
    assert_eq!(
        all.refusal("calls_something_refused"),
        Some("calls `takes_a_record`, which does not compile"),
        "the fixed point has to name what it was waiting on"
    );
}

/// The seam: a `Backend` over the whole program, compiled where it can be and walked where it
/// cannot, with the boundary askable rather than assumed.
#[test]
fn the_seam_runs_the_compiled_half_and_falls_back_for_the_rest() {
    linker!();
    let program = compile("refused.beck", REFUSED);
    let fallback = beck_eval::backend_for(program.clone());
    let dev = beck_clif::Dev::build(&program, fallback, Some(LIMIT))
        .expect("builds")
        .expect("there is a linker");
    assert_eq!(dev.name(), "cranelift");
    let scalar = &program.defs["scalar_and_fine"].body;
    let record = &program.defs["takes_a_record"].body;
    assert!(dev.compiled(scalar), "the scalar definition is compiled");
    assert!(!dev.compiled(record), "the record one is not");
    let f = dev.function(scalar).expect("prepares");
    assert_eq!(f(vec![Value::Int(21)]).expect("runs"), Value::Int(42));
    // …and the refused one still answers, from the tree-walker behind the seam.
    let g = dev.function(record).expect("prepares");
    let point = beck_core::Value::data(
        Arc::from("Point"),
        None,
        beck_core::core::Fields::from_iter([
            (Arc::from("x"), Value::Int(2)),
            (Arc::from("y"), Value::Int(3)),
        ]),
    );
    assert_eq!(g(vec![point]).expect("runs"), Value::Int(5));
}

/// Every corpus program produces an object the linker accepts.
///
/// Not "every corpus program compiles to native code" — most of them are records and maps and
/// compile almost nothing. What is asserted is that the *emitter* survives every shape the corpus
/// has, which is what a crash in a fixed point or a malformed function would break.
#[test]
fn every_corpus_program_produces_an_object_a_linker_accepts() {
    let linker = linker!();
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&corpus)
        .expect("the corpus is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    files.sort();
    let mut compiled = 0;
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(path).expect("readable");
        let program = compile(&name, &src);
        let artifact = ClifArtifact::build_bounded(&program, linker.clone(), None, Some(LIMIT))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        compiled += artifact.module().functions.len();
    }
    println!(
        "{} corpus programs linked; {compiled} definitions compiled between them",
        files.len()
    );
}

/// The object is a function of the program: the same source twice produces the same bytes.
///
/// [`92`](../../../../docs/92-sbom-report.md) §92.4 makes reproducibility a property this project
/// checks by building twice and comparing, and a code generator that embedded a timestamp or a
/// path would defeat it one layer down.
#[test]
fn the_generated_object_is_a_function_of_the_program() {
    linker!();
    let program = compile("arith.beck", ARITHMETIC);
    let a = beck_clif::emit::module(&program).expect("emits");
    let b = beck_clif::emit::module(&program).expect("emits");
    assert_eq!(a.object, b.object, "two builds differ");
    assert_eq!(a.clif, b.clif, "two renderings of the IR differ");
    let other = compile(
        "arith.beck",
        &format!("{ARITHMETIC}\ndef extra(n: Int) -> Int:\n    return n\n"),
    );
    let c = beck_clif::emit::module(&other).expect("emits");
    assert_ne!(
        a.object, c.object,
        "a changed program produced the same object"
    );
}

/// §7.3's reason for a second code generator, *reported* rather than gated.
///
/// The claim there is "~10× faster codegen step than LLVM", and this file cannot hold anybody to
/// it: a `cargo test` build of this workspace is a **debug** build, so the comparison would be our
/// unoptimised Cranelift against a distribution's optimised `clang` — which is not a comparison of
/// two code generators, and it is a factor of two the *wrong* way when measured. What runs here is
/// the shape that has no clock in it: both produce a module for the same program, and the times
/// are printed so a reader of the log can see them. [`measure_native.rs`](measure_native.rs) is
/// where the number is measured in a release build, per `AGENTS.md`.
#[test]
fn both_code_generators_answer_for_the_same_wide_program() {
    linker!();
    let mut src = String::new();
    for i in 0..200 {
        src.push_str(&format!(
            "def f{i}(a: Int, b: Int) -> Int:\n    return (a * b) + (a - b) + {i}\n\n"
        ));
    }
    let program = compile("wide.beck", &src);

    let started = std::time::Instant::now();
    let module = beck_clif::emit::module(&program).expect("emits");
    let clif = started.elapsed();
    assert_eq!(module.functions.len(), 200);

    let started = std::time::Instant::now();
    let llvm = beck_llvm::emit::module(&program);
    let text = started.elapsed();
    assert_eq!(llvm.functions.len(), 200);

    println!(
        "200 definitions, debug build: cranelift {:.1} ms to an object, llvm {:.1} ms to text \
         (before `clang` has run)",
        clif.as_secs_f64() * 1000.0,
        text.as_secs_f64() * 1000.0
    );
}
