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
use beck_llvm::{Artifact, Native, Scalar};

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

fn render(args: &[Value]) -> String {
    let parts: Vec<String> = args.iter().map(|a| a.display()).collect();
    format!("({})", parts.join(", "))
}

// -------------------------------------------------------------------------------------------
// Argument sets
// -------------------------------------------------------------------------------------------

/// A deterministic generator, so a failure is reproducible from the test name alone.
///
/// Numerical Recipes' 64-bit LCG. Not a good random number generator and not asked to be one: what
/// it has to do is produce the same sequence on every machine and every run, because a
/// differential that found a divergence yesterday and cannot find it today is not a gate.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
}

/// Integers a differential should care about: the boundaries, the small ones, and a sweep.
fn ints(seed: u64, count: usize) -> Vec<i64> {
    let mut out = vec![
        0,
        1,
        -1,
        2,
        -2,
        7,
        -7,
        i64::MAX,
        i64::MIN,
        i64::MAX - 1,
        i64::MIN + 1,
        4_294_967_296,
        -4_294_967_296,
    ];
    let mut lcg = Lcg(seed);
    while out.len() < count {
        out.push(lcg.next() as i64);
    }
    out
}

/// Reals a differential should care about: the two zeros, the infinities, a NaN, and a sweep of
/// whole bit patterns.
///
/// Whole bit patterns rather than plausible numbers, because the interesting disagreements are at
/// the representation — a subnormal, a signed zero, an exponent nobody would type. Every one goes
/// through `Value::float`, so what reaches both backends is what the language canonicalises it to.
fn floats(seed: u64, count: usize) -> Vec<f64> {
    let mut out = vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        2.0,
        1e308,
        -1e308,
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        f64::EPSILON,
    ];
    let mut lcg = Lcg(seed);
    while out.len() < count {
        out.push(f64::from_bits(lcg.next()));
    }
    out
}

/// Every pair from `xs`, as argument tuples.
fn pairs(xs: &[i64]) -> Vec<Vec<Value>> {
    let mut out = Vec::with_capacity(xs.len() * xs.len());
    for a in xs {
        for b in xs {
            out.push(vec![Value::Int(*a), Value::Int(*b)]);
        }
    }
    out
}

fn singles(xs: &[i64]) -> Vec<Vec<Value>> {
    xs.iter().map(|x| vec![Value::Int(*x)]).collect()
}

fn float_pairs(xs: &[f64]) -> Vec<Vec<Value>> {
    let mut out = Vec::with_capacity(xs.len() * xs.len());
    for a in xs {
        for b in xs {
            out.push(vec![Value::float(*a), Value::float(*b)]);
        }
    }
    out
}

// -------------------------------------------------------------------------------------------
// The programs
// -------------------------------------------------------------------------------------------

/// Every integer operation, each in a definition of its own so a divergence names the operator.
///
/// Total for every input: nothing here loops.
const ARITHMETIC: &str = r#"
def plus(a: Int, b: Int) -> Int:
    return a + b

def minus(a: Int, b: Int) -> Int:
    return a - b

def times(a: Int, b: Int) -> Int:
    return a * b

def over(a: Int, b: Int) -> Int:
    return a / b

def modulo(a: Int, b: Int) -> Int:
    return a % b

def negated(a: Int) -> Int:
    return negate(a)

def absolute(a: Int) -> Int:
    return abs(a)

def compares(a: Int, b: Int) -> Int:
    if a < b:
        return -1
    if a > b:
        return 1
    if a == b:
        return 0
    return 99

def orders(a: Int, b: Int) -> Bool:
    return (a <= b) == (not (a > b))

def logic(a: Int, b: Int) -> Bool:
    return ((a == b) and (a != b)) or (not (a >= b))

## Two operations in one expression, so a trap in the middle of a computation is exercised rather
## than only a trap that is the whole of one.
def chained(a: Int, b: Int) -> Int:
    return (a * b) + (a - b)
