//! The programs and the lists a differential over **collections** needs.
//!
//! Shared for the reason [`super::heapfix`] and [`super::textfix`] are: three backends are held to
//! these, and a second copy of the programs would be a second opinion about what the subset is.
//!
//! # What the lists are chosen to catch
//!
//! * **The empty list.** Its object is one header word and nothing else — the input where a
//!   comparison is asked for zero elements, a slice allocates nothing, and `list_get` has no
//!   element word it could legally read.
//! * **A list that is a prefix of another** — `[1]` beside `[1, 2]`. Comparing element by element
//!   runs out of elements before it runs out of answer, so the length has to decide afterwards or
//!   `<` is wrong in exactly one direction. It is [`super::textfix`]'s `"ab"`/`"abc"` one type up.
//! * **Lists that differ only after a shared prefix** — `[1, 2]` beside `[1, 3]`.
//! * **A list of `Str`**, because an element that is itself an offset is the case where comparing
//!   the *words* would answer that two equal lists differ.
//! * **A list of lists**, because that is the case where the element comparison recurses and where
//!   the layout table has to reach itself.
//! * **A list of records**, since a record is the third kind of element that is an offset.

#![allow(dead_code)] // each suite uses the half of this it needs

use std::sync::Arc;

use beck_core::Value;

fn list(xs: &[i64]) -> Value {
    Value::List(Arc::new(xs.iter().map(|n| Value::Int(*n)).collect()))
}

/// The lists every definition below is exercised over.
pub fn lists() -> Vec<Value> {
    vec![
        list(&[]),
        list(&[0]),
        list(&[1]),
        list(&[1, 2]),
        list(&[1, 3]),
        list(&[1, 2, 3]),
        list(&[-1, i64::MAX, i64::MIN]),
        list(&[7, 7, 7]),
    ]
}

/// Lists whose elements are themselves offsets, one kind each.
pub fn texts() -> Vec<Value> {
    [
        vec![],
        vec!["a"],
        vec!["a", "bb"],
        vec!["a", "bc"],
        vec![""],
    ]
    .iter()
    .map(|xs| Value::List(Arc::new(xs.iter().map(Value::str_).collect())))
    .collect()
}

pub fn nested() -> Vec<Value> {
    [
        vec![],
        vec![vec![]],
        vec![vec![1]],
        vec![vec![1], vec![2]],
        vec![vec![1, 2]],
    ]
    .iter()
    .map(|xss| Value::List(Arc::new(xss.iter().map(|xs| list(xs)).collect())))
    .collect()
}

pub fn singles(xs: &[Value]) -> Vec<Vec<Value>> {
    xs.iter().map(|x| vec![x.clone()]).collect()
}

pub fn pairs(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::with_capacity(xs.len() * xs.len());
    for a in xs {
        for b in xs {
            out.push(vec![a.clone(), b.clone()]);
        }
    }
    out
}

/// Each list with every index a `list_get` could be asked for, including outside it both ways.
pub fn indexed(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for v in xs {
        for i in [-2i64, -1, 0, 1, 2, 3, 9, i64::MAX, i64::MIN] {
            out.push(vec![v.clone(), Value::Int(i)]);
        }
    }
    out
}

/// Each list with every element it might be searched for, and some it does not hold.
pub fn searched(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for v in xs {
        for n in [-1i64, 0, 1, 2, 3, 7, i64::MAX, i64::MIN] {
            out.push(vec![v.clone(), Value::Int(n)]);
        }
    }
    out
}

/// Each list with every range a slice could be asked for, including ones the clamp decides.
pub fn ranges(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for v in xs {
        for at in [-1i64, 0, 1, 2, 5, i64::MAX] {
            for n in [-1i64, 0, 1, 2, 9, i64::MAX] {
                out.push(vec![v.clone(), Value::Int(at), Value::Int(n)]);
            }
        }
    }
    out
}

/// Each list with every count `list_take` and `list_drop` could be asked for.
pub fn counted(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for v in xs {
        for n in [-1i64, 0, 1, 2, 9, i64::MAX] {
            out.push(vec![v.clone(), Value::Int(n)]);
        }
    }
    out
}

