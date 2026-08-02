//! Failure as a row label, and `Result` as what a handler produces.
//!
//! [`docs/38-literature-survey.md`](../../../../docs/38-literature-survey.md) §38.4 adopts the
//! shape rather than a mechanism: "an error is a row label, a signature without it provably cannot
//! fail, and a handler converts the row entry into a value — `Result` is the *reified* form a
//! handler produces, not a parallel mechanism". Everything below is one of those clauses, tested.
//!
//! The claim that matters is the second one, and it is the reason this is an effect rather than a
//! calling convention: **a signature says whether the thing can fail, whether or not its author
//! thought about it.** Rows are inferred, so the label arrives by itself; `uses` makes it a bound;
//! `.becki` publishes it; and `--wire-compat` classifies a change to it. Failure inherits the whole
//! discipline the effect system already had, which is what "do not add a mechanism" buys.

use beck_core::{check_str, compile_str, Effect};

const REFUSAL: &str = "\
union Refusal:
    Blank
    TooLong
";

fn codes(src: &str) -> Vec<&'static str> {
    let (_, d, _) = check_str("t.beck", src);
    d.iter().map(|x| x.code).collect()
}

/// Checked, not placed. Every fragment below is a *library* — a few definitions with no merge
/// point — because failure is a property of a definition and wrapping each one in an application
/// would say nothing extra. `corpus/29-fallible.beck` is the whole-program version, and it runs in
/// the corpus harness with the rest.
fn checks(src: &str) -> beck_core::Program {
    let (program, d, map) = check_str("t.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&map));
    program
}

/// The row inferred for one definition, as printed atom names.
fn row_of(src: &str, name: &str) -> Vec<String> {
    checks(src)
        .defs
        .get(name)
        .unwrap_or_else(|| panic!("no `{name}`"))
        .effects
        .iter()
        .map(|e| e.name())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// 1. "An error is a row label"
// ---------------------------------------------------------------------------------------------

#[test]
fn a_raise_puts_the_errors_type_in_the_row() {
    let src = format!(
        "{REFUSAL}
def clean(text: Str) -> Str:
    if str_is_empty(text):
        raise Blank
    return text
"
    );
    assert_eq!(row_of(&src, "clean"), vec!["raises(Refusal)"]);
}

/// Inferred, and therefore inherited: a caller of a fallible function is fallible, and nobody had
/// to write that down. This is the property a `Result`-returning convention cannot have, because a
/// caller can always ignore a returned value.
#[test]
fn a_caller_of_something_fallible_is_fallible() {
    let src = format!(
        "{REFUSAL}
def clean(text: Str) -> Str:
    if str_is_empty(text):
        raise Blank
    return text

def shout(text: Str) -> Str:
    return str_trim(clean(text))
"
    );
    assert_eq!(row_of(&src, "shout"), vec!["raises(Refusal)"]);
}

/// And a signature *without* the label is a promise, enforced. `uses` is the published bound
/// (B0370), and failure is bound by it exactly as `durable` and `net.out` are.
#[test]
fn a_signature_that_does_not_declare_failure_may_not_fail() {
    let src = format!(
        "{REFUSAL}
def clean(text: Str) -> Str uses raises(Refusal):
    if str_is_empty(text):
        raise Blank
    return text

def promises_not_to(text: Str) -> Str uses log:
    return clean(text)
"
    );
    assert!(codes(&src).contains(&"B0370"), "{:?}", codes(&src));
}

/// The two error types are two labels, not one `error` effect. A handler for one is not a handler
/// for the other, and a row that carries both says so.
#[test]
fn two_error_types_are_two_labels() {
    let src = "\
union Parse:
    NotANumber

union Budget:
    Over

def both(text: Str) -> Int:
    if str_is_empty(text):
        raise NotANumber
    if str_is_empty(str_trim(text)):
        raise Over
    return 1
";
    let mut row = row_of(src, "both");
    row.sort();
    assert_eq!(row, vec!["raises(Budget)", "raises(Parse)"]);
}

#[test]
fn a_raised_value_must_have_a_declared_type() {
    let src = "\
def f(text: Str) -> Str:
    if str_is_empty(text):
        raise 4
    return text
";
    assert!(codes(src).contains(&"B0391"), "{:?}", codes(src));
}

// ---------------------------------------------------------------------------------------------
// 2. "`Result` is the reified form a handler produces"
// ---------------------------------------------------------------------------------------------

#[test]
fn a_try_discharges_the_label_and_yields_a_result() {
    let src = format!(
        "{REFUSAL}
def clean(text: Str) -> Str uses raises(Refusal):
    if str_is_empty(text):
        raise Blank
    return text

def checked(text: Str) -> Result[Str, Refusal]:
    return try: clean(text)
"
    );
    assert!(
        row_of(&src, "checked").is_empty(),
        "a handled failure is not a failure: {:?}",
        row_of(&src, "checked")
    );
}

/// A handler catches failure and launders nothing else. `durable` performed inside a `try:` is
/// still performed by the enclosing definition — which is also what keeps placement right, since
/// the tier is decided from that row.
#[test]
fn a_handler_catches_failure_and_nothing_else() {
    let src = format!(
        "{REFUSAL}
def clean(text: Str) -> Str uses raises(Refusal):
    if str_is_empty(text):
        raise Blank
    return text

def checked(text: Str) -> Result[Str, Refusal]:
    return try:
        clean(str(now()))
"
    );
    assert_eq!(row_of(&src, "checked"), vec!["nondet"]);
}

#[test]
fn a_try_over_something_that_cannot_fail_is_a_diagnostic() {
    let src = "\
def checked(text: Str) -> Result[Str, Refusal]:
    return try: str_trim(text)
";
    assert!(codes(src).contains(&"B0392"), "{:?}", codes(src));
}

/// A `Result` has one error type, so a block that can fail two ways has to say which. The
/// diagnostic names both and asks for the union, which is the answer in every case.
#[test]
fn a_block_that_fails_two_ways_is_refused_by_name() {
    let src = "\
union Parse:
    NotANumber

union Budget:
    Over

def both(text: Str) -> Int:
    if str_is_empty(text):
        raise NotANumber
    raise Over

def checked(text: Str) -> Result[Int, Parse]:
    return try: both(text)
";
    let (_, d, map) = check_str("t.beck", src);
    let rendered = d.render(&map);
    assert!(d.iter().any(|x| x.code == "B0393"), "{}", rendered);
    assert!(
        rendered.contains("`Budget`") && rendered.contains("`Parse`"),
        "the diagnostic must name both ways it can fail:\n{rendered}"
    );
}

// ---------------------------------------------------------------------------------------------
// 3. The handler is lexical, and a failure it cannot type keeps travelling
// ---------------------------------------------------------------------------------------------

/// POPL 2019's result, and §38.4's reason for adopting it here specifically: "in a language where
/// effects decide placement, accidental interception would mean accidental *re-placement*".
///
/// The runtime half is what this asserts — a `try:` for one error type does not swallow another,
/// even though both unwind the same way. The type is an argument to the handler for exactly this.
#[test]
fn a_handler_does_not_catch_a_failure_it_cannot_type() {
    let src = "\
union Parse:
    NotANumber

union Budget:
    Over

def inner(text: Str) -> Int uses raises(Budget):
    raise Over

def outer(text: Str) -> Result[Int, Parse]:
    return try:
        inner(text)
";
    // The block raises `Budget` and the handler is asked for a `Parse`, so the two disagree — and
    // the checker says so rather than the runtime silently mistyping an `Err`.
    let found = codes(src);
    assert!(
        found.contains(&"B0320") || found.contains(&"B0393"),
        "{found:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 4. Row aliases — §38.4's ergonomic warning, taken before it hurt
// ---------------------------------------------------------------------------------------------

#[test]
fn a_row_alias_expands_where_it_is_used() {
    let src = format!(
        "{REFUSAL}
row Fallible = raises(Refusal), log

def clean(text: Str) -> Str uses Fallible:
    if str_is_empty(text):
        raise Blank
    return text
"
    );
    // Both atoms, including the ambient one the alias also names: an alias is a name for a
    // bundle, and expanding it is all it does.
    let mut row = row_of(&src, "clean");
    row.sort();
    assert_eq!(row, vec!["log", "raises(Refusal)"]);
}

/// An alias may not shadow an atom: `row durable = …` would silently change what every signature
/// in the module means, so the atom is tried first and the alias is never reached.
#[test]
fn an_alias_cannot_shadow_an_effect_atom() {
    let src = "\
row durable = log

def f() -> Int uses durable:
    return 1
";
    assert!(checks(src)
        .defs
        .get("f")
        .expect("f")
        .effects
        .contains(&Effect::Durable));
}

#[test]
fn a_row_declared_twice_is_refused() {
    let src = "\
row A = log
row A = metrics

def f() -> Int uses A:
    return 1
";
    assert!(codes(src).contains(&"B0394"), "{:?}", codes(src));
}

#[test]
fn an_unknown_name_in_a_uses_clause_says_it_is_neither() {
    let src = "def f() -> Int uses teleport:\n    return 1\n";
    assert!(codes(src).contains(&"B0305"), "{:?}", codes(src));
}

// ---------------------------------------------------------------------------------------------
// 5. Failure is discharged on every tier, and does not move anything
// ---------------------------------------------------------------------------------------------

/// A `raises` atom must not force a placement. It is control flow, not a resource: a definition
/// that can fail is legal on the client, the server and the fold engine alike, and the solver has
/// no reason to prefer one. Before this was stated, `raises` made every fallible definition
/// unplaceable (`B0404`), which is the failure mode a new atom has by default.
#[test]
fn failing_does_not_decide_where_anything_runs() {
    use beck_core::Tier;
    let src = format!(
        "{REFUSAL}
def clean(text: Str) -> Str:
    if str_is_empty(text):
        raise Blank
    return text
"
    );
    for tier in [Tier::Client, Tier::Server, Tier::Data, Tier::Any] {
        assert!(
            tier.discharges(&Effect::Raises(std::sync::Arc::from("Refusal"))),
            "{tier:?} refuses to discharge a raise"
        );
    }
    checks(&src);

    // And the whole-program version: a corpus application whose `validate` is written with a
    // handler rather than by threading an `Err` back through every helper. It places itself, like
    // every other program in that directory.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/29-fallible.beck");
    let text = std::fs::read_to_string(path).expect("the corpus program is there");
    let (placed, d, map) = compile_str("corpus/29-fallible.beck", &text);
    assert!(!d.has_errors(), "{}", d.render(&map));
    assert!(placed.is_some());
}

/// And it does not break replay. A raise is deterministic in its input, so a fold that raises is
/// still a function of the log — the same reasoning `partial` already had.
#[test]
fn failing_does_not_break_replay() {
    assert!(!Effect::Raises(std::sync::Arc::from("Refusal")).breaks_replay());
}
