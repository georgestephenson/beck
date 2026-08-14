//! The programs a differential over **generic** definitions needs, and the arguments to run them on.
//!
//! Shared for the reason [`super::clofix`] is: three backends are held to these, and a second copy
//! of the programs would be a second opinion about what the subset is.
//!
//! # What these are chosen to catch
//!
//! A generic definition compiles by being **specialised**, so the question a differential over one
//! has to answer is not whether generics work — it is **which instantiation a site got**. Every case
//! below is a way of getting the right answer from the wrong function, which is what a differential
//! that only checked results would let through:
//!
//! * **One definition at several types**, including two that are the same width — `Int` and `Bool`
//!   are both immediates, so a backend that keyed an instantiation on its machine representation
//!   rather than on its type would merge them and answer `1` where the evaluator answers `true`.
//! * **A type argument that is itself generic** — `list[Int]`, `list[list[Int]]` — because the
//!   name of an instantiation has to distinguish them and the layout has to nest.
//! * **Two type parameters**, in both orders (`swapped` against `paired`), since a positional
//!   recovery that read them off in declaration order rather than in *use* order passes one and
//!   fails the other.
//! * **A parameter that appears only in the result** (`empty_of`), which is the case where the
//!   argument types say nothing and the instantiation is decided by what the caller wanted.
//! * **A parameter that appears twice** (`same_twice`), where two positions have to agree.
//! * **A generic calling a generic** (`second`), so an instantiation is discovered from inside
//!   another instantiation's body rather than from a concrete definition.
//! * **Ordinary recursion inside a template** (`counted`), where the recursive call is a reference
//!   to the very instantiation being built and must close on itself rather than build another.
//! * **A generic over a record and over a union**, because a type parameter standing for something
//!   with a layout is where a substitution that missed a node would show as a wrong field.
//! * **A generic answering a collection it also builds** (`repeated`), which puts the specialised
//!   body through the list runtime rather than through arithmetic.

#![allow(dead_code)] // each suite uses the half of this it needs

use beck_core::Value;

pub const GENERIC: &str = r#"
model Named:
    label: Str
    rank: Int

union Tagged:
    Word(text: Str)
    Number(n: Int)

# One definition, asked at every type this backend has.
def firstly[T](xs: list[T], fallback: T) -> T:
    match list_get(xs, 0):
        case Some(value):
            return value
        case None:
            return fallback

def of_ints(xs: list[Int]) -> Int:
    return firstly(xs, 0)

# `Bool` and `Int` are both one immediate word, so these two must not become one function.
def of_bools(xs: list[Bool]) -> Bool:
    return firstly(xs, false)

def of_texts(xs: list[Str]) -> Str:
    return firstly(xs, "none")

def of_records(xs: list[Named]) -> Str:
    return firstly(xs, Named(label = "none", rank = 0)).label

def of_unions(xs: list[Tagged]) -> Int:
    match firstly(xs, Number(n = 0)):
        case Word(text):
            return str_len(text)
        case Number(n):
            return n

# A type argument that is itself a collection, and one nested inside that.
def of_lists(xs: list[list[Int]]) -> Int:
    return list_len(firstly(xs, []))

def of_lists_of_lists(xs: list[list[list[Int]]]) -> Int:
    return list_len(firstly(xs, []))

# Two type parameters, used in both orders, so a positional recovery has to be a real one.
def paired[A, B](a: A, b: B) -> Str:
    return str(a) + "|" + str(b)

def swapped[A, B](a: A, b: B) -> Str:
    return paired(b, a)

def int_then_text(n: Int, s: Str) -> Str:
    return paired(n, s)

def text_then_int(s: Str, n: Int) -> Str:
    return swapped(s, n)

# A parameter that appears only in the **result**: nothing in the arguments says what `T` is, so
# the instantiation is decided by what the caller asked for.
def empty_of[T]() -> list[T]:
    return []

