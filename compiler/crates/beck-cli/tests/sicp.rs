//! The SICP suite's first chapter, and the walls the rest of it hits.
//!
//! `docs/25-benchmarks-and-expressiveness.md` proposes SICP as Beck's expressiveness benchmark,
//! on the grounds that the project's own premise (`docs/01` §1.1) is that Beck is SICP's three
//! moves made into a language. A proposal is not evidence, so this harness holds the report's
//! claims to the standard `AGENTS.md` sets: every number in §25.6 is produced by something here.
//!
//! Two halves, and the second is the point:
//!
//! * [`sicp/ch1.beck`](../../../sicp/ch1.beck) and [`ch2.beck`](../../../sicp/ch2.beck) are the
//!   parts of the book that run today, with its own stated answers as the oracle. They pass.
//! * [`sicp/refusals/`](../../../sicp/refusals/) is one file per wall *still standing*, each the
//!   smallest program that hits it. This harness asserts each one still fails, and with which
//!   diagnostic — so that a wall coming down is a test that starts failing rather than a fact
//!   somebody notices.
//!
//! [`sicp/felleisen.beck`](../../../sicp/felleisen.beck) is the third thing, and it is the
//! *formal* half of §25.9 rather than the running half: one section per special form SICP
//! introduces, each carrying the code that recovers it or the reorganisation that concedes it.
//! `docs/63` is the verdict table; the file is the evidence for it.
//!
//! All six §25.6 measured are down, and each left a test pointing the other way rather than no test
//! at all: docs/27 for the first three, docs/27 for tail calls, docs/27 for the reals and for
//! user-written polymorphism. Removing them wrote three more and those came down too — docs/27 for
//! taking a `list[T]` apart, docs/27 for a type that takes a parameter, docs/27 for exact
//! rationals. `refusals/` is **empty**, which is a claim this file asserts rather than a directory
//! it stopped using.

use std::process::Command;

use beck_core::Placed;
use beck_rt::testing::Options;

fn compile(name: &str, src: &str) -> (Option<Placed>, String) {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    (placed, diags.render(&map))
}

/// The same, admitting a library — which is what every SICP exercise is (docs/27 §27.2).
fn compile_module(name: &str, src: &str) -> (Option<Placed>, String) {
    let (placed, diags, map) = beck_core::compile_or_library_str(name, src);
    (placed, diags.render(&map))
}

/// The whole diagnostic text, for asserting on a code *and* on what it says.
fn errors(name: &str, src: &str) -> String {
    let (_, rendered) = compile(name, src);
    assert!(
        rendered.contains("error["),
        "`{name}` was expected to be refused, and it compiled:\n{rendered}"
    );
    rendered
}

// ---------------------------------------------------------------------------------------------
// 1. Chapter 1 runs, and the book is the oracle
// ---------------------------------------------------------------------------------------------

const CH1: &str = include_str!("../../../sicp/ch1.beck");
const CH2: &str = include_str!("../../../sicp/ch2.beck");

/// Chapter 1, in-process, on whatever stack `libtest` hands this thread.
///
/// It needs no thread of its own: `beck_rt::testing::run` asks the backend how much host stack it
/// requires and provides it. A tree-walker spends a frame on recursion that is not in tail
/// position, so the requirement is real — it is declared on the seam rather than left to the
/// caller, and exceeding it is a diagnostic. `docs/27` §27.2–§27.2.
#[test]
fn chapter_one_passes_against_the_books_own_answers() {
    let (placed, rendered) = compile_module("sicp/ch1.beck", CH1);
    let placed = placed.unwrap_or_else(|| panic!("chapter 1 compiles:\n{rendered}"));
    assert!(
        !placed.is_application(),
        "chapter 1 is a library, and running it as one is the point"
    );

    let backend = beck_eval::backend(&placed);
    let report = beck_rt::testing::run(&placed, backend, &Options::default());

    assert!(
        report.cases.len() >= 20,
        "chapter 1 is the evidence for §25.6 and has to carry the exercises it claims"
    );
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
    assert_eq!(
        report.skipped(),
        0,
        "nothing in chapter 1 performs an effect"
    );
}

#[test]
fn chapter_two_reaches_the_closure_property() {
    // §2.2 is "Hierarchical Data and the Closure Property", and it is the section §25.6 item 2 said
    // chapter 2 stopped at. It does not stop there now, and this file is a library — so it is
    // evidence for §27.2 and §27.2 at once, and could not have been written for either alone.
    let (placed, rendered) = compile_module("sicp/ch2.beck", CH2);
    let placed = placed.unwrap_or_else(|| panic!("chapter 2 compiles:\n{rendered}"));
    assert!(!placed.is_application(), "chapter 2 is a library");
    assert!(
        !CH2.contains("merge_clients"),
        "and it never needed a wrapper"
    );

    let backend = beck_eval::backend(&placed);
    let report = beck_rt::testing::run(&placed, backend, &Options::default());
    assert!(
        report.cases.len() >= 6,
        "chapter 2 has to carry the exercises it claims"
    );
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
    assert_eq!(
        report.skipped(),
        0,
        "nothing in chapter 2 performs an effect"
    );
}

