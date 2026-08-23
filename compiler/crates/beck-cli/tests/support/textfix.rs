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

/// The strings a trim is asked about: every shape the leading and trailing runs can have, and one
/// of each of the four **kinds** of whitespace character the encoding has —  one byte, two bytes,
/// and the two three-byte families (`E2 80 xx` and the singletons).
///
/// The last two are the ones that separate this from an ASCII trim: `"\u{2003}x\u{2003}"` is
/// trimmed by the evaluator and would not be by a backend that only knew `' '`, and
/// `"\u{2000}"` alone trims to the empty string. `"\u{200B}"` is the control — ZERO WIDTH SPACE is
/// *not* `White_Space`, so a backend that trimmed "anything in the space block" would answer
/// `""` where the evaluator answers the character back.
pub fn spaced() -> Vec<Value> {
    [
        "",
        " ",
        "  ",
        "a",
        " a",
        "a ",
        " a ",
        "  a  ",
        " a b ",
        "\t\n\r\x0b\x0c a \t\n\r",
        "\u{85}a\u{a0}",
        "\u{a0}\u{85}",
        "\u{1680}a\u{1680}",
        "\u{2000}",
        "\u{2003}x\u{2003}",
        "\u{200a}\u{2028}\u{2029}\u{202f}y\u{205f}",
        "\u{3000}\u{3000}z",
        "\u{200b}",
        " \u{200b} ",
        "héllo ",
        " 日本語",
        " 🎈x ",
        "a\0b ",
        " the seventeen ok ",
        "   ",
        "\u{2003}\u{3000}\u{a0}",
    ]
    .iter()
    .map(Value::str_)
    .collect()
}

/// **Every** code point Rust calls whitespace, four ways each — alone, leading, trailing and both.
///
/// Derived from `char::is_whitespace` rather than written out, so this list *is* the enumeration
/// and cannot drift from it: if a Rust upgrade adds a `White_Space` code point, the differential
/// starts asking about it on the next run, and the emitters answer wrongly until they are told.
/// `native.rs::the_whitespace_this_backend_knows_is_every_one_rust_does` is the other half — it
/// says how many there are and how long they are, which is what the emitters were written from.
///
/// The four at the end are the **near misses**, and they are why this is not simply "the space
/// block": ZERO WIDTH SPACE, MONGOLIAN VOWEL SEPARATOR, ZERO WIDTH NO-BREAK SPACE and WORD JOINER
/// all look like whitespace, are named like whitespace, and are not `White_Space` — so a backend
/// that trimmed a *range* rather than a set answers `""` where the evaluator answers them back.
pub fn every_whitespace() -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    let space = (0u32..0x11_0000)
        .filter_map(char::from_u32)
        .filter(|c| c.is_whitespace());
    let near = ['\u{200b}', '\u{180e}', '\u{feff}', '\u{2060}'].into_iter();
    for c in space.chain(near) {
        for shape in [
            format!("{c}"),
            format!("{c}x"),
            format!("x{c}"),
            format!("{c}x{c}"),
        ] {
            out.push(vec![Value::str_(&shape)]);
        }
    }
    out
}

/// Every string against every separator, plus the separators a split has edge cases for.
///
/// The four extra are the ones `str::split`'s answer is surprising for and where a hand-written
/// backend goes wrong: the **empty** separator, which is characters; a separator that is the whole
/// string, which answers two empty pieces; one that **overlaps** itself, where `"aaa"` split on
/// `"aa"` is `["", "a"]` and not `["", "", ""]`; and one longer than the string, which is found
/// nowhere and answers the string back.
pub fn separators(xs: &[Value]) -> Vec<Vec<Value>> {
    let seps: Vec<Value> = ["", ",", "a", "ab", "aa", "é", "the seventeen ok", "\0"]
        .iter()
        .map(Value::str_)
        .collect();
    let mut out = Vec::new();
    for x in xs {
        for sep in &seps {
            out.push(vec![x.clone(), sep.clone()]);
        }
    }
    out
}

/// The same calls, each asked at a handful of indices — including out of range at both ends.
pub fn indexed(calls: &[Vec<Value>]) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for call in calls {
        for i in [-1i64, 0, 1, 2, 5, i64::MAX] {
            let mut with = call.clone();
            with.push(Value::Int(i));
            out.push(with);
        }
    }
    out
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

/// The integers `str` has to render exactly as Rust does, including the one whose magnitude has no
/// signed representation.
pub fn integers() -> Vec<Vec<Value>> {
    [
        0i64,
        1,
        -1,
        9,
        10,
        -10,
        99,
        100,
        12345,
        -12345,
        i64::MAX,
        i64::MIN,
        i64::MIN + 1,
    ]
    .iter()
    .map(|n| vec![Value::Int(*n)])
    .collect()
}

