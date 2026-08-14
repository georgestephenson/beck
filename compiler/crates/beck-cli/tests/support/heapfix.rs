//! The programs and the values a differential over the **heap** subset needs.
//!
//! Shared for the reason [`super::scalar`] is shared: there are three backends to hold to these —
//! the tree-walker, LLVM ([`native.rs`]) and Cranelift ([`cranelift.rs`]) — and a second copy of
//! the programs would be a second opinion about what the subset is.
//!
//! # What the fixtures are chosen to catch
//!
//! A layout can be wrong in ways an answer still looks plausible, so three of these programs exist
//! to make one specific mistake visible:
//!
//! * **`Ranked`'s variants are declared out of alphabetical order.** `Value`'s derived `Ord`
//!   compares a variant by *name*, so `Big` sorts below `Small` however the `union` was written. A
//!   backend that made the tag a declaration index would answer `<` backwards, and
//!   `ranked_order` is the definition that says so.
//! * **`Key`'s fields are declared out of alphabetical order.** A record is compared field by
//!   field in *name* order (`docs/50` §50.6), so `name` decides before `score` even though `score`
//!   is written first. A layout in declaration order answers the opposite.
//! * **`Weighed` holds reals.** A real on the heap has to be the one the evaluator would have
//!   built — `-0.0` normalised to `0.0`, every NaN to one NaN — or `==` on two records disagrees
//!   with `==` on their fields. `negated` produces a negative zero on the heap rather than
//!   receiving one already canonicalised.

#![allow(dead_code)] // each suite uses the half of this it needs

use std::sync::Arc;

use beck_core::core::{Fields, Record};
use beck_core::Value;

// -------------------------------------------------------------------------------------------
// Building the values a call is given
// -------------------------------------------------------------------------------------------

/// A record with no variant.
pub fn record(ty: &str, fields: &[(&str, Value)]) -> Value {
    data(ty, None, fields)
}

/// A union variant.
pub fn variant(ty: &str, name: &str, fields: &[(&str, Value)]) -> Value {
    data(ty, Some(name), fields)
}

fn data(ty: &str, name: Option<&str>, fields: &[(&str, Value)]) -> Value {
    Value::Data(Arc::new(Record {
        ty: Arc::from(ty),
        variant: name.map(Arc::from),
        fields: fields
            .iter()
            .map(|(n, v)| (Arc::from(*n), v.clone()))
            .collect::<Fields>(),
    }))
}

pub fn point(x: i64, y: i64) -> Value {
    record("Point", &[("x", Value::Int(x)), ("y", Value::Int(y))])
}

pub fn weighed(w: f64, heavy: bool) -> Value {
    record(
        "Weighed",
        &[("weight", Value::float(w)), ("heavy", Value::Bool(heavy))],
    )
}

pub fn key(score: i64, name: i64) -> Value {
    record(
        "Key",
        &[("score", Value::Int(score)), ("name", Value::Int(name))],
    )
}

pub fn small(n: i64) -> Value {
    variant("Ranked", "Small", &[("n", Value::Int(n))])
}

pub fn big(n: i64) -> Value {
    variant("Ranked", "Big", &[("n", Value::Int(n))])
}

pub fn nothing() -> Value {
    variant("Ranked", "Nothing", &[])
}

pub fn leaf(n: i64) -> Value {
    variant("Tree", "Leaf", &[("value", Value::Int(n))])
}

pub fn node(left: Value, right: Value) -> Value {
    variant("Tree", "Node", &[("left", left), ("right", right)])
}

pub fn some(n: i64) -> Value {
    variant("Option", "Some", &[("value", Value::Int(n))])
}

pub fn none() -> Value {
    variant("Option", "None", &[])
}

pub fn id(n: i64) -> Value {
    record("Id", &[("value", Value::Int(n))])
}

/// Every pair from `xs`, as argument tuples.
pub fn pairs(xs: &[Value]) -> Vec<Vec<Value>> {
    let mut out = Vec::with_capacity(xs.len() * xs.len());
    for a in xs {
        for b in xs {
            out.push(vec![a.clone(), b.clone()]);
        }
    }
    out
}

pub fn singles(xs: &[Value]) -> Vec<Vec<Value>> {
    xs.iter().map(|x| vec![x.clone()]).collect()
}

