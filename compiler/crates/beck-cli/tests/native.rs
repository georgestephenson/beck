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
use support::clofix::{self, CLOSURES};
use support::failfix;
use support::genfix::{self, GENERIC};
use support::heapfix::{self, RECORDS, STILL_REFUSED, UNIONS};
use support::hostfix::{self, Stated, EFFECTS};
use support::libfix::{self, RUNTIME};
use support::listfix::{self, LISTS, PATTERNS};
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
/// only when nothing has been folded. The message is the claim; §93.15 says so.
type Outcome = Result<Value, String>;

fn outcome(r: Result<Value, beck_core::ExecError>) -> Outcome {
    r.map_err(|e| e.message)
}

/// Both backends over one program, so a test says what it means in one line per case.
struct Both {
    program: Arc<Program>,
    native: Artifact,
    evaluator: Arc<dyn Backend>,
    /// The stated host, when this program has effects. Rewound before each backend is driven, so
    /// that the *n*th question of a call gets the same answer whichever backend asked it.
    stated: Option<Arc<Stated>>,
}

impl Both {
    fn over(name: &str, src: &str) -> Both {
        let program = compile(name, src);
        Both {
            native: artifact(&program),
            evaluator: beck_eval::backend_for(program.clone()),
            program,
            stated: None,
        }
    }