/// The lists of text `str_join` is asked about, with each separator.
pub fn joins(seps: &[Value]) -> Vec<Vec<Value>> {
    let parts: Vec<Value> = [
        vec![],
        vec![""],
        vec!["a"],
        vec!["a", "b"],
        vec!["", "a", ""],
        vec!["héllo", "日本語", "a\0b"],
    ]
    .iter()
    .map(|p| Value::list(p.iter().map(Value::str_).collect()))
    .collect();
    let mut out = Vec::new();
    for xs in &parts {
        for sep in seps.iter().take(5) {
            out.push(vec![xs.clone(), sep.clone()]);
        }
    }
    out
}

/// Every `Option[Int]` a fallback could be asked about.
pub fn options() -> Vec<Vec<Value>> {
    [
        Value::some(Value::Int(0)),
        Value::some(Value::Int(7)),
        Value::some(Value::Int(i64::MIN)),
        Value::none(),
    ]
    .iter()
    .flat_map(|o| [-1i64, 0, 42].map(|f| vec![o.clone(), Value::Int(f)]))
    .collect()
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

## Building text out of something that is not text. `str` of an `Int` has to be Rust's decimal to
## the digit, including `i64::MIN`, whose magnitude has no signed representation.
def shown(n: Int) -> Str:
    return str(n)

def shown_bool(x: Bool) -> Str:
    return str(x)

def shown_str(s: Str) -> Str:
    return str(s)

def repeated(s: Str, n: Int) -> Str:
    return str_repeat(s, n)

def glued(xs: list[Str], sep: Str) -> Str:
    return str_join(xs, sep)

## The two ways an `Option` is taken apart without a `match`.
def or_else(o: Option[Int], fallback: Int) -> Int:
    return unwrap_or(o, fallback)

def present(o: Option[Int]) -> Bool:
    return is_some(o)

def sliced_or(s: Str, i: Int, fallback: Int) -> Int:
    return unwrap_or(str_index_of(s, str_slice(s, i, 1)), fallback)

## Answers with an `Option`, which is the prelude's union and has a layout like any other — the
## thing `docs/93` §93.9 said it did not.
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

## Trims. Two shapes, because what a trim answers is read two ways: as text, and as the length that
## says the answer has the right *characters* rather than the right bytes.
def trimmed(s: Str) -> Str:
    return str_trim(s)

def trimmed_len(s: Str) -> Int:
    return str_len(str_trim(s))

## A trim in the middle of an expression, so the answer is a fresh object something else then reads.
def blank(s: Str) -> Bool:
    return str_is_empty(str_trim(s))

## The accumulator, trimming each step: what this is for is the arena, since a trim allocates and a
## loop that trims once per step is where a copy nobody asked for would show as a shape.
def trimmed_up(s: Str, n: Int, acc: Str) -> Str:
    if n <= 0:
        return acc
    return trimmed_up(s, n - 1, acc + str_trim(s))

## Splitting, which answers with a list — so the differential reads both the list and its elements.
##
## `split_len` is the shape the refusal on record described as "two loops": the length of the answer
## and nothing else. `split_at` reads an element out of it, which is what says the *elements* were
## allocated correctly and not merely counted.
def parts(s: Str, sep: Str) -> list[Str]:
    return str_split(s, sep)

def split_len(s: Str, sep: Str) -> Int:
    return list_len(str_split(s, sep))

def split_at(s: Str, sep: Str, i: Int) -> Str:
    match list_get(str_split(s, sep), i):
        case Some(value):
            return value
        case None:
            return "<none>"

def rejoined(s: Str, sep: Str) -> Str:
    return str_join(str_split(s, sep), sep)

## The characters, which the evaluator answers for an empty separator — so these two are one
## function underneath and the differential asks both.
def letters(s: Str) -> list[Str]:
    return str_chars(s)

def letter_count(s: Str) -> Int:
    return list_len(str_chars(s))

def letter_at(s: Str, i: Int) -> Str:
    match list_get(str_chars(s), i):
        case Some(value):
            return value
        case None:
            return "<none>"

## Walks a string by character index, which is the loop `docs/70` made linear and the one that
## would be quadratic here if `str_len` counted or `str_slice` skipped.
def count_of(s: Str, c: Str, i: Int, acc: Int) -> Int:
    if i >= str_len(s):
        return acc
    if str_slice(s, i, 1) == c:
        return count_of(s, c, i + 1, acc + 1)
    return count_of(s, c, i + 1, acc)
"#;
