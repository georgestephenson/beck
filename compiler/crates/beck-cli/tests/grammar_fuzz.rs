//! Grammar-aware fuzzing of the front end: generate *structure*, and assert the front end answers.
//!
//! `docs/42` §42.9 pins this with a trigger — "grammar-aware fuzzing as the method that finds the
//! rest of this class (trigger: **the bound lands**)" — and the bound landed in `docs/44`. §42.11's
//! row asks for "a structure-aware generator over the corpus".
//!
//! # Why byte mutation was not enough, in that report's own words
//!
//! §42.1 ran 600 iterations of byte-level mutation over `compiler/corpus/*.beck` and found nothing,
//! and said why that was not reassuring:
//!
//! > random mutation cannot *generate structure*, so the one crash class the front end actually has
//! > is precisely the one this method is blind to.
//!
//! A mutated file is a *slightly wrong* file. The crash class is a *deeply nested* or *very long*
//! one, and no amount of flipping bytes in a 40-line corpus program produces 3,000 nested
//! parentheses. So this generator does not mutate: it **builds** programs from the grammar, with
//! the recursive productions parameterised by depth and the sequential ones by length, and the sizes
//! deliberately span the ceilings.
//!
//! # What is asserted
//!
//! One property, and it is the only one worth asserting about arbitrary input:
//!
//! > **The front end answers.** For every generated program, `beck check` either accepts it or
//! > produces diagnostics — it never aborts, never panics, and never fails to terminate.
//!
//! Not "it compiles": most of these are nonsense, and a generator that only produced valid programs
//! would be testing the wrong half. What is being ruled out is the failure that has no diagnostic.
//!
//! # `proptest` rather than `cargo-fuzz`
//!
//! §42.11's row names `cargo-fuzz`, which needs libFuzzer and therefore nightly; this workspace pins
//! stable 1.94.1 (`rust-toolchain.toml`), and taking a nightly toolchain for one harness is a larger
//! decision than this test. `proptest` is already a dev-dependency, already used by
//! `manifest_properties.rs`, shrinks failures to a minimal case, and — the part that matters — the
//! *generator* is the contribution here rather than the driver. Coverage-guided feedback is what
//! `cargo-fuzz` would add over this, and `docs/85` §85.6 records it as owed.

use proptest::prelude::*;

/// The recursive productions, each parameterised by how deep to go.
///
/// Every one of these is a place `beck_diag::depth::MAX_NESTING` has to be counted, and the Scriban
/// lesson (§42.2) is that bounding one is not bounding the others — so the generator's job is to
/// reach each of them independently.
#[derive(Debug, Clone, Copy)]
enum Shape {
    Parens,
    Calls,
    Lists,
    Records,
    Types,
    Blocks,
    Matches,
    /// The flat axis: sequential bindings, which nest not at all and recurse once each.
    FlatBlock,
    /// A chain of binary operators — flat in source, a tree in `Core`.
    Operators,
    /// Nested `ui:` blocks, which are a macro and expand into more structure than was written.
    Ui,
}