/// The values `RECORDS`' definitions are exercised over.
pub fn records() -> Vec<Value> {
    vec![
        point(0, 0),
        point(1, 2),
        point(2, 1),
        point(-1, i64::MAX),
        point(i64::MIN, 0),
    ]
}

pub fn weighted() -> Vec<Value> {
    vec![
        weighed(0.0, false),
        weighed(-0.0, true),
        weighed(1.5, false),
        weighed(f64::NAN, true),
        weighed(f64::INFINITY, false),
        weighed(f64::NEG_INFINITY, true),
    ]
}

pub fn keys() -> Vec<Value> {
    vec![key(0, 0), key(1, 0), key(0, 1), key(1, 1), key(-1, 5)]
}

pub fn ranked() -> Vec<Value> {
    vec![small(0), small(3), big(0), big(3), nothing()]
}

pub fn trees() -> Vec<Value> {
    vec![
        leaf(0),
        leaf(7),
        node(leaf(1), leaf(2)),
        node(node(leaf(1), leaf(2)), leaf(3)),
        node(leaf(3), node(leaf(1), leaf(2))),
    ]
}

pub fn options() -> Vec<Value> {
    vec![some(0), some(9), none()]
}

// -------------------------------------------------------------------------------------------
// The programs
// -------------------------------------------------------------------------------------------

/// Records: built, read, updated, compared — and one whose fields are reals.
pub const RECORDS: &str = r#"
model Point:
    x: Int
    y: Int

## Declared score-then-name on purpose: a record is compared by field *name*, so `name` decides.
model Key:
    score: Int
    name: Int

model Weighed:
    weight: Float
    heavy: Bool

model Segment:
    from: Point
    to: Point

def origin() -> Point:
    return Point(x=0, y=0)

def make(x: Int, y: Int) -> Point:
    return Point(x=x, y=y)

def sum_of(p: Point) -> Int:
    return p.x + p.y

def moved(p: Point, dx: Int) -> Point:
    return p.with(x = p.x + dx)

def swapped(p: Point) -> Point:
    return p.with(x = p.y, y = p.x)

## A field expression that can trap: the record is never built, and the message is the evaluator's.
def scaled(p: Point, by: Int) -> Point:
    return Point(x = p.x * by, y = p.y * by)

def same_point(a: Point, b: Point) -> Bool:
    return a == b

def point_order(a: Point, b: Point) -> Int:
    if a < b:
        return -1
    if a > b:
        return 1
    return 0

## The one that catches a layout in declaration order.
def key_order(a: Key, b: Key) -> Int:
    if a < b:
        return -1
    if a > b:
        return 1
    return 0

def heavier(a: Weighed, b: Weighed) -> Bool:
    return a > b

def same_weight(a: Weighed, b: Weighed) -> Bool:
    return a == b

## A negative zero and a NaN made *here* rather than handed in already canonicalised.
def negated(w: Weighed) -> Weighed:
    return Weighed(weight = w.weight * -1.0, heavy = not w.heavy)

def negated_is_zero(w: Weighed) -> Bool:
    return negated(w) == Weighed(weight=0.0, heavy=true)

## A record inside a record, so an offset is followed rather than only stored.
def span_of(a: Point, b: Point) -> Segment:
    return Segment(from=a, to=b)

def width(s: Segment) -> Int:
    return s.to.x - s.from.x

def segment_order(a: Point, b: Point) -> Int:
    x = Segment(from=a, to=b)
    y = Segment(from=b, to=a)
    if x < y:
        return -1
    if x > y:
        return 1
    return 0
"#;

/// Unions: matched, nested, guarded, ordered — and one built in a loop.
pub const UNIONS: &str = r#"
## Declared Small, Big, Nothing on purpose. Sorted by name that is Big, Nothing, Small.
union Ranked:
    Small(n: Int)
    Big(n: Int)
    Nothing

union Tree[T]:
    Leaf(value: T)
    Node(left: Tree[T], right: Tree[T])

type Id = newtype[Int]

def rank(r: Ranked) -> Int:
    match r:
        case Small(n):
            return n
        case Big(n):
            return n * 100
        case Nothing():
            return -1

def guarded(r: Ranked) -> Int:
    match r:
        case Small(n) if n > 10:
            return 1
        case Small(n):
            return 2
        case Big(_) | Nothing() if n_or_zero(r) == 0:
            return 3
        case _:
            return 4

