//! What a lambda may hand over, now that its body is analysed as the frame it is.
//!
//! `docs/70` made a last read a *move*: the frame hands the value over instead of lending it, so
//! `list_append` can push into a list nobody else holds. It excluded everything inside a lambda,
//! for a reason that is right about one thing and wrong about another — a closure outlives the
//! expression that built it, so a variable it takes from the scope around it must stay lent; but a
//! variable the lambda *itself* binds lives in the frame its own call makes, and that frame is as
//! private as a definition's.
//!
//! `docs/70` draws that line. The rule it adds is one word:
//!
//! > A read may be handed over only by **the frame that binds it**.
//!
//! These are the programs that would notice if it were drawn wrong. They are about answers, not
//! speed — `beck-cli/tests/scaling.rs` holds the shape, and `docs/70` §70.2 the numbers.
//!
//! A wrong analysis cannot corrupt a value silently: `Env::read` empties a frame only when
//! `Arc::get_mut` proves nothing else holds it, so the failure mode is a *missing* binding rather
//! than somebody else's. That makes these tests blunt instruments on purpose — each asserts an
//! answer, and either gets it or gets an unbound variable.

use beck_rt::testing::{Options, Outcome};

fn cases(src: &str) -> Vec<(std::sync::Arc<str>, Outcome)> {
    let (placed, d, m) = beck_core::compile_or_library_str("moves.beck", src);
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

/// The optimisation itself, asserted on the **contents** rather than the length.
///
/// A push into a list the fold's accumulator holds alone produces the same list a copy would. A
/// push into one somebody else holds does not, and a length would not notice.
#[test]
fn a_fold_that_appends_builds_the_list_it_would_have_copied() {
    all_pass(
        "\
def collected() -> list[Int]:
    return list_fold([1, 2, 3, 4], [], lambda acc, x: list_append(acc, x * 10))

test \"the fold's answer is the list, in order\":
    expect collected() == [10, 20, 30, 40]
    expect list_len(collected()) == 4
",
    );
}

/// A lambda's parameter is bound by the call, so **two calls do not share one**.
///
/// The caller reads `xs` twice; the first read is not its last, so what the first call receives is
/// shared and cannot be pushed into, and the second is handed over and can. Both answers have to
/// be the same, which is the whole promise: whether a value is moved or copied is invisible.
#[test]
fn a_list_passed_to_a_lambda_twice_survives_the_first_call() {
    all_pass(
        "\
def apply_twice(f: (list[Int]) -> list[Int], xs: list[Int]) -> Int:
    return list_len(f(xs)) + list_len(f(xs))

def grown() -> Int:
    return apply_twice(lambda ys: list_append(ys, 99), [1, 2, 3])

test \"both calls see the same three elements\":
    expect grown() == 8
",
    );
}

/// A variable a lambda takes from **the scope around it** is not the lambda's to hand over.
///
/// `xs` belongs to `keep`'s frame; the closure reads it and is called twice, and `keep` reads it
/// again afterwards. If a lambda body could mark a free variable's read as a last use, the first
/// call would empty a binding two more readers need.
#[test]
fn a_lambda_does_not_hand_over_what_it_took_from_the_scope_around_it() {
    all_pass(
        "\
def keep(xs: list[Int]) -> Int:
    grow = lambda i: list_len(list_append(xs, i))
    return grow(1) + grow(2) + list_len(xs)

test \"the enclosing binding outlives every call to the closure\":
    expect keep([1, 2, 3]) == 11
",
    );
}

/// A closure built **inside** a lambda still holds what it captured after that lambda returns.
///
/// `acc` is the outer lambda's own parameter, so it is exactly the kind of binding this change
/// makes movable — and it must stop being movable the moment something captures it. The captured
/// closures are called after every fold step has finished.
#[test]
fn a_closure_built_inside_a_lambda_keeps_what_it_captured() {
    all_pass(
        "\
def counters() -> list[() -> Int]:
    return list_fold([1, 2, 3], [], lambda acc, x: list_append(acc, lambda: list_len(acc) + x))

def totals() -> list[Int]:
    return map_list(counters(), lambda f: f())

test \"each closure remembers the accumulator it was built from\":
    expect totals() == [1, 3, 5]
",
    );
}

/// A lambda parameter read more than once: only the last read may be handed over.
#[test]
fn only_the_last_read_of_a_lambda_parameter_is_handed_over() {
    all_pass(
        "\
def doubled() -> list[Int]:
    return list_fold([1, 2], [], lambda acc, x: list_append(list_append(acc, x), list_len(acc)))

test \"an accumulator read twice in one body is intact for the second read\":
    expect doubled() == [1, 0, 2, 2]
",
    );
}

/// The same analysis inside a `test` block, which is where it did not run at all until `docs/70`.
///
/// The three passes walk `Program::defs`; a test's clauses are in `Program::tests`, so a lambda
/// written inside a `test` block was annotated by none of them. Now it is, and this is a program
/// whose answer depends on the lambda being right either way.
#[test]
fn a_lambda_written_inside_a_test_block_is_analysed_too() {
    all_pass(
        "\
def source() -> list[Int]:
    return [1, 2, 3]

test \"a fold written in the test block itself\":
    expect list_fold(source(), [], lambda acc, x: list_append(acc, x + 1)) == [2, 3, 4]
",
    );
}