#[test]
fn a_library_runs_its_own_tests_with_no_application_anywhere_near_it() {
    // Was §25.6 item 1's wall and §25.7's first item: "stage 1 cannot honestly start until the
    // wrapper in `ch1.beck` can be deleted". docs/27 §27.2 deleted it.
    //
    // Asserted two ways, because the interesting half is the *second*. The first says a library's
    // tests run; the second says chapter 1 no longer contains an application at all — which is the
    // thing that was being worked around, and the thing a `beck test` that merely tolerated
    // libraries would not have fixed.
    assert!(
        !CH1.contains("merge_clients"),
        "chapter 1 still carries the wrapper this test exists to have removed"
    );
    assert!(
        !CH1.contains("model State") && !CH1.contains("union Command"),
        "chapter 1 still carries the wrapper's declarations"
    );

    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sicp/ch1.beck");
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["test", file])
        .output()
        .expect("the compiler is built");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "`beck test` on a library:\n{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("0 failed"), "{text}");

    // And `beck check` still calls it a library, because it is one. Running a library's tests is
    // not the same claim as a library being an application, and nothing here should have blurred
    // the two.
    let check = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["check", file])
        .output()
        .expect("the compiler is built");
    assert!(check.status.success());
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("a library"),
        "chapter 1 is a library and `beck check` should still say so"
    );
}

#[test]
fn a_library_test_that_needs_an_application_is_refused_by_name() {
    // The other half, and the one that matters more. A library's `Placed` carries *placeholder*
    // roles, so a page expectation that quietly rendered nothing would report a pass for a test
    // that asserted nothing — worse than the refusal it replaced.
    //
    // `given`, `when` and `fold_of` never reach the runner at all: `B0706` refuses them while
    // checking, and its note already says why ("a program with no `merge_clients` → `decide` →
    // `durable(fold(…))` has nothing for `given` and `when` to mean"). `page` is the one that does
    // reach it, because a page expectation is well-typed in any module.
    let src = "
def double(n: Int) -> Int:
    return n * 2

test \"this one is fine\":
    expect double(21) == 42

test \"this one needs a page\":
    expect page contains \"x\"
";
    let file = std::env::temp_dir().join("beck-lib-with-page.beck");
    std::fs::write(&file, src).expect("a scratch file");
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["test", file.to_str().expect("a path")])
        .output()
        .expect("the compiler is built");
    let text = String::from_utf8_lossy(&out.stdout);
    let _ = std::fs::remove_file(&file);

    assert!(
        !out.status.success(),
        "a page expectation against a library has nothing to render:\n{text}"
    );
    assert!(
        text.contains("1 passed") && text.contains("1 failed"),
        "the pure test still runs, and only the one that needs an application fails:\n{text}"
    );
    assert!(
        text.contains("a library"),
        "and the refusal names what is missing rather than reporting a mystery:\n{text}"
    );
}

// ---------------------------------------------------------------------------------------------
// 2. The walls, each still standing
// ---------------------------------------------------------------------------------------------

#[test]
fn a_type_may_mention_itself_and_anything_declared_later() {
    // Was §25.6 item 2's wall — "§2.2 *is* Hierarchical Data and the Closure Property, so this ends
    // chapter 2 at §2.2 and takes chapters 4 and 5 with it" — and §25.7 called it the highest-value
    // item on the list for reasons that have nothing to do with SICP. docs/27 §27.2 is the fix.
    //
    // Three shapes, because "recursive" is three different problems for a checker that resolved
    // declarations in source order.
    for (what, src) in [
        (
            "a union through a list",
            "
union Tree:
    Leaf(value: Int)
    Node(kids: list[Tree])
",
        ),
        (
            "a union that mentions itself directly",
            "
union Chain:
    End
    Link(next: Chain)
",
        ),
        (
            "a forward reference, and a mutual one",
            "
model Outer:
    inner: Inner

model Inner:
    back: list[Outer]
",
        ),
    ] {
        let (_, rendered) = compile("recursive.beck", src);
        assert!(
            !rendered.contains("B0310"),
            "{what} is still refused:\n{rendered}"
        );
    }
}

#[test]
fn the_three_passes_are_stages_and_not_a_depth_limit() {
    // §27.2 resolves declarations in three passes where there was one, and "three" is a count of
    // *stages* rather than of how far a chain or a cycle may reach. Nothing in the design bounds
    // depth, and the pass that could plausibly have bounded it — `collect_aliases`, which resolves
    // each alias by resolving the aliases it names first — is exactly the one that recurses.
    //
    // Both halves are built to be long enough that an implementation with a fixed budget would
    // fail rather than pass slowly.
    let mut chain = String::new();
    for i in 0..40 {
        // Declared in ascending order so that every link is a *forward* reference: the wall this
        // replaced would have refused link zero.
        chain.push_str(&format!("type A{i} = A{}\n", i + 1));
    }
    chain.push_str("type A40 = Int\n");
    chain.push_str("def use(n: A0) -> Int:\n    return n + 1\n");
    let (_, rendered) = compile_module("alias-chain.beck", &chain);
    assert!(
        !rendered.contains("error["),
        "a forty-link alias chain:\n{rendered}"
    );

    // Six declarations, unions and models alternating, mutually recursive in a ring and in none of
    // the orders a single source-order pass could have accepted.
    let ring = "
union R0:
    Z
    S(next: R1)

model R1:
    a: R2
    b: list[R0]

union R2:
    Q
    T(m: R3, n: R1)

model R3:
    c: R4

union R4:
    W(back: R0)
";
    let (_, rendered) = compile_module("ring.beck", ring);
    assert!(
        !rendered.contains("error["),
        "six mutually recursive declarations:\n{rendered}"
    );
}

