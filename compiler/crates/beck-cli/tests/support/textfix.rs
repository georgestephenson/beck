//! The programs and the strings a differential over **text** needs.
//!
//! Shared for the reason [`super::heapfix`] is shared: three backends are held to these — the
//! tree-walker, LLVM (`native.rs`) and Cranelift (`cranelift.rs`) — and a second copy of the
//! programs would be a second opinion about what the subset is.
//!
//! # What the strings are chosen to catch
//!
//! A text layout can be wrong in ways an answer still looks plausible, and every string in
//! [`strings`] is there because one specific mistake would survive without it:
//!
//! * **The empty string.** Its object is two header words and no bytes, and it is the one input
//!   where `memcmp` is asked for zero bytes and a slice's allocation has nothing to pad.
//! * **A string containing a NUL byte.** The layout is length-prefixed, so a NUL is an ordinary
//!   byte — an implementation that reached for `strlen` anywhere would answer a shorter string,
//!   and nothing else in this suite would notice.
//! * **Two-, three- and four-byte characters** — `é`, `日本語`, `🎈`. A character index is a byte
//!   index only for ASCII, and `str_len` and `str_slice` answer in characters: a backend that
//!   confused the two gets the right answer on every ASCII input and the wrong one here.
//! * **A string whose first bytes are another's** — `"ab"` beside `"abc"`. `memcmp` over the
//!   shorter length answers `0` for that pair, so the length has to decide afterwards or `<` is
//!   wrong in exactly one direction.
//! * **A string long enough to be padded oddly** — a 17-byte one, which is neither a whole number
//!   of words nor one byte short of one.
//! * **A string that is also a literal of the program.** `"yes"` arrives both as an argument and
//!   out of the pool, and the two have to compare equal — which they do not if the pool's
//!   character count is computed differently from the host's.

#![allow(dead_code)] // each suite uses the half of this it needs

use beck_core::Value;

/// The strings every definition below is exercised over.
pub fn strings() -> Vec<Value> {
    [
        "",
        "a",
        "ab",
        "abc",
        "abd",
        "yes",
        "no",
        "héllo",
        "日本語",
        "🎈x",
        "a\0b",
        "the seventeen ok",
    ]
    .iter()
    .map(Value::str_)
    .collect()
}

/// The same, as one-argument calls.
pub fn singles(xs: &[Value]) -> Vec<Vec<Value>> {
    xs.iter().map(|x| vec![x.clone()]).collect()
}

/// Every ordered pair, which is what a comparison has to be asked for.
pub fn pairs(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::with_capacity(xs.len() * xs.len());
    for a in xs {
        for b in xs {
            out.push(vec![a.clone(), b.clone()]);
        }
    }
    out
}

/// Each string with each index and length a slice could be asked for.
///
/// The indices go past both ends and below zero, because `str_slice` **clamps** rather than
/// failing — so the interesting inputs are the ones where the clamp decides the answer, and a
/// backend that only ever slices inside a string never reaches them.
pub fn slices(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for s in xs {
        for at in [-2i64, -1, 0, 1, 2, 3, 5, 12, i64::MAX] {
            for n in [-1i64, 0, 1, 2, 5, 100, i64::MAX] {
                out.push(vec![s.clone(), Value::Int(at), Value::Int(n)]);
            }
        }
    }
    out
}

/// Each string with each single character of the alphabet the fixtures use.
pub fn with_char(xs: &[Value]) -> Vec<Vec<Value>> {
    let chars: Vec<Value> = ["a", "b", "é", "語", "\0", ""]
        .iter()
        .map(Value::str_)
        .collect();
    let mut out = Vec::new();
    for s in xs {
        for c in &chars {
            out.push(vec![s.clone(), c.clone(), Value::Int(0), Value::Int(0)]);
        }
    }
    out
}

/// Each string repeated a handful of times, which is what exercises the arena.
pub fn repeats(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for s in xs {
        for n in [0i64, 1, 2, 7] {
            out.push(vec![s.clone(), Value::Int(n), Value::str_("")]);
        }
    }
    out
}

/// Text, every way this backend can meet it.
pub const TEXT: &str = r#"
model Named:
    label: Str
    rank: Int