/// A list, every way this backend can meet one.
pub const LISTS: &str = r#"
model Bag:
    items: list[Int]
    rank: Int

union Holding:
    Some_(xs: list[Int])
    None_()

def size(xs: list[Int]) -> Int:
    return list_len(xs)

def empty(xs: list[Int]) -> Bool:
    return list_is_empty(xs)

def nth(xs: list[Int], i: Int) -> Option[Int]:
    return list_get(xs, i)

def nth_or(xs: list[Int], i: Int) -> Int:
    match list_get(xs, i):
        case Some(value):
            return value
        case None():
            return -99

def has(xs: list[Int], n: Int) -> Bool:
    return list_contains(xs, n)

# Growing one. `appended` is the operation; the three below it are what a shared data block does,
# and they are the whole reason `docs/113`'s layout is two objects rather than one.
def appended(xs: list[Int], n: Int) -> list[Int]:
    return list_append(xs, n)

# Two lists grown from **one**: the first extends the block it stands at the end of and the second
# finds the slot taken and copies. Both answers, and the original, have to be what they were.
def forked(xs: list[Int], n: Int) -> Int:
    ys = list_append(xs, n)
    zs = list_append(xs, 0 - n)
    return (list_len(xs) * 100) + (sum_of(ys) * 10) + sum_of(zs)

# The accumulator every loop in the language is written as. Quadratic before `docs/113`, and the
# reason `list_append` was refused rather than shipped.
def doubled_up(xs: list[Int]) -> list[Int]:
    return push_from(xs, 0, [])

def push_from(xs: list[Int], i: Int, acc: list[Int]) -> list[Int]:
    if i >= list_len(xs):
        return acc
    match list_get(xs, i):
        case Some(value):
            return push_from(xs, i + 1, list_append(acc, value * 2))
        case None():
            return acc

def sum_of(xs: list[Int]) -> Int:
    return list_fold(xs, 0, lambda acc, x: acc + x)

# Appending something that is an offset rather than a number, since an element's *word* is what the
# block holds and a `Str` is one.
def named(xs: list[Str], s: Str) -> list[Str]:
    return list_append(xs, s)

# Appending to a list that came out of a record, which is where a header the emitter did not
# allocate crosses into the operation.
def grown_bag(b: Bag, n: Int) -> list[Int]:
    return list_append(b.items, n)

def at_of(xs: list[Int], n: Int) -> Option[Int]:
    return list_index_of(xs, n)

def middle(xs: list[Int], at: Int, n: Int) -> list[Int]:
    return list_slice(xs, at, n)

def front(xs: list[Int], n: Int) -> list[Int]:
    return list_take(xs, n)

def back(xs: list[Int], n: Int) -> list[Int]:
    return list_drop(xs, n)

def flipped(xs: list[Int]) -> list[Int]:
    return list_reverse(xs)

## The six comparisons, each written out, because a three-way answer can be right for `<` and wrong
## for `<=` and only one of them would be asked otherwise.
def below(a: list[Int], b: list[Int]) -> Bool:
    return a < b

def above(a: list[Int], b: list[Int]) -> Bool:
    return a > b

def same(a: list[Int], b: list[Int]) -> Bool:
    return a == b

def differ(a: list[Int], b: list[Int]) -> Bool:
    return a != b

def not_after(a: list[Int], b: list[Int]) -> Bool:
    return a <= b

def not_before(a: list[Int], b: list[Int]) -> Bool:
    return a >= b

## Literals, including the empty one and one whose elements are computed.
def three() -> list[Int]:
    return [1, 2, 3]

def none_at_all() -> list[Int]:
    return []

def doubled(n: Int) -> list[Int]:
    return [n, n + n, n * n]

## Elements that are themselves offsets: text, a list, a record.
def texts_below(a: list[Str], b: list[Str]) -> Bool:
    return a < b

def texts_same(a: list[Str], b: list[Str]) -> Bool:
    return a == b

def nested_below(a: list[list[Int]], b: list[list[Int]]) -> Bool:
    return a < b

def nested_same(a: list[list[Int]], b: list[list[Int]]) -> Bool:
    return a == b

def nested_first(a: list[list[Int]]) -> Int:
    match list_get(a, 0):
        case Some(value):
            return list_len(value)
        case None():
            return -1

