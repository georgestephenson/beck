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
use beck_llvm::{Artifact as LlvmArtifact, Repr};

mod support;
use support::clofix::{self, CLOSURES};
use support::failfix;
use support::heapfix::{self, RECORDS, STILL_REFUSED, UNIONS};
use support::listfix::{self, LISTS};
use support::mapfix::{self, MAPS};
use support::scalar::{
    float_pairs, floats, ints, pairs, render, singles, ARITHMETIC, CONTROL, REALS, RECURSION,
    REFUSED,
};
use support::textfix::{self, TEXT};
use support::viewfix;

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
/// the other backend and what `docs/27`'s property rests on.
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
// The heap
// -------------------------------------------------------------------------------------------

/// Records, through the second emitter: built, read, updated and compared.
///
/// The interesting half is not that Cranelift can build a record — it is that the *layout* is
/// [`beck_llvm::heap`]'s rather than this emitter's, so the three backends have to agree about
/// which word a field is in and which rank a variant has. `docs/101` §101.3 is why that one
/// decision is shared where the emitters are not.
#[test]
fn the_three_backends_agree_on_records() {
    linker!();
    let all = All::over("records.beck", RECORDS);
    let ps = heapfix::records();
    let mut compared = 0;
    compared += all.agree("origin", &[vec![]]);
    compared += all.agree("make", &pairs(&ints(0x5eed_0021, 10)));
    for name in ["sum_of", "swapped"] {
        compared += all.agree(name, &heapfix::singles(&ps));
    }
    for name in ["same_point", "point_order", "span_of", "segment_order"] {
        compared += all.agree(name, &heapfix::pairs(&ps));
    }
    compared += all.agree("key_order", &heapfix::pairs(&heapfix::keys()));
    for name in ["heavier", "same_weight"] {
        compared += all.agree(name, &heapfix::pairs(&heapfix::weighted()));
    }
    for name in ["negated", "negated_is_zero"] {
        compared += all.agree(name, &heapfix::singles(&heapfix::weighted()));
    }
    let with_dx: Vec<Vec<Value>> = ps
        .iter()
        .flat_map(|p| [-1i64, 0, 1, i64::MAX].map(|d| vec![p.clone(), Value::Int(d)]))
        .collect();
    compared += all.agree("moved", &with_dx);
    compared += all.agree("scaled", &with_dx);
    println!("{compared} record calls compared across every backend on this machine");
}

#[test]
fn the_three_backends_agree_on_unions() {
    linker!();
    let all = All::over("unions.beck", UNIONS);
    let rs = heapfix::ranked();
    let ts = heapfix::trees();
    let mut compared = 0;
    for name in ["rank", "guarded", "either", "whole", "n_or_zero"] {
        compared += all.agree(name, &heapfix::singles(&rs));
    }
    for name in ["ranked_order", "same_ranked"] {
        compared += all.agree(name, &heapfix::pairs(&rs));
    }
    for name in ["total", "left_leaf", "first_number"] {
        compared += all.agree(name, &heapfix::singles(&ts));
    }
    compared += all.agree("tree_order", &heapfix::pairs(&ts));
    compared += all.agree("spine", &singles(&(0..12).collect::<Vec<_>>()));
    compared += all.agree(
        "chain",
        &(0..12)
            .map(|n| vec![Value::Int(n), heapfix::leaf(0)])
            .collect::<Vec<_>>(),
    );
    compared += all.agree("bigger", &singles(&ints(0x5eed_0022, 8)));
    compared += all.agree("wrap", &singles(&ints(0x5eed_0023, 8)));
    compared += all.agree("maybe", &singles(&ints(0x5eed_0024, 8)));
    compared += all.agree(
        "or_else",
        &heapfix::options()
            .iter()
            .map(|o| vec![o.clone(), Value::Int(-1)])
            .collect::<Vec<_>>(),
    );
    println!("{compared} union calls compared across every backend on this machine");
}

