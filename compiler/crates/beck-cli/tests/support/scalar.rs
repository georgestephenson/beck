//! The programs, the arguments and the two helpers a differential over the scalar subset needs.
//!
//! Shared because there are three backends to hold to them now — the tree-walker, LLVM
//! ([`native.rs`]) and Cranelift ([`cranelift.rs`]) — and a second copy of these programs would be
//! a second opinion about what the subset *is*. The fixtures are one definition; what differs
//! between the two suites is which backends they point at.

#![allow(dead_code)] // each suite uses the half of this it needs

use beck_core::Value;

pub fn render(args: &[Value]) -> String {
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
pub struct Lcg(u64);

impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
}

/// Integers a differential should care about: the boundaries, the small ones, and a sweep.
pub fn ints(seed: u64, count: usize) -> Vec<i64> {
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
pub fn floats(seed: u64, count: usize) -> Vec<f64> {
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
pub fn pairs(xs: &[i64]) -> Vec<Vec<Value>> {
    let mut out = Vec::with_capacity(xs.len() * xs.len());
    for a in xs {
        for b in xs {
            out.push(vec![Value::Int(*a), Value::Int(*b)]);
        }
    }
    out
}

pub fn singles(xs: &[i64]) -> Vec<Vec<Value>> {
    xs.iter().map(|x| vec![Value::Int(*x)]).collect()
}

pub fn float_pairs(xs: &[f64]) -> Vec<Vec<Value>> {
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
pub const ARITHMETIC: &str = r#"
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
pub const REALS: &str = r#"
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
pub const CONTROL: &str = r#"
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
pub const RECURSION: &str = r#"
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
///
/// The heap ([`super::heapfix`]) took the record and the union off this list, `docs/93` took text
/// off it, and `docs/93` took *reading* a collection off it — what is left of one is the half that
/// **grows** it. `str` of an `Int` came off it too, so the row here is a **real**, whose shortest
/// round-trip decimal is an algorithm rather than a loop. The **host effects** came off it when the
/// worker's protocol grew a second direction, and `reads_the_clock` stayed in the program as a
/// control rather than being deleted. What is left is what `docs/93` §93.15 names as not built —
/// and `scalar_and_fine` is the other control: a list of refusals with nothing on the other side of
/// it would pass against a backend that refused everything.
pub const REFUSED: &str = r#"
def grows_a_list(xs: list[list[Int]]) -> list[Int]:
    return list_flat_map(xs, lambda ys: ys)

def renders_a_real(x: Float) -> Str:
    return str(x)

def is_generic[T](x: T) -> T:
    return x

def calls_something_refused(n: Int) -> list[Int]:
    return grows_a_list([[n]])

# The control, twice over: a definition with nothing wrong with it, and one that reaches the host.
# The second was on the list above until the protocol grew a second direction.
def reads_the_clock() -> Int:
    return now()

def scalar_and_fine(n: Int) -> Int:
    return n * 2
"#;