## A list inside a record and inside a union. `items` sorts before `rank`, so the list is what
## decides `<` — a layout that put it anywhere but the first slot answers this one backwards.
def bagged(items: list[Int], rank: Int) -> Bag:
    return Bag(items = items, rank = rank)

def bag_items(bag: Bag) -> list[Int]:
    return bag.items

def bag_below(a: Bag, b: Bag) -> Bool:
    return a < b

def bag_same(a: Bag, b: Bag) -> Bool:
    return a == b

def rebagged(bag: Bag, items: list[Int]) -> Bag:
    return bag.with(items = items)

def held(xs: list[Int]) -> Holding:
    if list_is_empty(xs):
        return None_()
    return Some_(xs = xs)

def held_size(h: Holding) -> Int:
    match h:
        case Some_(xs):
            return list_len(xs)
        case None_():
            return 0

## Walks a list by index, which is the loop every reader of one is written as.
def total(xs: list[Int], i: Int, acc: Int) -> Int:
    if i >= list_len(xs):
        return acc
    match list_get(xs, i):
        case Some(value):
            return total(xs, i + 1, acc + value)
        case None():
            return acc

## Answers with text so the arena it left is on the wire: one element taken per step, so a slice
## that copied what it was taken *from* shows as a quadratic with no clock in the measurement.
def walked(xs: list[Int], i: Int, acc: list[Int]) -> list[Int]:
    if i >= list_len(xs):
        return acc
    return walked(xs, i + 1, list_slice(xs, i, 1))
"#;

/// The programs a differential over **list patterns** needs.
///
/// A list pattern is the one pattern whose test is a *length* and whose binders are read out of a
/// block, so what it can get wrong is different from a constructor's:
///
/// * **The length rule differs with a tail binder** — `[a, b]` is exactly two and `[a, *rest]` is at
///   least one — so both are here, at every length around their boundary.
/// * **An element is read only after the length test proves it is there.** A pattern that loaded
///   first would read past the block on a short list, and the empty list is where that shows.
/// * **The tail is a fresh list**, so a `*rest` of nothing and a `*rest` of everything both matter,
///   and a nested one (`[[a, *inner], *outer]`) reads a copy out of a copy.
/// * **A nested pattern inside an element** — `[Some(x), *_]` — because the element's repr is what
///   decides how the word is read, and a wrong one reads an offset as an integer.
/// * **Ordering between arms**: `[]`, `[x]` and `[x, *rest]` overlap, and the first that matches
///   wins, so an arm order that compiled to the wrong test answers the wrong branch rather than
///   failing.
pub const PATTERNS: &str = r#"
def described(xs: list[Int]) -> Str:
    match xs:
        case []:
            return "none"
        case [only]:
            return "one:" + str(only)
        case [a, b]:
            return "two:" + str(a + b)
        case [first, *rest]:
            return "many:" + str(first) + ":" + str(list_len(rest))

## The tail, returned rather than counted — so the differential compares the *list* and not a
## length that a wrong copy could still get right.
def tail(xs: list[Int]) -> list[Int]:
    match xs:
        case [_, *rest]:
            return rest
        case _:
            return []

## Two fixed elements before the tail, which is where an off-by-one in the copy's start shows.
def after_two(xs: list[Int]) -> list[Int]:
    match xs:
        case [_, _, *rest]:
            return rest
        case _:
            return []

## A constant inside a list pattern: the element is tested, not just bound.
def leading_one(xs: list[Int]) -> Bool:
    match xs:
        case [1, *_]:
            return True
        case _:
            return False

## Nested: a list of lists, taken apart at both levels.
def inner_first(xss: list[list[Int]]) -> Int:
    match xss:
        case [[a, *_], *_]:
            return a
        case _:
            return -1

## Text elements, so the element's repr is an offset rather than an integer.
def joined(xs: list[Str]) -> Str:
    match xs:
        case [a, b, *_]:
            return a + "|" + b
        case [a]:
            return a
        case _:
            return ""

## Exactly-two against at-least-two, which is the whole length rule in one definition.
def exactly_two(xs: list[Int]) -> Bool:
    match xs:
        case [_, _]:
            return True
        case _:
            return False
"#;
