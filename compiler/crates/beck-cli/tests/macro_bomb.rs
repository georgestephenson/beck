//! Expansion is bounded by what it **produces**, and this is the gate in both directions.
//!
//! [`docs/14`](../../../../docs/14-review-findings.md) F17 asked for it and
//! [`docs/42`](../../../../docs/42-security-assurance.md) §42.2 is why it matters here rather than
//! in the abstract: the playground compiles **a stranger's source in a browser tab**
//! ([`docs/98`](../../../../docs/98-playground-report.md)), so every limit the front end has is a
//! limit on what a visitor can do to the tab. Two of them existed —
//! [`adr/0012`](../../../../docs/adr/0012-the-front-end-counts-its-own-recursion.md)'s structural
//! nesting count and the expander's own re-expansion depth — and both bound how *deep* expansion
//! goes. Neither bounded how much it makes.
//!
//! That is [`docs/82`](../../../../docs/82-the-edge-report.md) §82.10's pattern for
//! the fourth time: *a limit added at the one production somebody thought of is bypassed through a
//! different one*. A macro that doubles its output at each of a few levels is shallow, terminates,
//! and is enormous.
//!
//! # Why the sizes here are what they are
//!
//! [`docs/82`](../../../../docs/82-the-edge-report.md) §82.7 is unflattering about a
//! generator calibrated against the limits you built rather than against the failures you had, so
//! these are calibrated against the *failure*: `refuses` doubles until the budget is gone, and
//! `allows` is the whole repository, which is the only honest statement of what a legitimate program
//! expands to.

use std::path::PathBuf;

/// The doubling macro, `n` deep. Six lines of source, `2^n` copies of the leaf.
fn bomb(n: usize) -> String {
    // `x + x` rather than a call, so that the program is *clean* as well as enormous: a macro that
    // names a top-level function trips hygiene here, and a fixture with an unrelated error in it
    // could not be used for the other half of this gate.
    let mut src = String::from("macro pair(x):\n    return quote:\n        ($x + $x)\n\n");
    src.push_str("def go() -> Int:\n    return ");
    let mut expr = String::from("1");
    for _ in 0..n {
        expr = format!("pair({expr})");
    }
    src.push_str(&expr);
    src.push('\n');
    src
}

/// The diagnostics a program produces, as codes.
///
/// `compile_or_library_str` rather than `compile_str`, because these fixtures are one definition
/// and no merge point — a library, which is what `B0500` says and is not what is being tested.
fn codes(name: &str, src: &str) -> Vec<String> {
    let (_, diags, _) = beck_core::compile_or_library_str(name, src);
    diags.iter().map(|d| d.code.to_string()).collect()
}

/// A macro that doubles its output is refused — **and the message is the one that says why**.
///
/// Twenty-four nestings: sixteen million nodes if nothing counted, and it is refused after a hundred
/// thousand of them, because the accounting stops when the budget does. A test that proved the point
/// by exhausting memory would not be a test.
#[test]
fn a_doubling_macro_is_refused() {
    let codes = codes("bomb.beck", &bomb(24));
    assert!(
        codes.iter().any(|c| c == "B0214"),
        "a doubling macro 24 deep should be refused by the expansion budget, and the diagnostics \
         were {codes:?}"
    );
    // Not the depth counters: both are satisfied by this program, which is the whole point of the
    // budget existing separately from them.
    assert!(
        !codes.iter().any(|c| c == "B0201" || c == "B0213"),
        "the budget should be what refuses this, not a depth limit: {codes:?}"
    );
    // Once, not once per call that would have expanded afterwards.
    assert_eq!(
        codes.iter().filter(|c| *c == "B0214").count(),
        1,
        "the budget should report once: {codes:?}"
    );
}

