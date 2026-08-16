//! The macro interpreter: what it computes, and that it computes what the program would.
//!
//! [`docs/02`](../../../../docs/02-syntax.md) §2.4 says macro bodies "run at compile time in the
//! compiler's own Beck interpreter". That is a second implementation of the language's pure part,
//! and a second implementation is a thing that agrees with the first *until it does not* — so the
//! centre of this file is a **differential**, the instrument
//! [`docs/04`](../../../../docs/04-compiler-architecture.md) §4.8 points at the backends, aimed
//! here at the two evaluators instead: the same expression is computed once by
//! `beck_macro::interp` while the module is expanding and once by `beck-eval` while the program
//! runs, and the answers must be equal.
//!
//! The rest is the two bounds — a macro body that does not terminate, and one that recurses
//! forever — and the control every bound in this project needs beside it: a macro a person would
//! write still compiles.

use beck_core::Placed;
use beck_rt::testing::{Options, Outcome};

fn compile(name: &str, src: &str) -> Placed {
    let (placed, d, m) = beck_core::compile_or_library_str(name, src);
    assert!(!d.has_errors(), "{}", d.render(&m));
    placed.expect("this program compiles")
}

fn codes(name: &str, src: &str) -> Vec<String> {
    let (_, diags, _) = beck_core::compile_or_library_str(name, src);
    diags.iter().map(|d| d.code.to_string()).collect()
}

/// Run a program's `test` blocks and say which failed, and why.
fn run(placed: &Placed) -> beck_rt::testing::Report {
    let backend = beck_eval::backend(placed);
    beck_rt::testing::run(placed, backend, &Options::default())
}

fn all_pass(name: &str, src: &str) {
    let placed = compile(name, src);
    let report = run(&placed);
    let failed: Vec<String> = report
        .cases
        .iter()
        .filter_map(|c| match &c.outcome {
            Outcome::Failed { why } => Some(format!("{}: {why}", c.name)),
            _ => None,
        })
        .collect();
    assert!(failed.is_empty(), "{failed:#?}");
    assert!(!report.cases.is_empty(), "the fixture has no tests in it");
}

// ---------------------------------------------------------------------------------------------
// The differential: one expression, two evaluators
// ---------------------------------------------------------------------------------------------

/// The expressions computed both ways, each one pure and each one written once.
///
/// Chosen for where two implementations *drift* rather than for coverage: the string operations
/// whose unit is characters rather than bytes, the ones whose table is somebody else's
/// (`beck-prim` — case mapping and replacement), integer division and remainder around zero and
/// negatives, and the higher-order list operations, which are the ones with an evaluation order to
/// get wrong.
const SAME_BOTH_WAYS: &[(&str, &str)] = &[
    // arithmetic, including the signs a wrapping implementation gets wrong
    ("7 / 2", "Int"),
    ("-7 / 2", "Int"),
    ("7 % 3", "Int"),
    ("-7 % 3", "Int"),
    ("2 * 3 + 4", "Int"),
    ("abs(-9)", "Int"),
    // text, in characters rather than bytes
    ("str_len(\"naïve\")", "Int"),
    ("str_slice(\"naïve\", 1, 3)", "Str"),
    ("str_upper(\"straße\")", "Str"),
    ("str_lower(\"ÅNGSTRÖM\")", "Str"),
    ("str_replace(\"a-b-c\", \"-\", \"+\")", "Str"),
    ("str_trim(\"  pad  \")", "Str"),
    ("str_repeat(\"ab\", 3)", "Str"),
    ("str_join(str_split(\"a,b,c\", \",\"), \"|\")", "Str"),
    ("str_join(str_split(\"abc\", \"\"), \"-\")", "Str"),
    ("str_join(str_chars(\"héllo\"), \".\")", "Str"),
    ("str(42) + str(true)", "Str"),
    ("str_contains(\"abc\", \"bc\")", "Bool"),
    // lists, including the higher-order operations, which have an evaluation order to get wrong
    ("list_len(list_append([1, 2], 3))", "Int"),
    (
        "list_fold([1, 2, 3, 4], 0, lambda acc, x: acc * 2 + x)",
        "Int",
    ),
    (
        "str_join(map_list([1, 2, 3], lambda x: str(x * x)), \",\")",
        "Str",
    ),
    (
        "list_len(filter_list([1, 2, 3, 4, 5], lambda x: x % 2 == 0))",
        "Int",
    ),
    ("list_len(concat_lists([[1], [2, 3]]))", "Int"),
    ("list_len(list_drop(list_take([1, 2, 3, 4], 3), 1))", "Int"),
];

