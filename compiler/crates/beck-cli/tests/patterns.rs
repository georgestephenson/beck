//! Nested patterns, and the exhaustiveness check that had to be rebuilt for them.
//!
//! [`docs/08-roadmap.md`](../../../../docs/08-roadmap.md)'s concurrency-and-errors bullet has named
//! "pattern matching completion" as its remaining half since Phase 3 began, and
//! [`docs/27`](../../../../docs/27-the-walls-come-down-report.md) §27.3 said what
//! was missing in one sentence: "**nested patterns are still refused** … patterns in Beck are one
//! level deep, which is what §3.1's exhaustiveness check needs and no more".
//! [`docs/90`](../../../../docs/90-pattern-matching-report.md) is what building them found.
//!
//! Three groups, and the middle one is the point:
//!
//! 1. **programs**, because a pattern that typechecks and binds the wrong value is the failure
//!    mode, so every shape here is *run* and its answer asserted rather than compiled and counted;
//! 2. **exhaustiveness**, which stopped being a set of variant names the moment `case Some(Circle)`
//!    could name a variant it does not cover — every case below is one the old check would have
//!    got wrong in one direction or the other, and half of them are programs it would have
//!    **wrongly refused**;
//! 3. **the ceiling**, because a pattern is an ordinary expression and nesting one is a second way
//!    to recurse in the front end — which is
//!    [`docs/82`](../../../../docs/82-the-edge-report.md)'s finding three times
//!    over.

use std::process::Command;

fn beck() -> Command {
    Command::new(env!("CARGO_BIN_EXE_beck"))
}

fn write(name: &str, src: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("beck-patterns");
    std::fs::create_dir_all(&dir).expect("a writable temporary directory");
    let path = dir.join(name);
    std::fs::write(&path, src).expect("the fixture is writable");
    path
}

