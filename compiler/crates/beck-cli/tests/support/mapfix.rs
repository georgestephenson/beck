//! The programs and the maps a differential over `Map[K, V]` needs.
//!
//! Shared for the reason [`super::listfix`] is: three backends are held to these, and a second copy
//! of the programs would be a second opinion about what the subset is.
//!
//! # What the maps are chosen to catch
//!
//! * **The empty map.** One header word and nothing else — where the binary search's window is
//!   empty on the first probe and `map_get` has no key it could legally read.
//! * **A map that is a prefix of another.** `PMap`'s order is pair by pair and then by length, so a
//!   comparison that ran out of entries before it ran out of answer orders them the wrong way. It
//!   is [`super::listfix`]'s `[1]` beside `[1, 2]` one type up.
//! * **Maps with the same keys and different values**, because the value has to decide *after* the
//!   key at each entry rather than at the end.
//! * **A key that is present, a key below every key, a key above every key, and a key between
//!   two** — the four ways a binary search ends, and the last is the one a window that shrinks
//!   wrongly loops on forever.
//! * **Enough entries to need more than one probe.** A search that happened to be linear would
//!   agree on a map of one and disagree on nothing else, so the alphabet has maps of eight.
//! * **`Str` keys**, because a key that is itself an offset is where comparing the *words* would
//!   answer that two equal maps differ — and because that is the shape every fold in this tree has.

#![allow(dead_code)] // each suite uses the half of this it needs

use beck_core::Value;

fn map(pairs: &[(&str, i64)]) -> Value {
    Value::Map(
        pairs
            .iter()
            .map(|(k, v)| (Value::str_(k), Value::Int(*v)))
            .collect(),
    )
}

/// The maps every definition below is exercised over.
pub fn maps() -> Vec<Value> {
    vec![
        map(&[]),
        map(&[("a", 1)]),
        map(&[("a", 2)]),
        map(&[("b", 1)]),
        map(&[("a", 1), ("b", 2)]),
        map(&[("a", 1), ("b", 3)]),
        map(&[("a", 1), ("b", 2), ("c", 3)]),
        map(&[
            ("a", 1),
            ("c", 2),
            ("e", 3),
            ("g", 4),
            ("i", 5),
            ("k", 6),
            ("m", 7),
            ("o", 8),
        ]),
    ]
}

/// Maps whose values are themselves lists, so an entry's value is an offset too.
pub fn nested() -> Vec<Value> {
    [
        vec![],
        vec![("a", vec![1])],
        vec![("a", vec![1, 2])],
        vec![("a", vec![1]), ("b", vec![])],
    ]
    .iter()
    .map(|pairs| {
        Value::Map(
            pairs
                .iter()
                .map(|(k, xs)| {
                    (
                        Value::str_(k),
                        Value::List(std::sync::Arc::new(
                            xs.iter().map(|n| Value::Int(*n)).collect::<Vec<_>>(),
                        )),
                    )
                })
                .collect(),
        )
    })
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

/// Each map with every key a search could be asked for: present, absent below, absent above, and
/// absent *between* two present ones — which is the case a badly shrinking window never leaves.
pub fn keyed(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for m in xs {
        for k in ["", "a", "b", "c", "d", "f", "h", "o", "p", "zz"] {
            out.push(vec![m.clone(), Value::str_(k)]);
        }
    }
    out
}

/// A map, every way this backend can meet one.
pub const MAPS: &str = r#"
model Counts:
    tally: Map[Str, Int]
    label: Str

union Holding:
    Held(m: Map[Str, Int])
    Empty()

def size(m: Map[Str, Int]) -> Int:
    return map_len(m)

def lookup(m: Map[Str, Int], k: Str) -> Option[Int]:
    return map_get(m, k)

def lookup_or(m: Map[Str, Int], k: Str) -> Int:
    match map_get(m, k):
        case Some(value):
            return value
        case None():
            return -99

def holds(m: Map[Str, Int], k: Str) -> Bool:
    return map_contains(m, k)

def names(m: Map[Str, Int]) -> list[Str]:
    return map_keys(m)

def totals(m: Map[Str, Int]) -> list[Int]:
    return map_values(m)

## The six comparisons, each written out, because a three-way answer can be right for `<` and wrong
## for `<=` and only one of them would be asked otherwise.
def below(a: Map[Str, Int], b: Map[Str, Int]) -> Bool:
    return a < b

def above(a: Map[Str, Int], b: Map[Str, Int]) -> Bool:
    return a > b

def same(a: Map[Str, Int], b: Map[Str, Int]) -> Bool:
    return a == b

def differ(a: Map[Str, Int], b: Map[Str, Int]) -> Bool:
    return a != b

def not_after(a: Map[Str, Int], b: Map[Str, Int]) -> Bool:
    return a <= b

def not_before(a: Map[Str, Int], b: Map[Str, Int]) -> Bool:
    return a >= b

## The only literal this backend builds, and the one every `durable` fold starts at.
def nothing() -> Map[Str, Int]:
    return {}

def is_nothing(m: Map[Str, Int]) -> Bool:
    return m == {}

## A value that is itself an offset.
def nested_below(a: Map[Str, list[Int]], b: Map[Str, list[Int]]) -> Bool:
    return a < b

def nested_same(a: Map[Str, list[Int]], b: Map[Str, list[Int]]) -> Bool:
    return a == b

def nested_at(m: Map[Str, list[Int]], k: Str) -> Int:
    match map_get(m, k):
        case Some(value):
            return list_len(value)
        case None():
            return -1

## A map inside a record and inside a union. `label` sorts after `tally`, so the map is what decides
## `<` — a layout that put it anywhere but the first slot answers this one backwards.
def counted(tally: Map[Str, Int], label: Str) -> Counts:
    return Counts(tally = tally, label = label)

def counts_tally(c: Counts) -> Map[Str, Int]:
    return c.tally

def counts_below(a: Counts, b: Counts) -> Bool:
    return a < b

def recounted(c: Counts, tally: Map[Str, Int]) -> Counts:
    return c.with(tally = tally)

def held(m: Map[Str, Int]) -> Holding:
    if map_len(m) == 0:
        return Empty()
    return Held(m = m)

def held_size(h: Holding) -> Int:
    match h:
        case Held(m):
            return map_len(m)
        case Empty():
            return 0

## Walks a map through its keys, which is what a view over a fold's state does.
def total(m: Map[Str, Int], i: Int, acc: Int) -> Int:
    if i >= map_len(m):
        return acc
    match list_get(map_keys(m), i):
        case Some(value):
            return total(m, i + 1, acc + lookup_or(m, value))
        case None():
            return acc
"#;