fn program(shape: Shape, n: usize) -> String {
    match shape {
        Shape::Parens => format!(
            "def f() -> Int:\n    return {}1{}\n",
            "(".repeat(n),
            ")".repeat(n)
        ),
        Shape::Calls => format!(
            "def g(x: Int) -> Int:\n    return x\n\ndef f() -> Int:\n    return {}1{}\n",
            "g(".repeat(n),
            ")".repeat(n)
        ),
        Shape::Lists => format!(
            "def f() -> Int:\n    xs = {}1{}\n    return 0\n",
            "[".repeat(n),
            "]".repeat(n)
        ),
        Shape::Records => {
            let mut src = String::from("model M:\n    v: Int\n\ndef f() -> Int:\n    return ");
            for _ in 0..n {
                src.push_str("M(v=");
            }
            src.push('1');
            src.push_str(&")".repeat(n));
            src.push_str(".v\n");
            src
        }
        Shape::Types => format!(
            "def f(x: {}Int{}) -> Int:\n    return 0\n",
            "list[".repeat(n),
            "]".repeat(n)
        ),
        Shape::Blocks => {
            let mut src = String::from("def f(x: Int) -> Int:\n");
            for i in 0..n {
                src.push_str(&"    ".repeat(i + 1));
                src.push_str("if x > 0:\n");
            }
            src.push_str(&"    ".repeat(n + 1));
            src.push_str("return 1\n");
            src.push_str("    return 0\n");
            src
        }
        Shape::Matches => {
            let mut src = String::from("def f(x: Int) -> Int:\n");
            for i in 0..n {
                src.push_str(&"    ".repeat(i + 1));
                src.push_str("match x:\n");
                src.push_str(&"    ".repeat(i + 2));
                src.push_str("case _:\n");
            }
            src.push_str(&"    ".repeat(2 * n + 1));
            src.push_str("0\n");
            src
        }
        Shape::FlatBlock => {
            let mut src = String::from("def f() -> Int:\n    v0 = 1\n");
            for i in 1..n.max(1) {
                src.push_str(&format!("    v{i} = v{} + 1\n", i - 1));
            }
            src.push_str(&format!("    return v{}\n", n.max(1) - 1));
            src
        }
        Shape::Operators => format!("def f() -> Int:\n    return 1{}\n", " + 1".repeat(n.max(1))),
        Shape::Ui => {
            let mut src = String::from("def f() -> Html:\n    return ui:\n");
            for i in 0..n {
                src.push_str(&"    ".repeat(i + 2));
                src.push_str("div:\n");
            }
            src.push_str(&"    ".repeat(n + 2));
            src.push_str("\"x\"\n");
            src
        }
    }
}

/// Check one generated program, and say whether the front end answered.
///
/// Runs on the declared front-end stack, because the thing under test recurses over the structure
/// the generator chose and this is a harness rather than the CLI.
fn front_end_answers(src: &str) -> bool {
    beck_diag::depth::on_the_front_end_stack(|| {
        let (_, d, _) = beck_core::compile_or_library_str("fuzz.beck", src);
        // Accepted, or refused with something a reader can act on. What is being ruled out is a
        // third outcome — an abort, which would take the process and never reach this line.
        !d.has_errors() || d.iter().count() > 0
    })
}

/// What "deep" costs in *source bytes*, which is not the same for every shape.
///
/// The indentation-based shapes write one line per level and indent it, so a file `n` levels deep
/// is `O(n²)` bytes: 20,000 levels is 1.6 GB of spaces. Their ceiling is `MAX_NESTING` anyway, so
/// there is nothing past a few hundred that is not a test of string building. The rest are linear
/// and are generated at full size, because that is where the failures live.
fn size_for(shape: Shape, raw: usize) -> usize {
    match shape {
        Shape::Blocks | Shape::Matches | Shape::Ui => raw % 400,
        _ => raw,
    }
}

fn shapes() -> impl Strategy<Value = Shape> {
    prop_oneof![
        Just(Shape::Parens),
        Just(Shape::Calls),
        Just(Shape::Lists),
        Just(Shape::Records),
        Just(Shape::Types),
        Just(Shape::Blocks),
        Just(Shape::Matches),
        Just(Shape::FlatBlock),
        Just(Shape::Operators),
        Just(Shape::Ui),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        // A bounded budget, per §42.11's own "a bounded budget per pull request". The generator is
        // cheap and the check is not, so the count is chosen to keep this suite in seconds.
        cases: 64,
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::WithSource("regressions")
        )),
        ..ProptestConfig::default()
    })]

    /// Sizes that **span** every ceiling and reach past where the stack used to give out.
    ///
    /// `MAX_NESTING` is 256 and `MAX_BLOCK` is 2,048; the aborts the counters replaced were at
    /// 3,785 nested parens (`docs/42` §42.2), 12,000 flat bindings (`docs/64` §64.4) and 300,000
    /// chained operators (`docs/85`), all in a debug build. The range has to cover the second
    /// numbers, not the first — `size_for` is what keeps that affordable.
    #[test]
    fn the_front_end_answers_for_any_shape_at_any_size(shape in shapes(), raw in 0usize..40_000) {
        let n = size_for(shape, raw);
        let src = program(shape, n);
        prop_assert!(front_end_answers(&src), "no diagnostic and no acceptance:\n{}", &src[..src.len().min(200)]);
    }
}