/// A program that computes each expression at compile time and at run time and compares them.
///
/// The expression is written **twice**: once in a macro body, where `beck_macro::interp`
/// evaluates it and `$v` lands the answer in the program as a literal, and once in a `def`, where
/// `beck-eval` evaluates it while the program runs. Writing it twice is the point — passing it to
/// one macro as an argument would only carry the *syntax* through, and both halves would be the
/// same evaluator.
///
/// Comparing them inside the program means the oracle is the language's own `==` rather than two
/// Rust renderings of two values.
fn differential_program() -> String {
    let mut src = String::new();
    for (i, (e, ty)) in SAME_BOTH_WAYS.iter().enumerate() {
        // A macro and a `def` each, so a failure names the expression rather than the table.
        src.push_str(&format!(
            "macro at_compile_time_{i}():\n    v = {e}\n    return quote:\n        $v\n\n\
             def compiled_{i}() -> {ty}:\n    return at_compile_time_{i}()\n\n\
             def ran_{i}() -> {ty}:\n    return {e}\n\n"
        ));
    }
    src.push_str("test \"the two evaluators agree\":\n");
    for (i, (e, _)) in SAME_BOTH_WAYS.iter().enumerate() {
        src.push_str(&format!(
            "    # {e}\n    expect compiled_{i}() == ran_{i}()\n"
        ));
    }
    src
}

#[test]
fn the_macro_interpreter_and_the_evaluator_agree_on_every_pure_expression() {
    all_pass("differential.beck", &differential_program());
}

/// The differential can fail, which is the half a differential most often lacks.
///
/// `docs/82` §82.10: a gate written by the person who knows the gap tests the shape of the fix
/// rather than the shape of the gap. So this asserts the *instrument*: change one side and the
/// comparison goes red. Without it, "the two evaluators agree" would also be the result of the
/// two halves never having been different.
#[test]
fn the_differential_notices_when_the_two_halves_disagree() {
    let src = "\
macro at_compile_time():
    v = 2 + 2
    return quote:
        $v

def compiled() -> Int:
    return at_compile_time()

def ran() -> Int:
    return 2 + 3

test \"deliberately unequal\":
    expect compiled() == ran()
";
    let placed = compile("skew.beck", src);
    let report = run(&placed);
    assert!(
        report
            .cases
            .iter()
            .any(|c| matches!(c.outcome, Outcome::Failed { .. })),
        "a differential that cannot go red is not a differential"
    );
}

// ---------------------------------------------------------------------------------------------
// What the interpreter is for: computation a template cannot do
// ---------------------------------------------------------------------------------------------