    /// The same, with both backends answering their host effects from one stated host.
    ///
    /// This is what makes a differential over `now()` mean anything: the two are asked the same
    /// question and told the same answer, so what is left to compare is what the *backends* did
    /// with it.
    fn answering(name: &str, src: &str, atoms: Arc<Stated>) -> Both {
        let program = compile(name, src);
        let toolchain = beck_llvm::Toolchain::find().expect("checked by the caller");
        let native = Artifact::build_bounded(&program, toolchain, None, Some(LIMIT))
            .expect("clang accepts the module")
            .answering(atoms.clone());
        let evaluator: Arc<dyn Backend> =
            Arc::new(beck_eval::Evaluator::new(program.clone()).answering(atoms.clone()));
        Both {
            native,
            evaluator,
            program,
            stated: Some(atoms),
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
        if let Some(stated) = &self.stated {
            stated.rewind();
        }
        let evaluated = beck_eval::on_the_evaluator_stack(|| {
            let f = self.evaluator.function(&def.body).expect("prepares");
            outcome(f(args.to_vec()))
        });
        if let Some(stated) = &self.stated {
            stated.rewind();
        }
        (evaluated, outcome(self.native.call(name, args)))
    }

    /// What the *evaluator* answers, for a test that needs a value only it can build.
    ///
    /// The one caller is the view differential, which hands a compiled definition a tree the
    /// tree-walker made: an argument this backend cannot construct is exactly the argument worth
    /// passing, because the encoder is the half of the boundary nothing else exercises.
    fn evaluated(&self, name: &str, args: &[Value]) -> Result<Value, String> {
        self.call(name, args).0
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
        "at",
    ] {
        compared += both.agree(name, &textfix::pairs(&ss));
    }
    compared += both.agree("thrice", &textfix::singles(&ss));
    compared += both.agree("cut", &textfix::slices(&ss));
    compared += both.agree("count_of", &textfix::with_char(&ss));
    compared += both.agree(
        "at_or",
        &textfix::pairs(&ss)
            .into_iter()
            .map(|mut t| {
                t.push(Value::Int(-1));
                t
            })
            .collect::<Vec<_>>(),
    );
    compared += both.agree("repeat", &textfix::repeats(&ss));

    // Building text out of something that is not text, and taking an `Option` apart without a
    // `match` — the primitives `docs/93`'s and `docs/93`'s layouts made reachable.
    compared += both.agree("shown", &textfix::integers());
    compared += both.agree(
        "shown_bool",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    compared += both.agree("shown_str", &textfix::singles(&ss));
    compared += both.agree(
        "repeated",
        &textfix::repeats(&ss)
            .into_iter()
            .map(|mut t| {
                t.truncate(2);
                t
            })
            .collect::<Vec<_>>(),
    );
    compared += both.agree("glued", &textfix::joins(&ss));
    for name in ["or_else", "present"] {
        compared += both.agree(
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
    compared += both.agree(
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

    // The trim, over the strings written for it: every shape a run of whitespace can have, one of
    // each **kind** of whitespace character the encoding has, and `U+200B` as the control, since it
    // is in the space block and is not `White_Space`.
    let sp = textfix::spaced();
    for name in ["trimmed", "trimmed_len", "blank"] {
        compared += both.agree(name, &textfix::singles(&sp));
    }
    compared += both.agree("trimmed_up", &textfix::repeats(&sp));

    // …and then every code point Rust calls whitespace, four ways each, plus the four that look
    // like whitespace and are not. The list is `char::is_whitespace` itself, so this asks about a
    // new one the day Rust learns about it.
    let ws = textfix::every_whitespace();
    for name in ["trimmed", "trimmed_len", "blank"] {
        compared += both.agree(name, &ws);
    }

    // Splitting, which answers with a **list** — so the differential reads the length *and* an
    // element, because a backend that counted the pieces correctly and allocated them wrongly
    // passes the first and fails the second.
    let cuts = textfix::separators(&ss);
    for name in ["parts", "split_len", "rejoined"] {
        compared += both.agree(name, &cuts);
    }
    compared += both.agree("split_at", &textfix::indexed(&cuts));
    for name in ["letters", "letter_count"] {
        compared += both.agree(name, &textfix::singles(&ss));
    }
    compared += both.agree("letter_at", &textfix::indexed(&textfix::singles(&ss)));

    println!("{compared} text calls compared, and both backends agreed on every one");
}

/// The fifteen primitives that are a call into the runtime library (`docs/93` §93.12).
///
/// What this compares is not two implementations of a digest — there is one, and both backends
/// reach it — but the **ABI** around it: the mark going in, the outcome record coming back, a
/// `Str` the library allocated being read by compiled code, and a failure the library described
/// being built as a declared value by the emitter. Every one of those is a place the answer can be
/// wrong while the library is right.
#[test]
fn the_two_backends_agree_on_the_runtime_library() {
    let _ = toolchain!();
    let both = Both::over("runtime.beck", RUNTIME);
    let ss = libfix::strings();
    let mut compared = 0;

    // The pure ones, over text chosen for the layout rather than for the primitive.
    for name in ["hashed", "hexed", "b64", "shout", "whisper", "fingerprint"] {
        compared += both.agree(name, &textfix::singles(&ss));
    }
    compared += both.agree("same_digest", &textfix::pairs(&ss));
    compared += both.agree("round_trip", &textfix::singles(&ss));
    compared += both.agree("twice_over", &textfix::singles(&ss));
    compared += both.agree("counted", &textfix::singles(&ss));

    // The decoders, each asked every string both are asked — including the ones each refuses, so
    // the raised value is compared and not only the successful answer.
    let encoded = libfix::encoded();
    for name in ["unhexed", "read_hex", "unb64", "read_b64"] {
        compared += both.agree(name, &textfix::singles(&encoded));
    }

    // Identifiers, where the answer is a *normalisation* and every spelling has to reach one.
    let ids = libfix::identifiers();
    for name in ["canonical", "which_version", "read_uuid"] {
        compared += both.agree(name, &textfix::singles(&ids));
    }

    // Rust's parser, and the `Option` the emitter builds around a status word.
    let numerals = libfix::numerals();
    for name in ["numbered", "defaulted"] {
        compared += both.agree(name, &textfix::singles(&numerals));
    }

    // The calendar, in both directions.
    compared += both.agree("stamped", &textfix::singles(&libfix::instants()));
    for name in ["instant", "read_time"] {
        compared += both.agree(name, &textfix::singles(&libfix::stamps()));
    }

    // `str_replace`, whose third argument is what made it a refusal: the answer's size is a
    // function of how many times the needle occurs.
    let mut swaps = Vec::new();
    for s in &ss {
        for needle in ["", "a", "abc", "é", "\0"] {
            for to in ["", "-", "longer"] {
                swaps.push(vec![s.clone(), Value::str_(needle), Value::str_(to)]);
            }
        }
    }
    compared += both.agree("swapped", &swaps);

    // The one primitive whose argument is a `secret[Str]`, which is the only place compiled code
    // opens one (adr/0014).
    let mut keyed = Vec::new();
    for key in ["", "k1", "a longer secret", "🎈"] {
        for message in &ss {
            keyed.push(vec![libfix::secret(key), message.clone()]);
        }
    }
    compared += both.agree("mac", &keyed);

    // The set has to *contain* the failures, or this passed by never reaching a raise.
    let (walked, compiled) = both.call("unhexed", &[Value::str_("zz")]);
    assert_eq!(walked, compiled);
    assert!(walked.is_err(), "`zz` is not hex");
    let (walked, compiled) = both.call("canonical", &[Value::str_("nope")]);
    assert_eq!(walked, compiled);
    assert!(walked.is_err(), "`nope` is not a UUID");
    let (walked, _) = both.call("read_hex", &[Value::str_("zz")]);
    let caught = walked.expect("a `try:` answers a value");
    assert_eq!(
        caught.variant(),
        Some("Err"),
        "the raise should have been caught, not travelled: {caught:?}"
    );

    println!("{compared} runtime-library calls compared, and both backends agreed on every one");
}

/// A runtime-library call costs its answer, and not its answer plus a record.
///
/// A shape gate with **no clock in it** (`AGENTS.md`), and it is the one this design most needs:
/// `beck_prim` writes its two-word outcome record *above* the arena's high-water mark, so the
/// caller reads it and the next allocation writes over it. Below the mark it would be correct and
/// leak sixteen bytes a call — which no differential could see, because every answer would still be
/// right.
///
/// Two sizes, and the *difference* rather than the total: the totals differ by the program's
/// literal pool, and what is being asserted is the per-call cost.
#[test]
fn a_linked_call_costs_its_answer_and_nothing_else() {
    let _ = toolchain!();
    let both = Both::over("runtime.beck", RUNTIME);
    let arena = |n: i64| {
        both.native
            .call_sized("hashes", &[Value::Int(n), Value::str_("")])
            .expect("runs")
            .1
    };
    let (small, big) = (arena(100), arena(900));
    assert_eq!(
        big - small,
        800 * libfix::DIGEST_BYTES,
        "800 more digests should cost 800 answers and nothing else: {small} then {big}"
    );
    println!(
        "hashes(100) left {small} bytes and hashes(900) left {big} — {} bytes a call at both sizes",
        (big - small) / 800
    );
}

/// A program that reaches none of the runtime library's primitives does not link it.
///
/// The archive is Rust's standard library, and linking it takes a program from 16 KiB to 4.9 MiB —
/// so "only when it is called" is a property worth holding rather than an intention. It is also the flag that
/// decides where the *arena* comes from, so a module that got it wrong would either link an
/// archive it never calls or call `beck_prim` with a heap the library does not own.
#[test]
fn a_module_links_the_runtime_library_only_when_it_calls_it() {
    let with = beck_llvm::module(&compile("runtime.beck", RUNTIME));
    assert!(with.links, "a module full of them should link it");
    assert!(
        with.ir.contains("declare i64 @beck_prim("),
        "and declare the call"
    );
    assert!(
        with.ir.contains("@beck_prim_arena"),
        "and take its arena from the library"
    );

    let without = beck_llvm::module(&compile("text.beck", TEXT));
    assert!(
        !without.links,
        "a program of text and arithmetic reaches no runtime-library primitive"
    );
    assert!(
        !without.ir.contains("beck_prim"),
        "so nothing in its module should name one"
    );
    assert!(
        without.ir.contains("declare ptr @malloc"),
        "and its arena is still the C library's"
    );
}

/// `White_Space` is a **closed set**, and this is the assertion that says so with a number.
///
/// `str_trim` is not the table `str_upper` is, and the argument rests on two facts about the host's
/// Unicode data rather than on anything either emitter does: the set
/// is **25 code points**, and **none of them is four bytes long**. Both emitters are written from
/// those facts — a switch over five lead bytes, with no four-byte arm at all — so this is where a
/// Rust upgrade that changed either one goes red, and it goes red *here* rather than as a
/// divergence in a differential that would say only that the two disagreed.
///
/// It runs on any machine, because it compiles nothing: the emitters' correctness against this set
/// is `the_two_backends_agree_on_text`'s, over `textfix::every_whitespace`, which enumerates the
/// same property from the same function.
#[test]
fn the_whitespace_this_backend_knows_is_every_one_rust_does() {
    let space: Vec<char> = (0u32..0x11_0000)
        .filter_map(char::from_u32)
        .filter(|c| c.is_whitespace())
        .collect();
    assert_eq!(
        space.len(),
        25,
        "`White_Space` is 25 code points and both emitters switch over the five lead bytes those \
         need; it is now {}, so `beck.str.ws` has an arm to add or drop: {:?}",
        space.len(),
        space.iter().map(|c| *c as u32).collect::<Vec<_>>()
    );
    let widest = space
        .iter()
        .map(|c| c.len_utf8())
        .max()
        .expect("25 of them");
    assert_eq!(
        widest, 3,
        "no whitespace character is four bytes long, which is why `beck.str.ws` has no four-byte \
         arm — one is now {widest} bytes"
    );
    // And the lead bytes themselves, because that is the shape of the switch rather than a
    // consequence of the two numbers above: a new whitespace character three bytes long behind a
    // sixth lead byte would satisfy both assertions and still be missed.
    let mut leads: Vec<u8> = space
        .iter()
        .map(|c| {
            let mut b = [0u8; 4];
            c.encode_utf8(&mut b).as_bytes()[0]
        })
        .filter(|b| *b >= 0x80)
        .collect();
    leads.sort_unstable();
    leads.dedup();
    assert_eq!(
        leads,
        vec![0xc2, 0xe1, 0xe2, 0xe3],
        "the non-ASCII whitespace lead bytes are the four both emitters switch on"
    );
}

/// Lists, over every backend this machine has.
///
/// The sweep that matters is the **pairs**: a lexicographic comparison can be right for `<` and
/// wrong for `<=`, and one that ran out of elements before it ran out of answer would order `[1]`
/// and `[1, 2]` the wrong way round. Every element kind that is itself an offset is here too —
/// text, a list, a record — because comparing the *words* answers correctly for an `Int` and
/// wrongly for all three.
#[test]
fn the_two_backends_agree_on_lists() {
    let _ = toolchain!();
    let both = Both::over("lists.beck", LISTS);
    let xs = listfix::lists();
    let mut compared = 0;
    for name in ["size", "empty", "flipped", "held"] {
        compared += both.agree(name, &listfix::singles(&xs));
    }
    for name in [
        "below",
        "above",
        "same",
        "differ",
        "not_after",
        "not_before",
    ] {
        compared += both.agree(name, &listfix::pairs(&xs));
    }
    for name in ["nth", "nth_or"] {
        compared += both.agree(name, &listfix::indexed(&xs));
    }
    for name in ["has", "at_of"] {
        compared += both.agree(name, &listfix::searched(&xs));
    }
    compared += both.agree("middle", &listfix::ranges(&xs));
    for name in ["front", "back"] {
        compared += both.agree(name, &listfix::counted(&xs));
    }
    compared += both.agree("three", &[vec![]]);
    compared += both.agree("none_at_all", &[vec![]]);
    // Growing one: the operation, the fork onto a shared block, and the accumulator.
    for name in ["appended", "forked"] {
        compared += both.agree(name, &listfix::searched(&xs));
    }
    for name in ["doubled_up", "sum_of"] {
        compared += both.agree(name, &listfix::singles(&xs));
    }
    compared += both.agree(
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
    compared += both.agree(
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
    compared += both.agree("doubled", &singles(&ints(0x5eed_0031, 12)));
    compared += both.agree(
        "total",
        &xs.iter()
            .map(|v| vec![v.clone(), Value::Int(0), Value::Int(0)])
            .collect::<Vec<_>>(),
    );
    compared += both.agree(
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
        compared += both.agree(name, &listfix::pairs(&ts));
    }
    let ns = listfix::nested();
    for name in ["nested_below", "nested_same"] {
        compared += both.agree(name, &listfix::pairs(&ns));
    }
    compared += both.agree("nested_first", &listfix::singles(&ns));

    // A list inside a record and inside a union.
    let bags: Vec<Value> = xs
        .iter()
        .map(|v| heapfix::record("Bag", &[("items", v.clone()), ("rank", Value::Int(1))]))
        .collect();
    compared += both.agree("bag_items", &listfix::singles(&bags));
    for name in ["bag_below", "bag_same"] {
        compared += both.agree(name, &listfix::pairs(&bags));
    }
    compared += both.agree(
        "rebagged",
        &bags
            .iter()
            .flat_map(|bag| xs.iter().map(move |v| vec![bag.clone(), v.clone()]))
            .collect::<Vec<_>>(),
    );
    compared += both.agree(
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
    compared += both.agree("held_size", &listfix::singles(&holdings));

    println!("{compared} list calls compared, and every backend agreed on every one");
}

/// Maps, over every backend this machine has.
///
/// The sweep that matters is `keyed`: a binary search ends four ways — on the key, below every key,
/// above every key, and **between** two — and the last is the one a window that shrinks wrongly
/// never leaves. And the pairs, because `PMap`'s order is pair by pair and then by length, so a
/// comparison that ran out of entries before it ran out of answer orders a prefix the wrong way.
#[test]
fn the_two_backends_agree_on_maps() {
    let _ = toolchain!();
    let both = Both::over("maps.beck", MAPS);
    let ms = mapfix::maps();
    let mut compared = 0;
    for name in ["size", "names", "totals", "is_nothing", "held"] {
        compared += both.agree(name, &mapfix::singles(&ms));
    }
    for name in [
        "below",
        "above",
        "same",
        "differ",
        "not_after",
        "not_before",
    ] {
        compared += both.agree(name, &mapfix::pairs(&ms));
    }
    for name in ["lookup", "lookup_or", "holds"] {
        compared += both.agree(name, &mapfix::keyed(&ms));
    }
    // Growing one: the three operations, the fork onto a shared tree, and the fold.
    for name in ["put", "branched"] {
        compared += both.agree(
            name,
            &mapfix::keyed(&ms)
                .iter()
                .map(|args| {
                    let mut args = args.clone();
                    args.push(Value::Int(7));
                    args
                })
                .collect::<Vec<_>>(),
        );
    }
    compared += both.agree("dropped", &mapfix::keyed(&ms));
    compared += both.agree("joined", &mapfix::pairs(&ms));
    for name in ["grown", "descending"] {
        compared += both.agree(
            name,
            &[0i64, 1, 2, 3, 7, 16, 33]
                .iter()
                .map(|n| vec![Value::Int(*n)])
                .collect::<Vec<_>>(),
        );
    }
    compared += both.agree("nothing", &[vec![]]);
    compared += both.agree(
        "total",
        &ms.iter()
            .map(|m| vec![m.clone(), Value::Int(0), Value::Int(0)])
            .collect::<Vec<_>>(),
    );

    // A value that is itself an offset.
    let ns = mapfix::nested();
    for name in ["nested_below", "nested_same"] {
        compared += both.agree(name, &mapfix::pairs(&ns));
    }
    compared += both.agree("nested_at", &mapfix::keyed(&ns));

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
    compared += both.agree("counts_tally", &mapfix::singles(&cs));
    compared += both.agree("counts_below", &mapfix::pairs(&cs));
    compared += both.agree(
        "recounted",
        &cs.iter()
            .flat_map(|c| ms.iter().map(move |m| vec![c.clone(), m.clone()]))
            .collect::<Vec<_>>(),
    );
    compared += both.agree(
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
    compared += both.agree("held_size", &mapfix::singles(&hs));

    println!("{compared} map calls compared, and every backend agreed on every one");
}

/// Generic definitions, on both backends — which is **monomorphisation**, differentially.
///
/// A generic definition compiles by being specialised, so the divergence to look for is not "does
/// this work" but *which instantiation a site got*. `beck_llvm::mono` recovers a call's type
/// arguments by matching the `Global` node's solved type against the definition's declared one, and
/// each way of picking the wrong one is a program in `genfix`: two parameters read in the wrong
/// order, an instantiation keyed on a machine representation so that `Int` and `Bool` merge, a type
/// argument that is itself a list, a parameter that appears only in the result.
///
/// The evaluator is the control and it does none of this: it runs the generic definition once,
/// uniformly, which is exactly what makes it the right thing to disagree with.
#[test]
fn the_two_backends_agree_on_generics() {
    let _ = toolchain!();
    let both = Both::over("generic.beck", GENERIC);
    let mut compared = 0;
    compared += both.agree("of_ints", &genfix::ints());
    compared += both.agree("of_bools", &genfix::bools());
    compared += both.agree("of_texts", &genfix::texts());
    compared += both.agree("of_records", &genfix::records());
    compared += both.agree("of_unions", &genfix::unions());
    compared += both.agree("of_lists", &genfix::lists());
    compared += both.agree("of_lists_of_lists", &genfix::nested());
    compared += both.agree("second_int", &genfix::ints());
    compared += both.agree("second_text", &genfix::texts());
    compared += both.agree("count_ints", &genfix::ints());
    compared += both.agree("count_texts", &genfix::texts());
    compared += both.agree("int_then_text", &genfix::int_and_text());
    compared += both.agree("text_then_int", &genfix::text_and_int());
    compared += both.agree("ints_agree", &genfix::int_pairs());
    compared += both.agree("texts_agree", &genfix::text_pairs());
    compared += both.agree("no_ints", &[vec![]]);
    compared += both.agree("no_texts", &[vec![]]);
    compared += both.agree("of_lists", &genfix::lists());
    compared += both.agree("three_ints", &genfix::scalars());
    compared += both.agree("three_texts", &genfix::singles());
    compared += both.agree("bound", &genfix::scalars());

    // …and the control: every one of those definitions compiled, so this is not passing because
    // the pair agreed on falling back. The instantiations are here by name, because a run that
    // built `firstly` once and called it three times would answer correctly and be wrong.
    let compiled: Vec<String> = both
        .native
        .module()
        .functions
        .iter()
        .map(|f| f.name.to_string())
        .collect();
    for wanted in [
        "firstly@Int",
        "firstly@Bool",
        "firstly@Str",
        "firstly@Named",
        "firstly@Tagged",
        "firstly@list[Int]",
        "firstly@list[list[Int]]",
        // One `paired`, not two. `swapped[Str, Int]` calls `paired(b, a)`, so its `paired` is
        // `paired[Int, Str]` — the same instantiation `int_then_text` asks for directly. That
        // sharing is the assertion: a recovery that read the arguments off in *declaration* order
        // rather than in use order would mint a second, differently-named function here.
        "paired@Int,Str",
        "swapped@Str,Int",
        "empty_of@Int",
        "empty_of@Str",
        "same_twice@Int",
        "same_twice@Str",
        "second@Int",
        "second@Str",
        "counted@Int",
        "counted@Str",
        "repeated@Int",
        "repeated@Str",
    ] {
        assert!(
            compiled.iter().any(|f| f == wanted),
            "`{wanted}` should be one of the compiled functions, and they are {compiled:?}"
        );
    }
    // And no template survives: a specialised generic is not a definition this backend has.
    for template in [
        "firstly",
        "paired",
        "swapped",
        "empty_of",
        "same_twice",
        "second",
        "counted",
        "repeated",
    ] {
        assert!(
            !compiled.iter().any(|f| f == template),
            "`{template}` is a template and should be gone once every site was specialised: \
             {compiled:?}"
        );
    }
    assert!(
        !compiled.iter().any(|f| f == "paired@Str,Int"),
        "`paired` should have one instantiation and not two: {compiled:?}"
    );
    println!("{compared} generic calls compared over {} compiled instantiations, and both backends agreed on every one", compiled.len());
}

/// A **polymorphically recursive** definition is refused by name, once, rather than compiled until
/// the machine stops.
///
/// `def growing[T](x: T, …)` calling `growing([x], …)` asks for `growing@Int`, which asks for
/// `growing@list[Int]`, which asks for `growing@list[list[Int]]`. The program is finite and the set
/// of instantiations is not, which is the one thing monomorphisation cannot do and the reason
/// `mono::MAX_INSTANTIATIONS` is a number rather than a hope.
///
/// Two assertions, and the second is the one worth having: the definition is refused, **and it is
/// refused once**. The first version of this pass gave up part way through and left sixty-four
/// instantiations behind, each refusing because it called the next — sixty-four refusals that were
/// all one refusal, and a reader who could not see which definition to fix.
#[test]
fn a_polymorphically_recursive_definition_is_refused_rather_than_compiled_forever() {
    let program = compile("growing.beck", genfix::RECURSIVE);
    let module = beck_llvm::module(&program);
    let names: Vec<String> = module.refusals.iter().map(|r| r.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["asks_for_it".to_string(), "growing".to_string()],
        "the template and its caller are what should be refused, and nothing else"
    );
    assert!(
        module.functions.is_empty(),
        "no instantiation of a definition this could not finish should survive: {:?}",
        module
            .functions
            .iter()
            .map(|f| f.name.to_string())
            .collect::<Vec<_>>()
    );
    // The reason a reader is given names the definition and says what is wrong with it, which is
    // `docs/93` §93.9's rule about a refusal being a claim.
    let why = beck_llvm::mono::specialise(&program).kept;
    assert!(
        why.get("growing")
            .is_some_and(|r| r.contains("instantiations")),
        "the reason should say what could not be finished: {why:?}"
    );
}

/// A generic called where **nothing decides** what its type parameter is is refused, not guessed.
///
/// `list_len(anything())` pins `T` against `list_len`'s own parameter and no further, so inference
/// finishes with a variable rather than a type. The program is legal and the evaluator runs it
/// happily — it has one uniform definition and an empty list is empty whatever it holds — and this
/// backend has no layout to pick.
///
/// The assertion that matters is the second one. Minting an instantiation named after a *variable*
/// would compile: the name would be stable within one run and would depend on an inference
/// counter rather than on the program, so two compiles of the same source could disagree about a
/// symbol. That is a determinism defect wearing a feature's clothes, and it is what
/// `the_module_is_a_function_of_the_program` exists to catch — this refuses before it gets there.
#[test]
fn a_generic_whose_type_nothing_decides_is_refused_rather_than_guessed() {
    let program = compile("undecided.beck", genfix::UNDECIDED);
    let module = beck_llvm::module(&program);
    let names: Vec<String> = module.refusals.iter().map(|r| r.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["anything".to_string(), "how_many".to_string()],
        "the template and its caller are what should be refused, and nothing else"
    );
    let why = beck_llvm::mono::specialise(&program).kept;
    assert!(
        why.get("anything")
            .is_some_and(|r| r.contains("not yet decided")),
        "the reason should say the type parameters were not decided: {why:?}"
    );
    assert!(
        !module.ir.contains('?'),
        "no symbol should be named after an inference variable"
    );
}

/// Closures, over every backend this machine has.
///
/// What the differential can see here is narrower than what it sees for a list, and the reason is
/// the feature's own line: a closure never crosses the boundary, so no call can *answer* with one.
/// Every case is therefore a definition that builds a closure, applies it — itself or through one of
/// the five loops — and answers with something the host can read. That is the whole of what a
/// program can observe about a closure anyway.
#[test]
fn the_two_backends_agree_on_closures() {
    let _ = toolchain!();
    let both = Both::over("closures.beck", CLOSURES);
    let ns: Vec<i64> = vec![0, 1, -1, 2, 7, -7, i64::MAX, i64::MIN];
    let mut compared = 0;

    // Applying one where it is built, and the three ways it can carry what it reads.
    for name in ["twice", "again", "through", "double"] {
        compared += both.agree(name, &clofix::each_of(&ns));
    }
    for name in ["add_on", "nested"] {
        compared += both.agree(name, &clofix::pairs_of(&ns));
    }
    compared += both.agree("between", &clofix::triples_of(&ns));
    // Two arms, and which one runs is a value.
    compared += both.agree("either", &clofix::flagged(&ns));

    // The five loops, over the lists and over the second argument the closure captures.
    let xs = clofix::lists();
    let bys: Vec<i64> = vec![0, 1, -1, 3, i64::MAX];
    for name in ["doubled", "summed", "flags", "tally", "risky"] {
        compared += both.agree(name, &clofix::singles(&xs));
    }
    for name in [
        "scaled",
        "kept",
        "biggest",
        "all_above",
        "any_above",
        "twice_over",
    ] {
        compared += both.agree(name, &clofix::with(&xs, &bys));
    }

    // An element that is an offset, and a result that is one.
    let ts = clofix::texts();
    for name in ["lengths", "shouted", "long_ones", "joined"] {
        compared += both.agree(name, &clofix::singles(&ts));
    }

    // Reals, where a word becomes a `double` and the answer is normalised on the way back.
    let rs = clofix::reals();
    for name in ["halved", "negated", "added"] {
        compared += both.agree(name, &clofix::singles(&rs));
    }

    // The last two list primitives. `by_rank` is the stability case: every key in one of those
    // lists is the same, so an unstable sort is free to answer anything and a stable one answers
    // the input order.
    compared += both.agree("flattened", &clofix::singles(&clofix::nested()));
    compared += both.agree("flat_texts", &clofix::singles(&clofix::nested_texts()));
    compared += both.agree("spread", &clofix::singles(&xs));
    for name in ["ascending", "descending", "by_sign"] {
        compared += both.agree(name, &clofix::singles(&xs));
    }
    for name in ["by_length", "by_text"] {
        compared += both.agree(name, &clofix::singles(&ts));
    }
    compared += both.agree("by_real", &clofix::singles(&rs));
    compared += both.agree("by_rank", &clofix::singles(&clofix::notes()));

    // Comparing two closures, which is `Closure`'s own order: the parameters, then where the body
    // starts — and *not* the captured frame, which `captures_ignored` is about.
    for name in ["same_lambda", "two_lambdas", "ordered"] {
        compared += both.agree(name, &[vec![]]);
    }
    compared += both.agree(
        "captures_ignored",
        &ns.iter()
            .flat_map(|a| ns.iter().map(move |b| vec![Value::Int(*a), Value::Int(*b)]))
            .collect::<Vec<_>>(),
    );

    println!("{compared} closure calls compared, and every backend agreed on every one");
}

/// The bytes a call's own arguments occupy, so what a body allocated is what is left over.
///
/// The reprs come from the *signature* rather than being written down, because an index into the
/// word table is a property of the program and a test that guessed one would measure another type.
fn arguments(both: &Both, name: &str, args: &[Value]) -> usize {
    let sig = both
        .native
        .module()
        .signature(name)
        .unwrap_or_else(|| panic!("`{name}` compiled"));
    both.native
        .module()
        .heap
        .encode_args(args, &sig.params)
        .expect("encodes")
        .1
        .len()
}

/// What a closure and a loop leave in the arena, at two sizes.
///
/// A shape gate with no clock in it (`AGENTS.md`), and the shape is the one that would be wrong if a
/// loop allocated per iteration: a fold that builds nothing must cost **one closure**, whatever the
/// list is, and a map must cost its answer and one closure and nothing else. The exact bytes are
/// asserted rather than a ratio, because the arithmetic is `heap::list_bytes` and
/// `heap::closure_bytes` and both are decided in one place.
#[test]
fn a_loop_costs_its_answer_and_one_closure() {
    let _ = toolchain!();
    let both = Both::over("closures.beck", CLOSURES);
    for n in [200usize, 1600] {
        let xs = Value::List(Arc::new((0..n as i64).map(Value::Int).collect()));
        // The arguments' own graph is on the wire before anything is allocated, so the arena a call
        // leaves is measured against what the call was given.
        let given = arguments(&both, "counted", std::slice::from_ref(&xs));

        // A fold that builds nothing: one closure, and the one-element list the answer is wrapped
        // in — the same bytes at both sizes, which is the whole claim.
        let (_, bytes) = both
            .native
            .call_sized("counted", std::slice::from_ref(&xs))
            .expect("runs");
        assert_eq!(
            bytes - given,
            beck_llvm::heap::closure_bytes(0) as usize + beck_llvm::heap::list_bytes(1) as usize,
            "counted over {n} elements allocated more than its closure and its answer"
        );

        // A map: the list it answers with, and one closure.
        let (_, bytes) = both.native.call_sized("doubled", &[xs]).expect("runs");
        assert_eq!(
            bytes - given,
            beck_llvm::heap::list_bytes(n as u64) as usize
                + beck_llvm::heap::closure_bytes(0) as usize,
            "doubled over {n} elements allocated more than the list and the closure"
        );
    }
}

/// A sort costs four runs of the list, and a concatenation costs its answer.
///
/// The second shape gate with no clock in it, and both halves say something a differential cannot.
/// **`concat_lists`** is the one whose refusal was *wrong* (`docs/93` §93.13 as corrected in the
/// changelog): if it grew a list the way `list_append` would have to, the arena would hold every
/// intermediate — so asserting it leaves exactly its answer is asserting the sum-then-allocate shape.
/// **`sort_by`** allocates the keys, the elements, and a scratch pair, and a merge sort that
/// allocated per level rather than reusing that scratch would pass every differential and fail here.
#[test]
fn a_sort_costs_four_runs_and_a_concatenation_costs_its_answer() {
    let _ = toolchain!();
    let both = Both::over("closures.beck", CLOSURES);
    for n in [200usize, 1600] {
        let xs = Value::List(Arc::new((0..n as i64).map(Value::Int).collect()));
        let given = arguments(&both, "ascending", std::slice::from_ref(&xs));
        let (_, bytes) = both
            .native
            .call_sized("ascending", std::slice::from_ref(&xs))
            .expect("runs");
        assert_eq!(
            bytes - given,
            4 * beck_llvm::heap::list_bytes(n as u64) as usize
                + beck_llvm::heap::closure_bytes(0) as usize,
            "sorting {n} elements allocated more than the keys, the elements, the scratch pair and \
             one closure"
        );

        // `n` inner lists of one element each, so the answer is `n` elements and the outer list is
        // what the call was given.
        let xss = Value::List(Arc::new(
            (0..n as i64)
                .map(|i| Value::List(Arc::new(vec![Value::Int(i)])))
                .collect(),
        ));
        let given = arguments(&both, "flattened", std::slice::from_ref(&xss));
        let (_, bytes) = both
            .native
            .call_sized("flattened", std::slice::from_ref(&xss))
            .expect("runs");
        assert_eq!(
            bytes - given,
            beck_llvm::heap::list_bytes(n as u64) as usize,
            "concatenating {n} lists of one allocated more than the list it answers with"
        );
    }
}

/// A tail call through an application is a tail call./// A tail call through an application is a tail call.
///
/// `docs/27` makes "a call in tail position is free" a property of the *language*, and an
/// application is a call — so a loop written as a closure calling itself must not grow the stack.
/// There are **two** hops to get wrong here rather than one: the call into the family's application,
/// and the arm inside it. Ten million iterations is past any host stack, so a frame spent on either
/// is a crash rather than a slow test.
///
/// Checked by making it red, because a gate on a property the optimiser might supply anyway is worth
/// nothing (`AGENTS.md`): with the application's own call site emitted as an ordinary call, this
/// answers `SIGSEGV` at this size. The arm inside the application is the hop that `tailcc` alone
/// would have got right — which is why `musttail` is there, since it is *enforced* rather than
/// hoped for (`docs/93` §93.4).
#[test]
fn a_tail_call_through_a_closure_costs_nothing() {
    let _ = toolchain!();
    let both = Both::over("closures.beck", CLOSURES);
    let (walked, compiled) = both.call("spin", &[Value::Int(1_000), Value::Int(0)]);
    assert_eq!(
        walked, compiled,
        "the two agree where the evaluator can answer"
    );
    let deep = both
        .native
        .call("spin", &[Value::Int(10_000_000), Value::Int(0)])
        .expect("ten million applications in tail position");
    assert_eq!(deep, Value::Int(50_000_005_000_000));
}

/// A closure is refused at every boundary the host would have to read one across./// A closure is refused at every boundary the host would have to read one across.
///
/// Two-sided, because a backend that refused everything would pass the first half: `applies` is
/// refused *and* `twice` in the program above compiles, so what is being asserted is a line rather
/// than an absence. The reasons are checked too — `docs/93` §93.9 is what happens when a refusal's
/// reason is nobody's business but the refusal's.
#[test]
fn a_closure_does_not_cross_the_boundary() {
    let _ = toolchain!();
    let both = Both::over("refused.beck", support::clofix::REFUSED);
    for (name, expect) in [
        ("applies", "parameter `f` is a function value"),
        ("picked", "returns a function value"),
        ("held", "whose field `apply_to` is a function value"),
        ("listed", "whose element is a function value"),
        ("spread_out", "`list_flat_map` answers a list whose length"),
    ] {
        let why = both
            .refusal(name)
            .unwrap_or_else(|| panic!("`{name}` should be refused, and it compiled"));
        assert!(
            why.contains(expect),
            "the reason for refusing `{name}` should mention {expect:?}, and is {why:?}"
        );
    }
    assert!(
        both.compiled().iter().any(|n| n == "double"),
        "`double` compiles, so this program is not passing by refusing everything"
    );
}

/// A tag is a variant's rank **by name**, and a field's slot is its rank by name./// A tag is a variant's rank **by name**, and a field's slot is its rank by name.
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

/// The ceiling on how deep a decoded value may be is one the machine can actually reach.
///
/// `beck_llvm::heap::MAX_DEPTH` is 2,048 and the decoder is recursive, so the *real* limit is the
/// host thread's stack — and for a while it was **smaller than the declared one**: adding a `list`
/// arm made the frame big enough that a value 800 deep aborted a debug build, which is
/// [`docs/46`](../../../../docs/46-standard-library-report.md) §46.16's "nine match arms cost
/// a thousand levels of recursion" in the host rather than the evaluator.
///
/// A comment saying the frame is small is not a gate. This decodes a value **at** the ceiling,
/// built by hand rather than by a compiled program so it needs no toolchain and runs in the same
/// default-stack thread `cargo test` gives everything else. That is
/// [`adr/0007`](../../../../docs/adr/0007-evaluator-stack-is-declared-not-discovered.md)'s property
/// — a ceiling is declared, not discovered — applied to the one place in this backend that recurses
/// over data somebody else produced.
#[test]
fn a_value_at_the_declared_ceiling_decodes_rather_than_aborting() {
    const SRC: &str = r#"
union Chain:
    Link(next: Chain)
    End()

def deep(c: Chain) -> Chain:
    return c
"#;
    let program = compile("chain.beck", SRC);
    let module = beck_llvm::module(&program);
    let repr = module.signature("deep").expect("compiles").ret;

    // The blob, by hand: `End()` at the bottom and a `Link` per level above it. Written rather than
    // encoded from a `Value`, because building the `Value` is the thing being tested.
    let (end, link) = {
        let beck_llvm::Repr::Obj(at) = repr else {
            panic!("a union is an object")
        };
        let l = module.heap.layout(at);
        (
            u64::from(l.tag_of(Some("End")).expect("End")),
            u64::from(l.tag_of(Some("Link")).expect("Link")),
        )
    };
    let mut blob: Vec<u8> = vec![0; 8];
    let mut cell = blob.len() as u64;
    blob.extend_from_slice(&end.to_ne_bytes());
    // One `End` and `MAX_DEPTH - 1` `Link`s, so the whole value is exactly `MAX_DEPTH` deep —
    // the ceiling counts nested values and the innermost one is the first of them.
    for _ in 0..beck_llvm::heap::MAX_DEPTH - 1 {
        let here = blob.len() as u64;
        blob.extend_from_slice(&link.to_ne_bytes());
        blob.extend_from_slice(&cell.to_ne_bytes());
        cell = here;
    }

    let value = module
        .heap
        .decode(cell, repr, &blob)
        .expect("a value at the ceiling decodes");
    // …and it really is that deep, so this did not pass by decoding something short.
    let mut depth = 0usize;
    let mut at = &value;
    while let Value::Data(record) = at {
        match record.fields.get("next") {
            Some(next) => {
                depth += 1;
                at = next;
            }
            None => break,
        }
    }
    assert_eq!(depth, beck_llvm::heap::MAX_DEPTH - 1);

    // One level past it is a message rather than a deeper walk.
    let here = blob.len() as u64;
    blob.extend_from_slice(&link.to_ne_bytes());
    blob.extend_from_slice(&cell.to_ne_bytes());
    let why = module
        .heap
        .decode(here, repr, &blob)
        .expect_err("one past the ceiling is refused");
    assert!(why.contains("nested more than"), "{why}");
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
    // of a 16-byte string is well past the arena (§93.7 is why that is quadratic and not a bug).
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

/// A corpus program's **fold** and its **page** compile, by name and over the whole corpus.
///
/// `apply_event` is a `durable` fold's step function — `(State, Envelope[Event]) -> State` — and
/// until `docs/93` its state was a `Map` and so it could not. `view` is `(State, Session) -> Html`
/// and until `docs/93` a page had no shape at all; this test asserted it was refused, and that row
/// moved here rather than being deleted, which is what the refusal lists in this file are for.
///
/// Both sets are floors rather than equalities. A corpus program acquiring one should not turn this
/// red; a corpus program *losing* one should. The other side is below: something in the corpus is
/// still refused, so this cannot pass by everything compiling.
#[test]
fn a_corpus_fold_compiles() {
    let mut folded: Vec<String> = Vec::new();
    let mut viewed: Vec<String> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for path in corpus_programs() {
        let src = std::fs::read_to_string(&path).expect("a corpus program");
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a file name")
            .to_string();
        let (placed, diags, map) = beck_core::compile_str(&name, &src);
        assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
        let program = Arc::new(placed.expect("compiles").program);
        let module = beck_llvm::module(&program);
        if module.signature("apply_event").is_some() {
            folded.push(name.clone());
        }
        if module.signature("view").is_some() {
            viewed.push(name);
        }
        refused.extend(module.refusals.iter().map(|r| r.reason.clone()));
    }
    assert!(
        folded.len() >= 9,
        "at least nine corpus folds compiled when `docs/93` was written, and {} do now: {folded:?}",
        folded.len()
    );
    assert!(
        viewed.len() >= 21,
        "twenty-one corpus pages compiled when `docs/93` was written, and {} do now: {viewed:?}",
        viewed.len()
    );
    // The other side, so this is not passing because everything compiles. What is still refused
    // across the corpus is a definition **generic over a type** — `docs/93` compiled the last
    // collection that grows and `str_trim` took the last Unicode row that was not one, so a type
    // parameter is what is left. This line has been rewritten once per change that removed the
    // previous answer, which is what it is for.
    assert!(
        refused.iter().any(|r| r.contains("generic over T")),
        "no corpus definition was refused for a type parameter, and this test would then be \
         asserting that everything compiles: {refused:?}"
    );
    println!(
        "{} corpus folds and {} corpus pages compile natively: {folded:?} / {viewed:?}",
        folded.len(),
        viewed.len()
    );
}

/// A lookup costs the same whatever the map holds, and the gate has no clock in it.
///
/// `measure_native.rs` cannot say this: a binary search over two thousand entries is a handful of
/// comparisons, and its own control shows the search is smaller than the loop that calls it. What
/// *is* sayable without a clock is the arena: `map_get` allocates one `Option` and nothing else, so
/// a lookup into a map of 1,600 leaves exactly what a lookup into a map of 200 leaves. A search
/// that built anything per entry — a copy of the keys, a list of candidates — would show here.
///
/// The second half is the one that would go red if `map_keys` stopped being one `memcpy`: it costs
/// the map's size and not the map's size squared.
#[test]
fn a_lookup_costs_the_same_whatever_the_map_holds() {
    let _ = toolchain!();
    let both = Both::over("maps.beck", MAPS);
    let of = |n: usize| {
        Value::Map(
            (0..n as i64)
                .map(|i| (Value::str_(format!("k{i:06}")), Value::Int(i)))
                .collect(),
        )
    };
    let mut looked = Vec::new();
    let mut keyed = Vec::new();
    for n in [200usize, 1600] {
        let m = of(n);
        let params = &both.native.module().signature("lookup").unwrap().params;
        let args = [m.clone(), Value::str_("k000007")];
        let arguments = both
            .native
            .module()
            .heap
            .encode_args(&args, params)
            .expect("encodes")
            .1
            .len();
        let (answer, bytes) = both.native.call_sized("lookup", &args).expect("runs");
        assert_eq!(
            answer,
            Value::some(Value::Int(7)),
            "and it found the right entry, so this is not measuring a search that gave up"
        );
        looked.push(bytes - arguments);

        // Its own arguments, because `names` is handed the map and not the key: subtracting
        // `lookup`'s would leave the key's own bytes in the answer.
        let params = &both.native.module().signature("names").unwrap().params;
        let just_the_map = both
            .native
            .module()
            .heap
            .encode_args(std::slice::from_ref(&m), params)
            .expect("encodes")
            .1
            .len();
        let (_, bytes) = both.native.call_sized("names", &[m]).expect("runs");
        keyed.push(bytes - just_the_map);
    }
    assert_eq!(
        looked[0], looked[1],
        "a lookup left {} bytes on a map of 200 and {} on a map of 1,600 — it is building \
         something per entry",
        looked[0], looked[1]
    );
    // Two words: `Some`'s tag and its payload.
    assert_eq!(looked[0], 16);

    // …and `map_keys` costs its answer: a list's two headers and one word per key.
    assert_eq!(keyed[0], 32 + 200 * 8);
    assert_eq!(keyed[1], 32 + 1600 * 8);
    println!(
        "a lookup left {} bytes at both sizes, and `map_keys` left {} then {}",
        looked[0], keyed[0], keyed[1]
    );
}

/// Building text in a loop costs the **square** of what it builds, and the gate has no clock in it.
///
/// `docs/93` §93.7's largest cost, asserted rather than only measured. `repeat(s, n, "")` builds
/// `n` strings whose lengths are `|s|, 2|s|, …, n|s|`, so the arena it leaves is `Θ(n²)` — where the
/// evaluator's is `Θ(n)`, because `docs/70` gave it an in-place `push_str` when `liveness` proves
/// the accumulator is a last use and an arena with no ownership in it cannot.
///
/// `measure_native.rs` prints the wall clock and asserts nothing about it, because that comparison
/// runs the other way in a debug build. This is the same claim with the clock taken out: four times
/// the steps has to cost about sixteen times the arena, and a run in which it cost four would mean
/// this backend had grown the analysis — which is a finding rather than a passing test.
#[test]
fn an_accumulator_costs_the_square_of_what_it_builds() {
    let _ = toolchain!();
    let both = Both::over("text.beck", TEXT);
    let unit = Value::str_("abcdefgh");
    let mut sizes = Vec::new();
    for n in [500usize, 2000] {
        let args = [unit.clone(), Value::Int(n as i64), Value::str_("")];
        let (_, bytes) = both.native.call_sized("repeat", &args).expect("runs");
        sizes.push((n, bytes));
    }
    let (small, big) = (sizes[0], sizes[1]);
    let growth = big.1 as f64 / small.1 as f64;
    let steps = (big.0 / small.0) as f64;
    assert!(
        growth > steps * steps * 0.9,
        "four times the steps left {growth:.1}× the arena, and a quadratic accumulator leaves \
         about {:.0}× — this backend appears to have grown an ownership analysis, which `docs/93` \
         §93.7 says it cannot have",
        steps * steps
    );
    println!(
        "repeat({}) left {} bytes and repeat({}) left {} — {growth:.1}× for {steps:.0}× the steps",
        small.0, small.1, big.0, big.1
    );
}

/// A slice of a list costs its answer, and the loop that walks one stays linear.
///
/// `a_slice_costs_its_answer_and_not_the_string_it_came_from` one type over, and for the same
/// reason: `docs/46` §46.14 found the quadratic in a list and `docs/70` §70.2 found it in text, so
/// the shape gate belongs on both. One element sliced out is a header, a block and the element — so
/// `n` steps are `40n` bytes, and a `list_slice` that copied what it was taken *from* would be
/// `O(n²)` with no clock in the measurement.
///
/// The constant went 16 → 40 at `docs/93`, which is that report's cost stated in a gate: a list is
/// two objects now, so the *smallest* one is five words rather than two. What this test is about is
/// unchanged, because what it asserts is that the number does not grow with `n`.
#[test]
fn a_list_slice_costs_its_answer_and_not_the_list_it_came_from() {
    let _ = toolchain!();
    let both = Both::over("lists.beck", LISTS);
    // A two-word header, a two-word block header, and one element.
    const PER_ELEMENT: usize = 40;
    let mut sizes = Vec::new();
    for n in [200usize, 1600] {
        let xs = Value::List(std::sync::Arc::new(
            (0..n as i64).map(Value::Int).collect::<Vec<_>>(),
        ));
        let empty = Value::List(std::sync::Arc::new(Vec::new()));
        let args = [xs, Value::Int(0), empty];
        let arguments = both
            .native
            .module()
            .heap
            .encode_args(
                &args,
                &both.native.module().signature("walked").unwrap().params,
            )
            .expect("encodes")
            .1
            .len();
        let (_, bytes) = both.native.call_sized("walked", &args).expect("runs");
        let allocated = bytes - arguments;
        assert_eq!(
            allocated,
            n * PER_ELEMENT,
            "walked over {n} elements should allocate {} bytes and allocated {allocated}",
            n * PER_ELEMENT
        );
        sizes.push((n, allocated));
    }
    let (small, big) = (sizes[0], sizes[1]);
    println!(
        "walked({}) allocated {} bytes and walked({}) allocated {} — {} bytes an element at both \
         sizes",
        small.0,
        small.1,
        big.0,
        big.1,
        (big.1 - small.1) / (big.0 - small.0)
    );
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
        ("grows_a_list", "`list_flat_map` answers a list"),
        ("renders_a_real", "`str` of Float"),
        ("is_generic", "generic over T"),
        ("calls_something_refused", "calls `grows_a_list`"),
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
    // `reads_the_clock` is here rather than above because the protocol grew a second direction:
    // it is compiled and not compared, since two backends reading the *process* clock one after
    // the other are not in the same millisecond — `the_two_backends_agree_on_the_host_effects` is
    // where they are asked the same question.
    assert_eq!(
        both.compiled(),
        vec!["reads_the_clock".to_string(), "scalar_and_fine".to_string()]
    );
    both.agree("scalar_and_fine", &singles(&ints(0x5eed_0007, 20)));

    // A refusal is not a silent fallback either: asking the artefact for a refused definition is
    // an error rather than an evaluator call wearing a native backend's name.
    let err = both
        .native
        .call("grows_a_list", &[Value::Int(1)])
        .expect_err("a refused definition is not callable natively");
    assert!(err.message.contains("did not compile natively"), "{err}");
}

/// What the heap does **not** reach, asserted as an absence.
///
/// `docs/93` §93.15 lists what is not built — collections, closures and every effect — and a
/// list in prose goes stale where a list with a test attached cannot (`docs/82` §82.10). Each of
/// these goes red the day its row starts compiling, which is the day the row should be deleted.
///
/// Seven rows were deleted that way: a `Str` got a layout, a `list` got one, the higher-order
/// primitives compiled, `str_trim` and `str_split` moved across when their stated reasons — a
/// Unicode table, and "two loops rather than one" — turned out to be false of them, the **host
/// effects** moved when the worker's protocol grew a second direction, and `str_upper` moved when
/// the runtime library made the table something a program **links** rather than something an
/// emitter would have to copy (`docs/93` §93.12). Each time, the row moved from this list to the
/// control below it in the same commit.
///
/// What is left is the one that renders a real, the two collection primitives with no layout to
/// answer with, and a definition generic over a type. `str_upper`'s row is the shape to copy: a
/// refusal whose reason is "this is somebody else's table" is a refusal the runtime library
/// answers, so it belongs in the control rather than here.
#[test]
fn what_the_heap_does_not_reach_is_refused_by_name() {
    let program = compile("still-refused.beck", STILL_REFUSED);
    let module = beck_llvm::module(&program);
    for (name, expect) in [
        ("renders_a_real", "`str` of Float"),
        ("grows", "`list_flat_map` answers a list"),
        ("is_generic", "generic over T"),
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
        vec![
            "splits_a_string".to_string(),
            "trims".to_string(),
            "upcases".to_string(),
            "mapped".to_string(),
            "double_it".to_string(),
            "names_it".to_string(),
            "reads_a_list".to_string(),
            "reads_the_clock".to_string(),
            "scalar_and_fine".to_string()
        ]
    );
}

/// A refusal's reason is a **claim**, and this asks whether the claim is true.
///
/// `docs/93` §93.9 refused `str_index_of` on the grounds that it "answers with an `Option`,
/// whose layout this backend resolves from a program's own types and not from the prelude's".
/// That sentence was false: the prelude's `Option` has had a layout since
/// [`docs/93`](../docs/93-the-native-backends-report.md), and `maybe(n) -> Option[Int]` compiled in
/// the very fixture beside it. Every gate around the refusal was green, because each asserted that
/// the refusal *said* something and none asked whether what it said was *so* — `docs/82` §82.10's
/// pattern, in the one place this project had not looked for it.
///
/// So: every type a refusal blames for having no layout is asked whether it has one, and the
/// control is the type that broke. It goes red the day a blamed type acquires a layout and the
/// refusal that blames it is not deleted.
#[test]
fn a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one() {
    const PROBE: &str = r#"
model Held:
    n: Int

def a_list(xs: list[Int]) -> Int:
    return list_len(xs)

def a_map(m: Map[Str, Int]) -> Int:
    return map_len(m)

def a_closure(f: (Int) -> Int, n: Int) -> Int:
    return f(n)

def an_option(n: Int) -> Option[Int]:
    return Some(value = n)

def a_record(h: Held) -> Int:
    return h.n

## The definition the false reason was about. It is on the compiled side now, which is the
## specific half of this gate: the general half below asserts the *fact* the reason denied.
def finds(a: Str, b: Str) -> Option[Int]:
    return str_index_of(a, b)
"#;
    let program = compile("probe.beck", PROBE);
    let module = beck_llvm::module(&program);
    let mut heap = module.heap.clone();
    let ty_of = |name: &str| program.defs[name].params[0].2.clone();

    // The last row, and this gate **fired on it**: `docs/93` gave a closure a shape, so "a closure,
    // which has no layout here" stopped being true while the refusal that said it stayed. What is
    // true now is narrower and is asserted as two things — the shape exists, and the *boundary* is
    // what refuses it — because a parameter is a value the host has to marshal and a closure is the
    // one value it cannot.
    let shape = heap
        .repr(&ty_of("a_closure"), &program)
        .expect("a closure has a shape: a rank and its captures");
    let why = beck_llvm::heap::Heap::crossing(shape)
        .expect_err("and it may not cross a boundary the host reads");
    assert!(
        why.contains("inside one \ncompiled call") || why.contains("inside one compiled call"),
        "`a_closure`'s parameter should be refused for crossing, and the reason is {why:?}"
    );

    // …and the control, which is the assertion that did not exist. `Option[Int]` is the prelude's
    // and has a layout, so no refusal may blame it for not having one.
    let option = program.defs["an_option"].ret.clone();
    assert!(
        heap.repr(&option, &program).is_ok(),
        "`Option[Int]` has a layout, and a refusal that says otherwise is wrong"
    );
    for name in ["an_option", "a_record", "finds", "a_list", "a_map"] {
        assert!(
            module.signature(name).is_some(),
            "`{name}` compiles — a primitive that answers with a prelude union is not a wall, and \
             this program is not passing by refusing everything"
        );
    }
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
## A closure rather than a record, a `Str`, a `list` or a `Map`: `docs/93` gave the record a
## layout, `docs/93` gave text one, `docs/93` gave a list one and `docs/93` gave a map one, and
## what a refusal has to travel *from* is something the heap still does not reach.
def bottom(n: Int) -> Int:
    return apply(double_it, n)

def apply(f: (Int) -> Int, n: Int) -> Int:
    return f(n)

def double_it(n: Int) -> Int:
    return n * 2

def middle(n: Int) -> Int:
    return bottom(n)

def top(n: Int) -> Int:
    return middle(n) + 1

## Mutually recursive, and both must go: `ping` is only refusable through `pong` and the other way
## round, so a single pass in either direction keeps one of them.
def ping(n: Int) -> Int:
    if n == 0:
        return bottom(0)
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
/// can do is notice that the worker stopped and say why it probably did. §93.15 is what closing it
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

    let refused = &program.defs["grows_a_list"].body;
    assert!(!native.compiled(refused));

    let f = beck_eval::on_the_evaluator_stack(|| native.function(refused).expect("prepares"));
    let xss = Value::List(Arc::new(vec![
        Value::List(Arc::new(vec![Value::Int(2), Value::Int(3)])),
        Value::List(Arc::new(vec![Value::Int(4)])),
    ]));
    let got = beck_eval::on_the_evaluator_stack(|| f(vec![xss]).expect("the fallback answers"));
    assert_eq!(
        got,
        Value::List(Arc::new(vec![Value::Int(2), Value::Int(3), Value::Int(4)]))
    );
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
    // evaluator reaches its depth ceiling and the worker exhausts its stack. §93.15 records that
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

/// Every `.beck` file in `corpus/`, including the multi-file project under it.
fn corpus_programs() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .join("corpus");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&root)
        .expect("the corpus is there")
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "beck") {
            files.push(path);
        }
    }
    files.sort();
    assert!(
        files.len() > 30,
        "only found {} corpus programs",
        files.len()
    );
    files
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
    let mut refused = 0;
    let mut assembled = 0;
    let mut blames: Vec<(String, String)> = Vec::new();
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
        refused += module.refusals.len();
        blames.extend(
            module
                .refusals
                .iter()
                .map(|r| (format!("{name}::{}", r.name), r.reason.clone())),
        );
        assembled += 1;
        // Assembled, not merely emitted: `-c` stops before the link, because what is being checked
        // is that LLVM accepts the IR and not that a program with no `main`-worthy content links.
        Artifact::build_bounded(&program, toolchain.clone(), None, Some(LIMIT))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    // Both halves printed, because a report that quotes "so many more compile" needs the
    // denominator: each file is compiled **alone**, so a library's generic definitions have no
    // caller and stay refused (`docs/93` §93.10), and the count only means something beside the
    // one before it.
    println!(
        "{assembled} programs assembled, {compiled} definitions compiled to native code, \
         {refused} left to the evaluator"
    );
    assert!(compiled > 100, "only {compiled} definitions compiled");
    // A primitive that compiles cannot be the reason for a refusal, and the claim a report makes
    // is over the *whole tree* rather than over a fixture. Each of these was a refusal reason
    // until the release that took it off the list, so this is the sentence "it no longer appears
    // anywhere" with a test attached rather than a grep somebody ran once.
    let refusal_reasons = blames;
    let blamed: Vec<String> = refusal_reasons
        .iter()
        .filter(|(_, reason)| {
            [
                "`now`",
                "`uuid`",
                "`secret_env`",
                "`http_fetch`",
                "`raise`",
                // The fifteen the runtime library answers (`docs/93` §93.12). Refused by both emitters
                // until it existed, and a call now — so a refusal blaming one is a refusal that
                // has not noticed.
                "`digest`",
                "`digest_keyed`",
                "`digest_eq`",
                "`hex_encode`",
                "`hex_decode`",
                "`base64_encode`",
                "`base64_decode`",
                "`uuid_parse`",
                "`uuid_version`",
                "`str_upper`",
                "`str_lower`",
                "`str_to_int`",
                "`str_replace`",
                "`time_format`",
                "`time_parse`",
            ]
            .iter()
            .any(|p| reason.starts_with(p))
        })
        .map(|(name, reason)| format!("{name}: {reason}"))
        .collect();
    assert!(
        blamed.is_empty(),
        "these primitives compile, so nothing may be refused because of one: {blamed:#?}"
    );
    // …and the same question about the *prose*, which is the half that got away.
    //
    // `a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one` catches a reason that
    // names a **type** with a layout. It could not catch "a collection is not on this heap yet",
    // which named no type at all and stayed true-sounding for three reports after `docs/93` made
    // it false. So these are the sentences this backend may no longer say about itself, and the
    // list grows whenever a report retires a class rather than a name.
    let stale: Vec<String> = refusal_reasons
        .iter()
        .filter(|(_, reason)| {
            [
                "not on this heap",
                "no collection",
                "text is not",
                // `docs/93` §93.12 retired the class, not the names: a primitive that is a table or
                // somebody else's parser is a call into the runtime library now, so a refusal may
                // no longer describe one as a reason for refusing anything.
                "is a table rather than an operation",
                "agree with Rust's parser",
            ]
            .iter()
            .any(|dead| reason.contains(dead))
        })
        .map(|(name, reason)| format!("{name}: {reason}"))
        .collect();
    assert!(
        stale.is_empty(),
        "a heap that holds text, lists and maps may not refuse anything by saying it does not: \
         {stale:#?}"
    );
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
    assert_eq!(module.functions.len(), 2);
    assert_eq!(&*module.functions[1].name, "scalar_and_fine");
    assert_eq!(module.functions[1].params, vec![Repr::Int]);
    assert_eq!(module.functions[1].ret, Repr::Int);
    assert_eq!(module.functions[1].index, 1);
    assert!(module.refusals.len() >= 4, "{:?}", module.refusals);
    for refusal in &module.refusals {
        assert!(
            !refusal.reason.is_empty(),
            "`{}` was refused without a reason",
            refusal.name
        );
    }
}

/// A page, compiled — and the tree it bakes into is the tree the evaluator built.
///
/// This is the differential over `docs/93`'s recipe. What crosses the pipe is the *call* rather
/// than the tree, so the assertion that matters is not that the bytes arrived: it is that
/// `beck_core::html::element` and `Value`'s equality — which includes every structural hash — say
/// the two trees are one. A recipe that dropped an attribute in the wrong place, kept a key as an
/// attribute, or folded two attributes in the other order renders identically and fails here.
#[test]
fn the_two_backends_agree_on_views() {
    let _ = toolchain!();
    let both = Both::over("views.beck", viewfix::VIEWS);
    let cards = viewfix::cards();
    let lists = viewfix::lists();
    let mut compared = 0;

    // A text node of every shape a value has here: the repr is a *datum* in this one place, and a
    // wrong index reads the right word as the wrong thing.
    compared += both.agree("just_text", &viewfix::singles(&textfix::strings()));
    compared += both.agree("a_number", &singles(&[0, 1, -1, i64::MAX, i64::MIN]));
    compared += both.agree(
        "a_flag",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    compared += both.agree(
        "a_real",
        &[0.0, -0.0, 1.5, f64::INFINITY, f64::NAN]
            .iter()
            .map(|f| vec![Value::float(*f)])
            .collect::<Vec<_>>(),
    );
    compared += both.agree("a_record", &viewfix::singles(&cards));
    compared += both.agree("a_list", &viewfix::singles(&lists));

    // The elements, the attributes and the two rules that make a recipe and a tree differ.
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
        compared += both.agree(name, &viewfix::singles(&cards));
    }
    compared += both.agree("blank", &[vec![]]);
    for name in ["rows", "attrs_from"] {
        compared += both.agree(name, &viewfix::singles(&lists));
    }
    compared += both.agree("whole", &viewfix::with(&cards, &lists));

    // The direction nothing in a program needs: a baked tree back *in*, which the host has to
    // write as a recipe whose leaves are text. Every argument here is a tree the evaluator built.
    let mut trees: Vec<Value> = Vec::new();
    for name in ["titled", "keyed", "handled", "nested"] {
        for c in &cards {
            trees.push(
                both.evaluated(name, std::slice::from_ref(c))
                    .expect("the evaluator builds it"),
            );
        }
    }
    trees.push(
        both.evaluated("just_text", &[Value::str_("hello")])
            .expect("the evaluator builds it"),
    );
    compared += both.agree("again", &viewfix::singles(&trees));
    compared += both.agree("beside", &viewfix::with(&trees, &cards));

    println!("{compared} view calls compared, LLVM against the tree-walker");
    assert!(compared >= 200, "only {compared} calls compared");
}

/// The same page written with `ui:`, which is what a program actually contains.
///
/// `views.beck` exercises the five primitives; this exercises what the macro lowers to — a keyed
/// list built by a loop, a conditional class that is empty on one branch, two handlers carrying
/// records, and text built by `+` — and it is `examples/todo.beck`'s own `render` with the types
/// inlined.
#[test]
fn a_ui_block_compiles_and_agrees() {
    let _ = toolchain!();
    let both = Both::over("page.beck", viewfix::PAGE);
    let lefts = [0i64, 1, 7];
    let tuples: Vec<Vec<Value>> = viewfix::todos()
        .iter()
        .flat_map(|ts| lefts.iter().map(move |n| vec![ts.clone(), Value::Int(*n)]))
        .collect();
    let compared = both.agree("page", &tuples);
    println!("{compared} `ui:` pages compared, LLVM against the tree-walker");
}

/// What a view still cannot do, each with the reason a reader is given.
///
/// Every row is an **ordering**, and the reason is one sentence in `Repr::order`: a node in the
/// arena is the call that builds a tree, so two of them can be ordered by what they render and not
/// by what they are. The rows are three because the demand can arrive three ways — a search over a
/// list of them, a record that holds one being compared, and `==` between two directly — and
/// `Heap::ordered` is what has to walk to each.
#[test]
fn a_view_has_no_order_and_the_refusal_says_why() {
    let program = compile("refused-views.beck", viewfix::REFUSED);
    let module = beck_llvm::module(&program);
    for name in ["sorted_views", "same_panel", "same_view"] {
        let reason = module
            .refusals
            .iter()
            .find(|r| &*r.name == name)
            .unwrap_or_else(|| panic!("`{name}` compiled, and a view has no order"))
            .reason
            .clone();
        assert!(
            reason.contains("carried as the call that builds it"),
            "the reason for refusing `{name}` should be the one `Repr::order` gives, and is \
             {reason:?}"
        );
    }
    // A record *holding* a view is refused only when it is compared: building one is fine, and a
    // rule that refused the type outright would take a page's own model with it.
    assert!(
        module.signature("same_panel").is_none(),
        "comparing a record with a view in it has no answer"
    );

    // …and the boundary's one directional rule, which is §93.6's and not §93.6's: an `Attr` may
    // be answered with and may not be taken, because the host writes a handler back as the plain
    // attribute it would become and a bare one has not become it yet.
    for name in ["takes_an_attr", "takes_a_list_of_attrs"] {
        let reason = module
            .refusals
            .iter()
            .find(|r| &*r.name == name)
            .unwrap_or_else(|| panic!("`{name}` compiled, and an `Attr` may not be taken"))
            .reason
            .clone();
        assert!(
            reason.contains("may answer with and may not take"),
            "the reason for refusing `{name}` should be the directional one, and is {reason:?}"
        );
    }
    // The other side of *that* rule, so it is not refusing the type outright: answering with one
    // compiles, and so does taking a whole tree.
    let built = compile("views.beck", viewfix::VIEWS);
    let views = beck_llvm::module(&built);
    for name in ["one_attr", "one_key", "one_handler", "again", "beside"] {
        assert!(
            views.signature(name).is_some(),
            "`{name}` should compile: an `Attr` crosses outward and an `Html` crosses both ways"
        );
    }
}

/// What a page costs is a function of the page, and the gate has **no clock in it**.
///
/// `AGENTS.md`'s shape gate, for the one thing a recipe could most plausibly get wrong: a node
/// holds two lists, so a builder that reallocated a list per child — or that copied the children
/// built so far on every step — would be `O(n²)` in the arena and would still answer correctly at
/// every size anybody would run in a test. The per-row cost is the whole assertion, and it must be
/// the same number at 100 rows and at 800.
///
/// The constant is written down rather than derived, because deriving it here would be the layout
/// spelled a second time (`beck_llvm::heap`'s own argument): a row is an `li` (four words), its
/// empty attribute list (four), its child list (five), its text node (four) and the word it occupies
/// in the page's own child list — eighteen words, 144 bytes. It was twelve words until `docs/93`
/// made a list two objects. What is left over is the page itself and
/// the literal pool, and *that* number has to be the same at both sizes too, which is the half of
/// this test a per-row division would hide.
#[test]
fn a_page_costs_its_own_nodes_and_nothing_per_page() {
    let _ = toolchain!();
    let both = Both::over("views.beck", viewfix::VIEWS);
    const PER_ROW: usize = 144;
    let mut sizes = Vec::new();
    for n in [100usize, 800] {
        let xs = viewfix::ints(&(0..n as i64).collect::<Vec<_>>());
        let (_, bytes) = both.native.call_sized("rows", &[xs]).expect("runs");
        // The argument list is in the arena too — a header and one word an element — and it is the
        // page that is being measured.
        sizes.push((n, bytes - heap_bytes(n + 1)));
    }
    let (small, big) = (sizes[0], sizes[1]);
    let per = (big.1 - small.1) / (big.0 - small.0);
    assert_eq!(
        per, PER_ROW,
        "a row costs {per} bytes and the layout says {PER_ROW}"
    );
    assert_eq!(
        small.1 - PER_ROW * small.0,
        big.1 - PER_ROW * big.0,
        "the page itself should cost the same whatever is in it: {small:?} against {big:?}"
    );
    println!(
        "a page of {} rows costs {} bytes of arena and a page of {} costs {} — {PER_ROW} bytes a \
         row and {} bytes a page at both sizes",
        small.0,
        small.1,
        big.0,
        big.1,
        small.1 - PER_ROW * small.0
    );
}

/// A `raise` and a `try:`, compiled — and the three things that have to agree.
///
/// The value a caught failure becomes, the `Result` it is wrapped in, and the **message** when
/// nothing catches it: `beck-eval` renders a raise as ``raised `TooBig{n: 101}` ``, so the arena
/// travels with that one failure and the host builds the same sentence out of the same value. A
/// backend that reported "something was raised" would pass a test that only compared the fact of a
/// failure, and fails this one.
///
/// The cases that matter most are the ones where a handler must **not** fire — a fault inside a
/// `try:`, and a different error type raised inside it — because a handler that caught by trap code
/// rather than by type name would answer an `Err` where the evaluator fails.
#[test]
fn the_two_backends_agree_on_failure() {
    let _ = toolchain!();
    let both = Both::over("failure.beck", failfix::FAILURE);
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
        compared += both.agree(name, &ns);
    }
    compared += both.agree("named", &failfix::texts());
    compared += both.agree("several", &failfix::lists());
    compared += both.agree("all_checked", &failfix::lists());
    println!("{compared} fallible calls compared, LLVM against the tree-walker");
    assert!(compared >= 80, "only {compared} calls compared");
}

/// The message a raise crosses the boundary with is the evaluator's, value and all.
///
/// Asserted directly as well as differentially, because the differential compares the two backends
/// against each other and this says what the string *is*: a reader of a failing call sees which
/// value was raised, and a regression to "the compiled program failed" would still agree with
/// itself.
#[test]
fn an_uncaught_raise_names_the_value_it_carried() {
    let _ = toolchain!();
    let both = Both::over("failure.beck", failfix::FAILURE);
    for (n, want) in [(101, "raised `TooBig{n: 101}`"), (0, "raised `Blank`")] {
        let (walked, compiled) = both.call("uncaught", &[Value::Int(n)]);
        assert_eq!(compiled, Err(want.to_string()), "`uncaught({n})`");
        assert_eq!(walked, compiled);
    }
    // The control: the same definition, on an argument that does not raise.
    let (walked, compiled) = both.call("uncaught", &[Value::Int(2)]);
    assert_eq!(compiled, Ok(Value::Int(5)));
    assert_eq!(walked, compiled);
}

/// Unwinding costs nothing per frame, and the gate has **no clock in it**.
///
/// `AGENTS.md`'s shape gate for the one thing an error mechanism most easily gets wrong. A raise is
/// two words of arena for the pair and however many the value takes — a constant — and that has to
/// be true whether it was raised one frame down or two hundred. A scheme that allocated per frame
/// on the way out (a trace, a boxed error per level, a copy of the value at each check) would be
/// linear in the depth and would still answer correctly at every size.
///
/// `deeply` is deliberately **not** tail-recursive: every frame on the way out reads the error cell
/// and returns, which is the path being measured. The two depths are eight times apart.
#[test]
fn unwinding_costs_nothing_per_frame() {
    let _ = toolchain!();
    let both = Both::over("failure.beck", failfix::FAILURE);
    let mut sizes = Vec::new();
    for n in [25i64, 200] {
        let (answer, bytes) = both
            .native
            .call_sized("deeply_caught", &[Value::Int(n)])
            .expect("runs");
        assert_eq!(
            answer.variant(),
            Some("Err"),
            "`deeply_caught({n})` should have caught the raise at the bottom"
        );
        sizes.push((n, bytes));
    }
    assert_eq!(
        sizes[0].1, sizes[1].1,
        "a raise from {} frames down cost {} bytes of arena and one from {} cost {} — unwinding \
         is allocating per frame",
        sizes[0].0, sizes[0].1, sizes[1].0, sizes[1].1
    );
    println!(
        "a raise caught {} frames up and one caught {} frames up both leave {} bytes of arena",
        sizes[0].0, sizes[1].0, sizes[0].1
    );
}

/// An accumulator built with `list_append` is **linear**, and the gate has no clock in it.
///
/// This is `docs/93`'s claim and the reason the operation could be compiled at all. The idiom is
/// the one every loop in the language is written as — `f(…, list_append(acc, x))` in tail position —
/// and it was refused rather than shipped because with the count in front of the elements an append
/// can only copy, which is `Θ(n²)` where `beck-eval` is `Θ(n)` (`docs/46` §46.14).
///
/// Four times the elements must cost about four times the arena. A copying append leaves about
/// sixteen, which is what the text accumulator beside this still does — `docs/93` §93.7, and the
/// two tests are worth reading together: one asserts a quadratic and one asserts a linear, on the
/// same shape, in the same backend, because only one of the two layouts was separated.
#[test]
fn an_appended_accumulator_is_linear() {
    let _ = toolchain!();
    let both = Both::over("lists.beck", LISTS);
    let mut sizes = Vec::new();
    for n in [500usize, 2000] {
        let xs = Value::List(std::sync::Arc::new(
            (0..n as i64).map(Value::Int).collect::<Vec<_>>(),
        ));
        let arguments = both
            .native
            .module()
            .heap
            .encode_args(
                std::slice::from_ref(&xs),
                &both.native.module().signature("doubled_up").unwrap().params,
            )
            .expect("encodes")
            .1
            .len();
        let (answer, bytes) = both.native.call_sized("doubled_up", &[xs]).expect("runs");
        assert_eq!(
            answer.as_list().map(|xs| xs.len()),
            Some(n),
            "`doubled_up({n})` should answer with {n} elements"
        );
        sizes.push((n, bytes - arguments));
    }
    let (small, big) = (sizes[0], sizes[1]);
    let growth = big.1 as f64 / small.1 as f64;
    let steps = (big.0 / small.0) as f64;
    assert!(
        growth < steps * 2.0,
        "four times the elements left {growth:.1}× the arena, and an append that copies leaves \
         about {:.0}× — this is the quadratic `docs/93` exists to remove",
        steps * steps
    );
    println!(
        "doubled_up({}) left {} bytes and doubled_up({}) left {} — {growth:.1}× for {steps:.0}× \
         the elements",
        small.0, small.1, big.0, big.1
    );
}

/// A split costs **its answer**, and the gate has no clock in it.
///
/// The refusal on record said `str_split` "answers with a list whose elements it also allocates,
/// which is two loops rather than the one every list this backend builds has". Two loops is a
/// description of the code, not a cost: the first counts the pieces and the second fills them, so
/// the answer is allocated once and never grown. That is the claim, and this is where it is held.
///
/// Four times the separators must cost about four times the arena. What it would look like to be
/// wrong is a split that appended piece by piece, doubling a block it had already filled — which
/// is linear in the *pieces* and quadratic in the bytes it copies.
#[test]
fn a_split_costs_its_answer_and_nothing_per_call() {
    let _ = toolchain!();
    let both = Both::over("text.beck", TEXT);
    let mut sizes = Vec::new();
    for n in [500usize, 2000] {
        // `n` separators between `n + 1` one-character pieces, so the answer's size is the input's.
        // The definition answers the **list**, because a scalar reply ships no arena — and the
        // arena is where a split that had grown its answer would have left the blocks it abandoned.
        let args = vec![Value::str_(vec!["x"; n + 1].join(",")), Value::str_(",")];
        let arguments = both
            .native
            .module()
            .heap
            .encode_args(
                &args,
                &both.native.module().signature("parts").unwrap().params,
            )
            .expect("encodes")
            .1
            .len();
        let (answer, bytes) = both.native.call_sized("parts", &args).expect("runs");
        assert_eq!(
            answer.as_list().map(|xs| xs.len()),
            Some(n + 1),
            "splitting on {n} separators should answer {} pieces",
            n + 1
        );
        sizes.push((n, bytes - arguments));
    }
    let (small, big) = (sizes[0], sizes[1]);
    let growth = big.1 as f64 / small.1 as f64;
    let steps = (big.0 / small.0) as f64;
    assert!(
        growth < steps * 2.0,
        "four times the separators left {growth:.1}× the arena, and a split that grew its answer \
         piece by piece would leave about {:.0}×",
        steps * steps
    );
    println!(
        "parts on {} separators left {} bytes and on {} left {} — {growth:.1}× for {steps:.0}× \
         the separators",
        small.0, small.1, big.0, big.1
    );
}

/// A fold that keeps a `Map` is **not quadratic**, and the gate has no clock in it.
///
/// This is `docs/93`'s claim and the reason the operation could be compiled at all. `map_insert`
/// over a sorted run copies the whole run, so `n` inserts cost `Θ(n²)` — where `beck_core::pmap` is
/// `Θ(n log n)` because it rebuilds one path and shares the rest. `docs/93` §93.7 refused to ship
/// the first, and this asserts the second.
///
/// Four times the entries costs about *five* times the arena — `n log n` — where a copying insert
/// costs sixteen. The bound is generous on purpose: what separates the two is a factor of three, and
/// a gate that split them at 5.1 would be measuring the balance constants rather than the asymptote.
#[test]
fn a_fold_over_a_map_is_not_quadratic() {
    let _ = toolchain!();
    let both = Both::over("maps.beck", MAPS);
    let mut sizes = Vec::new();
    for n in [500i64, 2000] {
        let (answer, bytes) = both
            .native
            .call_sized("grown", &[Value::Int(n)])
            .expect("runs");
        assert_eq!(
            answer.as_map().map(beck_core::PMap::len),
            Some(n as usize),
            "`grown({n})` should answer with {n} entries"
        );
        sizes.push((n, bytes));
    }
    let (small, big) = (sizes[0], sizes[1]);
    let growth = big.1 as f64 / small.1 as f64;
    let steps = (big.0 / small.0) as f64;
    assert!(
        growth < steps * 2.0,
        "four times the entries left {growth:.1}× the arena, and an insert that copies the run \
         leaves about {:.0}× — this is the quadratic `docs/93` exists to remove",
        steps * steps
    );
    println!(
        "grown({}) left {} bytes and grown({}) left {} — {growth:.1}× for {steps:.0}× the entries",
        small.0, small.1, big.0, big.1
    );
}

/// The four primitives that ask the host, over both backends and one stated host.
///
/// What this is actually asserting is that a **question is answered the same way twice**: the
/// evaluator calls `Atoms` directly and the compiled program asks across a pipe, and the whole
/// point of `beck_llvm::service` is that the second one arrives at the first. A test that let the
/// two read the process clock would compare two instants and pass on a backend that answered the
/// wrong question.
#[test]
fn the_two_backends_agree_on_the_host_effects() {
    let _ = toolchain!();
    let atoms = Stated::new();
    let both = Both::answering("effects.beck", EFFECTS, atoms.clone());
    let mut compared = 0;
    for (name, args) in hostfix::calls() {
        compared += both.agree(name, std::slice::from_ref(&args));
    }
    // Both backends made every outbound call, rather than one of them being the evaluator twice:
    // five of the cases reach `http_fetch` once each, and the count is per backend.
    assert_eq!(
        atoms.asked(),
        10,
        "every `http_fetch` case has to have been asked by both backends"
    );
    // The set has to *contain* the failure, or this passed by never carrying a raise across the
    // boundary.
    let (walked, compiled) = both.call("unreachable", &[]);
    assert_eq!(walked, compiled);
    assert!(
        walked
            .expect_err("nowhere.invalid is unreachable")
            .contains("HttpUnreachable"),
        "an uncaught raise carries the value, not the fact of one"
    );
    println!("{compared} host-effect calls compared, and both backends agreed on every one");
}

/// What a question **carries**, counted rather than timed.
///
/// The protocol has one decision in it a reader would want checked rather than believed: a
/// question sends the live arena when — and only when — an argument could point into it. `now()`
/// takes nothing, so its question is 32 bytes and some words however much the program has
/// allocated; `secret_env` is handed text the program built, so the host cannot read it without
/// the bytes.
///
/// Two sizes, because one measurement cannot tell a constant from a slope (`AGENTS.md`), and no
/// clock, because this is a claim about what crosses the pipe rather than about how fast it does —
/// `docs/64` §64.1's pattern, and the kind of gate that does not flake on a shared runner.
#[test]
fn what_a_question_carries_is_a_decision_and_not_an_accident() {
    let _ = toolchain!();
    let atoms = Stated::new();
    let both = Both::answering("carried.beck", EFFECTS, atoms);

    // A question with no arguments: nothing of the arena travels, at either size.
    let mut clock = Vec::new();
    for n in [16, 4096] {
        both.native
            .call("clock_after", &[Value::Int(n)])
            .expect("the clock is answered");
        let (questions, carried) = both.native.questions();
        assert_eq!(questions, 1, "`now()` is one question");
        assert_eq!(
            carried, 0,
            "`now()` cannot point into the heap, so none of it travels — and {n} elements are live"
        );
        clock.push(carried);
    }

    // A question whose argument is text: the arena travels, and grows with what is live. This is
    // asserted as *growing* rather than smoothed away, because it is the cost of the decision and
    // a reader deciding whether to call `secret_env` in a loop needs to know it.
    let mut carried_by_size = Vec::new();
    for n in [16, 4096] {
        both.native
            .call("secret_after", &[Value::Int(n)])
            .expect("the secret is answered");
        let (questions, carried) = both.native.questions();
        assert_eq!(questions, 1, "`secret_env` is one question");
        carried_by_size.push(carried);
    }
    let [small, large] = carried_by_size[..] else {
        unreachable!("two sizes")
    };
    assert!(
        large > small * 8,
        "a question that can point into the arena carries it: {small} bytes at 16 elements and \
         {large} at 4,096"
    );

    // And the count is per *call of the primitive*, not per call of the definition: a loop that
    // mints four ids asks four times.
    both.native
        .call("several", &[Value::Int(4)])
        .expect("four ids");
    assert_eq!(both.native.questions().0, 4, "one question per iteration");
    println!(
        "a question with no arguments carries {} bytes at both sizes; one with text carries \
         {small} bytes over 16 elements and {large} over 4,096",
        clock[0]
    );
}

/// A list, taken apart by a **pattern** — the length test, the elements and the tail.
///
/// `docs/93` is what this is the differential for. The refusal it replaces said "a collection is
/// not on this heap yet", which stopped being true at [`docs/93`](../docs/93-the-native-backends-report.md)
/// and nothing noticed — the third time a refusal's *stated reason* has outlived the thing it
/// stated ([`docs/93`](../docs/93-the-native-backends-report.md) §93.9).
#[test]
fn the_two_backends_agree_on_list_patterns() {
    let _ = toolchain!();
    let both = Both::over("patterns.beck", PATTERNS);
    let xs = listfix::lists();
    let mut compared = 0;
    for name in [
        "described",
        "tail",
        "after_two",
        "leading_one",
        "exactly_two",
    ] {
        compared += both.agree(name, &listfix::singles(&xs));
    }
    compared += both.agree("inner_first", &listfix::singles(&listfix::nested()));
    compared += both.agree("joined", &listfix::singles(&listfix::texts()));
    // The set has to *contain* the boundaries, or this passed without reaching one.
    for (name, arg, want) in [
        ("described", vec![], "none"),
        ("described", vec![9], "one:9"),
        ("described", vec![1, 2, 3], "many:1:2"),
    ] {
        let list = Value::List(Arc::new(arg.into_iter().map(Value::Int).collect()));
        let (walked, compiled) = both.call(name, &[list]);
        assert_eq!(walked, compiled);
        assert_eq!(walked.expect("answers"), Value::str_(want));
    }
    println!("{compared} list-pattern calls compared, and both backends agreed on every one");
}