/// The sizes that matter, tried exhaustively rather than sampled.
///
/// A property test picks `n` at random and may never land on 256 or 2,048. The failures in this
/// class live *at* the ceiling — one past it, exactly on it — so those are enumerated.
#[test]
fn every_shape_answers_on_both_sides_of_every_ceiling() {
    let nesting = beck_diag::depth::MAX_NESTING as usize;
    let block = beck_diag::depth::MAX_BLOCK as usize;
    // The last two are the point. A ceiling is cheap to test just past — the counter stops
    // immediately — but the failure this harness exists to catch is the one where *no* counter
    // stops it and the **stack** does. `docs/64` §64.4 measured that at 12,000 flat bindings in a
    // debug build and 100,000 in a release one, and `docs/42` §42.2 at 3,785 nested parens in a
    // debug build. A generator whose sizes stop short of those numbers cannot find either, which
    // is the mistake the first version of this file made: it went to 3,000.
    let sizes = [
        0,
        1,
        2,
        nesting - 1,
        nesting,
        nesting + 1,
        nesting * 4,
        block - 1,
        block,
        block + 1,
        20_000,
        120_000,
    ];
    for shape in [
        Shape::Parens,
        Shape::Calls,
        Shape::Lists,
        Shape::Records,
        Shape::Types,
        Shape::Blocks,
        Shape::Matches,
        Shape::FlatBlock,
        Shape::Operators,
        Shape::Ui,
    ] {
        for n in sizes {
            // The indentation-based shapes make a line per level *and* indent it, so a deep one is
            // quadratic in source bytes; past a few hundred levels that is a file nobody would
            // write and a test that measures string building.
            let deep_indent = matches!(shape, Shape::Blocks | Shape::Matches | Shape::Ui);
            if deep_indent && n > nesting + 1 {
                continue;
            }
            assert!(
                front_end_answers(&program(shape, n)),
                "{shape:?} at {n}: the front end neither accepted nor diagnosed"
            );
        }
    }
}

/// The refusals are the *counted* ones, at both ceilings, for every shape that reaches them.
///
/// "The front end answers" is satisfied by any diagnostic, including an accidental one — a parse
/// error from a file so large the lexer gave up would pass the property above and mean nothing. So
/// this asserts the answer is the ceiling's own diagnostic: `B0121` for nesting, `B0389` for a flat
/// block. It is what turns "it did not crash" into "it refused for the stated reason".
#[test]
fn past_a_ceiling_the_refusal_is_the_ceilings_own_diagnostic() {
    let over = beck_diag::depth::MAX_NESTING as usize * 4;
    for shape in [Shape::Parens, Shape::Calls, Shape::Lists, Shape::Types] {
        let codes = beck_diag::depth::on_the_front_end_stack(|| {
            let (_, d, _) = beck_core::compile_or_library_str("fuzz.beck", &program(shape, over));
            d.iter().map(|x| x.code.to_string()).collect::<Vec<_>>()
        });
        assert!(
            codes.iter().any(|c| c == "B0121" || c == "B0390"),
            "{shape:?} at {over} deep should meet a counted ceiling, got {codes:?}"
        );
    }

    let long = beck_diag::depth::MAX_BLOCK as usize + 16;
    let codes = beck_diag::depth::on_the_front_end_stack(|| {
        let (_, d, _) =
            beck_core::compile_or_library_str("fuzz.beck", &program(Shape::FlatBlock, long));
        d.iter().map(|x| x.code.to_string()).collect::<Vec<_>>()
    });
    assert!(
        codes.iter().any(|c| c == "B0389"),
        "a flat block past MAX_BLOCK should meet its own ceiling, got {codes:?}"
    );
}