#[test]
fn what_bounds_a_recursive_types_depth_is_the_evaluator_and_not_the_checker() {
    // The honest limit, so that §27.2's "nothing bounds depth" is not read as "nothing at all".
    // A recursive *type* is unbounded; a recursive *value* built by recursion that is not in tail
    // position is bounded by `beck-eval`'s depth ceiling, and that bound is a *diagnostic*
    // (`docs/27` §27.2).
    //
    // Through the binary, because the thing being checked is that the process survives.
    let program = |depth: u32| {
        format!(
            "
union Tree:
    Leaf(value: Int)
    Node(kids: list[Tree])

def build(n: Int) -> Tree:
    if n == 0:
        return Leaf(value=1)
    return Node(kids=[build(n - 1)])

def leaves(t: Tree) -> list[Tree]:
    match t:
        case Leaf(value):
            return [t]
        case Node(kids):
            return concat_lists(map_list(kids, leaves))

test \"a spine\":
    expect list_len(leaves(build({depth}))) == 1
"
        )
    };
    let run = |src: &str, name: &str| {
        let file = std::env::temp_dir().join(name);
        std::fs::write(&file, src).expect("a scratch file");
        let out = Command::new(env!("CARGO_BIN_EXE_beck"))
            .args(["test", file.to_str().expect("a path")])
            .output()
            .expect("the compiler is built");
        let _ = std::fs::remove_file(&file);
        out
    };

    // 1,000 rather than the 100 this used to carry, and now a round number rather than a hedged
    // one: the depth that fits is no longer a property of the build profile, because what stops it
    // is a counted ceiling rather than whatever stack the process happened to have.
    let shallow = run(&program(1_000), "beck-spine-shallow.beck");
    assert!(
        shallow.status.success(),
        "a tree 1,000 deep is an ordinary value in either profile:\n{}{}",
        String::from_utf8_lossy(&shallow.stdout),
        String::from_utf8_lossy(&shallow.stderr)
    );

    let deep = run(&program(50_000), "beck-spine-deep.beck");
    assert!(
        !deep.status.success(),
        "a tree 50,000 deep is past the ceiling and the test that builds it has to fail"
    );
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&deep.stdout),
        String::from_utf8_lossy(&deep.stderr)
    );
    assert!(
        !out.contains("overflowed its stack"),
        "and it must not be by aborting the process — that was the old finding:\n{out}"
    );
    assert!(
        out.contains("which is the evaluator's limit"),
        "it must be by the diagnostic, which names tail position as the way out:\n{out}"
    );
}

#[test]
fn an_alias_that_is_defined_in_terms_of_itself_is_still_refused() {
    // The one cycle that is not a feature. A `union` may be recursive because a variant is a finite
    // tag plus fields; an alias is *transparent*, so `type A = list[A]` describes an infinitely
    // large type and `type A = B; type B = A` describes none at all. Refusing them is what stops
    // the fix of §27.2 turning a source-order limitation into a hang.
    for src in ["type Chain = list[Chain]\n", "type A = B\ntype B = A\n"] {
        let out = errors("alias-cycle.beck", src);
        assert!(out.contains("B0312"), "{out}");
        assert!(out.contains("defined in terms of itself"), "{out}");
    }
}

#[test]
fn a_recursive_type_survives_every_pass_a_corpus_program_is_carried_through() {
    // Compiling one is the easy half. `corpus/25-thread.beck` is the other half, and it is in the
    // *corpus* rather than in `sicp/` on purpose: the corpus harnesses carry every program through
    // placement, the slicer, the plan, the incremental engine against its recompute oracle, replay
    // determinism, `Repr`, the value generator, the cost model, `Sendable` and `beck iface`. Each
    // of those walks a type, and each is somewhere a type containing itself can loop forever.
    //
    // This test asserts only that the program is *there* and places itself; the harnesses that
    // would hang are the ones in `corpus.rs`, `incremental_engine.rs` and `replay.rs`, and they run
    // it because it is in the directory.
    let src = include_str!("../../../corpus/25-thread.beck");
    let (placed, rendered) = compile("25-thread.beck", src);
    let placed =
        placed.unwrap_or_else(|| panic!("the recursive corpus program slices:\n{rendered}"));
    assert!(
        !src.contains("@on("),
        "a corpus program carries no placement annotations and has to place itself"
    );
    // The tree really is recursive, or this file is measuring nothing.
    let comment = placed
        .program
        .types
        .get("Comment")
        .expect("`Comment` is declared");
    let beck_core::TyDecl::Union { variants, .. } = comment else {
        panic!("`Comment` is a union")
    };
    assert!(
        variants.iter().any(|v| v
            .fields
            .iter()
            .any(|(_, t)| format!("{t}").contains("Comment"))),
        "`Comment` no longer mentions itself, so this program stopped being the test"
    );
}

