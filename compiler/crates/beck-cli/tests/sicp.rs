//! The SICP suite's first chapter, and the walls the rest of it hits.
//!
//! [`docs/25-benchmarks-and-expressiveness.md`] proposes SICP as Beck's expressiveness benchmark,
//! on the grounds that the project's own premise ([`docs/01`] §1.1) is that Beck is SICP's three
//! moves made into a language. A proposal is not evidence, so this harness holds the report's
//! claims to the standard `AGENTS.md` sets: every number in §25.6 is produced by something here.
//!
//! Two halves, and the second is the point:
//!
//! * [`sicp/ch1.beck`](../../../sicp/ch1.beck) is the part of chapter 1 that runs today, with the
//!   book's own stated answers as the oracle. It passes.
//! * [`sicp/refusals/`](../../../sicp/refusals/) is one file per wall, each the smallest program
//!   that hits it. This harness asserts each one *still* fails, and with which diagnostic — so
//!   that a wall coming down is a test that starts failing rather than a fact somebody notices.
//!
//! A refusal here is not a bug report against the compiler. Five of the six are features Beck has
//! not built yet and the roadmap names; the sixth (`higher-order.beck`) is a defect, and is
//! labelled as one.

use std::process::Command;

use beck_core::Placed;
use beck_rt::testing::Options;

fn compile(name: &str, src: &str) -> (Option<Placed>, String) {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    (placed, diags.render(&map))
}

/// The same, admitting a library — which is what every SICP exercise is (docs/27 §27.4).
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

/// Chapter 1 needs more stack than `libtest` hands a test thread, and *that is the finding*.
///
/// §25.6 item 5: the evaluator has no proper tail calls and spends host stack per Beck-level call.
/// A debug build spends several times more of it per frame, and `libtest` runs each test on a
/// thread with ~2 MiB rather than the main thread's 8 — so `beck test sicp/ch1.beck` passes from
/// the command line, in both profiles, while the same thirteen tests abort inside the harness.
/// That combination is worth stating plainly: **the suite's own evidence for the missing-tail-call
/// gap is that collecting the evidence trips over it.**
///
/// Raising the stack here is the honest fix rather than shrinking the exercises: `count_change(100)`
/// is 292 because SICP says it is 292, and trimming the book's own answer to fit an interpreter's
/// frame size would be measuring the harness instead of the language. When tail calls land, this
/// wrapper goes away and the `RUST_MIN_STACK`-shaped workaround goes with it.
const CH1_STACK: usize = 32 * 1024 * 1024;

#[test]
fn chapter_one_passes_against_the_books_own_answers() {
    std::thread::Builder::new()
        .stack_size(CH1_STACK)
        .spawn(|| {
            let (placed, rendered) = compile_module("sicp/ch1.beck", CH1);
            let placed = placed.unwrap_or_else(|| panic!("chapter 1 compiles:\n{rendered}"));
            assert!(
                !placed.is_application(),
                "chapter 1 is a library, and running it as one is the point"
            );

            let backend = beck_eval::backend(&placed);
            let report = beck_rt::testing::run(&placed, backend, &Options::default());

            assert!(
                report.cases.len() >= 13,
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
        })
        .expect("a thread")
        .join()
        .expect("chapter 1 runs without exhausting a 32 MiB stack");
}

#[test]
fn chapter_two_reaches_the_closure_property() {
    // §2.2 is "Hierarchical Data and the Closure Property", and it is the section §25.6 item 2 said
    // chapter 2 stopped at. It does not stop there now, and this file is a library — so it is
    // evidence for §27.3 and §27.4 at once, and could not have been written for either alone.
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
    // wrapper in `ch1.beck` can be deleted". docs/27 §27.4 deleted it.
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
    // item on the list for reasons that have nothing to do with SICP. docs/27 §27.3 is the fix.
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
    // §27.3 resolves declarations in three passes where there was one, and "three" is a count of
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
    // The honest limit, so that §27.3's "nothing bounds depth" is not read as "nothing at all".
    // A recursive *type* is unbounded; a recursive *value* is bounded by the host stack the
    // evaluator spends per Beck-level call, which is §25.6 item 5 — wall 4, still standing.
    //
    // Run through the binary because the far end of it is a `SIGABRT` rather than a `Result`, which
    // is the same finding `a_tail_call_consumes_stack_…` records for a different program shape.
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

    // 100 rather than a rounder number, because the depth that fits is a property of the *profile*
    // and not of the language: a debug build spends several times more host stack per Beck-level
    // call than a release one, and this harness runs whichever `cargo test` was asked for. The
    // release binary carries 500 comfortably. That the number moves with the build is itself wall
    // 4 — a language with proper tail calls would not have one.
    let shallow = run(&program(100), "beck-spine-shallow.beck");
    assert!(
        shallow.status.success(),
        "a tree 100 deep is an ordinary value:\n{}{}",
        String::from_utf8_lossy(&shallow.stdout),
        String::from_utf8_lossy(&shallow.stderr)
    );

    let deep = run(&program(50_000), "beck-spine-deep.beck");
    assert!(
        !deep.status.success(),
        "a tree 50,000 deep is expected to exhaust the host stack; if this passes, Beck grew proper \
         tail calls and wall 4 is down (happily)"
    );
    assert!(
        String::from_utf8_lossy(&deep.stderr).contains("overflowed its stack"),
        "and to die by the evaluator's stack rather than by anything the checker did"
    );
}

#[test]
fn an_alias_that_is_defined_in_terms_of_itself_is_still_refused() {
    // The one cycle that is not a feature. A `union` may be recursive because a variant is a finite
    // tag plus fields; an alias is *transparent*, so `type A = list[A]` describes an infinitely
    // large type and `type A = B; type B = A` describes none at all. Refusing them is what stops
    // the fix of §27.3 turning a source-order limitation into a hang.
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

#[test]
fn a_user_cannot_write_a_polymorphic_definition_so_map_cannot_be_built() {
    let out = errors(
        "generic.beck",
        include_str!("../../../sicp/refusals/generic.beck"),
    );
    assert!(out.contains("B0120"), "{out}");
}

#[test]
fn there_is_no_real_arithmetic_so_newtons_method_does_not_typecheck() {
    let out = errors(
        "real.beck",
        include_str!("../../../sicp/refusals/real.beck"),
    );
    assert!(out.contains("B0320"), "{out}");
    assert!(
        out.contains("expected `Int`, found `Float`"),
        "the wall is that `+` is Int-only, not that `Float` is unknown:\n{out}"
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
fn a_tail_call_consumes_stack_so_an_iterative_process_is_not_iterative() {
    // §1.2.1's distinction, which Beck cannot currently make. This one is run through the binary
    // rather than in-process because the failure is a `SIGABRT`, not a `Result` — which is itself
    // half of the finding.
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sicp/refusals/tail.beck");
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(["test", file])
        .output()
        .expect("the compiler is built");

    assert!(
        !out.status.success(),
        "a tail call eight thousand deep is expected to die; if this passes, Beck grew proper \
         tail calls and §25.6 needs rewriting (happily)"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("overflowed its stack"),
        "and to die by exhausting the host stack specifically:\n{err}"
    );
}