def no_texts() -> Int:
    xs: list[Str] = empty_of()
    return list_len(xs)

def no_ints() -> Int:
    xs: list[Int] = empty_of()
    return list_len(xs)

# A parameter in two positions, which have to agree.
def same_twice[T](a: T, b: T) -> Bool:
    return a == b

def ints_agree(a: Int, b: Int) -> Bool:
    return same_twice(a, b)

def texts_agree(a: Str, b: Str) -> Bool:
    return same_twice(a, b)

# A generic calling a generic: `second`'s instantiation is discovered from inside `firstly`'s
# caller rather than from a concrete definition.
def second[T](xs: list[T], fallback: T) -> T:
    return firstly(list_drop(xs, 1), fallback)

def second_int(xs: list[Int]) -> Int:
    return second(xs, 0)

def second_text(xs: list[Str]) -> Str:
    return second(xs, "none")

# Ordinary recursion inside a template: the recursive call is the instantiation being built.
def counted[T](xs: list[T], seen: Int) -> Int:
    if list_is_empty(xs):
        return seen
    return counted(list_drop(xs, 1), seen + 1)

def count_ints(xs: list[Int]) -> Int:
    return counted(xs, 0)

def count_texts(xs: list[Str]) -> Int:
    return counted(xs, 0)

# A generic that builds the collection it answers with, so the specialised body goes through the
# list runtime rather than through arithmetic.
def repeated[T](x: T, n: Int, acc: list[T]) -> list[T]:
    if n <= 0:
        return acc
    return repeated(x, n - 1, list_append(acc, x))

def three_ints(x: Int) -> Int:
    return list_len(repeated(x, 3, []))

def three_texts(x: Str) -> Str:
    return str_join(repeated(x, 3, []), "-")

# A generic bound to a name before it is called, which is the reference this pass has to rewrite
# **outside** an application — the node is a bare `Global` and its type is the instantiated function
# type all the same.
def bound(n: Int) -> Int:
    pick = firstly
    return pick([n, n + 1], 0)
"#;

/// **Polymorphic recursion**, which is the one thing monomorphisation cannot do.
///
/// `T` is a type parameter of a definition that calls itself at `list[T]`, so the set of
/// instantiations is infinite where the program is finite. It has to be refused by name, once,
/// rather than compiled sixty-four times.
pub const RECURSIVE: &str = r#"
def growing[T](x: T, n: Int) -> Int:
    if n <= 0:
        return 0
    return growing([x], n - 1) + 1

def asks_for_it() -> Int:
    return growing(1, 5)
"#;

/// A generic called where **nothing decides** what its type parameter is.
///
/// `list_len(anything())` constrains `T` against `list_len`'s own parameter and no further, so
/// inference finishes with a variable rather than a type. The evaluator does not care — it runs one
/// uniform definition over a list that is empty whatever it holds — and this backend cannot pick a
/// layout for a type nobody named. The program is legal and this is a real edge of the pass, so it
/// is written down rather than left to be discovered.
pub const UNDECIDED: &str = r#"
def anything[T]() -> list[T]:
    return []

def how_many() -> Int:
    return list_len(anything())
"#;

pub fn ints() -> Vec<Vec<Value>> {
    [
        vec![],
        vec![7],
        vec![1, 2, 3],
        vec![0, 0],
        vec![-1, i64::MAX],
    ]
    .into_iter()
    .map(|xs| {
        vec![Value::List(std::sync::Arc::new(
            xs.into_iter().map(Value::Int).collect(),
        ))]
    })
    .collect()
}

pub fn bools() -> Vec<Vec<Value>> {
    [vec![], vec![true], vec![false], vec![true, false]]
        .into_iter()
        .map(|xs| {
            vec![Value::List(std::sync::Arc::new(
                xs.into_iter().map(Value::Bool).collect(),
            ))]
        })
        .collect()
}