def n_or_zero(r: Ranked) -> Int:
    match r:
        case Small(n) | Big(n):
            return n
        case Nothing():
            return 0

def either(r: Ranked) -> Int:
    match r:
        case Small(n) | Big(n):
            return n
        case Nothing():
            return 0

def whole(r: Ranked) -> Int:
    match r:
        case all @ Big(n):
            return rank(all) + n
        case _:
            return 0

## The one that catches a tag in declaration order.
def ranked_order(a: Ranked, b: Ranked) -> Int:
    if a < b:
        return -1
    if a > b:
        return 1
    return 0

def same_ranked(a: Ranked, b: Ranked) -> Bool:
    return a == b

def bigger(n: Int) -> Ranked:
    if n > 0:
        return Big(n=n)
    return Nothing()

def total(t: Tree[Int]) -> Int:
    match t:
        case Leaf(value):
            return value
        case Node(left, right):
            return total(left) + total(right)

## A nested constructor pattern: the field is only read once the tag says it is there.
def left_leaf(t: Tree[Int]) -> Int:
    match t:
        case Node(left=Leaf(value), right=_):
            return value
        case Node(left=_, right=_):
            return -1
        case Leaf(value):
            return value

## Nested and alternative at once, which is what makes an arm two arms.
def first_number(t: Tree[Int]) -> Int:
    match t:
        case Leaf(value) | Node(left=Leaf(value), right=_):
            return value
        case _:
            return 0

## Allocation in a loop, so the arena is asked for more than one object.
def spine(n: Int) -> Tree[Int]:
    if n <= 0:
        return Leaf(value=0)
    return Node(left=Leaf(value=n), right=spine(n - 1))

## Tail-recursive, so what bounds it is the arena rather than the host stack — which is what
## makes it the definition that can be asked for more heap than there is.
def chain(n: Int, acc: Tree[Int]) -> Tree[Int]:
    if n <= 0:
        return acc
    return chain(n - 1, Node(left=Leaf(value=n), right=acc))

def tree_order(a: Tree[Int], b: Tree[Int]) -> Int:
    if a < b:
        return -1
    if a > b:
        return 1
    return 0

def wrap(n: Int) -> Id:
    return Id(n)

def unwrap(i: Id) -> Int:
    return i.value

def maybe(n: Int) -> Option[Int]:
    if n > 0:
        return Some(value=n)
    return None()

def or_else(o: Option[Int], fallback: Int) -> Int:
    match o:
        case Some(value):
            return value
        case None():
            return fallback
"#;

/// Definitions the heap does **not** reach, one per reason.
///
/// The list is what `docs/101` §101.5 said is not built, less the row `docs/105` removed, in the
/// form that goes red the day one of them starts compiling — which is the point: an absence
/// asserted as a test is an absence that cannot go stale (`docs/83` §83.7).
///
/// Text is no longer on it, and neither is *reading* a list. `names_it` and `reads_a_list` are here
/// instead, on the *other* side of the list — so a removal is a thing this fixture asserts rather
/// than one a reader infers from a row that is missing. What is left of collections is the half
/// that **grows** one (`grows`) and the half that takes a function (`mapped`).
pub const STILL_REFUSED: &str = r#"
model Named:
    label: Str

def renders_a_real(x: Float) -> Str:
    return str(x)

def splits_a_string(s: Str) -> Int:
    return list_len(str_split(s, ","))

def trims(s: Str) -> Str:
    return str_trim(s)

def upcases(s: Str) -> Str:
    return str_upper(s)

def grows(xs: list[list[Int]]) -> list[Int]:
    return list_flat_map(xs, lambda ys: ys)

def mapped(xs: list[Int]) -> list[Int]:
    return map_list(xs, double_it)

def double_it(n: Int) -> Int:
    return n * 2

def is_generic[T](x: T) -> T:
    return x

def reads_the_clock() -> Int:
    return now()

def calls_something_refused(n: Int) -> list[Int]:
    return grows([[n]])

def names_it(label: Str) -> Named:
    return Named(label = label)

def reads_a_list(xs: list[Int]) -> Int:
    return list_len(xs)

def scalar_and_fine(n: Int) -> Int:
    return n * 2
"#;