/// Text, over every backend this machine has.
///
/// `native.rs`'s sweep with Cranelift added, and it is the sweep that matters rather than the
/// sample: the two emitters write the search, the clamp and the three-way comparison twice, so
/// this is where writing them twice is worth something.
#[test]
fn the_three_backends_agree_on_text() {
    linker!();
    let all = All::over("text.beck", TEXT);
    let ss = textfix::strings();
    let mut compared = 0;
    for name in [
        "size", "empty", "first", "rest", "greeting", "is_yes", "echoed", "which", "tag", "thrice",
    ] {
        compared += all.agree(name, &textfix::singles(&ss));
    }
    for name in [
        "joined",
        "below",
        "above",
        "same",
        "differ",
        "not_after",
        "not_before",
        "inside",
        "opens",
        "closes",
        "at",
    ] {
        compared += all.agree(name, &textfix::pairs(&ss));
    }
    compared += all.agree("cut", &textfix::slices(&ss));
    compared += all.agree("count_of", &textfix::with_char(&ss));
    compared += all.agree(
        "at_or",
        &textfix::pairs(&ss)
            .into_iter()
            .map(|mut t| {
                t.push(Value::Int(-1));
                t
            })
            .collect::<Vec<_>>(),
    );
    compared += all.agree("repeat", &textfix::repeats(&ss));

    // Building text out of something that is not text, and taking an `Option` apart without a
    // `match` — the primitives `docs/105`'s and `docs/106`'s layouts made reachable.
    compared += all.agree("shown", &textfix::integers());
    compared += all.agree(
        "shown_bool",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    compared += all.agree("shown_str", &textfix::singles(&ss));
    compared += all.agree(
        "repeated",
        &textfix::repeats(&ss)
            .into_iter()
            .map(|mut t| {
                t.truncate(2);
                t
            })
            .collect::<Vec<_>>(),
    );
    compared += all.agree("glued", &textfix::joins(&ss));
    for name in ["or_else", "present"] {
        compared += all.agree(
            name,
            &textfix::options()
                .into_iter()
                .map(|mut t| {
                    if name == "present" {
                        t.truncate(1);
                    }
                    t
                })
                .collect::<Vec<_>>(),
        );
    }
    compared += all.agree(
        "sliced_or",
        &textfix::with_char(&ss)
            .into_iter()
            .map(|mut t| {
                // `(s, c, i, acc)` for `count_of`; `sliced_or` wants `(s, i, fallback)`.
                t.remove(1);
                t
            })
            .collect::<Vec<_>>(),
    );

    let named: Vec<Value> = ss
        .iter()
        .map(|s| heapfix::record("Named", &[("label", s.clone()), ("rank", Value::Int(1))]))
        .collect();
    compared += all.agree("label_of", &textfix::singles(&named));
    for name in ["named_below", "named_same"] {
        compared += all.agree(name, &textfix::pairs(&named));
    }
    compared += all.agree(
        "relabel",
        &named
            .iter()
            .flat_map(|n| ss.iter().map(move |s| vec![n.clone(), s.clone()]))
            .collect::<Vec<_>>(),
    );
    let tagged: Vec<Value> = ss
        .iter()
        .map(|s| heapfix::variant("Tagged", "Word", &[("text", s.clone())]))
        .chain([heapfix::variant(
            "Tagged",
            "Number",
            &[("n", Value::Int(3))],
        )])
        .collect();
    compared += all.agree("untag", &textfix::singles(&tagged));

    println!("{compared} text calls compared across every backend on this machine");
}

/// Lists, over every backend this machine has.
///
/// The sweep that matters is the **pairs**: a lexicographic comparison can be right for `<` and
/// wrong for `<=`, and one that ran out of elements before it ran out of answer would order `[1]`
/// and `[1, 2]` the wrong way round. Every element kind that is itself an offset is here too —
/// text, a list, a record — because comparing the *words* answers correctly for an `Int` and
/// wrongly for all three.
#[test]
fn the_three_backends_agree_on_lists() {
    linker!();
    let all = All::over("lists.beck", LISTS);
    let xs = listfix::lists();
    let mut compared = 0;
    for name in ["size", "empty", "flipped", "held"] {
        compared += all.agree(name, &listfix::singles(&xs));
    }
    for name in [
        "below",
        "above",
        "same",
        "differ",
        "not_after",
        "not_before",
    ] {
        compared += all.agree(name, &listfix::pairs(&xs));
    }
    for name in ["nth", "nth_or"] {
        compared += all.agree(name, &listfix::indexed(&xs));
    }
    for name in ["has", "at_of"] {
        compared += all.agree(name, &listfix::searched(&xs));
    }
    compared += all.agree("middle", &listfix::ranges(&xs));
    for name in ["front", "back"] {
        compared += all.agree(name, &listfix::counted(&xs));
    }
    compared += all.agree("three", &[vec![]]);
    compared += all.agree("none_at_all", &[vec![]]);
    // Growing one: the operation, the fork onto a shared block, and the accumulator.
    for name in ["appended", "forked"] {
        compared += all.agree(name, &listfix::searched(&xs));
    }
    for name in ["doubled_up", "sum_of"] {
        compared += all.agree(name, &listfix::singles(&xs));
    }
    compared += all.agree(
        "named",
        &listfix::texts()
            .iter()
            .flat_map(|v| {
                ["", "z", "aa"]
                    .iter()
                    .map(move |s| vec![v.clone(), Value::str_(s)])
            })
            .collect::<Vec<_>>(),
    );
    compared += all.agree(
        "grown_bag",
        &xs.iter()
            .map(|v| {
                vec![
                    Value::record("Bag", None, [("items", v.clone()), ("rank", Value::Int(1))]),
                    Value::Int(9),
                ]
            })
            .collect::<Vec<_>>(),
    );
    compared += all.agree("doubled", &singles(&ints(0x5eed_0031, 12)));
    compared += all.agree(
        "total",
        &xs.iter()
            .map(|v| vec![v.clone(), Value::Int(0), Value::Int(0)])
            .collect::<Vec<_>>(),
    );
    compared += all.agree(
        "walked",
        &xs.iter()
            .map(|v| {
                vec![
                    v.clone(),
                    Value::Int(0),
                    Value::List(std::sync::Arc::new(Vec::new())),
                ]
            })
            .collect::<Vec<_>>(),
    );

    // An element that is itself an offset, one kind each.
    let ts = listfix::texts();
    for name in ["texts_below", "texts_same"] {
        compared += all.agree(name, &listfix::pairs(&ts));
    }
    let ns = listfix::nested();
    for name in ["nested_below", "nested_same"] {
        compared += all.agree(name, &listfix::pairs(&ns));
    }
    compared += all.agree("nested_first", &listfix::singles(&ns));

    // A list inside a record and inside a union.
    let bags: Vec<Value> = xs
        .iter()
        .map(|v| heapfix::record("Bag", &[("items", v.clone()), ("rank", Value::Int(1))]))
        .collect();
    compared += all.agree("bag_items", &listfix::singles(&bags));
    for name in ["bag_below", "bag_same"] {
        compared += all.agree(name, &listfix::pairs(&bags));
    }
    compared += all.agree(
        "rebagged",
        &bags
            .iter()
            .flat_map(|bag| xs.iter().map(move |v| vec![bag.clone(), v.clone()]))
            .collect::<Vec<_>>(),
    );
    compared += all.agree(
        "bagged",
        &xs.iter()
            .map(|v| vec![v.clone(), Value::Int(3)])
            .collect::<Vec<_>>(),
    );
    let holdings: Vec<Value> = xs
        .iter()
        .map(|v| heapfix::variant("Holding", "Some_", &[("xs", v.clone())]))
        .chain([heapfix::variant("Holding", "None_", &[])])
        .collect();
    compared += all.agree("held_size", &listfix::singles(&holdings));

    println!("{compared} list calls compared, and every backend agreed on every one");
}

/// Maps, over every backend this machine has.
///
/// The sweep that matters is `keyed`: a binary search ends four ways — on the key, below every key,
/// above every key, and **between** two — and the last is the one a window that shrinks wrongly
/// never leaves. And the pairs, because `PMap`'s order is pair by pair and then by length, so a
/// comparison that ran out of entries before it ran out of answer orders a prefix the wrong way.
#[test]
fn the_three_backends_agree_on_maps() {
    linker!();
    let all = All::over("maps.beck", MAPS);
    let ms = mapfix::maps();
    let mut compared = 0;
    for name in ["size", "names", "totals", "is_nothing", "held"] {
        compared += all.agree(name, &mapfix::singles(&ms));
    }
    for name in [
        "below",
        "above",
        "same",
        "differ",
        "not_after",
        "not_before",
    ] {
        compared += all.agree(name, &mapfix::pairs(&ms));
    }
    for name in ["lookup", "lookup_or", "holds"] {
        compared += all.agree(name, &mapfix::keyed(&ms));
    }
    compared += all.agree("nothing", &[vec![]]);
    compared += all.agree(
        "total",
        &ms.iter()
            .map(|m| vec![m.clone(), Value::Int(0), Value::Int(0)])
            .collect::<Vec<_>>(),
    );

    // A value that is itself an offset.
    let ns = mapfix::nested();
    for name in ["nested_below", "nested_same"] {
        compared += all.agree(name, &mapfix::pairs(&ns));
    }
    compared += all.agree("nested_at", &mapfix::keyed(&ns));

    // A map inside a record and inside a union.
    let cs: Vec<Value> = ms
        .iter()
        .map(|m| {
            heapfix::record(
                "Counts",
                &[("tally", m.clone()), ("label", Value::str_("x"))],
            )
        })
        .collect();
    compared += all.agree("counts_tally", &mapfix::singles(&cs));
    compared += all.agree("counts_below", &mapfix::pairs(&cs));
    compared += all.agree(
        "recounted",
        &cs.iter()
            .flat_map(|c| ms.iter().map(move |m| vec![c.clone(), m.clone()]))
            .collect::<Vec<_>>(),
    );
    compared += all.agree(
        "counted",
        &ms.iter()
            .map(|m| vec![m.clone(), Value::str_("k")])
            .collect::<Vec<_>>(),
    );
    let hs: Vec<Value> = ms
        .iter()
        .map(|m| heapfix::variant("Holding", "Held", &[("m", m.clone())]))
        .chain([heapfix::variant("Holding", "Empty", &[])])
        .collect();
    compared += all.agree("held_size", &mapfix::singles(&hs));

    println!("{compared} map calls compared, and every backend agreed on every one");
}

/// Closures, over all three backends.
///
/// `native.rs`'s sweep with the third implementation in it. What it is for is the same thing the
/// record and the map differentials were for: the two emitters lower an application differently —
/// the other writes a `switch` and this writes a chain of comparisons — and a differential over the
/// *answers* is what says that a difference in shape is not a difference in meaning.
#[test]
fn the_three_backends_agree_on_closures() {
    linker!();
    let all = All::over("closures.beck", CLOSURES);
    let ns: Vec<i64> = vec![0, 1, -1, 2, 7, -7, i64::MAX, i64::MIN];
    let mut compared = 0;
    for name in ["twice", "again", "through", "double"] {
        compared += all.agree(name, &clofix::each_of(&ns));
    }
    for name in ["add_on", "nested"] {
        compared += all.agree(name, &clofix::pairs_of(&ns));
    }
    compared += all.agree("between", &clofix::triples_of(&ns));
    compared += all.agree("either", &clofix::flagged(&ns));

    let xs = clofix::lists();
    let bys: Vec<i64> = vec![0, 1, -1, 3, i64::MAX];
    for name in ["doubled", "summed", "flags", "tally", "risky"] {
        compared += all.agree(name, &clofix::singles(&xs));
    }
    for name in [
        "scaled",
        "kept",
        "biggest",
        "all_above",
        "any_above",
        "twice_over",
    ] {
        compared += all.agree(name, &clofix::with(&xs, &bys));
    }
    let ts = clofix::texts();
    for name in ["lengths", "shouted", "long_ones", "joined"] {
        compared += all.agree(name, &clofix::singles(&ts));
    }
    let rs = clofix::reals();
    for name in ["halved", "negated", "added"] {
        compared += all.agree(name, &clofix::singles(&rs));
    }
    // The last two list primitives, and `by_rank` is the stability case: every key in one of those
    // lists is the same, so an unstable sort is free to answer anything.
    compared += all.agree("flattened", &clofix::singles(&clofix::nested()));
    compared += all.agree("flat_texts", &clofix::singles(&clofix::nested_texts()));
    compared += all.agree("spread", &clofix::singles(&xs));
    for name in ["ascending", "descending", "by_sign"] {
        compared += all.agree(name, &clofix::singles(&xs));
    }
    for name in ["by_length", "by_text"] {
        compared += all.agree(name, &clofix::singles(&ts));
    }
    compared += all.agree("by_real", &clofix::singles(&rs));
    compared += all.agree("by_rank", &clofix::singles(&clofix::notes()));

    for name in ["same_lambda", "two_lambdas", "ordered"] {
        compared += all.agree(name, &[vec![]]);
    }
    compared += all.agree("captures_ignored", &clofix::pairs_of(&ns));

    println!("{compared} closure calls compared, and every backend agreed on every one");
}

/// A tail call through an application is a tail call here too.
///
/// `native.rs`'s gate on the third backend, and it is not the same mechanism: that one emits
/// `musttail` and this one emits `return_call`, which Cranelift **asserts** on rather than merely
/// honouring. Both hops have to be one — the call into the application and the arm inside it — and
/// ten million iterations is past any host stack, so a frame spent on either is a crash.
///
/// The gate was checked by making it red: with the application's own call site emitted as an
/// ordinary call, the other backend answers `SIGSEGV` at this size.
#[test]
fn a_tail_call_through_a_closure_costs_nothing() {
    linker!();
    let all = All::over("closures.beck", CLOSURES);
    let deep = all
        .clif
        .call("spin", &[Value::Int(10_000_000), Value::Int(0)])
        .expect("ten million applications in tail position");
    assert_eq!(deep, Value::Int(50_000_005_000_000));
}

/// The same shape gate the other backend has, with the same two sizes and the same arithmetic.
///
/// Written again rather than shared because the *allocator* is written again: this is where a
/// bump pointer that rounded, or a layout that padded, would show up on this backend and not the
/// other. No clock in it (`AGENTS.md`).
#[test]
fn the_arena_costs_the_same_per_object_at_every_size() {
    linker!();
    let all = All::over("unions.beck", UNIONS);
    for n in [100usize, 800] {
        let (_, bytes) = all
            .clif
            .call_sized("chain", &[Value::Int(n as i64), heapfix::leaf(0)])
            .expect("runs");
        assert_eq!(
            bytes,
            8 + 2 * 8 + n * 5 * 8,
            "chain({n}) left {bytes} bytes of arena"
        );
    }
}

/// A slice costs its answer here too, and the allocator is written again so this is written again.
///
/// `native.rs`'s gate, at the same two sizes and with the same arithmetic. What it catches on
/// *this* backend is a `beck.str.slice` that copied the string it was taken from — which the
/// differential cannot see, because copying too much still answers correctly.
#[test]
fn a_slice_costs_its_answer_and_not_the_string_it_came_from() {
    linker!();
    let all = All::over("text.beck", TEXT);
    const PER_CHARACTER: usize = 24;
    for n in [200usize, 1600] {
        let s = Value::str_("x".repeat(n));
        let args = [s.clone(), Value::Int(0), Value::str_("")];
        let arguments = all
            .clif
            .module()
            .heap
            .encode_args(&args, &[Repr::Str, Repr::Int, Repr::Str])
            .expect("encodes")
            .1
            .len();
        let (_, bytes) = all.clif.call_sized("walked", &args).expect("runs");
        assert_eq!(
            bytes - arguments,
            n * PER_CHARACTER,
            "walked over {n} characters left {bytes} bytes of arena"
        );
    }
}

/// A program with no object in it gets the object file it got before there was a heap.
#[test]
fn a_program_with_no_object_has_no_arena() {
    let scalar = beck_clif::module(&compile("arithmetic.beck", ARITHMETIC)).expect("compiles");
    assert!(
        !scalar.clif.contains("beck.alloc"),
        "a program of pure arithmetic must not reserve a heap"
    );
    assert!(scalar.heap.is_empty(), "and must have no layout at all");
    let heaped = beck_clif::module(&compile("records.beck", RECORDS)).expect("compiles");
    assert_eq!(
        heaped.heap.layouts().count(),
        4,
        "Point, Key, Weighed and Segment"
    );
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
        ("records.beck", RECORDS),
        ("unions.beck", UNIONS),
        ("still-refused.beck", STILL_REFUSED),
        ("closures.beck", CLOSURES),
        ("closures-refused.beck", support::clofix::REFUSED),
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
        "grows_a_map",
        "renders_a_real",
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
        Some("calls `grows_a_map`, which does not compile"),
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
    let mappy = &program.defs["grows_a_map"].body;
    assert!(dev.compiled(scalar), "the scalar definition is compiled");
    assert!(!dev.compiled(mappy), "the one that grows a map is not");
    let f = dev.function(scalar).expect("prepares");
    assert_eq!(f(vec![Value::Int(21)]).expect("runs"), Value::Int(42));
    // …and the refused one still answers, from the tree-walker behind the seam.
    let g = dev.function(mappy).expect("prepares");
    let m = Value::Map([(Value::str_("a"), Value::Int(1))].into_iter().collect());
    assert_eq!(
        g(vec![m, Value::str_("b"), Value::Int(2)]).expect("runs"),
        Value::Map(
            [
                (Value::str_("a"), Value::Int(1)),
                (Value::str_("b"), Value::Int(2))
            ]
            .into_iter()
            .collect()
        )
    );
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

/// Views, over all three backends.
///
/// `native.rs`'s sweep with the third implementation in it, and the reason it is worth running
/// twice is `docs/97` §97.3's: the two emitters write these five primitives separately, and the
/// only thing that says the subset is one subset is that the answers agree. A view is where that
/// bites hardest, because a node in the arena is a *recipe* — four words whose meaning depends on
/// a tag and on a repr index — so a backend that wrote the words in a different order would still
/// produce a tree, and it would be the wrong one.
#[test]
fn the_three_backends_agree_on_views() {
    linker!();
    let all = All::over("views.beck", viewfix::VIEWS);
    let cards = viewfix::cards();
    let lists = viewfix::lists();
    let mut compared = 0;

    compared += all.agree("just_text", &viewfix::singles(&textfix::strings()));
    compared += all.agree("a_number", &singles(&[0, 1, -1, i64::MAX, i64::MIN]));
    compared += all.agree(
        "a_flag",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    compared += all.agree(
        "a_real",
        &[0.0, -0.0, 1.5, f64::INFINITY, f64::NAN]
            .iter()
            .map(|f| vec![Value::float(*f)])
            .collect::<Vec<_>>(),
    );
    compared += all.agree("a_record", &viewfix::singles(&cards));
    compared += all.agree("a_list", &viewfix::singles(&lists));
    for name in [
        "titled",
        "maybe_done",
        "ordered",
        "keyed",
        "keyed_number",
        "handled",
        "handled_nullary",
        "wrapped",
        "nested",
        "one_attr",
        "one_key",
        "one_handler",
        "panelled",
    ] {
        compared += all.agree(name, &viewfix::singles(&cards));
    }
    compared += all.agree("blank", &[vec![]]);
    for name in ["rows", "attrs_from"] {
        compared += all.agree(name, &viewfix::singles(&lists));
    }
    compared += all.agree("whole", &viewfix::with(&cards, &lists));

    println!("{compared} view calls compared, and every backend agreed on every one");
}

/// The `ui:` block, over all three backends.
#[test]
fn the_three_backends_agree_on_a_ui_block() {
    linker!();
    let all = All::over("page.beck", viewfix::PAGE);
    let lefts = [0i64, 1, 7];
    let tuples: Vec<Vec<Value>> = viewfix::todos()
        .iter()
        .flat_map(|ts| lefts.iter().map(move |n| vec![ts.clone(), Value::Int(*n)]))
        .collect();
    let compared = all.agree("page", &tuples);
    println!("{compared} `ui:` pages compared, and every backend agreed on every one");
}

/// The two emitters accept and refuse the same views.
///
/// `docs/97` §97.3's assertion, one type over: the subset is written twice, so the thing to check
/// is that both wrote the same one. A view is the case where the two halves could most easily
/// drift, because neither generates a runtime function for it — there is nothing to link against
/// that would notice.
#[test]
fn the_two_emitters_agree_on_which_views_compile() {
    for (name, src) in [("views.beck", viewfix::VIEWS), ("page.beck", viewfix::PAGE)] {
        let program = compile(name, src);
        let llvm = beck_llvm::module(&program);
        let clif = beck_clif::emit::module(&program).expect("emits");
        let names =
            |fs: &[beck_llvm::Signature]| fs.iter().map(|f| f.name.to_string()).collect::<Vec<_>>();
        assert_eq!(
            names(&llvm.functions),
            names(&clif.functions),
            "the two emitters disagree about which of `{name}`'s definitions compile"
        );
    }
}

/// A `raise` and a `try:`, over all three backends.
///
/// `native.rs`'s sweep with the third implementation in it, and the reason it is worth running
/// twice is `docs/97` §97.3's — but there is a sharper one here: the two emitters write the handler
/// differently (one branches to a label, the other jumps to a block) and both have to get the same
/// two questions right, in the same order, about a cell whose shape is the *protocol*.
#[test]
fn the_three_backends_agree_on_failure() {
    linker!();
    let all = All::over("failure.beck", failfix::FAILURE);
    let ns = failfix::ints(&failfix::numbers());
    let mut compared = 0;
    for name in [
        "checked",
        "uncaught",
        "caught",
        "described",
        "overflows",
        "wrong_type",
        "nested",
    ] {
        compared += all.agree(name, &ns);
    }
    compared += all.agree("named", &failfix::texts());
    compared += all.agree("several", &failfix::lists());
    compared += all.agree("all_checked", &failfix::lists());
    println!("{compared} fallible calls compared, and every backend agreed on every one");
}