/// A macro body that loops, binds, calls a `def` of the module's, and reflects over syntax.
///
/// Every one of these raised `B0205` before the interpreter existed — the whole of §2.4's "not
/// built" list except the typed half. They are asserted by *running the program they expand to*,
/// because a macro's output is only right if what it produces means what it should.
#[test]
fn a_macro_body_computes() {
    let src = "\
def triple(n: Int) -> Int:
    return n * 3

macro sum_to(n):
    total = 0
    i = 1
    while i <= 5:
        total = total + i
        i = i + 1
    return quote:
        $total + $n

macro tripled():
    got = triple(14)
    return quote:
        $got

macro count_args(items):
    n = 0
    for a in node_args(items):
        n = n + 1
    return quote:
        $n

macro named(x):
    upper = str_upper(node_head(x))
    return quote:
        $upper

macro doubled_each(items):
    out = []
    for a in node_args(items):
        out = list_append(out, node_form(\"*\", [a, 2]))
    return quote:
        [$*out]

def a() -> Int:
    return sum_to(1)

def b() -> Int:
    return tripled()

def c() -> Int:
    return count_args([7, 8, 9])

def d() -> Str:
    return named(hello)

def e() -> Int:
    return list_fold(doubled_each([1, 2, 3]), 0, lambda acc, x: acc + x)

test \"a macro body computes\":
    expect a() == 16
    expect b() == 42
    expect c() == 3
    expect d() == \"HELLO\"
    expect e() == 12
";
    all_pass("computes.beck", src);
}

/// `splice([…])` returns several definitions where one was written.
///
/// The shape §2.4's `derive` has — "return `splice([do, *impls])`" — and the reason
/// `expand_module` flattens a `do` at the top of a module. `derive` itself needs `.as_model()`,
/// which needs the checker; this is the half that does not.
#[test]
fn a_macro_can_return_more_than_one_definition() {
    let src = "\
macro pair_of_readers(name):
    return quote:
        def one() -> Str:
            return $name
        def two() -> Str:
            return $name

pair_of_readers(\"shared\")

test \"both definitions arrived\":
    expect one() == \"shared\"
    expect two() == \"shared\"
";
    all_pass("splice.beck", src);
}

/// A lambda in a macro body is a compile-time value, and closes over what it saw.
#[test]
fn a_macro_body_has_first_class_functions() {
    let src = "\
macro scaled(items, by):
    factor = 10
    out = map_list(node_args(items), lambda a: node_form(\"*\", [a, factor]))
    return quote:
        [$*out]

def f() -> Int:
    return list_fold(scaled([1, 2, 3], 0), 0, lambda acc, x: acc + x)

test \"a closure captured the frame\":
    expect f() == 60
";
    all_pass("closure.beck", src);
}

// ---------------------------------------------------------------------------------------------
// The bounds, and the control beside them
// ---------------------------------------------------------------------------------------------

/// `while true:` in a macro body is a compiler that does not finish, and it is refused.
#[test]
fn a_macro_body_that_does_not_terminate_is_refused() {
    let src = "\
macro spin(x):
    i = 0
    while true:
        i = i + 1
    return quote:
        $x

def f() -> Int:
    return spin(1)
";
    let all = codes("spin.beck", src);
    assert!(
        all.iter().any(|c| c == "B0215"),
        "the step budget should refuse this: {all:?}"
    );
    // Once, not once per macro that would have run afterwards.
    assert_eq!(all.iter().filter(|c| *c == "B0215").count(), 1, "{all:?}");
}

/// …and so is a compile-time call chain with no base case, by the count rather than by the stack.
#[test]
fn a_macro_body_that_recurses_forever_is_refused_with_a_diagnostic() {
    let src = "\
def down(n: Int) -> Int:
    return down(n + 1)

macro deep(x):
    y = down(1)
    return quote:
        $x

def f() -> Int:
    return deep(1)
";
    let all = codes("deep.beck", src);
    assert!(
        all.iter().any(|c| c == "B0216"),
        "an unbounded compile-time recursion should be a diagnostic rather than an abort: {all:?}"
    );
}

/// The control: the macros this repository actually contains still expand.
///
/// `macro_bomb.rs` makes the same argument about the production budget and says why — a limit
/// calibrated against the limits you built rather than against the programs you have is not
/// calibrated. These two files are the tree's macro-heavy ones.
#[test]
fn the_macros_a_person_wrote_still_compile() {
    for path in ["../../sicp/felleisen.beck", "../../sicp/ch3.beck"] {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let src =
            std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()));
        let (_, diags, map) = beck_core::compile_or_library_str(path, &src);
        assert!(!diags.has_errors(), "{path}: {}", diags.render(&map));
    }
}

/// What the most expensive macro body in this repository costs, against [`MAX_STEPS`].
///
/// The budget's doc comment quotes a number and this is where the number comes from — the rule
/// being that a limit separating *legitimate* from *absurd* has to know what legitimate costs.
/// Printed rather than merely asserted, so a change that makes expansion dearer is visible in the
/// output before it is a failure.
///
/// [`MAX_STEPS`]: beck_macro::MAX_STEPS
#[test]
fn the_step_budget_is_far_above_what_a_real_macro_spends() {
    // A body doing more than anything in the tree: five iterations of a loop that appends and
    // reflects, which is `docs/02` §2.4's `derive` in miniature.
    let src = "\
macro biggest(items):
    out = []
    for a in node_args(items):
        out = list_append(out, node_form(\"*\", [a, 2]))
    total = 0
    for a in out:
        total = total + 1
    return quote:
        [$*out]

def f() -> Int:
    return list_len(biggest([1, 2, 3, 4, 5]))
";
    let before = beck_macro::MAX_STEPS;
    let spent = steps_spent(src);
    println!("the largest macro body here spends {spent} steps of {before}");
    assert!(
        spent * 100 < before,
        "a real macro body spends {spent} steps against a budget of {before}: the budget is no \
         longer separating legitimate from absurd"
    );
}

/// How many steps expanding this source took, by asking the expander for what is left.
fn steps_spent(src: &str) -> u64 {
    let mut map = beck_diag::SourceMap::new();
    let file = map.add("cost.beck", src);
    let mut diags = beck_diag::Diagnostics::new();
    let parsed = beck_syntax::parse_file(file, "cost", src, &mut diags);
    let (_, left) = beck_macro::expand_module_measured(&parsed, &mut diags);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    beck_macro::MAX_STEPS - left
}
