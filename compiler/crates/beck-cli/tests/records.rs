//! What a record literal promises, now that the compiler decides where its fields go.
//!
//! `docs/70` replaced the sort a record literal ran on every construction with a permutation
//! computed once, in `beck_core::fields`. Two things rest on that, and only the first of them is
//! about speed:
//!
//! > **A record holds its fields in name order, whatever order they were written in** — the order
//! > is the `Map`'s iteration, the value order and the state digest (`docs/54`) — **and its field
//! > expressions are still evaluated in the order they were written**, because one of them can
//! > `raise`.
//!
//! The pass's own unit tests check the permutation, including all 40,320 arrangements of a record
//! at the packing's full width. These check what a *program* would see if it were wrong, which is
//! two records that should be equal comparing as different, or the wrong failure coming out of a
//! literal with two fallible fields.
//!
//! Nothing here is about speed. `docs/70` §70.1 has the numbers.

use beck_rt::testing::{Options, Outcome};

fn cases(src: &str) -> Vec<(std::sync::Arc<str>, Outcome)> {
    let (placed, d, m) = beck_core::compile_or_library_str("records.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&m));
    let placed = placed.expect("this program compiles");
    let backend = beck_eval::backend(&placed);
    beck_rt::testing::run(&placed, backend, &Options::default())
        .cases
        .into_iter()
        .map(|c| (c.name, c.outcome))
        .collect()
}

fn all_pass(src: &str) {
    let cases = cases(src);
    assert!(!cases.is_empty(), "the program declares no tests");
    for (name, outcome) in cases {
        assert!(outcome.is_pass(), "`{name}`: {outcome:?}");
    }
}

/// The invariant itself: how a literal is written cannot change what the record *is*.
///
/// Declaration order, name order and reverse order all have to produce one value.
///
/// `Cycle` is here because of the classic way to get a permutation wrong — applying its inverse.
/// Every arrangement of two or three fields written in the tests below is its own inverse, so an
/// inverted layout passes all of them; `Cycle`, whose three fields are written `c, a, b`, is a
/// three-cycle and is not. Inverting the pass is a mutation this file has been run against.
#[test]
fn a_record_is_the_same_whatever_order_its_fields_are_written_in() {
    all_pass(
        "\
model Ball:
    x: Int
    y: Int
    x_vel: Int
    y_vel: Int

model Cycle:
    c: Int
    a: Int
    b: Int

def cycle_written_out_of_order() -> Cycle:
    return Cycle(c=3, a=1, b=2)

def cycle_written_another_way() -> Cycle:
    return Cycle(b=2, c=3, a=1)

def cycle_written_in_name_order() -> Cycle:
    return Cycle(a=1, b=2, c=3)

def declared() -> Ball:
    return Ball(x=1, y=2, x_vel=3, y_vel=4)

def by_name() -> Ball:
    return Ball(x=1, x_vel=3, y=2, y_vel=4)

def backwards() -> Ball:
    return Ball(y_vel=4, x_vel=3, y=2, x=1)

test \"three orders, one value\":
    expect declared() == by_name()
    expect by_name() == backwards()
    expect declared().x == 1
    expect declared().y == 2
    expect declared().x_vel == 3
    expect declared().y_vel == 4
    expect backwards().x_vel == 3
    expect cycle_written_out_of_order() == cycle_written_in_name_order()
    expect cycle_written_another_way() == cycle_written_in_name_order()
    expect cycle_written_out_of_order() < Cycle(a=2, b=0, c=0)
",
    );
}

/// The order is the *value* order, so it is observable through `<` and not only through `==`.
///
/// `zebra` is declared first and sorts last, so a record that kept its written order would compare
/// on `zebra` and answer the other way round.
#[test]
fn a_record_compares_by_name_order_however_it_was_written() {
    all_pass(
        "\
model Declared:
    zebra: Int
    alpha: Int

test \"alpha decides, from either notation\":
    expect Declared(zebra=2, alpha=0) < Declared(zebra=1, alpha=9)
    expect Declared(alpha=0, zebra=2) < Declared(alpha=9, zebra=1)
    expect sort_by([Declared(zebra=1, alpha=9), Declared(alpha=0, zebra=2)], lambda d: d) == [Declared(zebra=2, alpha=0), Declared(zebra=1, alpha=9)]
",
    );
}

/// A record wider than the packed layout takes the run-time path, and must not be able to tell.
///
/// Nine fields is one past [`beck_core::fields::MAX_ORDERED`], so this is the fallback under test
/// rather than the permutation — the branch that also carries every program built by something
/// that never runs the pass.
#[test]
fn a_record_too_wide_for_the_packing_still_orders_by_name() {
    assert_eq!(
        beck_core::fields::MAX_ORDERED,
        8,
        "the width this test is one past"
    );
    all_pass(
        "\
model Wide:
    i: Int
    h: Int
    g: Int
    f: Int
    e: Int
    d: Int
    c: Int
    b: Int
    a: Int

def backwards() -> Wide:
    return Wide(i=9, h=8, g=7, f=6, e=5, d=4, c=3, b=2, a=1)

test \"nine fields, written backwards\":
    expect backwards() == Wide(a=1, b=2, c=3, d=4, e=5, f=6, g=7, h=8, i=9)
    expect backwards().a == 1
    expect backwards().i == 9
    expect backwards() < Wide(i=0, h=0, g=0, f=0, e=0, d=0, c=0, b=0, a=2)
",
    );
}

/// `with` rebuilds a record that was placed, and a literal builds one directly. They must agree.
#[test]
fn a_record_updated_with_with_equals_the_literal_it_stands_for() {
    all_pass(
        "\
model Task:
    priority: Int
    id: Int
    done: Bool

def base() -> Task:
    return Task(priority=2, id=7, done=false)

test \"an update is a literal\":
    expect base().with(done=true) == Task(id=7, priority=2, done=true)
    expect base().with(priority=1, id=8) == Task(done=false, id=8, priority=1)
",
    );
}

/// The half the permutation deliberately did **not** move: the fields are still *evaluated* left
/// to right, so which of two fallible ones fails first is still the written order.
///
/// This is the test that would have caught placing the values as they were produced rather than
/// producing them all and then placing them. `late` sorts before `early` by name, so a literal
/// that evaluated in field order would raise `Second` here.
#[test]
fn field_expressions_are_evaluated_in_the_order_they_are_written() {
    all_pass(
        "\
model Pair:
    late: Int
    early: Int

union Which:
    First
    Second

def boom(w: Which) -> Int uses raises(Which):
    raise w

def build() -> Pair uses raises(Which):
    return Pair(early=boom(First), late=boom(Second))

## The failure a literal with two fallible fields produces, as a value.
def which() -> Which:
    match (try: build()):
        case Ok(value):
            return Second
        case Err(error):
            return error

test \"the first written field fails first\":
    expect which() == First
",
    );
}