pub fn texts() -> Vec<Vec<Value>> {
    [vec![], vec!["a"], vec!["beck", "é"], vec![""]]
        .into_iter()
        .map(|xs| {
            vec![Value::List(std::sync::Arc::new(
                xs.into_iter().map(Value::str_).collect(),
            ))]
        })
        .collect()
}

pub fn lists() -> Vec<Vec<Value>> {
    [vec![], vec![vec![]], vec![vec![1, 2], vec![3]]]
        .into_iter()
        .map(|xss| {
            vec![Value::List(std::sync::Arc::new(
                xss.into_iter()
                    .map(|xs| {
                        Value::List(std::sync::Arc::new(
                            xs.into_iter().map(Value::Int).collect(),
                        ))
                    })
                    .collect(),
            ))]
        })
        .collect()
}

pub fn nested() -> Vec<Vec<Value>> {
    let inner = |xs: Vec<i64>| {
        Value::List(std::sync::Arc::new(
            xs.into_iter().map(Value::Int).collect(),
        ))
    };
    let middle =
        |xss: Vec<Vec<i64>>| Value::List(std::sync::Arc::new(xss.into_iter().map(inner).collect()));
    vec![
        vec![Value::List(std::sync::Arc::new(vec![]))],
        vec![Value::List(std::sync::Arc::new(vec![middle(vec![])]))],
        vec![Value::List(std::sync::Arc::new(vec![
            middle(vec![vec![1], vec![2, 3]]),
            middle(vec![]),
        ]))],
    ]
}

pub fn records() -> Vec<Vec<Value>> {
    let one = |label: &str, rank: i64| {
        Value::record(
            "Named",
            None,
            [("label", Value::str_(label)), ("rank", Value::Int(rank))],
        )
    };
    vec![
        vec![Value::List(std::sync::Arc::new(vec![]))],
        vec![Value::List(std::sync::Arc::new(vec![one("a", 1)]))],
        vec![Value::List(std::sync::Arc::new(vec![
            one("é", -1),
            one("b", 2),
        ]))],
    ]
}

pub fn unions() -> Vec<Vec<Value>> {
    let word = |s: &str| Value::record("Tagged", Some("Word"), [("text", Value::str_(s))]);
    let number = |n: i64| Value::record("Tagged", Some("Number"), [("n", Value::Int(n))]);
    vec![
        vec![Value::List(std::sync::Arc::new(vec![]))],
        vec![Value::List(std::sync::Arc::new(vec![word("beck")]))],
        vec![Value::List(std::sync::Arc::new(vec![number(7), word("x")]))],
    ]
}

/// An `Int` and a `Str`, in that order.
pub fn int_and_text() -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for n in [-1i64, 0, 7] {
        for s in ["", "a", "é"] {
            out.push(vec![Value::Int(n), Value::str_(s)]);
        }
    }
    out
}

/// The same two, the other way round.
pub fn text_and_int() -> Vec<Vec<Value>> {
    int_and_text()
        .into_iter()
        .map(|mut pair| {
            pair.reverse();
            pair
        })
        .collect()
}

pub fn int_pairs() -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for a in [-1i64, 0, 7, i64::MAX] {
        for b in [-1i64, 0, 7] {
            out.push(vec![Value::Int(a), Value::Int(b)]);
        }
    }
    out
}

pub fn text_pairs() -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for a in ["", "a", "é"] {
        for b in ["", "a", "beck"] {
            out.push(vec![Value::str_(a), Value::str_(b)]);
        }
    }
    out
}

pub fn scalars() -> Vec<Vec<Value>> {
    [-1i64, 0, 1, 7, i64::MAX, i64::MIN]
        .into_iter()
        .map(|n| vec![Value::Int(n)])
        .collect()
}

pub fn singles() -> Vec<Vec<Value>> {
    ["", "a", "beck", "é"]
        .into_iter()
        .map(|s| vec![Value::str_(s)])
        .collect()
}
