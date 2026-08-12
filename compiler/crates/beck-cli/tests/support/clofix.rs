//! The programs a differential over **closures** needs, and the arguments to run them on.
//!
//! Shared for the reason [`super::listfix`] is: three backends are held to these, and a second copy
//! of the programs would be a second opinion about what the subset is.
//!
//! # What these are chosen to catch
//!
//! * **A closure that captures nothing** beside one that captures — the object is one word in the
//!   first case and two in the second, and a backend that read a capture out of the first would
//!   read the word after the closure.
//! * **A definition named as a value**, whose arm calls the definition itself rather than a lambda
//!   of its own. Its rank has to be the one the survey gave the definition's own outermost `lam`,
//!   and the arm has to pass no closure — one operand fewer than every other arm.
//! * **Two closures of one family**, so the application's switch has more than one arm. A switch
//!   with a single arm is the case where every rank answers correctly by accident.
//! * **A closure applied twice**, so a rank read once is not what makes the second call work.
//! * **An element that is an offset** — a list of `Str` mapped to their lengths — because the loops
//!   hand a *word* to a closure and the conversion is what the repr decides.
//! * **Reals**, because a word becomes a `double` on the way in and is normalised on the way out:
//!   `-0.0` stored into a list where the evaluator stores `0.0` is a divergence the answer shows.
//! * **The empty list**, which is every loop's zero-iteration case — including `list_all`, whose
//!   answer on it is `true` and comes from the path no element reaches.
//! * **A closure that traps**, because a loop that carried on after one would run the remaining
//!   iterations of a program that has already failed.
//! * **Comparing two closures**, which the evaluator answers from the parameters and where the body
//!   starts, and which a rank reproduces only if ranks are ordered the same way.

#![allow(dead_code)] // each suite uses the half of this it needs

use std::sync::Arc;

use beck_core::Value;

pub fn ints(xs: &[i64]) -> Value {
    Value::List(Arc::new(xs.iter().map(|n| Value::Int(*n)).collect()))
}