/// Compile and run a program's own tests, returning what `beck test` printed.
fn run_tests(name: &str, src: &str) -> String {
    let path = write(name, src);
    let out = beck().arg("test").arg(&path).output().expect("beck runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Check a program and return every diagnostic it produced.
fn check(name: &str, src: &str) -> String {
    let path = write(name, src);
    let out = beck().arg("check").arg(&path).output().expect("beck runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const SHAPES: &str = r#"
union Shape:
    Circle(r: Int)
    Square(side: Int)

union Found:
    Yes(shape: Shape)
    No
"#;

// ---------------------------------------------------------------------------------------------
// 1. The programs
// ---------------------------------------------------------------------------------------------

#[test]
fn a_pattern_matches_inside_a_field_and_binds_what_is_there() {
    // The shape `docs/27` §27.3 refused, and the reason to want it: without the inner pattern this
    // is a `match` inside a `match`, which is three more lines and a name for a value that has one.
    let out = run_tests(
        "nested.beck",
        &format!(
            "{SHAPES}
def area(f: Found) -> Int:
    match f:
        case Yes(Circle(r)):
            return 3 * r * r
        case Yes(Square(side)):
            return side * side
        case No:
            return 0

test \"the inner variant decides, and the inner binder is what it bound\":
    expect area(Yes(shape=Circle(r=2))) == 12
    expect area(Yes(shape=Square(side=3))) == 9
    expect area(No) == 0
"
        ),
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn a_pattern_nests_inside_a_list_pattern() {
    let out = run_tests(
        "in-a-list.beck",
        &format!(
            "{SHAPES}
def first_circle(xs: list[Shape]) -> Int:
    match xs:
        case []:
            return -1
        case [Circle(r), *rest]:
            return r
        case [_, *rest]:
            return first_circle(rest)

test \"a nested pattern inside a list picks the element and keeps the tail\":
    expect first_circle([Square(side=1), Circle(r=7), Square(side=2)]) == 7
    expect first_circle([Square(side=1)]) == -1
    expect first_circle([]) == -1
"
        ),
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn a_literal_nests_where_a_binder_would_go() {
    // `Const` was already a pattern at the top level; what is new is that it can be a *part*.
    let out = run_tests(
        "literal.beck",
        &format!(
            "{SHAPES}
def describe(f: Found) -> Str:
    match f:
        case Yes(Circle(0)):
            return \"a point\"
        case Yes(Circle(r)):
            return \"a circle\"
        case Yes(Square(side)):
            return \"a square\"
        case No:
            return \"nothing\"

test \"a literal inside a constructor is a pattern like any other\":
    expect describe(Yes(shape=Circle(r=0))) == \"a point\"
    expect describe(Yes(shape=Circle(r=5))) == \"a circle\"
    expect describe(No) == \"nothing\"
"
        ),
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn a_nested_pattern_reaches_through_a_type_parameter() {
    // `Option[Shape]` is the prelude's union at an argument, so the inner pattern is checked
    // against `Shape` and not against the declaration's `T`. Getting this wrong typechecks — the
    // inner binder would just be a fresh variable — so it is asserted by running.
    let out = run_tests(
        "generic.beck",
        &format!(
            "{SHAPES}
def radius(o: Option[Shape]) -> Int:
    match o:
        case Some(Circle(r)):
            return r
        case Some(Square(side)):
            return side
        case None:
            return 0

test \"the inner pattern sees the type argument\":
    expect radius(Some(value=Circle(r=4))) == 4
    expect radius(Some(value=Square(side=6))) == 6
    expect radius(None) == 0
"
        ),
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

// ---------------------------------------------------------------------------------------------
// 2. Exhaustiveness, which is where the old check would have been wrong
// ---------------------------------------------------------------------------------------------

#[test]
fn covering_every_inner_variant_covers_the_outer_one() {
    // The case that decides whether this feature is usable. `case Yes(Circle)` and
    // `case Yes(Square)` together cover `Yes`, and a check that counted variant *names* would
    // demand a `case _` that can never run — which is a compile error the program does not deserve
    // and an unreachable arm as the only way to silence it.
    let out = check(
        "covered.beck",
        &format!(
            "{SHAPES}
def area(f: Found) -> Int:
    match f:
        case Yes(Circle(r)):
            return r
        case Yes(Square(side)):
            return side
        case No:
            return 0
"
        ),
    );
    assert!(
        !out.contains("B0341"),
        "wrongly refused as inexhaustive:\n{out}"
    );
}

#[test]
fn a_missing_inner_variant_is_named_as_a_whole_value() {
    let out = check(
        "missing-inner.beck",
        &format!(
            "{SHAPES}
def area(f: Found) -> Int:
    match f:
        case Yes(Circle(r)):
            return r
        case No:
            return 0
"
        ),
    );
    assert!(out.contains("B0341"), "{out}");
    // Not "missing: Yes" — `Yes` is written. The counterexample is the value that escapes.
    assert!(out.contains("missing: Yes(Square)"), "{out}");
}

#[test]
fn a_literal_pattern_never_completes_a_type() {
    // An `Int` has constructors nobody can enumerate, so covering `0` and `1` is not covering an
    // `Int`. The old check never asked the question; this one has to answer it the honest way.
    let out = check(
        "literals.beck",
        "def f(n: Int) -> Str:\n    \
         match n:\n        \
         case 0:\n            \
         return \"none\"\n        \
         case 1:\n            \
         return \"one\"\n",
    );
    assert!(out.contains("B0341"), "{out}");
}

#[test]
fn a_nested_list_pattern_is_exhaustive_when_both_shapes_are_covered() {
    let out = check(
        "list-nested.beck",
        &format!(
            "{SHAPES}
def first_r(xs: list[Shape]) -> Int:
    match xs:
        case []:
            return 0
        case [Circle(r), *rest]:
            return r
        case [Square(side), *rest]:
            return side
"
        ),
    );
    assert!(
        !out.contains("B0341"),
        "wrongly refused as inexhaustive:\n{out}"
    );
}

#[test]
fn a_fixed_length_list_pattern_names_the_length_that_escapes() {
    // `case [a, b]` covers exactly one length, so the honest answer names a list the program
    // cannot handle rather than "a list with elements" — which is what the shape-set check said,
    // and which does not tell the author what to test with.
    let out = check(
        "fixed.beck",
        "def f(xs: list[Int]) -> Int:\n    \
         match xs:\n        \
         case [a, b]:\n            \
         return a\n",
    );
    assert!(out.contains("B0341"), "{out}");
    assert!(out.contains("the empty list"), "{out}");
    assert!(out.contains("[_]"), "{out}");
}

#[test]
fn an_arm_the_arms_above_it_already_cover_is_reported() {
    // The other half of the same algorithm, and the half nested patterns make easy to write by
    // accident: `Yes(_)` swallows every `Yes`, so the `Yes(Square)` below it is dead.
    let out = check(
        "dead-arm.beck",
        &format!(
            "{SHAPES}
def area(f: Found) -> Int:
    match f:
        case Yes(Circle(r)):
            return r
        case Yes(_):
            return 1
        case Yes(Square(side)):
            return side
        case No:
            return 0
"
        ),
    );
    assert!(out.contains("B0355"), "{out}");
    assert!(out.contains("can never match"), "{out}");
    // A warning: the program still compiles, because `case _` after every variant is a habit and
    // refusing a habit is a change to what compiles rather than a diagnostic.
    assert!(
        !out.contains("error["),
        "an unreachable arm is not an error:\n{out}"
    );
}

#[test]
fn a_reachable_arm_is_not_reported() {
    // The control, and it is what stops the check above from being satisfied by a warning on
    // everything: the same program with the arms in an order where each one can run.
    let out = check(
        "live-arms.beck",
        &format!(
            "{SHAPES}
def area(f: Found) -> Int:
    match f:
        case Yes(Circle(r)):
            return r
        case Yes(Square(side)):
            return side
        case No:
            return 0
"
        ),
    );
    assert!(!out.contains("B0355"), "{out}");
}

// ---------------------------------------------------------------------------------------------
// 3. The ceiling
// ---------------------------------------------------------------------------------------------

#[test]
fn a_pattern_that_nests_too_deep_is_a_diagnostic_rather_than_a_crash() {
    // `docs/44` bounded the front end's recursion as a *count*, and `docs/82` found three
    // productions that got past it. A nested pattern is a new recursion in the checker, so this is
    // the same question asked of the new one: a pattern deeper than the ceiling must produce a
    // diagnostic with a span, in every build profile, rather than abort the process.
    //
    // What answers today is the **reader's** counter, `B0121`, and not the checker's: a pattern is
    // an ordinary call form, so the parser meets it first and the two ceilings are the same
    // number. The checker counts its own recursion anyway, and `docs/90` §90.7 says why that is
    // not belt-and-braces — three times in this project's history a limit at one production has
    // been reached through another.
    let depth = beck_diag::depth::MAX_NESTING as usize + 64;
    let src = format!(
        "union Wrap:\n    W(inner: Wrap)\n\ndef f(w: Wrap) -> Int:\n    match w:\n        case {}:\n            return 0\n        case _:\n            return 1\n",
        nest("W(", "x", ")", depth)
    );
    let out = check("deep.beck", &src);
    assert!(
        out.contains("B0121") || out.contains("B0390"),
        "a pattern {depth} deep neither checked nor refused:\n{out}"
    );
    // The point of the ceiling: a refusal, with a place in the file, and a process still running.
    assert!(out.contains("-->"), "the refusal has no span:\n{out}");
}

#[test]
fn a_pattern_just_under_the_ceiling_still_checks() {
    // The other half, and the one that says the ceiling is a ceiling rather than a wall: a
    // pattern nested well below the limit compiles and runs.
    let out = run_tests(
        "deep-enough.beck",
        &format!(
            "union Wrap:\n    W(inner: Wrap)\n    End(n: Int)\n\n\
             def f(w: Wrap) -> Int:\n    \
             match w:\n        \
             case {}:\n            \
             return n\n        \
             case _:\n            \
             return -1\n\n\
             test \"a pattern thirty deep binds what is at the bottom of it\":\n    \
             expect f({}) == 9\n",
            nest("W(", "End(n)", ")", 30),
            nest("W(inner=", "End(n=9)", ")", 30)
        ),
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

fn nest(open: &str, leaf: &str, close: &str, depth: usize) -> String {
    let mut s = leaf.to_string();
    for _ in 0..depth {
        s = format!("{open}{s}{close}");
    }
    s
}

// ---------------------------------------------------------------------------------------------
// 4. Or-patterns and guards — the rest of `docs/90` §90.8's list
// ---------------------------------------------------------------------------------------------

#[test]
fn an_or_pattern_binds_one_name_from_either_alternative() {
    let out = run_tests(
        "or.beck",
        "union Shape:\n    \
         Circle(r: Int)\n    \
         Square(side: Int)\n    \
         Point\n\n\
         def size(s: Shape) -> Int:\n    \
         match s:\n        \
         case Circle(r) | Square(r):\n            \
         return r\n        \
         case Point:\n            \
         return 0\n\n\
         test \"either alternative binds the one name the body reads\":\n    \
         expect size(Circle(r=3)) == 3\n    \
         expect size(Square(side=4)) == 4\n    \
         expect size(Point) == 0\n",
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn an_or_pattern_covers_what_its_alternatives_cover() {
    // Two variants covered by one arm, and the third by another: exhaustive with no `case _`.
    // A check that read the arm as one constructor would refuse this.
    let out = check(
        "or-covers.beck",
        "union Shape:\n    \
         Circle(r: Int)\n    \
         Square(side: Int)\n    \
         Point\n\n\
         def size(s: Shape) -> Int:\n    \
         match s:\n        \
         case Circle(r) | Square(r):\n            \
         return r\n        \
         case Point:\n            \
         return 0\n",
    );
    assert!(
        !out.contains("B0341"),
        "wrongly refused as inexhaustive:\n{out}"
    );
}

#[test]
fn an_or_pattern_that_does_not_cover_a_variant_still_says_so() {
    // The control for the test above: the same shape with a variant left out. Without this, an
    // or-pattern read as a wildcard would pass both.
    let out = check(
        "or-misses.beck",
        "union Shape:\n    \
         Circle(r: Int)\n    \
         Square(side: Int)\n    \
         Point\n\n\
         def size(s: Shape) -> Int:\n    \
         match s:\n        \
         case Circle(r) | Square(r):\n            \
         return r\n",
    );
    assert!(out.contains("B0341"), "{out}");
    assert!(out.contains("Point"), "{out}");
}

#[test]
fn an_or_pattern_nested_inside_a_constructor_is_not_a_wildcard() {
    // The shape that would go wrong if the exhaustiveness matrix ever inspected an alternative
    // without expanding it: the or-pattern is a *field* rather than the whole arm, so it reaches
    // column zero only after specialising into `Yes`.
    let out = check(
        "or-nested.beck",
        &format!(
            "{SHAPES}
def area(f: Found) -> Int:
    match f:
        case Yes(Circle(r) | Square(r)):
            return r
        case No:
            return 0
"
        ),
    );
    assert!(
        !out.contains("B0341"),
        "wrongly refused as inexhaustive:\n{out}"
    );

    // …and with one alternative removed it is not exhaustive, which is the half that would pass
    // anyway if an unexpanded alternative were read as a wildcard.
    let out = check(
        "or-nested-partial.beck",
        &format!(
            "{SHAPES}
def area(f: Found) -> Int:
    match f:
        case Yes(Circle(r)):
            return r
        case No:
            return 0
"
        ),
    );
    assert!(out.contains("B0341"), "{out}");
}

#[test]
fn the_alternatives_of_an_or_pattern_have_to_bind_the_same_names() {
    let out = check(
        "or-mismatch.beck",
        "union Shape:\n    \
         Circle(r: Int)\n    \
         Point\n\n\
         def size(s: Shape) -> Int:\n    \
         match s:\n        \
         case Circle(r) | Point:\n            \
         return r\n",
    );
    assert!(out.contains("B0356"), "{out}");
    assert!(out.contains("does not bind r"), "{out}");
}

#[test]
fn a_pipe_outside_a_pattern_is_refused() {
    // Beck has no bitwise operators, so `|` means one thing. The checker is where that is said,
    // because the token is in the expression grammar — the same division `*rest` has.
    let out = check(
        "pipe.beck",
        "def f(a: Int, b: Int) -> Int:\n    return a | b\n",
    );
    assert!(out.contains("B0357"), "{out}");
}

#[test]
fn a_guard_falls_through_to_the_next_arm() {
    // The whole of what makes a guard a guard rather than an `if` in the body: a false one does
    // not take the arm, it moves on.
    let out = run_tests(
        "guard.beck",
        "def classify(n: Int) -> Str:\n    \
         match n:\n        \
         case 0:\n            \
         return \"zero\"\n        \
         case x if x < 0:\n            \
         return \"negative\"\n        \
         case _:\n            \
         return \"positive\"\n\n\
         test \"a false guard falls through\":\n    \
         expect classify(0) == \"zero\"\n    \
         expect classify(-5) == \"negative\"\n    \
         expect classify(7) == \"positive\"\n",
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn a_guarded_arm_covers_nothing() {
    // The rule a guard forces on exhaustiveness, and the one that is easy to get wrong: whether
    // a guarded arm matches depends on a *value*, so it cannot be counted as covering a shape.
    // `case Square(side) if side > 0` leaves `Square` uncovered.
    let out = check(
        "guard-covers.beck",
        "union Shape:\n    \
         Circle(r: Int)\n    \
         Square(side: Int)\n\n\
         def size(s: Shape) -> Int:\n    \
         match s:\n        \
         case Circle(r):\n            \
         return r\n        \
         case Square(side) if side > 0:\n            \
         return side\n",
    );
    assert!(out.contains("B0341"), "{out}");
    assert!(out.contains("Square"), "{out}");
}

#[test]
fn a_guarded_arm_above_does_not_make_an_arm_unreachable() {
    // The other side of the same rule. `case Circle(r) if r > 0` does not swallow every `Circle`,
    // so the arm below it is live — and reporting it dead would be a warning about a correct
    // program, which is worse than no warning.
    let out = check(
        "guard-live.beck",
        "union Shape:\n    \
         Circle(r: Int)\n    \
         Square(side: Int)\n\n\
         def size(s: Shape) -> Int:\n    \
         match s:\n        \
         case Circle(r) if r > 0:\n            \
         return r\n        \
         case Circle(r):\n            \
         return 0\n        \
         case Square(side):\n            \
         return side\n",
    );
    assert!(!out.contains("B0355"), "{out}");
    assert!(!out.contains("B0341"), "{out}");
}

#[test]
fn a_guard_reads_a_binding_and_the_analyses_see_it() {
    // `Arm` gained a field, and fourteen passes walk an arm's expressions — liveness, the frame
    // pass, the plan's free variables, placement, the effect walk. A guard those passes did not
    // see is not a compile error: it is a variable liveness never marks and a slot `frames` never
    // reserves, which shows up as a missing binding on a program that uses one. This is that
    // program, and it binds inside the guard as well as reading through it.
    let out = run_tests(
        "guard-analyses.beck",
        "def f(xs: list[Int]) -> Int:\n    \
         match xs:\n        \
         case [a, *rest] if list_len(rest) + a > 3:\n            \
         return a\n        \
         case [a, *rest]:\n            \
         return 0 - a\n        \
         case []:\n            \
         return 0\n\n\
         test \"a guard reads the pattern's binders and the passes see it\":\n    \
         expect f([9, 1, 2]) == 9\n    \
         expect f([1]) == -1\n    \
         expect f([]) == 0\n",
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn an_at_binding_names_the_value_a_pattern_takes_apart() {
    let out = run_tests(
        "at.beck",
        &format!(
            "{SHAPES}
def keep(f: Found, fallback: Shape) -> Shape:
    match f:
        case Yes(whole @ Circle(r)) if r > 100:
            return whole
        case Yes(whole @ Square(_)):
            return whole
        case _:
            return fallback

test \"the name is the whole value the pattern matched\":
    expect keep(Yes(shape=Circle(r=200)), Square(side=1)) == Circle(r=200)
    expect keep(Yes(shape=Circle(r=2)), Square(side=1)) == Square(side=1)
    expect keep(Yes(shape=Square(side=7)), Circle(r=1)) == Square(side=7)
"
        ),
    );
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn an_at_binding_covers_what_the_pattern_under_it_covers() {
    // A name refuses nothing, so `whole @ Circle(r)` covers exactly `Circle`. Both directions,
    // because a check that read the binder as irrefutable would call the first program exhaustive
    // and both would pass.
    let covered = check(
        "at-covers.beck",
        "union Shape:\n    \
         Circle(r: Int)\n    \
         Square(side: Int)\n\n\
         def size(s: Shape) -> Int:\n    \
         match s:\n        \
         case whole @ Circle(r):\n            \
         return r\n",
    );
    assert!(covered.contains("B0341"), "{covered}");
    assert!(covered.contains("Square"), "{covered}");

    let full = check(
        "at-full.beck",
        "union Shape:\n    \
         Circle(r: Int)\n    \
         Square(side: Int)\n\n\
         def size(s: Shape) -> Int:\n    \
         match s:\n        \
         case whole @ Circle(r):\n            \
         return r\n        \
         case Square(side):\n            \
         return side\n",
    );
    assert!(!full.contains("B0341"), "{full}");
}

#[test]
fn an_at_outside_a_pattern_is_refused() {
    let out = check(
        "at-expr.beck",
        "def f(a: Int, b: Int) -> Int:\n    return a @ b\n",
    );
    assert!(out.contains("B0357"), "{out}");
}
