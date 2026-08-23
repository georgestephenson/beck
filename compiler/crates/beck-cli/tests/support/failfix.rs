//! The programs a differential over **failure** needs, and the arguments to run them on.
//!
//! Shared for the reason [`super::clofix`] is: three backends are held to these, and a second copy
//! of the programs would be a second opinion about what the subset is.
//!
//! # What these are chosen to catch
//!
//! A `raise` is the one failure a compiled program has that is not a fault: it carries a **value**,
//! it is what the program's own type says can happen, and a `try:` may turn it into an `Err`. So
//! what a differential over one has to look for is three different things agreeing — the value that
//! comes back, the `Result` that comes back, and the *message* when nothing catches it.
//!
//! * **A raise nothing catches**, which crosses the boundary. `beck-eval` renders it as
//!   ``raised `TooBig{n: 101}` ``, so a backend that said "something was raised" would be a
//!   divergence the outcome comparison shows.
//! * **A raise a `try:` catches**, which never leaves the call.
//! * **A raise of a variant with no fields**, because an object of one word is where a payload read
//!   past the end would land.
//! * **A `try:` whose block does not fail**, which is the `Ok` side and the one that would still
//!   pass if the handler were never reached.
//! * **A `try:` that must not catch**: a *different* error type raised inside it, which belongs to a
//!   handler further out, and an **overflow** inside it, because a fault is not a failure. Both are
//!   the evaluator's own rule, and both are cases where a handler that caught by code rather than by
//!   type would answer an `Err` where the evaluator fails.
//! * **A nested `try:`**, where the inner one catches one type and the outer the other, so the
//!   handler stack has to be a stack.
//! * **A raise from a callee**, which is the shape every program has: the decision is where it
//!   belongs and the handler is at the boundary.
//! * **A raise inside a loop's closure**, since a `map_list` is a generated loop and a failure has
//!   to leave it rather than carry on to the next element.
//! * **A raise carrying text and a list**, because the value is decoded by the host out of the arena
//!   and a reference field is where a shape read as the wrong thing shows.
//! * **A `try:` in tail position**, which is the case where the block must *not* be emitted as a
//!   tail call — one of those does not check the error cell and would walk straight through the
//!   handler.

#![allow(dead_code)] // each suite uses the half of this it needs

use beck_core::Value;

pub const FAILURE: &str = r#"
union Bad:
    TooBig(n: Int)
    Blank
    Named(who: Str)
    Several(ns: list[Int])

union Other:
    Wrong

# The decision, where it belongs.
def checked(n: Int) -> Int uses raises(Bad):
    if n > 100:
        raise TooBig(n=n)
    if n == 0:
        raise Blank
    return n * 2

# Nothing catches: the raise crosses the boundary and the host builds the message.
def uncaught(n: Int) -> Int uses raises(Bad):
    return checked(n) + 1

# The handler, at the boundary. In tail position, which is the case a `musttail` would break.
def caught(n: Int) -> Result[Int, Bad]:
    return try:
        checked(n)

# The same, not in tail position: the `Result` is taken apart afterwards.
def described(n: Int) -> Str:
    r = try:
        checked(n)
    match r:
        case Ok(value):
            return str(value)
        case Err(error):
            match error:
                case TooBig(n):
                    return "too big"
                case Blank:
                    return "blank"
                case Named(who):
                    return who
                case Several(ns):
                    return str(list_len(ns))

# A raise carrying text, and one carrying a list: the value is decoded out of the arena.
def named(s: Str) -> Result[Int, Bad]:
    return try:
        if str_is_empty(s):
            raise Named(who="nobody")
        str_len(s)

def several(xs: list[Int]) -> Result[Int, Bad]:
    return try:
        if list_is_empty(xs):
            raise Several(ns=xs)
        list_len(xs)

# A fault inside a `try:` is not a failure: this overflows, and the handler must not catch it.
def overflows(n: Int) -> Result[Int, Bad]:
    return try:
        if n < 0:
            raise Blank
        checked(n) * 9223372036854775807

# A *different* error type raised inside a `try:` belongs to a handler further out.
def wrongly(n: Int) -> Int uses raises(Other):
    if n < 0:
        raise Wrong
    return n

def wrong_type(n: Int) -> Result[Int, Bad] uses raises(Other):
    return try:
        checked(wrongly(n))

# Two handlers, one inside the other, catching two different types.
def nested(n: Int) -> Result[Int, Other]:
    return try:
        inner: Result[Int, Bad] = try:
            checked(wrongly(n))
        match inner:
            case Ok(value):
                value
            case Err(error):
                0 - 1

# A raise inside a generated loop, which has to leave the loop rather than run the next element.
def all_checked(xs: list[Int]) -> Result[list[Int], Bad]:
    return try:
        map_list(xs, lambda x: checked(x))

# Raised at the bottom of `n` frames, and **not** in tail position — so every one of those frames
# checks the cell and returns. What it is for is the arena: unwinding must cost nothing per frame.
def deeply(n: Int) -> Int uses raises(Bad):
    if n <= 0:
        raise Blank
    return deeply(n - 1) + 1

def deeply_caught(n: Int) -> Result[Int, Bad]:
    return try:
        deeply(n)
"#;

/// What is still refused, each with the reason a reader is given.
pub const REFUSED: &str = r#"
union Bad:
    TooBig(n: Int)

def raises_a_number(n: Int) -> Int uses raises(Bad):
    if n > 0:
        raise TooBig(n=n)
    return n
"#;

pub fn ints(xs: &[i64]) -> Vec<Vec<Value>> {
    xs.iter().map(|n| vec![Value::Int(*n)]).collect()
}

/// The numbers every fallible definition is run on: the two that raise, the two that do not, and
/// the boundaries where an overflow is the failure instead.
pub fn numbers() -> Vec<i64> {
    vec![-1, 0, 1, 2, 50, 100, 101, 1_000, i64::MAX, i64::MIN]
}

pub fn texts() -> Vec<Vec<Value>> {
    ["", "a", "beck", "é"]
        .iter()
        .map(|s| vec![Value::str_(s)])
        .collect()
}

pub fn lists() -> Vec<Vec<Value>> {
    [vec![], vec![1], vec![1, 2, 3], vec![0], vec![101, 1]]
        .into_iter()
        .map(|xs| vec![Value::list(xs.into_iter().map(Value::Int).collect())])
        .collect()
}