/// Wall 6 down (docs/27) — the last of §25.6's six, and the one §25.7 called "the largest".
///
/// `sicp/refusals/generic.beck` held `def map[T, U]` and asserted `B0120: expected \`(\`, found
/// \`[\``. The definition is now in `ch2.beck` and used at four element types. What this asserts is
/// the property a passing `ch2.beck` does *not* prove: that a type parameter is **rigid** inside
/// the body it belongs to. A `T` that could unify with `Int` would let a definition claiming to
/// work for every type work only for one, and every call site would still typecheck.
#[test]
fn a_type_parameter_is_rigid_inside_the_body_and_fresh_at_every_call() {
    // Fresh at every call: one definition, three element types, in one expression each.
    let src = "
def pick[T](xs: list[T], fallback: T) -> T:
    if list_is_empty(xs):
        return fallback
    return fallback

def swap[A, B](a: A, b: B) -> Map[Str, Str]:
    return {\"a\": str(a), \"b\": str(b)}

test \"instantiated afresh\":
    expect pick([1], 2) == 2
    expect pick([\"x\"], \"y\") == \"y\"
    expect pick([1.5], 2.5) == 2.5
    expect map_len(swap(1, \"two\")) == 2
";
    let (placed, rendered) = compile_module("generic.beck", src);
    let placed = placed.unwrap_or_else(|| panic!("a polymorphic definition compiles:\n{rendered}"));
    let report = beck_rt::testing::run(&placed, beck_eval::backend(&placed), &Options::default());
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );

    // Rigid inside the body: each of these would typecheck if `T` were an ordinary inference
    // variable, and each is a definition that lies about what it works for.
    for (body, note) in [
        (
            "def f[T](x: T) -> Int:\n    return x + 1\n",
            "arithmetic on a parameter",
        ),
        (
            "def f[T](x: T) -> T:\n    return 1\n",
            "returning a concrete type",
        ),
        (
            "def f[T](x: T) -> T:\n    return str(x)\n",
            "returning something else's",
        ),
    ] {
        let out = errors("rigid.beck", body);
        assert!(out.contains("B0320"), "{note}:\n{out}");
        assert!(
            out.contains('T'),
            "and the message has to name the parameter the programmer wrote, not `?7` — {note}:\n{out}"
        );
    }

    // A parameter that shadows a type, or repeats, is refused rather than resolved: there is no
    // syntax to disambiguate either afterwards.
    assert!(errors("shadow.beck", "def f[Int](x: Int) -> Int:\n    return x\n").contains("B0314"));
    assert!(errors("dup.beck", "def f[T, T](x: T) -> T:\n    return x\n").contains("B0315"));
}

/// The wall docs/27 §27.3 named, down the report after it named it.
///
/// `sicp/refusals/list-destructuring.beck` held `accumulate` and asserted `B0343: \`list\` is not a
/// constructor`. `accumulate` is now in `ch2.beck`, and §2.2.3 — "Sequences as Conventional
/// Interfaces", the section the whole of chapter 2's second half is built out of — went with it.
///
/// What is asserted here is the half `ch2.beck` cannot assert about itself: that a `match` over a
/// list is checked for **exhaustiveness**. A list is empty or it is not, and a fold that handles
/// one of those is the shape that fails on the input nobody tested.
#[test]
fn a_list_can_be_taken_apart_and_a_match_on_one_has_to_cover_both_shapes() {
    let src = "
def accumulate[T, U](xs: list[T], seed: U, combine: (T, U) -> U) -> U:
    match xs:
        case []:
            return seed
        case [first, *rest]:
            return combine(first, accumulate(rest, seed, combine))

def total(xs: list[Int]) -> Int:
    return accumulate(xs, 0, lambda x, acc: x + acc)

def joined(xs: list[Str]) -> Str:
    return accumulate(xs, \"\", lambda s, acc: s + acc)

test \"one fold, two element types\":
    expect total([1, 2, 3]) == 6
    expect total([]) == 0
    expect joined([\"a\", \"b\"]) == \"ab\"
";
    let (placed, rendered) = compile_module("accumulate.beck", src);
    let placed = placed.unwrap_or_else(|| panic!("`accumulate` compiles at last:\n{rendered}"));
    let report = beck_rt::testing::run(&placed, beck_eval::backend(&placed), &Options::default());
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );

    // Each of these is a `match` on a list that misses a shape, and each names the shape it misses.
    // The third names a *length* — a fixed-length pattern leaves the empty list and a
    // one-element list uncovered, and saying `[_]` gives the author an input to test with
    // (`docs/90`).
    for (arms, missing) in [
        (
            "        case []:\n            return 0\n",
            "a list with elements",
        ),
        (
            "        case [first, *rest]:\n            return first\n",
            "the empty list",
        ),
        (
            "        case [a, b]:\n            return a\n",
            "the empty list — `case []`, [_]",
        ),
    ] {
        let src = format!("def f(xs: list[Int]) -> Int:\n    match xs:\n{arms}");
        let out = errors("partial.beck", &src);
        assert!(out.contains("B0341"), "{out}");
        assert!(out.contains(missing), "expected `{missing}` in:\n{out}");
    }

    // And `*rest` is the tail, so it goes at the end and nowhere else.
    let out = errors(
        "misplaced.beck",
        "def f(xs: list[Int]) -> Int:\n    match xs:\n        case [*rest, last]:\n            return last\n",
    );
    assert!(out.contains("B0346"), "{out}");
}

/// The refusals directory is empty, and the harness says so out loud.
///
/// It held one file per wall still standing, and the last of them — `rational.beck`, which needed
/// `+` to reach a type the compiler does not know about — came down in `docs/27`. An empty
/// directory is a claim, so it is asserted rather than left to be noticed: every wall this project
/// has *found* has been removed. It is not the claim that Beck expresses all of SICP, and §27.10
/// says which chapters are simply unattempted.
///
/// A `.beck` file appearing here again should make this test fail, because the file would have a
/// refusal to assert and nothing would be asserting it.
#[test]
fn every_wall_this_suite_measured_has_come_down() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sicp/refusals");
    let standing: Vec<String> = std::fs::read_dir(dir)
        .expect("the refusals directory is checked in")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".beck"))
        .collect();
    assert!(
        standing.is_empty(),
        "a wall is standing again and nothing asserts it: {standing:?}"
    );
    assert!(
        std::path::Path::new(dir).join("README.md").exists(),
        "an empty refusals directory has to say what that means"
    );
}

/// Wall 5 down (docs/27), and the strongest oracle in the suite.
///
/// `sicp/refusals/real.beck` asserted that Newton's method did not typecheck. What replaced it is
/// not "it typechecks" — it is that `sqrt(9.0)` prints **3.00009155413138**, which is the number on
/// the page of SICP, digit for digit, because both sides are IEEE 754 doubles running the same
/// sequence of operations. Three more of the book's printed reals go with it, and they live in
/// `ch1.beck` where the rest of the chapter is.
///
/// What is asserted here is the part `ch1.beck` cannot assert about itself: that the resolution of
/// `+` from its operands did not quietly make `Int` arithmetic mean something else, and that mixing
/// the tiers is still refused rather than coerced.
#[test]
fn real_arithmetic_is_resolved_from_its_operands_and_the_tiers_do_not_mix() {
    let ok = "
def half(x: Float) -> Float:
    return x / 2.0

def halves(n: Int) -> Int:
    return n / 2

def both(n: Int, x: Float) -> Float:
    return half(x) + float(halves(n))

test \"the tiers coexist\":
    expect halves(7) == 3
    expect half(7.0) == 3.5
    expect both(7, 7.0) == 6.5
    expect abs(-2) == 2
    expect abs(-2.5) == 2.5
    expect sqrt(16.0) == 4.0
";
    let (placed, rendered) = compile_module("tiers.beck", ok);
    let placed = placed.unwrap_or_else(|| panic!("both tiers compile:\n{rendered}"));
    let report = beck_rt::testing::run(&placed, beck_eval::backend(&placed), &Options::default());
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );

    // Ad-hoc resolution is not coercion. `1 + 1.0` has no answer, and inventing one — promoting the
    // `Int`, the way C does — is the decision this deliberately does not take (docs/27 §27.2).
    let out = errors(
        "mixed.beck",
        "def f(n: Int, x: Float) -> Float:\n    return n + x\n",
    );
    assert!(out.contains("B0320"), "{out}");
    assert!(
        out.contains("expected `Int`, found `Float`"),
        "the left operand decides, and the right has to match it:\n{out}"
    );
}

/// The ordering defect the reals brought with them, which no SICP exercise would have caught.
///
/// `Value::Float` is a `u64` because a fold's accumulator needs a total order. It used to hold
/// `f64::to_bits`, which orders `-1.0` *above* `1.0` — so `<` answered backwards for every negative
/// real and `sort_by` reversed them. docs/27 §27.8. The fix is an order-preserving key, and this is
/// what says it stayed fixed.
#[test]
fn comparing_and_sorting_reals_agrees_with_arithmetic() {
    let src = "
def neg(x: Float) -> Float:
    return 0.0 - x

test \"negatives compare as numbers, not as bit patterns\":
    expect -1.0 < 1.0
    expect -2.5 < -1.5
    expect not (-1.0 > 1.0)
    expect neg(3.0) < 0.0
    expect 0.0 - 0.0 == 0.0

test \"and sort by the same order\":
    expect sort_by([1.5, -2.5, 0.0, -0.5], lambda x: x) == [-2.5, -0.5, 0.0, 1.5]
";
    let (placed, rendered) = compile_module("reals.beck", src);
    let placed = placed.unwrap_or_else(|| panic!("compiles:\n{rendered}"));
    let report = beck_rt::testing::run(&placed, beck_eval::backend(&placed), &Options::default());
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
}

#[test]
fn an_if_over_two_function_values_is_typed_by_joining_them() {
    // Was §25.6 item 6's wall, and the only one of the six that was a defect. §25.7 put it third;
    // docs/27 §27.2 is the fix. The test is not deleted with the refusal file — it is turned round,
    // because a defect that has been fixed is a defect that can come back, and the shape it comes
    // back in is somebody making one branch the expectation of the other again.
    //
    // Exercise 1.43 itself now lives in `ch1.beck` and is run against the book's answers by
    // `chapter_one_passes_against_the_books_own_answers`.
    let src = "
def identity(n: Int) -> Int:
    return n

def compose(f: Int -> Int, g: Int -> Int) -> Int -> Int:
    return lambda x: f(g(x))

def pick(b: Bool, f: Int -> Int) -> Int -> Int:
    if b:
        return identity
    return compose(f, f)
";
    let (_, rendered) = compile("higher-order.beck", src);
    assert!(
        !rendered.contains("B0320"),
        "a branch returning a call's result and a branch returning a pure definition are \
         alternatives, not an expectation and a candidate:\n{rendered}"
    );
}

#[test]
fn joining_two_branches_keeps_the_effects_of_both() {
    // The soundness direction of that fix, and the reason it is a *join* rather than a shrug. The
    // row of an `if` over two function values must contain whatever either branch might perform,
    // or exercise 1.43 would have been bought by losing an effect — which §3.2 does not permit and
    // which placement, `beck iface` and the security proofs all read.
    //
    // Asserted on the *caller's* inferred row, because that is the number every downstream pass
    // reads: placement, `beck iface`, the infrastructure derivation and §3.5's proofs.
    let src = "
def pure_one() -> Int:
    return 1

def stamped() -> Int uses nondet:
    return now()

def pick(b: Bool) -> Int:
    if b:
        return pure_one()
    return stamped()
";
    // `check_str` rather than `compile_str`: this is a library, so there is nothing to slice, and
    // that is item 1's wall rather than this one's.
    let (program, diags, map) = beck_core::check_str("join-effects.beck", src);
    let rendered = diags.render(&map);
    assert!(!rendered.contains("B0320"), "{rendered}");
    let pick = program.defs.get("pick").expect("`pick` is defined");
    assert!(
        pick.effects.iter().any(|e| e.name() == "nondet"),
        "the join dropped the effectful branch: `pick` performs {:?}",
        pick.effects.iter().map(|e| e.name()).collect::<Vec<_>>()
    );
}

#[test]
fn a_tail_call_costs_nothing_so_an_iterative_process_is_iterative() {
    // §1.2.1's distinction, which `docs/27` built and `ch1.beck`'s `count_to` exercise asserts
    // from the language's side.
    //
    // Run through the binary rather than in-process: if the trampoline regresses the failure is a
    // dead process rather than a `Result`, and a subprocess is what can tell the two apart.
    let deep = "
def count_to(acc: Int, n: Int) -> Int:
    if n == 0:
        return acc
    return count_to(acc + 1, n - 1)

test \"a tail call, a million deep\":
    expect count_to(0, 1000000) == 1000000
";
    let file = std::env::temp_dir().join("beck-tail-million.beck");
    std::fs::write(&file, deep).expect("a scratch file");
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["test", file.to_str().expect("a path")])
        .output()
        .expect("the compiler is built");
    let _ = std::fs::remove_file(&file);

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("overflowed its stack"),
        "a tail call must not spend host stack, at any depth:\n{err}"
    );
    assert!(
        out.status.success(),
        "a million tail calls is an ordinary program:\n{}{err}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A tail loop, at a depth, as a program `beck test` will run.
#[cfg(target_os = "linux")]
fn tail_loop(n: u64) -> String {
    format!(
        "
def count_to(acc: Int, n: Int) -> Int:
    if n == 0:
        return acc
    return count_to(acc + 1, n - 1)

test \"a tail call, {n} deep\":
    expect count_to(0, {n}) == {n}
"
    )
}

/// The peak resident memory of a `beck test` run, in KiB, sampled while the run is alive.
///
/// `VmHWM` is the kernel's own high-water mark and only ever moves up, so the last reading taken
/// before the child exits is that run's peak. `None` means no reading was taken at all — the run
/// finished inside the first sampling interval — which is a reason to conclude nothing rather than
/// to conclude the memory was flat.
#[cfg(target_os = "linux")]
fn peak_rss_kib(source: &str, scratch: &str) -> Option<u64> {
    let file = std::env::temp_dir().join(scratch);
    std::fs::write(&file, source).expect("a scratch file");
    let mut child = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["test", file.to_str().expect("a path")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the compiler is built");

    let status = format!("/proc/{}/status", child.id());
    let mut peak = None;
    loop {
        if let Ok(text) = std::fs::read_to_string(&status) {
            if let Some(kib) = text
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|kib| kib.parse::<u64>().ok())
            {
                peak = Some(kib);
            }
        }
        // Sampled before the liveness check, so the last successful reading is one taken while the
        // child still had a `/proc` entry.
        if child.try_wait().expect("the child is ours").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let done = child.wait().expect("the child is ours");
    let _ = std::fs::remove_file(&file);
    assert!(
        done.success(),
        "the probe loop must run at all before its memory means anything: {scratch} exited \
         {done} with {peak:?} KiB resident — a tail loop that cannot finish is the same \
         regression this measures, reached by its other end"
    );
    peak
}

/// The heap half of the same property, which the host-stack measurement in `beck-eval` does not
/// reach.
///
/// A tail call that extended the *caller's* environment rather than the closure's would spend no
/// host stack and a linear amount of heap instead — the same unbounded growth wearing a different
/// hat (`docs/27` §27.2). `bind` extends the closure's, so the environment chain is as short on the
/// millionth iteration as on the first, and resident memory does not move. Nothing else asserts
/// that; the stack test above would pass with the chain growing under it.
#[test]
fn a_tail_call_holds_no_heap_either() {
    #[cfg(not(target_os = "linux"))]
    println!(
        "skipping: resident memory is read from /proc, which this target does not have — the \
         heap half of the tail-call property is unmeasured here"
    );

    #[cfg(target_os = "linux")]
    {
        // Ten times apart, because "small" and "constant" are different claims and only the second
        // one is a proper tail call. A chain that grew per level would cost the deep run hundreds
        // of megabytes against the shallow one's.
        let (shallow, deep) = (
            peak_rss_kib(&tail_loop(100_000), "beck-heap-shallow.beck"),
            peak_rss_kib(&tail_loop(1_000_000), "beck-heap-deep.beck"),
        );
        let (Some(shallow), Some(deep)) = (shallow, deep) else {
            println!("skipping: the probe runs finished before resident memory could be sampled");
            return;
        };

        // A window, not an equality: resident memory is what the allocator asked the kernel for
        // rather than what the evaluator holds, and it drifts by a few hundred KiB between runs.
        // The growth this exists to catch is two orders of magnitude larger than the window.
        const WINDOW_KIB: u64 = 4 * 1024;
        println!("tail: {shallow} KiB resident at 100,000 levels, {deep} at 1,000,000");
        assert!(
            deep < shallow + WINDOW_KIB,
            "ten times the tail calls must not cost resident memory: 100,000 levels held \
             {shallow} KiB and 1,000,000 held {deep}, which is growth rather than drift"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Felleisen's table
// ---------------------------------------------------------------------------------------------

const FELLEISEN: &str = include_str!("../../../sicp/felleisen.beck");

/// §25.9's Felleisen deliverable: seven forms, seven verdicts, and the code behind each.
///
/// The table in `docs/63` is only worth reading if the "recovered" rows are recovered *here* rather
/// than in prose, so this runs the file. What it adds beyond a pass is the **shape** of the result:
/// §25.9 forecast one concession out of seven and said why seven out of seven would be a result
/// worth double-checking, so the count is asserted rather than left to a reader of the report.
#[test]
fn felleisens_table_is_seven_forms_with_the_code_behind_each_verdict() {
    let (placed, rendered) = compile_module("sicp/felleisen.beck", FELLEISEN);
    let placed = placed.unwrap_or_else(|| panic!("the Felleisen file compiles:\n{rendered}"));
    assert!(
        !placed.is_application(),
        "it is a library, like the chapters"
    );

    let report = beck_rt::testing::run(&placed, beck_eval::backend(&placed), &Options::default());
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
    assert_eq!(report.skipped(), 0, "nothing here performs an effect");
    assert!(
        report.cases.len() >= 10,
        "every row that claims `recovered` has to have a test making the claim"
    );

    // Seven forms, and exactly one conceded. Both halves matter: a row quietly dropped would make
    // the table shorter than SICP's, and a concession quietly upgraded would make the result the
    // one §25.9 said to distrust.
    for form in [
        "`cond`, `and`, `or`, `not`",
        "`let`, `let*`",
        "`delay` / `force` and `cons-stream`",
        "`quote` and quasiquote",
        "`set!` and `begin`",
        "`amb`",
        "`define-syntax`",
    ] {
        assert!(
            FELLEISEN.contains(form),
            "{form} is one of §25.9's seven rows and the file no longer covers it"
        );
    }
    assert_eq!(
        FELLEISEN.matches("GLOBAL").count(),
        1,
        "§25.9 forecast one concession out of seven; a different number is a result to re-derive \
         rather than a count to update"
    );
}

/// The bound that blocked the most interesting row, asserted where the row is.
///
/// `() -> T` — a thunk, which is what `delay` expands to — parsed and then failed to check, because
/// `ty_from_node_inner` required a function type to have at least two parts and a nullary one has
/// one. `docs/63` §63.3. Nothing in the corpus, the prelude or either SICP chapter had ever written
/// a function type taking no arguments, so the whole language was reachable without it until a
/// macro needed to expand into one.
#[test]
fn a_function_type_may_take_no_arguments() {
    let src = "
def force[T](promise: () -> T) -> T:
    return promise()

def thunk() -> Int:
    return 7

test \"a thunk is a value\":
    expect force(thunk) == 7
    expect force(lambda: \"x\") == \"x\"
";
    let (placed, rendered) = compile_module("thunk.beck", src);
    let placed = placed.unwrap_or_else(|| panic!("a nullary function type checks:\n{rendered}"));
    let report = beck_rt::testing::run(&placed, beck_eval::backend(&placed), &Options::default());
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
}

// ---------------------------------------------------------------------------------------------
// 5. Chapter 3 — the chapter the language disagrees with, and then agrees with
// ---------------------------------------------------------------------------------------------

const CH3: &str = include_str!("../../../sicp/ch3.beck");

/// Chapter 3, run like the other two.
///
/// It is the first chapter whose *register* is mostly "re-expressed" rather than "translated"
/// (`sicp/README.md`, §25.5): §3.1 introduces `set!` and Beck has no assignment, so every object
/// in it becomes a function of the state it hid. The oracles are still the book's — the §3.1.1
/// transcript, the §3.3.4 half-adder's trace at times 8, 11 and 16, and three tables of doubles in
/// §3.5.3 asserted digit for digit, the way `ch1.beck` asserts `3.00009155413138`.
#[test]
fn chapter_three_ends_where_beck_begins() {
    let (placed, rendered) = compile_module("sicp/ch3.beck", CH3);
    let placed = placed.unwrap_or_else(|| panic!("chapter 3 compiles:\n{rendered}"));
    assert!(!placed.is_application(), "chapter 3 is a library");

    let backend = beck_eval::backend(&placed);
    let report = beck_rt::testing::run(&placed, backend, &Options::default());
    assert!(
        report.cases.len() >= 25,
        "chapter 3 is the evidence for docs/87 and has to carry the exercises it claims"
    );
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
    assert_eq!(
        report.skipped(),
        0,
        "nothing in chapter 3 performs an effect"
    );
}

/// §3.5.5 is the chapter's own last word, and it is this project's premise.
///
/// The book rewrites §3.1.1's bank account as a stream and says of it: "this implementation is
/// fully functional ... yet it embodies changing state." `ch3.beck` asserts that the stream and
/// the fold produce the same balances; what this asserts is that the file really does contain both
/// halves, because a test that agreed with itself about one implementation would prove nothing.
#[test]
fn the_chapter_holds_both_halves_of_the_sentence_it_ends_on() {
    assert!(
        CH3.contains("def answers_of(") && CH3.contains("list_fold("),
        "§3.1.1's account has to be a fold"
    );
    assert!(
        CH3.contains("def stream_withdraw(") && CH3.contains("cons_seq(balance"),
        "§3.5.5's account has to be a stream"
    );
    assert!(
        CH3.contains("by_stream(100, [25, 25, 15]) == by_fold(100, [25, 25, 15])"),
        "and the file has to assert that they agree"
    );
}

/// §3.4 — "Concurrency: Time Is of the Essence" — REFUSED, and here is the refusal.
///
/// §3.4.1 draws a timing diagram of two people withdrawing from one account and shows the balance
/// ending up wrong. `ch3.beck` cannot contain that program, because a chapter file has to compile;
/// this is where the thirteen pages land. `docs/80`'s rule is that no child of a `parallel:` may
/// perform an effect another child could observe, and a shared balance in a `durable` fold is
/// exactly that.
#[test]
fn the_interleaving_section_3_4_is_about_is_a_compile_error() {
    let src = "\
def logged(balance: Signal[Int]) -> Signal[Int]:
    return durable(balance)

def peter_and_paul(balance: Signal[Int]) -> Int:
    return parallel:
        withdrawn_by_peter = str_len(str(logged(balance)))
        withdrawn_by_paul = str_len(str(logged(balance)))
        withdrawn_by_peter + withdrawn_by_paul
";
    let rendered = errors("sicp/3-4.beck", src);
    assert!(rendered.contains("B0399"), "{rendered}");

    // And exercise 3.8's question — whether `(+ (f 0) (f 1))` depends on the order of evaluation —
    // cannot even be posed: the two children may not name each other, so there is no `f` with a
    // hidden `balance` for the exercise to be about.
    let names_a_sibling = "\
def order() -> Int:
    return parallel:
        first = 1
        second = first + 1
        first + second
";
    assert!(
        errors("sicp/3-8.beck", names_a_sibling).contains("B0398"),
        "a child that names another is the shape exercise 3.8 needs"
    );
}

/// §3.3.2's own argument for mutating — and the functional answer measured rather than asserted.
///
/// The book does not reach for `set-cdr!` for elegance. It gives a cost: "if we represent a queue
/// as an ordinary list, `insert-queue!` must traverse the list to find the end". `ch3.beck`'s queue
/// is two lists and is Θ(1) *amortised*, and this is that claim as a shape rather than a comment,
/// in the form `scaling.rs` uses: steps per operation must not grow with the number of operations,
/// which needs no clock and therefore cannot flake (`docs/13` §13.7).
///
/// Measured at 53 steps per insert-and-delete pair either side of 500 and 4,000; 70 is that with
/// room to spare, and nowhere near the `n / 2` a scan-to-the-end would need. It was 380 and 2,661
/// before `docs/63` §63.11 — this gate is what found that, and it is the reason it is a gate.
#[test]
fn the_functional_queue_costs_the_same_per_operation_however_long_it_gets() {
    const PER_OPERATION: usize = 70;
    for n in [500usize, 4_000] {
        let program = format!(
            "{CH3}\n\
             def churn(i: Int, n: Int, q: Queue[Str]) -> Int uses raises(QueueError):\n\
             \x20   if i >= n:\n\
             \x20       return list_len(queue_items(q))\n\
             \x20   return churn(i + 1, n, delete_queue(insert_queue(insert_queue(q, \"a\"), \"b\")))\n\
             \n\
             test \"churn\":\n\
             \x20   expect (try: churn(0, {n}, empty_queue_of_str())) == Ok({n})\n"
        );
        let file = std::env::temp_dir().join(format!("beck-sicp-queue-{n}.beck"));
        std::fs::write(&file, program).expect("a scratch file");
        let out = Command::new(env!("CARGO_BIN_EXE_beck"))
            .args([
                "test",
                file.to_str().expect("a path"),
                // The whole chapter is appended so that the queue under measurement is the
                // chapter's own, and `--fuel` is per expectation — so the filter is what keeps the
                // budget about this loop rather than about §3.5.3's tableau.
                "--filter",
                "churn",
                "--fuel",
                &(PER_OPERATION * n).to_string(),
            ])
            .output()
            .expect("the compiler is built");
        let _ = std::fs::remove_file(&file);
        assert!(
            out.status.success(),
            "{n} queue operations needed more than {PER_OPERATION} steps each, which is the shape \
             §3.3.2 reaches for `set-cdr!` to avoid:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