"#;

/// The real half. `Value::float` canonicalises `-0.0` and NaN, and the comparisons go through the
/// order key rather than `fcmp`, so both are what these are for.
const REALS: &str = r#"
def rplus(a: Float, b: Float) -> Float:
    return a + b

def rminus(a: Float, b: Float) -> Float:
    return a - b

def rtimes(a: Float, b: Float) -> Float:
    return a * b

def rover(a: Float, b: Float) -> Float:
    return a / b

def rnegated(a: Float) -> Float:
    return negate(a)

def rabs(a: Float) -> Float:
    return abs(a)

def rsqrt(a: Float) -> Float:
    return sqrt(a)

def rsin(a: Float) -> Float:
    return sin(a)

def rcos(a: Float) -> Float:
    return cos(a)

def truncated(a: Float) -> Int:
    return trunc(a)

def widened(a: Int) -> Float:
    return float(a)

## The signed zero the evaluator normalises away, reachable only through a division.
def reciprocal_of_product(a: Float, b: Float) -> Float:
    return 1.0 / (a * b)

## …and the same zero reaching a *comparison* rather than a division. `0.0 * -1.0` is a negative
## zero, and the language says the two zeros are one value — so these must answer `true` and `0`.
def product_is_zero(a: Float, b: Float) -> Bool:
    return (a * b) == 0.0

def product_order(a: Float, b: Float) -> Int:
    p = a * b
    if p < 0.0:
        return -1
    if p > 0.0:
        return 1
    return 0

## A negative zero carried through three more operations before anything looks at it.
def zero_through_sqrt(a: Float, b: Float) -> Bool:
    return sqrt(negate(abs(a * b))) == 0.0

## And one returned across the boundary, where the host is what normalises it.
def signed_zero(a: Float, b: Float) -> Float:
    return a * b

def rless(a: Float, b: Float) -> Bool:
    return a < b

def requal(a: Float, b: Float) -> Bool:
    return a == b

def rorder(a: Float, b: Float) -> Int:
    if a < b:
        return -1
    if a > b:
        return 1
    return 0
"#;

/// Control flow: `match` with constants, or-patterns, guards, `@`, and a `let`.
const CONTROL: &str = r#"
def classify(n: Int) -> Int:
    match n:
        case 0:
            return 100
        case 1 | 2 | 3:
            return 200
        case whole @ 7:
            return whole * 3
        case k if k > 1000:
            return k - 1000
        case k if k < 0:
            return negate(k)
        case _:
            return -1

## A `match` on a Bool constant. The second arm is a wildcard rather than `case false:` because
## the checker does not read two Bool constants as exhaustive — a limitation of `check`, not of
## this backend, and not this suite's to assert about.
def truthy(b: Bool) -> Int:
    match b:
        case true:
            return 1
        case _:
            return 0

def nested(a: Int, b: Int) -> Int:
    x = a + 1
    y = b + 2
    if x < y:
        z = x * 2
        return z + y
    return y - x

def shadowing(a: Int) -> Int:
    x = a + 1
    y = x * 2
    return x + y

def guard_falls_through(n: Int) -> Int:
    match n:
        case k if k > 100:
            return 1
        case k if k > 10:
            return 2
        case 5:
            return 3
        case _:
            return 4
"#;

/// Recursion, direct and mutual, with tail calls and without.
///
/// Every one of these is bounded by its argument, so the tuples they are given are small on
/// purpose — see the module comment.
const RECURSION: &str = r#"
def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def gcd(a: Int, b: Int) -> Int:
    if b == 0:
        return a
    return gcd(b, a % b)

def even(n: Int) -> Bool:
    if n == 0:
        return true
    return odd(n - 1)

def odd(n: Int) -> Bool:
    if n == 0:
        return false
    return even(n - 1)

def sum_to(n: Int, acc: Int) -> Int:
    if n <= 0:
        return acc
    return sum_to(n - 1, acc + n)