/// The lists of `Int` every loop is exercised over.
pub fn lists() -> Vec<Value> {
    vec![
        ints(&[]),
        ints(&[0]),
        ints(&[1]),
        ints(&[-1]),
        ints(&[1, 2]),
        ints(&[2, 1]),
        ints(&[-3, 0, 3]),
        ints(&[7, 7, 7]),
        ints(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
    ]
}

/// Lists whose elements are offsets rather than numbers.
pub fn texts() -> Vec<Value> {
    [
        vec![],
        vec![""],
        vec!["a"],
        vec!["a", "bb"],
        vec!["é", "aa"],
    ]
    .iter()
    .map(|xs| Value::List(Arc::new(xs.iter().map(Value::str_).collect())))
    .collect()
}

/// Lists of reals, including the two zeros and the values that are not numbers.
pub fn reals() -> Vec<Value> {
    [
        vec![],
        vec![0.0],
        vec![-0.0],
        vec![1.5, -2.5],
        vec![f64::INFINITY, f64::NEG_INFINITY],
        vec![f64::NAN, 0.0],
    ]
    .iter()
    .map(|xs| Value::List(Arc::new(xs.iter().map(|f| Value::float(*f)).collect())))
    .collect()
}

/// Every pair of these numbers, as arguments.
pub fn pairs_of(ns: &[i64]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for a in ns {
        for b in ns {
            out.push(vec![Value::Int(*a), Value::Int(*b)]);
        }
    }
    out
}

/// Every triple, which `between` needs: a low, a high and the number between or outside them.
pub fn triples_of(ns: &[i64]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for a in ns {
        for b in ns {
            for c in ns {
                out.push(vec![Value::Int(*a), Value::Int(*b), Value::Int(*c)]);
            }
        }
    }
    out
}

/// Each of these numbers on its own.
pub fn each_of(ns: &[i64]) -> Vec<Vec<Value>> {
    ns.iter().map(|n| vec![Value::Int(*n)]).collect()
}

/// Each number with each `Bool`, which `either` needs to reach both arms of one switch.
pub fn flagged(ns: &[i64]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for n in ns {
        for up in [true, false] {
            out.push(vec![Value::Int(*n), Value::Bool(up)]);
        }
    }
    out
}

pub fn singles(xs: &[Value]) -> Vec<Vec<Value>> {
    xs.iter().map(|x| vec![x.clone()]).collect()
}

/// Each list with a second argument the closure captures.
pub fn with(xs: &[Value], ns: &[i64]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for x in xs {
        for n in ns {
            out.push(vec![x.clone(), Value::Int(*n)]);
        }
    }
    out
}

/// A closure, every way this backend can meet one.
pub const CLOSURES: &str = r#"
def double(n: Int) -> Int:
    return n * 2

## Built and applied where it is written, capturing nothing.
def twice(x: Int) -> Int:
    f = lambda y: y + y
    return f(x)

## Capturing a parameter, which is the word the closure has to carry.
def add_on(x: Int, y: Int) -> Int:
    f = lambda z: z + x
    return f(y)

## Two captures, so the order the object holds them in is what decides the answer.
def between(low: Int, high: Int, n: Int) -> Bool:
    inside = lambda x: x >= low and x <= high
    return inside(n)

## A definition named as a value: the closure carries nothing and the arm calls `double`.
def through(n: Int) -> Int:
    g = double
    return g(n) + 1

## Two closures of one shape, so the switch has two arms and the choice is made at run time.
def either(n: Int, up: Bool) -> Int:
    grow = lambda x: x * 3
    shrink = lambda x: x - 3
    f = grow if up else shrink
    return f(n)

## One closure applied twice.
def again(n: Int) -> Int:
    f = lambda x: x + 1
    return f(f(n))

## A closure inside a closure, capturing from both levels.
def nested(a: Int, b: Int) -> Int:
    outer = lambda x: (lambda y: y + a + b)(x)
    return outer(1)

## A closure that traps, so the loop stops where the evaluator stops.
def risky(xs: list[Int]) -> list[Int]:
    return map_list(xs, lambda x: 9223372036854775807 + x)

## The five loops.
def doubled(xs: list[Int]) -> list[Int]:
    return map_list(xs, double)

def scaled(xs: list[Int], by: Int) -> list[Int]:
    return map_list(xs, lambda x: x * by)

def kept(xs: list[Int], least: Int) -> list[Int]:
    return filter_list(xs, lambda x: x >= least)

def summed(xs: list[Int]) -> Int:
    return list_fold(xs, 0, lambda acc, x: acc + x)

def biggest(xs: list[Int], start: Int) -> Int:
    return list_fold(xs, start, lambda acc, x: acc if acc > x else x)

def all_above(xs: list[Int], least: Int) -> Bool:
    return list_all(xs, lambda x: x > least)

def any_above(xs: list[Int], least: Int) -> Bool:
    return list_any(xs, lambda x: x > least)

## An element that is an offset, and a result that is not.
def lengths(xs: list[Str]) -> list[Int]:
    return map_list(xs, lambda s: str_len(s))

## An element and a result that are both offsets.
def shouted(xs: list[Str]) -> list[Str]:
    return map_list(xs, lambda s: s + "!")

def long_ones(xs: list[Str]) -> list[Str]:
    return filter_list(xs, lambda s: str_len(s) > 1)

def joined(xs: list[Str]) -> Str:
    return list_fold(xs, "", lambda acc, s: acc + s)

## Reals, so the conversion and the normalisation both run.
def halved(xs: list[Float]) -> list[Float]:
    return map_list(xs, lambda x: x / 2.0)

def negated(xs: list[Float]) -> list[Float]:
    return map_list(xs, lambda x: x * -1.0)

def added(xs: list[Float]) -> Float:
    return list_fold(xs, 0.0, lambda acc, x: acc + x)

## Bools, which are the other repr a word is not.
def flags(xs: list[Int]) -> list[Bool]:
    return map_list(xs, lambda x: x > 0)

## An accumulator that is a record, so a fold carries an offset through.
model Run:
    count: Int
    sum: Int

def tally(xs: list[Int]) -> Run:
    return list_fold(xs, Run(count=0, sum=0), lambda acc, x: Run(count=acc.count + 1, sum=acc.sum + x))

## A closure over a closure: `map_list` given a function that a fold built.
def twice_over(xs: list[Int], by: Int) -> list[Int]:
    step = lambda x: x + by
    return map_list(map_list(xs, step), step)

## A tail call *through* an application: the closure is this definition itself, so every hop is
## apply-then-lambda and neither may spend a frame.
def spin(n: Int, acc: Int) -> Int:
    if n <= 0:
        return acc
    f = spin
    return f(n - 1, acc + n)

## What a loop leaves in the arena. The fold builds nothing, so the arena it leaves is one closure
## and the one-element list this answers with — the same at any length. The answer is a list rather
## than the number because a call whose result is not on the heap is sent back with no heap at all,
## and then there is nothing to measure.
def counted(xs: list[Int]) -> list[Int]:
    return [list_fold(xs, 0, lambda acc, x: acc + 1)]

## The comparisons, which the evaluator answers from where a lambda is written.
def same_lambda() -> Bool:
    f = lambda x: x + 1
    g = f
    return f == g

def two_lambdas() -> Bool:
    f = lambda x: x + 1
    g = lambda x: x + 2
    return f == g

def ordered() -> Bool:
    f = lambda x: x + 1
    g = lambda x: x + 2
    return f < g

## One shape, two closures, compared — with a capture, since a captured frame is deliberately not
## part of what `Closure`'s `Ord` looks at.
def captures_ignored(a: Int, b: Int) -> Bool:
    f = lambda x: x + a
    g = lambda x: x + b
    return f == g
"#;

/// What a closure may **not** be: every boundary the host would have to read one across.
pub const REFUSED: &str = r#"
def double(n: Int) -> Int:
    return n * 2

## A parameter: the caller would have to marshal one.
def applies(f: (Int) -> Int, n: Int) -> Int:
    return f(n)

## A result: the reply would have to carry one.
def picked(up: Bool) -> (Int) -> Int:
    return double if up else double

## A field of a record that crosses.
model Rule:
    apply_to: (Int) -> Int

def held(n: Int) -> Int:
    r = Rule(apply_to=double)
    f = r.apply_to
    return f(n)

## An element of a list.
def listed(n: Int) -> Int:
    fs = [double]
    return list_len(fs) + n

## The two higher-order primitives that are not a pass over a list.
def sorted_by(xs: list[Int]) -> list[Int]:
    return sort_by(xs, lambda x: 0 - x)

def spread(xs: list[Int]) -> list[Int]:
    return list_flat_map(xs, lambda x: [x, x])
"#;