/// The typed expander has its own budget, and it is bounded the same way.
///
/// A second expander is a second place the F17 hole can open, and it would open **silently**: the
/// budget the untyped pass spends is not the one the checker's pass spends, so a gate written only
/// against `macro` would stay green while `typed macro` had no ceiling at all. The doubling macro
/// is written twice here for that reason — one word apart, one refusal each.
///
/// It went red the first time it was run, and on the worse half of the hole rather than the
/// obvious one. The budget *was* being charged; the checker infers a call's arguments inside a
/// rollback, the argument here is itself a doubling call, so the refusal was reported **inside the
/// probe and thrown away with it** — after which every expansion produced nothing, the definition
/// checked as `unit`, and `beck check` said the program was fine. A budget is spent once and
/// reported once, so a discarded report is the only one there will ever be.
#[test]
fn a_doubling_typed_macro_is_refused_by_its_own_budget() {
    let typed = bomb(24).replacen("macro pair", "typed macro pair", 1);
    let refused = codes("typed-bomb.beck", &typed);
    assert!(
        refused.iter().any(|c| c == "B0214"),
        "a doubling typed macro should be refused by the expansion budget: {refused:?}"
    );
    // Refused, not silently emptied: the shape this went red on checked clean.
    assert!(
        beck_core::compile_or_library_str("typed-bomb.beck", &typed)
            .1
            .has_errors(),
        "the program has to be refused, not checked as `unit`"
    );
    // And the person's depth still compiles through the checker's expander too, which is what makes
    // the budget a judgement on both sides rather than a wall on one.
    let small = bomb(8).replacen("macro pair", "typed macro pair", 1);
    assert!(
        codes("typed-small.beck", &small).is_empty(),
        "eight nestings through the typed expander should compile"
    );
}