## A tail call to a definition of a *different* arity — the one the C calling convention's
## `musttail` cannot express, and the reason the emitted functions use `tailcc`.
def double(acc: Int) -> Int:
    return acc * 2

def drain(n: Int, acc: Int) -> Int:
    if n <= 0:
        return double(acc)
    return drain(n - 1, acc + 1)

def ackermann(m: Int, n: Int) -> Int:
    if m == 0:
        return n + 1
    if n == 0:
        return ackermann(m - 1, 1)
    return ackermann(m - 1, ackermann(m, n - 1))
"#;

/// Definitions this backend must refuse, one per reason.
const REFUSED: &str = r#"
model Point:
    x: Int
    y: Int

union Shape:
    Circle(r: Int)
    Square(s: Int)

def takes_a_record(p: Point) -> Int:
    return p.x + p.y

def builds_a_record(x: Int) -> Point:
    return Point(x=x, y=x)

def takes_a_list(xs: list[Int]) -> Int:
    return list_len(xs)

def builds_a_string(n: Int) -> Str:
    return str(n)

def matches_a_union(s: Shape) -> Int:
    match s:
        case Circle(r):
            return r
        case Square(s):
            return s

def is_generic[T](x: T) -> T:
    return x

def reads_the_clock() -> Int:
    return now()

def calls_something_refused(n: Int) -> Int:
    return takes_a_record(Point(x=n, y=n))

def scalar_and_fine(n: Int) -> Int:
    return n * 2
"#;

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
/// lowering, is what every other language does, and is wrong here, because `docs/32` §32.2 made
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
        ("takes_a_record", "parameter `p` is `Point`"),
        ("builds_a_record", "returns `Point`"),
        ("takes_a_list", "parameter `xs` is `list[Int]`"),
        ("builds_a_string", "returns `Str`"),
        ("matches_a_union", "parameter `s` is `Shape`"),
        ("is_generic", "generic over T"),
        (
            "reads_the_clock",
            "`now` is not one of the scalar primitives",
        ),
        ("calls_something_refused", "calls `takes_a_record`"),
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
        .call("takes_a_record", &[Value::Int(1)])
        .expect_err("a refused definition is not callable natively");
    assert!(err.message.contains("did not compile natively"), "{err}");
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
model Box:
    v: Int

def bottom(b: Box) -> Int:
    return b.v

def middle(n: Int) -> Int:
    return bottom(Box(v=n))

def top(n: Int) -> Int:
    return middle(n) + 1

## Mutually recursive, and both must go: `ping` is only refusable through `pong` and the other way
## round, so a single pass in either direction keeps one of them.
def ping(n: Int) -> Int:
    if n == 0:
        return bottom(Box(v=0))
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
/// `docs/31` §31.2 makes this a property of the *language*, and until now it was a property of one
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

    // A backend that wraps a tree-walker inherits its stack requirement — the trap `docs/31` §31.3
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

    let refused = &program.defs["takes_a_record"].body;
    assert!(!native.compiled(refused));

    let f = beck_eval::on_the_evaluator_stack(|| native.function(refused).expect("prepares"));
    let point = Value::data(
        Arc::from("Point"),
        None,
        beck_core::core::Fields::from_pairs(vec![
            (Arc::from("x"), Value::Int(2)),
            (Arc::from("y"), Value::Int(3)),
        ]),
    );
    let got = beck_eval::on_the_evaluator_stack(|| f(vec![point]).expect("the fallback answers"));
    assert_eq!(got, Value::Int(5));
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
    assert_eq!(module.functions[0].params, vec![Scalar::Int]);
    assert_eq!(module.functions[0].ret, Scalar::Int);
    assert_eq!(module.functions[0].index, 0);
    assert!(module.refusals.len() >= 8, "{:?}", module.refusals);
    for refusal in &module.refusals {
        assert!(
            !refusal.reason.is_empty(),
            "`{}` was refused without a reason",
            refusal.name
        );
    }
}