union Tagged:
    Word(text: Str)
    Number(n: Int)

def joined(a: Str, b: Str) -> Str:
    return a + b

def thrice(a: Str) -> Str:
    return a + a + a

def size(s: Str) -> Int:
    return str_len(s)

def empty(s: Str) -> Bool:
    return str_is_empty(s)

def cut(s: Str, at: Int, n: Int) -> Str:
    return str_slice(s, at, n)

def first(s: Str) -> Str:
    return str_slice(s, 0, 1)

def rest(s: Str) -> Str:
    return str_slice(s, 1, str_len(s))

## The six comparisons, each written out, because a three-way answer can be right for `<` and
## wrong for `<=` and only one of them would be asked otherwise.
def below(a: Str, b: Str) -> Bool:
    return a < b

def above(a: Str, b: Str) -> Bool:
    return a > b

def same(a: Str, b: Str) -> Bool:
    return a == b

def differ(a: Str, b: Str) -> Bool:
    return a != b

def not_after(a: Str, b: Str) -> Bool:
    return a <= b

def not_before(a: Str, b: Str) -> Bool:
    return a >= b

def inside(a: Str, b: Str) -> Bool:
    return str_contains(a, b)

def opens(a: Str, b: Str) -> Bool:
    return str_starts_with(a, b)

def closes(a: Str, b: Str) -> Bool:
    return str_ends_with(a, b)

## Answers with an `Option`, which is the prelude's union and has a layout like any other — the
## thing `docs/104` §104.4 said it did not.
def at(a: Str, b: Str) -> Option[Int]:
    return str_index_of(a, b)

## …and the same answer taken apart, so a wrong tag is a wrong `Int` rather than a value nobody
## looks inside.
def at_or(a: Str, b: Str, fallback: Int) -> Int:
    match str_index_of(a, b):
        case Some(value):
            return value
        case None():
            return fallback

## Literals: the pool, one of them read twice in one expression.
def greeting(s: Str) -> Str:
    return "hello, " + s + "!"

def is_yes(s: Str) -> Bool:
    return s == "yes"

def echoed(s: Str) -> Str:
    return "«" + s + "»"

def which(s: Str) -> Int:
    match s:
        case "one":
            return 1
        case "two":
            return 2
        case _:
            return 0

## Text in a record. `label` sorts before `rank`, so the `Str` field is what decides `<` — a layout
## that put text anywhere but the first slot answers this one backwards.
def named(label: Str, rank: Int) -> Named:
    return Named(label = label, rank = rank)

def relabel(n: Named, label: Str) -> Named:
    return n.with(label = label)

def label_of(n: Named) -> Str:
    return n.label

def named_below(a: Named, b: Named) -> Bool:
    return a < b

def named_same(a: Named, b: Named) -> Bool:
    return a == b

## Text in a union, on one side of it only.
def tag(s: Str) -> Tagged:
    if str_is_empty(s):
        return Number(n = 0)
    return Word(text = s)

def untag(t: Tagged) -> Int:
    match t:
        case Word(text):
            return str_len(text)
        case Number(n):
            return n

## Built in a loop: the accumulator every Beck loop is written as, so the arena is exercised by
## something that allocates once per step rather than once per call.
def repeat(s: Str, n: Int, acc: Str) -> Str:
    if n <= 0:
        return acc
    return repeat(s, n - 1, acc + s)

## The same walk, answering with text so the arena it left is on the wire: one character taken per
## step, so a slice that copied what it was taken *from* shows as a quadratic here with no clock in
## the measurement.
def walked(s: Str, i: Int, acc: Str) -> Str:
    if i >= str_len(s):
        return acc
    return walked(s, i + 1, str_slice(s, i, 1))

## Walks a string by character index, which is the loop `docs/70` made linear and the one that
## would be quadratic here if `str_len` counted or `str_slice` skipped.
def count_of(s: Str, c: Str, i: Int, acc: Int) -> Int:
    if i >= str_len(s):
        return acc
    if str_slice(s, i, 1) == c:
        return count_of(s, c, i + 1, acc + 1)
    return count_of(s, c, i + 1, acc)
"#;