/// A typed macro nested `d` deep, each expansion producing `width` nodes of its own.
///
/// `list_sum` of a flat literal rather than a chain of `+`: the production has to be *wide*, or the
/// front end's structural nesting count ([`adr/0012`](../../../../docs/adr/0012-the-front-end-counts-its-own-recursion.md))
/// refuses the fixture before the budget ever sees it. The macro is the identity on its argument —
/// `list_sum([x, 0, 0, …])` is `x` — so nesting it means what it looks like it means, and the
/// program's *real* expansion is `d` copies of one output rather than anything that compounds.
fn nested(d: usize, width: usize) -> String {
    let zeros = vec!["0"; width].join(", ");
    let mut src = format!(
        "\
typed macro grow(x):
    t = node_ty(x)
    if t.name != \"Int\":
        refuse(\"only Int\")
    return quote:
        list_sum([$x, {zeros}])

def f() -> Int:
    return "
    );
    let mut expr = String::from("1");
    for _ in 0..d {
        expr = format!("grow({expr})");
    }
    src.push_str(&expr);
    src.push('\n');
    src
}

/// **What a nested typed macro is charged for does not grow with the nesting.**
///
/// A *shape* rather than a threshold, which is [`compile_speed.rs`](compile_speed.rs)'s form and
/// [`docs/64`](../../../../docs/64-compile-speed-report.md)'s pattern: hold the production per
/// nesting level constant, grow the level, and what the module is charged has to stay flat. A
/// charge that grew with the level refuses the deeper members of the sweep, and that is exactly
/// what it did — 8,000 nodes per level was accepted two deep and refused at four and at six,
/// because a call whose argument is another typed-macro call was expanded **twice**: once in the
/// probe that infers the argument and once inside what the enclosing macro wrote. The probe's
/// output is discarded, so `2^d - d` of those expansions were charges for code the program does
/// not contain.
///
/// The budget is defined as a bound on what expansion *produces*
/// ([`docs/42`](../../../../docs/42-security-assurance.md) §42.6), so this is a correctness gate
/// and not a speed one: 48,000 nodes of real production is comfortably inside a 100,000-node
/// budget, and a program refused for producing forty-eight thousand nodes when it produced
/// forty-eight thousand nodes is a compiler telling the truth. One refused for producing
/// half a million is not.
#[test]
fn what_a_nested_typed_macro_is_charged_for_does_not_grow_with_the_nesting() {
    // 8,000 a level: two levels is 16,000 and six is 48,000, both well inside the budget, while
    // the same sweep charged per *expansion* reaches 512,000 by six.
    for depth in [2, 4, 6] {
        let all = codes(&format!("nested-{depth}.beck"), &nested(depth, 8_000));
        assert!(
            all.is_empty(),
            "8,000 nodes a level, {depth} deep, is {} nodes of production against a \
             {}-node budget and has to compile: {all:?}",
            depth * 8_000,
            beck_macro::MAX_EXPANSION,
        );
    }
}

/// The fifteen-deep fixture the defect was reported with, compiling.
///
/// Three nodes a level, forty-five nodes of total expansion, and it was refused with "macro
/// expansion produced too much … the budget is 100000 nodes for the whole module" — followed by a
/// second diagnostic that made it worse, because once the budget is spent every later expansion
/// produces nothing, so the macro's own `refuse` fired on a type it could no longer see and the
/// program was told it had two problems it did not have.
#[test]
fn the_shape_the_defect_was_reported_with_compiles() {
    let src = nested(15, 1);
    let all = codes("nested-15.beck", &src);
    assert!(
        all.is_empty(),
        "fifteen levels of a macro that produces a handful of nodes is not a bomb: {all:?}"
    );
}

/// **And the hole this must not open**: a typed macro that genuinely doubles is still refused.
///
/// [`macro_bomb.rs`'s own doubling gate](fn.a_doubling_typed_macro_is_refused_by_its_own_budget.html)
/// is the statement of record and is deliberately left alone; this is the same claim standing
/// beside the sweep above, because the two are one decision. The charge that the sweep removes is
/// the *probe's*, whose output is thrown away — not the charge for output that is really there. A
/// macro writing `[$x, $x]` really does produce `2^d` nodes, so `2^d` is what it owes, and a fix
/// that memoised the charge per call site rather than per use would pass the sweep and hand back
/// the hole [`docs/102`](../../../../docs/102-the-macro-interpreter-report.md) §102.9 found the
/// first version of this expander opening.
#[test]
fn a_doubling_typed_macro_is_still_refused_beside_the_sweep() {
    let typed = bomb(24).replacen("macro pair", "typed macro pair", 1);
    let refused = codes("still-a-bomb.beck", &typed);
    assert!(
        refused.iter().any(|c| c == "B0214"),
        "output that is really produced is still charged for: {refused:?}"
    );
}

/// **An argument nobody gets is not charged for — and is not a way to spend the compile either.**
///
/// The case that decides whether a probe may stop charging, and the reason the answer is not
/// simply "the probe's output is discarded, so charge nothing". A typed macro that *throws its
/// argument away* expands that argument in the probe and nowhere else, so nothing it produced is
/// in the program and nothing is owed — but the walk of it was, until this was fixed, the
/// `2^d` the budget was cutting short. Free and exponential is the one combination F17 exists to
/// prevent ([`docs/42`](../../../../docs/42-security-assurance.md) §42.2: the playground compiles
/// a stranger's source in a browser tab).
///
/// Both halves, because either alone is satisfied by the wrong fix: the program **compiles**,
/// since a doubling macro inside a discarded argument produces nothing a doubling macro inside a
/// *kept* one would, and it compiles **quickly**, because the expansion is remembered rather than
/// walked once per place the checker passes it. A budget is not a schedule; what bounds this is
/// that the work is linear, not that the meter runs out.
#[test]
fn a_bomb_in_an_argument_a_macro_discards_is_neither_charged_nor_walked() {
    let src = "\
typed macro pair(x):
    return quote:
        ($x + $x)

typed macro drop_it(x):
    return quote:
        0

def go() -> Int:
    return drop_it("
        .to_string();
    let mut expr = String::from("1");
    for _ in 0..24 {
        expr = format!("pair({expr})");
    }
    let src = format!("{src}{expr})\n");

    let started = std::time::Instant::now();
    let all = codes("discarded.beck", &src);
    let took = started.elapsed();
    assert!(
        all.is_empty(),
        "a discarded argument produces nothing, so there is nothing to refuse: {all:?}"
    );
    // Twenty-four levels: `2^24` walks of the argument if the memo is not doing its job, which is
    // minutes rather than the millisecond this takes. Generous by three orders of magnitude,
    // because a gate that flakes gets deleted (`docs/13` §13.7) and the failure it is written
    // against is not a slow machine.
    assert!(
        took < std::time::Duration::from_secs(20),
        "a discarded argument was walked once per place it does not appear: {took:?}"
    );
}

/// **A typed literal's parser is a macro body, and is bounded like one.**
///
/// This is the half of F17 that `docs/43` §43.4 recorded as having "nothing to bound yet": the
/// finding named a *typed-literal parser's own work* as a second, separate thing to bound, and
/// when typed literals arrived it turned out there was no second thing — `date"…"` desugars to a
/// macro call (`docs/02` §2.5), so the parser inside it is spending the same budget every other
/// macro body spends. Written as a gate rather than as an argument, because "it is the same
/// mechanism" is exactly the kind of claim that stops being true when the mechanism is changed:
/// red the day a sigil is expanded anywhere but through `apply_macro`.
#[test]
fn a_typed_literals_parser_spends_the_same_budget_every_macro_body_does() {
    let forever = "\
macro spin_sigil(raw):
    n = 0
    while true:
        n = n + 1
    return quote:
        $n

def f() -> Int:
    return spin\"x\"
";
    let all = codes("sigil-spin.beck", forever);
    assert!(
        all.contains(&"B0215".to_string()),
        "a body that does not terminate is refused wherever it was called from: {all:?}"
    );

    let doubling = "\
macro two(x):
    return quote:
        [$x, $x]

macro wide_sigil(raw):
    return quote:
        two(two(two(two(two(two(two(two(two(two(two(two(two(two(two(two(two(two(two(two(
            two(two(two(two(1))))))))))))))))))))))))

def f() -> Int:
    return wide\"x\"
";
    let all = codes("sigil-wide.beck", doubling);
    assert!(
        all.contains(&"B0214".to_string()),
        "and so is one that produces too much: {all:?}"
    );
}

/// …and the same macro at a depth a person would write compiles.
///
/// The other direction, and the one that makes the number a *judgement* rather than a wall: eight
/// nestings is 256 copies of the leaf, which is more than any program in this repository generates
/// and is nowhere near the budget.
#[test]
fn a_macro_a_person_would_write_still_compiles() {
    let codes = codes("small.beck", &bomb(8));
    assert!(
        codes.is_empty(),
        "eight nestings is 256 copies and should compile: {codes:?}"
    );
}

/// Every program in the tree still expands, which is the control the assertion above needs.
///
/// A budget that refused a bomb by refusing everything would pass the test above. This is the
/// statement that it does not, over the corpus, both benchmark suites, both SICP chapters, the
/// examples and the standard library — and it is also where the number came from: the largest total
/// expansion any of them performs is 138 nodes.
#[test]
fn every_program_in_the_tree_still_expands() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .to_path_buf();
    beck_diag::depth::on_the_front_end_stack(|| check(&root));
}

/// The walk, on the stack the front end declares — the printer and the checker both recurse over a
/// program's shape, and every entry point in this workspace dispatches onto it.
fn check(root: &std::path::Path) {
    let mut seen = 0;
    for dir in ["corpus", "awfy", "clbg", "sicp", "examples", "lib"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "beck") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable");
            let name = path.display().to_string();
            let (_, diags, map) = beck_core::compile_or_library_str(&name, &src);
            assert!(
                !diags.iter().any(|d| d.code == "B0214"),
                "{name} hit the expansion budget:\n{}",
                diags.render(&map)
            );
            seen += 1;
        }
    }
    assert!(seen > 40, "only {seen} programs were checked");
}
