//! The environment — what a call's frame promises, now that a `let` writes into it.
//!
//! `docs/70` replaced the scope a `let` used to allocate with a slot reserved by the call that
//! contains it, which is worth about four fifths of what a binding cost. The correctness of that
//! rests on one invariant, and it is the kind of invariant a program only violates in the
//! circumstance nobody wrote a test for:
//!
//! > **A slot is written at most once per call**, and a frame a closure has captured is never
//! > written at all.
//!
//! `beck_core::frames` gets the first half by reserving for what can be live at once — summing
//! what runs in sequence, taking the maximum over branches that cannot both run — and the
//! evaluator gets the second from `Arc::get_mut`, which refuses when a closure's clone is holding
//! the frame. Its unit tests cover the counting. These cover what a *program* would see if either
//! half were wrong, which is a closure quietly answering with somebody else's value.
//!
//! Nothing here is about speed. `docs/70` §70.3 has the numbers.

use beck_rt::testing::{Options, Outcome};

fn passes(src: &str) -> Vec<(std::sync::Arc<str>, Outcome)> {
    let (placed, d, m) = beck_core::compile_or_library_str("frames.beck", src);
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
    let cases = passes(src);
    assert!(!cases.is_empty(), "the program declares no tests");
    for (name, outcome) in cases {
        assert!(outcome.is_pass(), "`{name}`: {outcome:?}");
    }
}

/// The invariant itself: bindings made *after* a closure is built must not reach it.
///
/// `later` is bound after `f` captures the environment, so a frame that reused `captured`'s slot —
/// or that let a write through to a frame somebody is holding — would have `f()` answer 100
/// instead of 1.
#[test]
fn a_closure_keeps_the_bindings_it_captured_and_sees_no_others() {
    all_pass(
        "\
def answer() -> Int:
    captured = 1
    f = lambda: captured
    later = 100
    other = later + captured
    return f() + other * 0

test \"the closure answers with what it captured\":
    expect answer() == 1
",
    );
}

/// Three closures alive at once, each over a binding made by a different call.
///
/// Every call makes a frame of its own, so the closures cannot share a slot however many of them
/// there are. A frame reused between calls would have all three answer the same.
#[test]
fn closures_from_separate_calls_each_keep_their_own_binding() {
    all_pass(
        "\
def adder(n: Int) -> (Int) -> Int:
    bump = n * 10
    return lambda x: x + bump

def three() -> list[Int]:
    f1 = adder(1)
    f2 = adder(2)
    f3 = adder(3)
    return [f1(0), f2(0), f3(0)]

test \"each closure kept its own call\":
    expect three() == [10, 20, 30]
",
    )
}

/// A name bound in one arm of a `match` and a name bound in another share a reservation, because
/// only one arm can run. Both arms are exercised, and each has bindings of its own after the
/// pattern's.
#[test]
fn the_arms_of_a_match_do_not_see_each_others_bindings() {
    all_pass(
        "\
union Shape:
    Dot
    Line(a: Int, b: Int)

def measure(s: Shape) -> Int:
    base = 1000
    match s:
        case Dot:
            only = 7
            twice = only * 2
            return base + twice
        case Line(a, b):
            span = b - a
            scaled = span * 3
            return base + scaled

test \"each arm answers from its own bindings\":
    expect measure(Dot) == 1014
    expect measure(Line(a=2, b=6)) == 1012
",
    )
}

/// An inner binding of the same *name* is a different variable, and reading it must find the inner
/// one — which is what a frame holding every binding of a body at once has to get right, since
/// both now live in the same array rather than in nested scopes.
#[test]
fn an_inner_binding_shadows_an_outer_one_in_the_same_frame() {
    all_pass(
        "\
def shadowed(n: Int) -> Int:
    x = n
    outer = x * 100
    if n > 0:
        x = n + 1
        return outer + x
    return outer

test \"the inner binding is the one that is read\":
    expect shadowed(5) == 506
",
    )
}

/// A body with far more bindings than anything reserves for, to exercise the fallback: when a
/// frame runs out of slots the evaluator chains a scope, exactly as it did before reservations
/// existed, and the answer is the same either way.
#[test]
fn a_body_deeper_than_its_reservation_still_answers() {
    let mut src = String::from("def deep(n: Int) -> Int:\n");
    for i in 0..64 {
        src.push_str(&format!("    a{i} = n + {i}\n"));
    }
    src.push_str("    return ");
    src.push_str(
        &(0..64)
            .map(|i| format!("a{i}"))
            .collect::<Vec<_>>()
            .join(" + "),
    );
    // 64 bindings of `n + i`, summed: 64n + (0 + 1 + … + 63).
    src.push_str("\n\ntest \"a deep body answers\":\n    expect deep(1) == 2080\n");
    all_pass(&src);
}
