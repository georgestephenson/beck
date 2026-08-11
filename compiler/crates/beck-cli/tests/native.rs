//! The native backend, against the tree-walker.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.8 names a
//! differential test *between backends* as what keeps two of them honest, and
//! `tests/backend_seam.rs` §"two_backends_over_one_program_agree" has carried that shape since
//! Phase 1 with the same evaluator on both sides — stated there as asserting the harness rather
//! than the agreement. This is the file where there are two implementations to point it at.
//!
//! # What is compared, and why the errors matter as much as the values
//!
//! For every definition [`beck_llvm`] compiles, both backends are called on the same arguments and
//! the **whole outcome** is compared: the value, or the failure *and its message*. An integer
//! overflow is a value in this language — `beck-eval` answers `"`+` overflowed"` rather than
//! wrapping — so a backend that wrapped, or that failed for a different reason, or that failed
//! where the other succeeded, is a divergence. Comparing only the successes would have passed a
//! backend that trapped on everything hard.
//!
//! # Why the arguments are chosen rather than swept
//!
//! There is no fuel in compiled code (`beck_llvm::worker`), so a definition given an argument that
//! makes it run for a year runs for a year, and a differential that generated `factorial(i64::MAX)`
//! would be a differential that hangs. Every argument here is either a boundary value or small,
//! and each program says which of its definitions are total and which are bounded by their input.
//! The worker also carries a wall-clock limit, so a mistake in that judgement is a failing test
//! rather than a stuck one.
//!
//! # Skipping
//!
//! There is no `clang` on every machine. With none, these tests **print why they skipped** and
//! pass; with `BECK_REQUIRE_LLVM=1` a missing toolchain is a failure, and that is what CI sets.
//! `docs/19` §19.4 item 10 is why the skip is loud: an artefact nobody executed that reports
//! success is worse than one that reports nothing.

use std::sync::Arc;
use std::time::Duration;

use beck_core::backend::Backend;
use beck_core::{Program, Value};
use beck_llvm::{Artifact, Native, Repr};

mod support;
use support::heapfix::{self, RECORDS, STILL_REFUSED, UNIONS};
use support::scalar::{
    float_pairs, floats, ints, pairs, render, singles, ARITHMETIC, CONTROL, REALS, RECURSION,
    REFUSED,
};
use support::textfix::{self, TEXT};

/// One call may not take longer than this. Nothing here should come close; it is the difference
/// between a red test and a hung suite.
const LIMIT: Duration = Duration::from_secs(30);

fn require_llvm() -> bool {
    std::env::var("BECK_REQUIRE_LLVM").is_ok_and(|v| v == "1")
}

/// The toolchain, or a printed skip.
macro_rules! toolchain {
    () => {
        match beck_llvm::Toolchain::find() {
            Some(t) => t,
            None => {
                assert!(
                    !require_llvm(),
                    "BECK_REQUIRE_LLVM=1 and there is no `clang` on the path"
                );
                println!(
                    "skipped: no LLVM toolchain — no `clang` on the path, and BECK_CLANG does not \
                     name a working one. Set BECK_REQUIRE_LLVM=1 to make this a failure."
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

fn artifact(program: &Program) -> Artifact {
    let toolchain = beck_llvm::Toolchain::find().expect("checked by the caller");
    Artifact::build_bounded(program, toolchain, None, Some(LIMIT))
        .expect("clang accepts the module")
}

/// What one backend answered: a value, or the message it failed with.
///
/// The span is deliberately *not* compared. Both sides carry one and both point into the same
/// file, but the evaluator's is the span of the `Core` node it was walking and the native one is
/// the span the emitter recorded for the trapping instruction, and those are the same location
/// only when nothing has been folded. The message is the claim; §93.7 says so.
type Outcome = Result<Value, String>;

fn outcome(r: Result<Value, beck_core::ExecError>) -> Outcome {
    r.map_err(|e| e.message)
}

/// Both backends over one program, so a test says what it means in one line per case.
struct Both {
    program: Arc<Program>,
    native: Artifact,
    evaluator: Arc<dyn Backend>,
}

impl Both {
    fn over(name: &str, src: &str) -> Both {
        let program = compile(name, src);
        Both {
            native: artifact(&program),
            evaluator: beck_eval::backend_for(program.clone()),
            program,
        }
    }

    fn compiled(&self) -> Vec<String> {
        self.native
            .module()
            .functions
            .iter()
            .map(|f| f.name.to_string())
            .collect()
    }

    fn refusal(&self, name: &str) -> Option<&str> {
        self.native
            .module()
            .refusals
            .iter()
            .find(|r| &*r.name == name)
            .map(|r| r.reason.as_str())
    }

    /// Call `name` on both, and answer both outcomes.
    fn call(&self, name: &str, args: &[Value]) -> (Outcome, Outcome) {
        let def = self
            .program
            .defs
            .get(name)
            .unwrap_or_else(|| panic!("no definition `{name}`"));
        // The tree-walker spends host frames on recursion that is not in tail position, and says
        // how much stack that needs. Every entry point in the workspace honours it; so does this.
        let evaluated = beck_eval::on_the_evaluator_stack(|| {
            let f = self.evaluator.function(&def.body).expect("prepares");
            outcome(f(args.to_vec()))
        });
        (evaluated, outcome(self.native.call(name, args)))
    }

    /// Assert the two agree on every tuple, and answer how many were compared.
    fn agree(&self, name: &str, tuples: &[Vec<Value>]) -> usize {
        assert!(
            self.compiled().iter().any(|n| n == name),
            "`{name}` did not compile natively, so this compares the evaluator with itself"
        );
        for args in tuples {
            let (walked, compiled) = self.call(name, args);
            assert_eq!(
                walked,
                compiled,
                "`{name}` disagreed on {}: the evaluator said {walked:?} and the native backend \
                 said {compiled:?}",
                render(args)
            );
        }
        tuples.len()
    }
}

// -------------------------------------------------------------------------------------------
// The differential
// -------------------------------------------------------------------------------------------

#[test]
fn the_two_backends_agree_on_integer_arithmetic() {
    let _ = toolchain!();
    let both = Both::over("arithmetic.beck", ARITHMETIC);
    let xs = ints(0x5eed_0001, 24);
    let mut compared = 0;
    for name in [
        "plus", "minus", "times", "over", "modulo", "compares", "orders", "logic", "chained",
    ] {
        compared += both.agree(name, &pairs(&xs));
    }
    for name in ["negated", "absolute"] {
        compared += both.agree(name, &singles(&xs));
    }
    // The set has to *contain* the failures, or this passed by never reaching one.
    let (walked, compiled) = both.call("over", &[Value::Int(1), Value::Int(0)]);
    assert_eq!(walked, compiled);
    assert!(walked.is_err(), "a division by zero has to fail");
    let (walked, _) = both.call("times", &[Value::Int(i64::MAX), Value::Int(2)]);
    assert!(walked.is_err(), "an overflow has to fail");
    println!("{compared} integer calls compared, and both backends agreed on every one");
}

#[test]
fn the_two_backends_agree_on_reals() {
    let _ = toolchain!();
    let both = Both::over("reals.beck", REALS);
    let xs = floats(0x5eed_0002, 22);
    let mut compared = 0;
    for name in [
        "rplus",
        "rminus",
        "rtimes",
        "rover",
        "reciprocal_of_product",
        "product_is_zero",
        "product_order",
        "zero_through_sqrt",
        "signed_zero",
        "rless",
        "requal",
        "rorder",
    ] {
        compared += both.agree(name, &float_pairs(&xs));
    }
    let one_real: Vec<Vec<Value>> = xs.iter().map(|x| vec![Value::float(*x)]).collect();
    for name in ["rnegated", "rabs", "rsqrt", "rsin", "rcos", "truncated"] {
        compared += both.agree(name, &one_real);
    }
    compared += both.agree("widened", &singles(&ints(0x5eed_0003, 24)));
    println!("{compared} real calls compared, and both backends agreed on every one");
}

/// The two IEEE values `Value::float` refuses to distinguish, and the arithmetic that reaches them.
///
/// This is the test that would have caught the backend emitting `fcmp` — which is the obvious
/// lowering, is what every other language does, and is wrong here, because `docs/27` §27.8 made
/// Beck's `==` on reals *structural* so that a fold's accumulator could have a total order.
#[test]
fn a_negative_zero_and_a_nan_mean_here_what_they_mean_in_the_evaluator() {
    let _ = toolchain!();
    let both = Both::over("reals.beck", REALS);
    let z = Value::float(0.0);
    let nz = Value::float(-0.0);
    let nan = Value::float(f64::NAN);
    let one = Value::float(1.0);

    // `-0.0` is canonicalised on the way in, so it is `0.0` by the time either backend sees it —
    // and `1.0 / (x * y)` is the arithmetic that would otherwise manufacture one inside a
    // computation, where no canonicalisation happens.
    for (a, b) in [
        (nz.clone(), one.clone()),
        (nz.clone(), nz.clone()),
        (z.clone(), Value::float(-1.0)),
    ] {
        let (walked, compiled) = both.call("reciprocal_of_product", &[a.clone(), b.clone()]);
        assert_eq!(walked, compiled, "1 / ({} * {})", a.display(), b.display());
    }
    assert_eq!(
        both.call("requal", &[z.clone(), nz.clone()]),
        (Ok(Value::Bool(true)), Ok(Value::Bool(true))),
        "the language says the two zeros are one value"
    );
    // NaN is the maximum of the order `Value` derives, and `fcmp` would answer `false` to both.
    assert_eq!(
        both.call("rless", &[one.clone(), nan.clone()]),
        (Ok(Value::Bool(true)), Ok(Value::Bool(true))),
        "NaN is the top of the total order §3.7 needs"
    );
    assert_eq!(
        both.call("requal", &[nan.clone(), nan.clone()]),
        (Ok(Value::Bool(true)), Ok(Value::Bool(true))),
        "and it equals itself, which IEEE 754 denies and a map key requires"
    );
}

/// A NaN a *computation* produced, rather than one the host canonicalised on the way in.
///
/// `Value::float` maps every NaN to one NaN and the emitted code does not, on the argument that
/// the operations here produce the platform's default quiet NaN — which is the same one. That is
/// an assumption about the target, so it is a test rather than a sentence in a comment.
#[test]
fn nan_is_the_same_nan_on_both_sides() {
    let _ = toolchain!();
    let both = Both::over("reals.beck", REALS);
    let zero = Value::float(0.0);
    let inf = Value::float(f64::INFINITY);
    for (name, args) in [
        ("rover", vec![zero.clone(), zero.clone()]),
        ("rminus", vec![inf.clone(), inf.clone()]),
        ("rtimes", vec![zero.clone(), inf.clone()]),
        ("rsqrt", vec![Value::float(-1.0)]),
    ] {
        let (walked, compiled) = both.call(name, &args);
        assert_eq!(walked, compiled, "{name}{}", render(&args));
        assert!(
            matches!(&walked, Ok(v) if v.as_f64().is_some_and(f64::is_nan)),
            "{name}{} should be a NaN, got {walked:?}",
            render(&args)
        );
    }
}

#[test]
fn the_two_backends_agree_on_control_flow() {
    let _ = toolchain!();
    let both = Both::over("control.beck", CONTROL);
    let xs = ints(0x5eed_0004, 30);
    let mut compared = 0;
    for name in ["classify", "shadowing", "guard_falls_through"] {
        compared += both.agree(name, &singles(&xs));
    }
    compared += both.agree("nested", &pairs(&ints(0x5eed_0005, 16)));
    compared += both.agree(
        "truthy",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    println!("{compared} control-flow calls compared, and both backends agreed on every one");
}

#[test]
fn the_two_backends_agree_on_recursion() {
    let _ = toolchain!();
    let both = Both::over("recursion.beck", RECURSION);
    let small: Vec<i64> = (0..20).collect();
    let mut compared = 0;
    compared += both.agree("fib", &singles(&small));
    compared += both.agree("even", &singles(&small));
    compared += both.agree("odd", &singles(&small));
    compared += both.agree("gcd", &pairs(&ints(0x5eed_0006, 18)));
    compared += both.agree(
        "sum_to",
        &small
            .iter()
            .map(|n| vec![Value::Int(*n), Value::Int(0)])
            .collect::<Vec<_>>(),
    );
    compared += both.agree(
        "drain",
        &small
            .iter()
            .map(|n| vec![Value::Int(*n), Value::Int(0)])
            .collect::<Vec<_>>(),
    );
    // Ackermann grows too fast to sweep, and that is the point of including it: `A(3, 5)` is
    // 42,438 calls, none of them in tail position.
    let ack: Vec<Vec<Value>> = (0..4)
        .flat_map(|m| (0..5).map(move |n| vec![Value::Int(m), Value::Int(n)]))
        .collect();
    compared += both.agree("ackermann", &ack);
    println!("{compared} recursive calls compared, and both backends agreed on every one");
}

// -------------------------------------------------------------------------------------------
// The heap
// -------------------------------------------------------------------------------------------

#[test]
fn the_two_backends_agree_on_records() {
    let _ = toolchain!();
    let both = Both::over("records.beck", RECORDS);
    let ps = heapfix::records();
    let mut compared = 0;
    compared += both.agree("origin", &[vec![]]);
    compared += both.agree("make", &pairs(&ints(0x5eed_0011, 12)));
    compared += both.agree("sum_of", &heapfix::singles(&ps));
    compared += both.agree("swapped", &heapfix::singles(&ps));
    for name in ["same_point", "point_order"] {
        compared += both.agree(name, &heapfix::pairs(&ps));
    }
    compared += both.agree("key_order", &heapfix::pairs(&heapfix::keys()));
    for name in ["heavier", "same_weight"] {
        compared += both.agree(name, &heapfix::pairs(&heapfix::weighted()));
    }
    for name in ["negated", "negated_is_zero"] {
        compared += both.agree(name, &heapfix::singles(&heapfix::weighted()));
    }
    for name in ["span_of", "segment_order"] {
        compared += both.agree(name, &heapfix::pairs(&ps));
    }
    let with_dx: Vec<Vec<Value>> = ps
        .iter()
        .flat_map(|p| [-1i64, 0, 1, i64::MAX].map(|d| vec![p.clone(), Value::Int(d)]))
        .collect();
    compared += both.agree("moved", &with_dx);
    compared += both.agree("scaled", &with_dx);

    // The set has to *contain* a failure, or this passed by never reaching one: a field
    // expression that overflows means the record is never built, and the message is the
    // evaluator's.
    let (walked, compiled) = both.call("scaled", &[heapfix::point(i64::MAX, 0), Value::Int(2)]);
    assert_eq!(walked, compiled);
    assert!(walked.is_err(), "an overflow in a field has to fail");
    println!("{compared} record calls compared, and both backends agreed on every one");
}

#[test]
fn the_two_backends_agree_on_unions() {
    let _ = toolchain!();
    let both = Both::over("unions.beck", UNIONS);
    let rs = heapfix::ranked();
    let ts = heapfix::trees();
    let mut compared = 0;
    for name in ["rank", "guarded", "either", "whole", "n_or_zero"] {
        compared += both.agree(name, &heapfix::singles(&rs));
    }
    for name in ["ranked_order", "same_ranked"] {
        compared += both.agree(name, &heapfix::pairs(&rs));
    }
    compared += both.agree("bigger", &singles(&ints(0x5eed_0012, 10)));
    for name in ["total", "left_leaf", "first_number"] {
        compared += both.agree(name, &heapfix::singles(&ts));
    }
    compared += both.agree("tree_order", &heapfix::pairs(&ts));
    compared += both.agree("spine", &singles(&(0..12).collect::<Vec<_>>()));
    compared += both.agree(
        "chain",
        &(0..12)
            .map(|n| vec![Value::Int(n), heapfix::leaf(0)])
            .collect::<Vec<_>>(),
    );
    compared += both.agree("wrap", &singles(&ints(0x5eed_0013, 8)));
    compared += both.agree(
        "unwrap",
        &heapfix::singles(&[heapfix::id(0), heapfix::id(7)]),
    );
    compared += both.agree("maybe", &singles(&ints(0x5eed_0014, 8)));
    compared += both.agree(
        "or_else",
        &heapfix::options()
            .iter()
            .map(|o| vec![o.clone(), Value::Int(-1)])
            .collect::<Vec<_>>(),
    );
    println!("{compared} union calls compared, and both backends agreed on every one");
}

/// Text, compared against the tree-walker over every string in [`textfix`] and every clamp.
///
/// The point of the sweep is the *pairs*: a three-way comparison can be right for `<` and wrong for
/// `<=`, and a `memcmp` over the shorter length answers `0` for `"ab"` against `"abc"` — so every
/// operator is asked about every ordered pair rather than about a sample.
#[test]
fn the_two_backends_agree_on_text() {
    let _ = toolchain!();
    let both = Both::over("text.beck", TEXT);
    let ss = textfix::strings();
    let mut compared = 0;
    for name in [
        "size", "empty", "first", "rest", "greeting", "is_yes", "echoed", "which", "tag",
    ] {
        compared += both.agree(name, &textfix::singles(&ss));
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
    ] {
        compared += both.agree(name, &textfix::pairs(&ss));
    }
    compared += both.agree("thrice", &textfix::singles(&ss));
    compared += both.agree("cut", &textfix::slices(&ss));
    compared += both.agree("count_of", &textfix::with_char(&ss));
    compared += both.agree("repeat", &textfix::repeats(&ss));

    // Text inside a record and inside a union, so a `Str` in a field is compared, rebuilt by
    // `with`, read back out and ordered against another record's.
    let named: Vec<Value> = ss
        .iter()
        .map(|s| heapfix::record("Named", &[("label", s.clone()), ("rank", Value::Int(1))]))
        .collect();
    compared += both.agree("label_of", &textfix::singles(&named));
    for name in ["named_below", "named_same"] {
        compared += both.agree(name, &textfix::pairs(&named));
    }
    compared += both.agree(
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
    compared += both.agree("untag", &textfix::singles(&tagged));

    println!("{compared} text calls compared, and both backends agreed on every one");
}

/// A tag is a variant's rank **by name**, and a field's slot is its rank by name.
///
/// The differential above covers both, and it covers them by comparing against an oracle rather
/// than against an expectation — so this pins the oracle. `Ranked` is declared `Small, Big,
/// Nothing` and `Key` is declared `score, name`, and both answers below are the opposite of what a
/// layout in declaration order gives. Without this, a day when `Value`'s own `Ord` changed would
/// move both sides together and the differential would stay green.
#[test]
fn a_layout_is_ordered_by_name_and_not_by_declaration() {
    let _ = toolchain!();
    let unions = Both::over("unions.beck", UNIONS);
    assert_eq!(
        unions
            .native
            .call("ranked_order", &[heapfix::big(9), heapfix::small(0)])
            .expect("runs"),
        Value::Int(-1),
        "`Big` sorts below `Small` because \"Big\" < \"Small\", whatever order they are declared in"
    );
    let records = Both::over("records.beck", RECORDS);
    assert_eq!(
        records
            .native
            .call("key_order", &[heapfix::key(1, 0), heapfix::key(0, 1)])
            .expect("runs"),
        Value::Int(-1),
        "`name` decides before `score` because \"name\" < \"score\", and `score` is declared first"
    );
}

/// A program with no object in it gets the module it got before there was a heap.
///
/// Not a preference: `docs/93` §93.5's numbers were measured against a module with no allocator, no
/// globals and nothing on the wire but the arguments, and a heap that appeared in every module
/// would have quietly changed what those numbers are about.
#[test]
fn a_program_with_no_object_has_no_arena() {
    let scalar = beck_llvm::module(&compile("arithmetic.beck", ARITHMETIC));
    assert!(
        !scalar.ir.contains("@malloc") && !scalar.ir.contains("beck.alloc"),
        "a program of pure arithmetic must not reserve a heap"
    );
    assert!(scalar.heap.is_empty(), "and must have no layout at all");

    let heaped = beck_llvm::module(&compile("records.beck", RECORDS));
    assert!(
        heaped.ir.contains("call ptr @malloc") && heaped.ir.contains("@\"beck.alloc\""),
        "a program with a record in it reserves one"
    );
    assert_eq!(
        heaped.heap.layouts().count(),
        4,
        "Point, Key, Weighed and Segment"
    );
}

/// The literal pool is a function of the program, and of nothing else.
///
/// Four things have to hold for a literal to be an offset a compiled instruction can *contain*
/// rather than something built at run time.
///
/// **The survey decides it, before any body is emitted.** That is the discriminating assertion
/// here, and it is what a `case "one":` nearly broke: a pattern's constant is not an expression,
/// so the walk that collects these reaches it only because it was told to. Deleting that line
/// leaves a pool that emission fills in as it goes — which happens to work, and makes the pool a
/// function of the fixed point rather than of the program.
///
/// Then: it is the same twice, so the module is still the same bytes twice
/// (`the_generated_module_is_a_function_of_the_program`); the offsets are consecutive and aligned;
/// and a program with no literal in it has no pool, so nothing is added to the wire for a program
/// that never asked for text.
#[test]
fn the_literal_pool_is_a_function_of_the_program() {
    let program = compile("text.beck", TEXT);

    // Surveyed and not emitted: this heap has never seen a body.
    let mut surveyed = beck_llvm::Heap::new();
    beck_llvm::heap::survey(&program, &mut surveyed);
    let surveyed: Vec<String> = surveyed.strings().map(|(_, s, _)| s.to_string()).collect();
    for literal in ["hello, ", "!", "yes", "«", "»", "one", "two"] {
        assert!(
            surveyed.iter().any(|s| s == literal),
            "the survey should have found {literal:?}, and found {surveyed:?}"
        );
    }

    let a = beck_llvm::module(&program);
    let b = beck_llvm::module(&program);
    assert_eq!(
        a.heap.strings().count(),
        surveyed.len(),
        "emitting must not discover a literal the survey missed"
    );

    let pool = |m: &beck_llvm::Module| -> Vec<(String, u64)> {
        m.heap
            .strings()
            .map(|(_, s, at)| (s.to_string(), at))
            .collect()
    };
    assert_eq!(pool(&a), pool(&b), "the pool has to be the same twice");
    assert_eq!(a.ir, b.ir, "and therefore so does the module");

    // Consecutive, word-aligned and starting past the reserved first word — which is what makes an
    // offset a constant the emitter can write into an instruction.
    let mut expect = 8u64;
    for (s, at) in pool(&a) {
        assert_eq!(at, expect, "{s:?} should be at {expect}");
        expect += 16 + (s.len() as u64).next_multiple_of(8);
    }
    assert_eq!(a.heap.pool_bytes(), expect - 8);

    // The host writes it in front of every request, so a call whose arguments are all scalars still
    // carries it — a definition taking two `Int`s may still compare one against a literal.
    let (_, blob) = a
        .heap
        .encode_args(&[Value::Int(1)], &[Repr::Int])
        .expect("encodes");
    assert_eq!(blob.len() as u64, 8 + a.heap.pool_bytes());

    // …and a program with no literal in it has none of this.
    let scalar = beck_llvm::module(&compile("arithmetic.beck", ARITHMETIC));
    assert_eq!(scalar.heap.strings().count(), 0);
    assert_eq!(scalar.heap.pool_bytes(), 0);
    assert!(
        scalar
            .heap
            .encode_args(&[Value::Int(1)], &[Repr::Int])
            .expect("encodes")
            .1
            .is_empty(),
        "a program of pure arithmetic still puts nothing on the wire"
    );
}

/// The arena is bounded, and running out is a **message** rather than whatever the machine does.
///
/// The one failure a compiled program has that the evaluator does not, so there is nothing to
/// compare it against: what is asserted is that it is a diagnostic.
#[test]
fn running_out_of_heap_is_a_diagnostic() {
    let _ = toolchain!();
    let both = Both::over("unions.beck", UNIONS);
    // A `Leaf` and a `Node` per step, five words in all, so 256 MiB is gone before eight million.
    let err = both
        .native
        .call("chain", &[Value::Int(8_000_000), heapfix::leaf(0)])
        .expect_err("eight million nodes is more than the arena holds");
    assert!(
        err.message.contains("used all") && err.message.contains("MiB of its heap"),
        "{err}"
    );

    // Text runs out the same way, and through a different allocation: `beck.str.alloc` sizes an
    // object the program never named, so its failure has to reach the same cell the constructor's
    // does. `repeat` builds `n` strings of growing length, which is `O(n²)` bytes — 30,000 copies
    // of a 16-byte string is well past the arena (§104.6 is why that is quadratic and not a bug).
    let text = Both::over("text.beck", TEXT);
    let err = text
        .native
        .call(
            "repeat",
            &[
                Value::str_("the seventeen ok"),
                Value::Int(200_000),
                Value::str_(""),
            ],
        )
        .expect_err("two hundred thousand growing copies is more than the arena holds");
    assert!(
        err.message.contains("used all") && err.message.contains("MiB of its heap"),
        "{err}"
    );
}

/// What the heap costs per object does not grow with the number of objects.
///
/// A shape gate with **no clock in it** (`AGENTS.md`): `chain(n)` builds exactly `n` `Node`s and
/// `n` `Leaf`s, so the arena it leaves behind is a known number of bytes at every `n`. A layout
/// that grew, a `with` that copied the spine, or an allocator that rounded up by a fraction would
/// all show here, and none of them needs the program to be timed.
#[test]
fn the_arena_costs_the_same_per_object_at_every_size() {
    let _ = toolchain!();
    let both = Both::over("unions.beck", UNIONS);
    // A `Leaf` is a tag and a field; a `Node` is a tag and two. Five words a step, plus the
    // reserved first word and the argument the chain started from.
    let expect = |n: usize| heap_bytes(2) + n * 5 * 8;
    let mut sizes = Vec::new();
    for n in [100usize, 800] {
        let (_, bytes) = both
            .native
            .call_sized("chain", &[Value::Int(n as i64), heapfix::leaf(0)])
            .expect("runs");
        sizes.push((n, bytes));
        assert_eq!(
            bytes,
            expect(n),
            "chain({n}) should leave {} bytes of arena and left {bytes}",
            expect(n)
        );
    }
    let (small, big) = (sizes[0], sizes[1]);
    println!(
        "chain({}) used {} bytes and chain({}) used {} — {} bytes an element at both sizes",
        small.0,
        small.1,
        big.0,
        big.1,
        (big.1 - small.1) / (big.0 - small.0)
    );
}

/// What a slice costs does not grow with the string it is taken from.
///
/// A shape gate with **no clock in it** (`AGENTS.md`), and the one this backend most needed: text
/// is where `docs/70` §70.2 found a quadratic, and the loop it found it in — walk a string by
/// character index, take one character each step — is `walked`. One character is 24 bytes here (two
/// header words and one padded byte), so `n` steps are `24n` bytes and a `str_slice` that copied
/// what it was taken *from* would leave `O(n²)`.
///
/// The second assertion is the one the first cannot make: `first` takes one character out of
/// strings of two very different lengths and has to leave the same arena behind, because the answer
/// is the same size. That is `str_slice` costing its *answer* rather than its input.
#[test]
fn a_slice_costs_its_answer_and_not_the_string_it_came_from() {
    let _ = toolchain!();
    let both = Both::over("text.beck", TEXT);
    // Two header words and the bytes padded to a word: one ASCII character is three words.
    const PER_CHARACTER: usize = 24;
    let mut sizes = Vec::new();
    for n in [200usize, 1600] {
        let s = Value::str_("x".repeat(n));
        let (_, bytes) = both
            .native
            .call_sized("walked", &[s.clone(), Value::Int(0), Value::str_("")])
            .expect("runs");
        // The pool, the reserved word, the string itself and the empty accumulator are the
        // arguments; everything past them is what the loop allocated.
        let arguments = both
            .native
            .module()
            .heap
            .encode_args(
                &[s, Value::Int(0), Value::str_("")],
                &[Repr::Str, Repr::Int, Repr::Str],
            )
            .expect("encodes")
            .1
            .len();
        let allocated = bytes - arguments;
        assert_eq!(
            allocated,
            n * PER_CHARACTER,
            "walked over {n} characters should allocate {} bytes and allocated {allocated}",
            n * PER_CHARACTER
        );
        sizes.push((n, allocated));
    }
    let (small, big) = (sizes[0], sizes[1]);
    println!(
        "walked({}) allocated {} bytes and walked({}) allocated {} — {} bytes a character at both \
         sizes",
        small.0,
        small.1,
        big.0,
        big.1,
        (big.1 - small.1) / (big.0 - small.0)
    );

    // …and one character out of a long string costs what one character costs.
    let mut one = Vec::new();
    for n in [200usize, 1600] {
        let s = Value::str_("y".repeat(n));
        let arguments = both
            .native
            .module()
            .heap
            .encode_args(std::slice::from_ref(&s), &[Repr::Str])
            .expect("encodes")
            .1
            .len();
        let (_, bytes) = both.native.call_sized("first", &[s]).expect("runs");
        one.push(bytes - arguments);
    }
    assert_eq!(
        one[0], one[1],
        "taking one character should cost the same out of a short string and a long one"
    );
    assert_eq!(one[0], PER_CHARACTER);
}

/// The reserved first word, plus one object of `words` words: what the host writes for an argument.
fn heap_bytes(words: usize) -> usize {
    8 + words * 8
}

// -------------------------------------------------------------------------------------------
// Refusal, and the shape of it
// -------------------------------------------------------------------------------------------

/// Everything outside the subset is refused **by name, with a reason**.
///
/// The failure this guards against is not a wrong answer: it is a backend that quietly compiled
/// something it does not understand. So the assertion is two-sided — the refusals are there, and
/// the one definition that should have compiled did.
#[test]
fn what_cannot_be_compiled_is_refused_by_name_and_with_a_reason() {
    let _ = toolchain!();
    let both = Both::over("refused.beck", REFUSED);

    for (name, expect) in [
        ("takes_a_list", "parameter `xs` is a `list`"),
        (
            "renders_a_number",
            "`str` converts between text and a number",
        ),
        ("is_generic", "generic over T"),
        (
            "reads_the_clock",
            "`now` is not one of the scalar primitives",
        ),
        ("calls_something_refused", "calls `takes_a_list`"),
    ] {
        let reason = both
            .refusal(name)
            .unwrap_or_else(|| panic!("`{name}` was compiled natively, and should not have been"));
        assert!(
            reason.contains(expect),
            "the reason for refusing `{name}` should mention {expect:?}, and is {reason:?}"
        );
    }

    // …and the definition that has nothing wrong with it still compiles, so this program did not
    // pass by refusing everything.
    assert_eq!(both.compiled(), vec!["scalar_and_fine".to_string()]);
    both.agree("scalar_and_fine", &singles(&ints(0x5eed_0007, 20)));

    // A refusal is not a silent fallback either: asking the artefact for a refused definition is
    // an error rather than an evaluator call wearing a native backend's name.
    let err = both
        .native
        .call("takes_a_list", &[Value::Int(1)])
        .expect_err("a refused definition is not callable natively");
    assert!(err.message.contains("did not compile natively"), "{err}");
}

/// What the heap does **not** reach, asserted as an absence.
///
/// `docs/101` §101.5 lists what is not built — collections, closures and every effect — and a
/// list in prose goes stale where a list with a test attached cannot (`docs/83` §83.7). Each of
/// these goes red the day its row starts compiling, which is the day the row should be deleted.
///
/// The text row was deleted that way: `docs/104` gave a `Str` a layout, so what is left of text
/// here is the primitives that answer with a collection or read a Unicode table, one row each.
#[test]
fn what_the_heap_does_not_reach_is_refused_by_name() {
    let program = compile("still-refused.beck", STILL_REFUSED);
    let module = beck_llvm::module(&program);
    for (name, expect) in [
        ("takes_a_list", "a `list`"),
        (
            "renders_a_number",
            "`str` converts between text and a number",
        ),
        ("splits_a_string", "`str_split` answers with a list"),
        ("upcases", "`str_upper` is Unicode case mapping"),
        ("takes_a_boxed", "whose field `items` is a `list`"),
        ("matches_a_held", "whose field `values` is a `list`"),
        ("is_generic", "generic over T"),
        (
            "reads_the_clock",
            "`now` is not one of the scalar primitives",
        ),
        ("calls_something_refused", "which does not compile"),
    ] {
        let reason = module
            .refusals
            .iter()
            .find(|r| &*r.name == name)
            .unwrap_or_else(|| panic!("`{name}` compiled, and this list says it cannot"))
            .reason
            .clone();
        assert!(
            reason.contains(expect),
            "the reason for refusing `{name}` should mention {expect:?}, and is {reason:?}"
        );
    }
    // …and the control, because a list of refusals with nothing on the other side of it would pass
    // against a backend that refused everything.
    assert_eq!(
        module
            .functions
            .iter()
            .map(|f| f.name.to_string())
            .collect::<Vec<_>>(),
        vec!["names_it".to_string(), "scalar_and_fine".to_string()]
    );
}

/// The refusal a call *inherits*, and the fixed point that computes it.
///
/// `calls_something_refused` is refused because of what it calls, not because of what it is, and
/// that has to survive a definition being dropped in a later round than the one that dropped its
/// callee. Mutual recursion is the case that makes it a fixed point rather than one pass.
#[test]
fn a_refusal_travels_to_whoever_calls_it() {
    let _ = toolchain!();
    let src = r#"
## A `list` rather than a record or a `Str`: `docs/101` gave the record a layout and `docs/104`
## gave text one, and what a refusal has to travel *from* is something the heap still does not
## reach.
def bottom(xs: list[Int]) -> Int:
    return list_len(xs)

def middle(n: Int) -> Int:
    return bottom([n])

def top(n: Int) -> Int:
    return middle(n) + 1

## Mutually recursive, and both must go: `ping` is only refusable through `pong` and the other way
## round, so a single pass in either direction keeps one of them.
def ping(n: Int) -> Int:
    if n == 0:
        return bottom([])
    return pong(n - 1)

def pong(n: Int) -> Int:
    return ping(n - 1)

## Mutually recursive and perfectly fine, so the fixed point is not just refusing every cycle.
def tick(n: Int) -> Int:
    if n <= 0:
        return 0
    return tock(n - 1)

def tock(n: Int) -> Int:
    return tick(n - 1) + 1
"#;
    let both = Both::over("travels.beck", src);
    for name in ["bottom", "middle", "top", "ping", "pong"] {
        assert!(
            both.refusal(name).is_some(),
            "`{name}` should have been refused, and the module compiled it"
        );
    }
    let compiled = both.compiled();
    assert!(
        compiled.contains(&"tick".to_string()) && compiled.contains(&"tock".to_string()),
        "a sound mutually recursive pair should compile, and {compiled:?} did"
    );
}

// -------------------------------------------------------------------------------------------
// The two things compiled code does that the tree-walker cannot
// -------------------------------------------------------------------------------------------

/// A tail call is a jump, so a loop written as recursion has no depth at all.
///
/// `docs/27` §27.2 makes this a property of the *language*, and until now it was a property of one
/// backend's trampoline. The numbers are the ones that report uses: a shallow recursion and one
/// forty times deeper have to cost the same, and here they must also be *possible* — the
/// tree-walker refuses past [`beck_eval::DEFAULT_MAX_DEPTH`], and compiled code has no such
/// ceiling because it spends no frame.
#[test]
fn a_tail_call_costs_nothing_and_has_no_ceiling() {
    let _ = toolchain!();
    let both = Both::over("recursion.beck", RECURSION);

    // Where the evaluator can still answer, the two agree.
    let (walked, compiled) = both.call("sum_to", &[Value::Int(1_000), Value::Int(0)]);
    assert_eq!(walked, compiled);

    // And where it cannot, the native backend can. Fifty million tail calls is far past anything
    // a trampoline with a fuel budget will do, and past any host stack.
    let deep = both
        .native
        .call("sum_to", &[Value::Int(50_000_000), Value::Int(0)])
        .expect("fifty million tail calls");
    assert_eq!(deep, Value::Int(1_250_000_025_000_000));

    // The cross-arity tail call too, which is what `tailcc` buys over the C convention.
    let deep = both
        .native
        .call("drain", &[Value::Int(20_000_000), Value::Int(0)])
        .expect("twenty million tail calls into a function of another arity");
    assert_eq!(deep, Value::Int(40_000_000));
}

/// A compiled program that will not stop is stopped, and the call says so.
///
/// There is no fuel in machine code, so this is the coarse thing that stands in for it. It is
/// tested rather than asserted because the mechanism is a watchdog thread and a `kill`, and both
/// of those are the kind of thing that works until somebody reorders a lock.
#[test]
fn a_compiled_program_that_will_not_stop_is_stopped() {
    let toolchain = toolchain!();
    let program = compile(
        "forever.beck",
        r#"
def forever(n: Int) -> Int:
    if n == 0:
        return 0
    return forever(n)
"#,
    );
    let artifact =
        Artifact::build_bounded(&program, toolchain, None, Some(Duration::from_millis(500)))
            .expect("clang accepts the module");
    let started = std::time::Instant::now();
    let err = artifact
        .call("forever", &[Value::Int(1)])
        .expect_err("a loop that never ends must not answer");
    assert!(
        err.message.contains("did not answer within"),
        "the message should name the limit, and is {:?}",
        err.message
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the watchdog took {:?}",
        started.elapsed()
    );
}

/// A recursion that is *not* in tail position has no ceiling here, and the message says so.
///
/// This is the one place the native backend is **weaker** than the tree-walker, and it is a test
/// rather than a paragraph because that is the difference between a known gap and a forgotten one.
/// `docs/adr/0007` records that `beck-eval` replaced a `SIGSEGV` with a counted ceiling; compiled
/// code spends a real frame per level and counts nothing, so the abort is back — and all a host
/// can do is notice that the worker stopped and say why it probably did. §93.7 is what closing it
/// would cost.
#[test]
fn a_native_recursion_without_a_ceiling_says_what_happened() {
    let toolchain = toolchain!();
    let program = compile(
        "deep.beck",
        r#"
## Not a tail call, and not one LLVM can make into a loop either: the caller branches on what the
## callee answered, so there is something left to do after the call and a frame to do it in.
def deep(n: Int) -> Int:
    if n == 0:
        return 0
    inner = deep(n - 1)
    if inner > 1000000000:
        return inner
    return inner + 1
"#,
    );
    let artifact =
        Artifact::build_bounded(&program, toolchain, None, Some(Duration::from_secs(20)))
            .expect("clang accepts the module");

    // A hundred thousand frames is fine, and is already twenty-five times the tree-walker's
    // ceiling — which is the half of this that is an *advantage*.
    assert_eq!(
        artifact
            .call("deep", &[Value::Int(100_000)])
            .expect("answers"),
        Value::Int(100_000)
    );

    // A billion is not. Whether the stack runs out or the watchdog gets there first depends on the
    // machine, and both are honest answers; what must not happen is the standard library's
    // "failed to fill whole buffer" reaching a person.
    let err = artifact
        .call("deep", &[Value::Int(1_000_000_000)])
        .expect_err("a billion frames is not a thing a stack has");
    assert!(
        err.message.contains("stopped without answering")
            || err.message.contains("did not answer within"),
        "the message has to say what happened, and is {:?}",
        err.message
    );
}

// -------------------------------------------------------------------------------------------
// The seam
// -------------------------------------------------------------------------------------------

/// `Backend::function` is handed an *expression*, and has to recognise the ones it compiled.
///
/// This is the property that makes this a backend rather than a side tool: the runtime never says
/// a name, so a native backend that could only be called by name would never be reached.
#[test]
fn the_seam_recognises_a_compiled_definition() {
    let _ = toolchain!();
    let program = compile("recursion.beck", RECURSION);
    let evaluator = beck_eval::backend_for(program.clone());
    let native = Native::build(&program, evaluator.clone(), Some(LIMIT))
        .expect("builds")
        .expect("a toolchain, checked above");

    assert_eq!(native.name(), "native");
    assert!(
        native.compiled(&program.defs["fib"].body),
        "`fib` compiled, so the seam has to recognise its body"
    );

    let f = native
        .function(&program.defs["fib"].body)
        .expect("prepares");
    assert_eq!(f(vec![Value::Int(20)]).expect("answers"), Value::Int(6765));

    // A backend that wraps a tree-walker inherits its stack requirement — the trap `docs/27` §27.2
    // records, one layer along. Compiled code needs none of it; the half behind it needs all of it.
    assert_eq!(native.stack_bytes(), beck_eval::STACK_BYTES);
}

/// What the native half cannot answer, the fallback does — and the caller can tell which.
#[test]
fn what_is_not_compiled_falls_back_and_says_so() {
    let _ = toolchain!();
    let program = compile("refused.beck", REFUSED);
    let evaluator = beck_eval::backend_for(program.clone());
    let native = Native::build(&program, evaluator, Some(LIMIT))
        .expect("builds")
        .expect("a toolchain, checked above");

    let refused = &program.defs["takes_a_list"].body;
    assert!(!native.compiled(refused));

    let f = beck_eval::on_the_evaluator_stack(|| native.function(refused).expect("prepares"));
    let xs = Value::List(Arc::new(vec![Value::Int(2), Value::Int(3)]));
    let got = beck_eval::on_the_evaluator_stack(|| f(vec![xs]).expect("the fallback answers"));
    assert_eq!(got, Value::Int(2));
}

// -------------------------------------------------------------------------------------------
// Real programs
// -------------------------------------------------------------------------------------------

/// The differential over code somebody else wrote for another purpose.
///
/// `sicp/ch1.beck` is the expressiveness benchmark and `awfy/mandelbrot.beck` is a port of a
/// published benchmark; neither was written with a native backend in mind, and between them they
/// exercise the scalar subset the way real code does rather than the way a test does. The
/// arguments are small because these definitions are bounded by their input — see the module
/// comment.
#[test]
fn the_two_backends_agree_on_the_benchmark_and_the_book() {
    let _ = toolchain!();

    let both = Both::over("sicp/ch1.beck", include_str!("../../../sicp/ch1.beck"));
    let small: Vec<i64> = (0..16).collect();
    let mut compared = 0;
    for name in [
        "factorial",
        "factorial_iterative",
        "fib",
        "count_change",
        "ex_1_11_recursive",
        "ex_1_11_iterative",
        "square",
        "even",
        "smallest_divisor",
        "is_prime",
        "inc",
        "identity",
        "cube",
        "count_up",
    ] {
        compared += both.agree(name, &singles(&small));
    }
    for name in ["gcd", "fast_expt_iterative", "divides"] {
        compared += both.agree(name, &pairs(&small));
    }
    // Pascal's triangle, inside its domain. `pascal(0, 1)` is not a value the function has an
    // answer for — it recurses without bottoming out — and neither backend survives it: the
    // evaluator reaches its depth ceiling and the worker exhausts its stack. §93.7 records that
    // asymmetry rather than this suite papering over it.
    let triangle: Vec<Vec<Value>> = (0..14)
        .flat_map(|row| (0..=row).map(move |col| vec![Value::Int(row), Value::Int(col)]))
        .collect();
    compared += both.agree("pascal", &triangle);
    // Newton's method, on the arguments it converges for: a negative or a NaN never satisfies
    // `good_enough`, and neither backend would stop.
    let positives: Vec<Vec<Value>> = (1..40)
        .map(|n| vec![Value::float(f64::from(n) / 3.0)])
        .collect();
    compared += both.agree("sqrt", &positives);
    compared += both.agree("square_real", &positives);

    let mandel = Both::over(
        "awfy/mandelbrot.beck",
        include_str!("../../../awfy/mandelbrot.beck"),
    );
    let bytes: Vec<i64> = (0..256).step_by(7).collect();
    compared += mandel.agree("xor_of", &pairs(&bytes));
    compared += mandel.agree(
        "shifted_left",
        &bytes
            .iter()
            .flat_map(|n| (0..9).map(move |by| vec![Value::Int(*n), Value::Int(by)]))
            .collect::<Vec<_>>(),
    );
    // The benchmark's inner loop, over the square of the plane it actually walks.
    let mut escapes = Vec::new();
    for x in 0..24 {
        for y in 0..24 {
            escapes.push(vec![
                Value::float(0.0),
                Value::float(0.0),
                Value::float(0.0),
                Value::float(2.0 * f64::from(x) / 24.0 - 1.5),
                Value::float(2.0 * f64::from(y) / 24.0 - 1.0),
                Value::Int(0),
            ]);
        }
    }
    compared += mandel.agree("escapes", &escapes);

    println!("{compared} calls over the book and the benchmark, and both backends agreed");
}

/// Every program in the corpus assembles, whatever it turns out to contain.
///
/// The failure this catches is the emitter producing IR that `clang` rejects — which is not a
/// wrong answer but a build that does not happen, and which no differential can find because a
/// module that will not assemble has nothing to compare. `docs/34` §34's "`beck doc` runs over the
/// corpus" is the same gate for the same reason.
#[test]
fn every_corpus_program_produces_a_module_llvm_accepts() {
    let toolchain = toolchain!();
    // The front end and the emitter both recurse on nesting, and a test thread's stack is not the
    // one a `beck` process gives them. Every entry point in the workspace goes through here.
    beck_eval::on_the_evaluator_stack(|| corpus(toolchain));
}

fn corpus(toolchain: beck_llvm::Toolchain) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .to_path_buf();

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for dir in ["corpus", "awfy", "clbg", "sicp", "examples", "lib"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "beck") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(files.len() > 40, "only found {} programs", files.len());

    let mut compiled = 0;
    let mut assembled = 0;
    for path in &files {
        let name = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let src = std::fs::read_to_string(path).expect("readable");
        let (placed, diags, _) = beck_core::compile_or_library_str(&name, &src);
        // A library that imports another module does not compile on its own, and this is not the
        // suite that checks that: `stdlib.rs` is.
        let Some(placed) = placed.filter(|_| !diags.has_errors()) else {
            continue;
        };
        let program = placed.program;
        let module = beck_llvm::module(&program);
        compiled += module.functions.len();
        assembled += 1;
        // Assembled, not merely emitted: `-c` stops before the link, because what is being checked
        // is that LLVM accepts the IR and not that a program with no `main`-worthy content links.
        Artifact::build_bounded(&program, toolchain.clone(), None, Some(LIMIT))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    println!("{assembled} programs assembled, {compiled} definitions compiled to native code");
    assert!(compiled > 100, "only {compiled} definitions compiled");
}

/// The same module, twice, byte for byte.
///
/// `beck build` has to be able to put a `.ll` in an artefact store and a second build has to
/// produce the same one, so the emitter may not depend on iteration order of anything hashed.
#[test]
fn the_generated_module_is_a_function_of_the_program() {
    let program = compile("recursion.beck", RECURSION);
    let a = beck_llvm::module(&program);
    let b = beck_llvm::module(&program);
    assert_eq!(a.ir, b.ir);
    // …and of the program rather than of the process: a second compilation of the same source
    // must agree with the first.
    let again = compile("recursion.beck", RECURSION);
    assert_eq!(a.ir, beck_llvm::module(&again).ir);
    assert!(
        a.ir.contains("musttail call tailcc"),
        "a tail call has to be emitted as one"
    );
}

/// The refusal reasons and the signature rendering, without a toolchain.
///
/// Everything above needs `clang`; this does not, so the part of the backend that decides *what*
/// to compile is still gated on a machine that has none.
#[test]
fn the_subset_is_decided_without_a_toolchain() {
    let program = compile("refused.beck", REFUSED);
    let module = beck_llvm::module(&program);
    assert_eq!(module.functions.len(), 1);
    assert_eq!(&*module.functions[0].name, "scalar_and_fine");
    assert_eq!(module.functions[0].params, vec![Repr::Int]);
    assert_eq!(module.functions[0].ret, Repr::Int);
    assert_eq!(module.functions[0].index, 0);
    assert!(module.refusals.len() >= 5, "{:?}", module.refusals);
    for refusal in &module.refusals {
        assert!(
            !refusal.reason.is_empty(),
            "`{}` was refused without a reason",
            refusal.name
        );
    }
}
