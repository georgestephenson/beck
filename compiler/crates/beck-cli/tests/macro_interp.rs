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

// -------------------------------------------------------------------------------------------
// A macro that decorates a declaration — docs/02 §2.4's `derive`
// -------------------------------------------------------------------------------------------
//
// §2.4's sketch takes a `model`, reads what is in it, and emits code per field. Four things had to
// become true for that to parse, and each is one rule made uniform rather than a case added for
// `derive`: a block passed to a macro **in item position** holds declarations; a `quote:` holds
// them too; `$` unquotes where a type and where a field name go; and a `do` at module level is
// flattened all the way down, because `derive` returns the block it was given beside what it
// generated and that is a `do` inside a `do`.
//
// `lib/json.beck` is the program that ships it, and the last test in this file derives from it
// across an import. These are the rules on their own, and — the half that matters more — the places
// they deliberately do not reach.

/// **A macro's block may declare things, and the macro may emit more of them.**
///
/// The whole of `derive`'s shape in eight lines: a `model` goes in, the model and an `impl` for it
/// come out, and the `impl` names the type by unquoting the model's own name rather than writing
/// it — which is what hygiene makes necessary, since a name written in the template would carry a
/// fresh scope and refer to nothing.
#[test]
fn a_macro_takes_a_declaration_and_emits_declarations() {
    all_pass(
        "derive.beck",
        r#"
trait Named:
    def label(self) -> Str

macro tagged(do):
    decl = node_args(do)[0]
    name = node_args(decl)[0]
    return quote:
        $do

        impl Named for $name:
            def label(self):
                return "a shape"

tagged:
    model Point:
        x: Int

test "the model and the impl both arrived":
    expect Point(x=1).label() == "a shape"
"#,
    );
}

/// **`$` where a field name goes**, which is what lets generated code read a field it was handed.
#[test]
fn a_macro_reads_a_field_whose_name_it_was_given() {
    all_pass(
        "field.beck",
        r#"
macro sum_of(do):
    decl = node_args(do)[0]
    parts = node_args(decl)
    name = parts[0]
    total = quote:
        0
    i = 2
    while i < list_len(parts):
        f = node_args(parts[i])[0]
        total = quote:
            $total + it.$f
        i = i + 1
    return quote:
        $do

        def total(it: $name) -> Int:
            return $total

sum_of:
    model Q:
        a: Int
        b: Int

test "every field was read, and only the ones the model has":
    expect total(Q(a=3, b=4)) == 7
"#,
    );
}

/// **A declaration inside a *value* block is still refused**, and this is the half that would be
/// forgotten.
///
/// The rule is about position, not about macros: `tagged:` written as a module item takes a
/// `model`, and the same call inside a `def` takes statements — because a `model` in a function
/// body is not a thing this language has, and reading one there would turn a mistake into a
/// mystery. Without this the change would have been "declarations parse anywhere", which is a
/// different and much larger claim.
#[test]
fn a_declaration_inside_a_function_body_is_still_not_an_item() {
    let src = r#"
macro tagged(do):
    return quote:
        $do

def f() -> Int:
    tagged:
        model Inner:
            x: Int
    return 1
"#;
    let codes = codes("inner.beck", src);
    assert!(
        !codes.is_empty(),
        "a `model` inside a `def` body compiled, so the block rule is not about position"
    );
}

/// A `quote:` may hold a declaration, because a `quote` builds syntax and what may be written in
/// one is what may be written in a program. Whether the result belongs where the macro was called
/// stays the checker's question — asked about the expansion, not about the template.
#[test]
fn a_quote_may_hold_a_declaration_the_caller_could_have_written() {
    all_pass(
        "quoted.beck",
        r#"
macro pair():
    return quote:
        model Made:
            n: Int

        def made() -> Made:
            return Made(n=7)

pair()

test "a model and a def came out of one quote":
    expect made().n == 7
"#,
    );
}

// -------------------------------------------------------------------------------------------
// A macro crosses an import — docs/02 §2.4
// -------------------------------------------------------------------------------------------
//
// Until this, expansion ran per module on the parsed file, *before* any import was resolved, so a
// macro was usable where it was declared and nowhere else. Nothing refused it — the name was simply
// not there — which is the kind of absence a test cannot find by asking for a diagnostic. So these
// ask the other way round: a project on disk, a macro in one module and a call in another, and the
// binary run over it.
//
// It is what turns every one of §8.5.4's macro successors from a mechanism a program can use into a
// facility a library can ship, and `lib/json.beck` is the first thing to ship one.

/// Run `beck` over a scratch project and return whether it succeeded and what it said.
fn beck_in(dir: &std::path::Path, files: &[(&str, &str)], args: &[&str]) -> (bool, String) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("a scratch directory");
    for (name, src) in files {
        std::fs::write(dir.join(name), src).expect("a scratch file");
    }
    let mut argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    argv.push(dir.join(files[0].0).to_string_lossy().to_string());
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(&argv)
        .output()
        .expect("the compiler is built");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("beck-macro-import-{name}"))
}

/// **A macro declared in one module is usable in another that imports it.**
///
/// The whole of the item, and the reason it is asserted on a `derive`-shaped macro rather than on
/// something simpler: what crosses is not only the macro but the *hygiene* it was written with, and
/// the `impl` this generates names a type it was handed by unquoting the caller's own syntax. A
/// crossing that lost the scope would compile the macro and fail to find the type.
#[test]
fn a_macro_declared_in_one_module_is_usable_in_another() {
    let dir = scratch("crosses");
    let (ok, text) = beck_in(
        &dir,
        &[
            (
                "main.beck",
                r#"
import shapes

name_it:
    model Point:
        x: Int

test "the macro came from the other module":
    expect Point(x=1).label() == "a shape"
"#,
            ),
            (
                "shapes.beck",
                r#"
trait Named:
    def label(self) -> Str

macro name_it(do):
    decl = node_args(do)[0]
    name = node_args(decl)[0]
    return quote:
        $do

        impl Named for $name:
            def label(self):
                return "a shape"
"#,
            ),
        ],
        &["test"],
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok && text.contains("1 passed"), "{text}");
}

/// **And a module that does not import it does not get it**, which is the half that says the
/// crossing follows the import rather than the directory.
///
/// The message matters as much as the refusal: before macros crossed at all, "this is not a
/// top-level item" was the whole truth. Now the likeliest cause is a missing `import`, and the note
/// says so.
#[test]
fn a_macro_does_not_reach_a_module_that_did_not_import_it() {
    let dir = scratch("uncrossed");
    let (ok, text) = beck_in(
        &dir,
        &[
            ("main.beck", "name_it:\n    model Point:\n        x: Int\n"),
            (
                "shapes.beck",
                "macro name_it(do):\n    return quote:\n        $do\n",
            ),
        ],
        &["check"],
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !ok,
        "a macro reached a module that never imported it:\n{text}"
    );
    assert!(text.contains("B0307"), "{text}");
    assert!(
        text.contains("import one that does"),
        "the refusal does not name the missing import:\n{text}"
    );
}

/// **The flat namespace decides a collision, exactly as it does for a `def`.**
///
/// Beck links modules into one namespace with no qualified reference (`B0601`), so two macros of
/// one name cannot both be in scope — and `B0200`, which has always refused a module that declared
/// one twice, is what refuses this too. That is the behaviour worth pinning: the crossing added no
/// second rule about names.
#[test]
fn two_macros_of_one_name_collide_wherever_they_came_from() {
    let dir = scratch("collide");
    let (ok, text) = beck_in(
        &dir,
        &[
            (
                "main.beck",
                "import shapes\n\nmacro name_it(do):\n    return quote:\n        $do\n\ntest \"x\":\n    expect true\n",
            ),
            ("shapes.beck", "macro name_it(do):\n    return quote:\n        $do\n"),
        ],
        &["check"],
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!ok, "two macros of one name were both accepted:\n{text}");
    assert!(text.contains("B0200"), "{text}");
}

/// **`lib/json.beck` is the first library to ship a macro**, and this is the row it closes:
/// [`docs/46`](../../../../docs/46-standard-library-report.md) §46.16's "`@derive` for JSON — not
/// built".
///
/// Asserted from *outside* the library directory, because that is the claim — not "the file
/// compiles" but "a program somewhere else imports it and derives".
#[test]
fn a_program_imports_the_standard_library_and_derives_a_json_encoder() {
    let dir = scratch("json");
    let (ok, text) = beck_in(
        &dir,
        &[(
            "app.beck",
            r#"
import json

derive_json:
    model Todo:
        id: Str
        text: Str
        done: Bool

test "the derived encoder names every field":
    expect json_render(Todo(id="1", text="milk", done=false).to_json()) == "{\"done\":false,\"id\":\"1\",\"text\":\"milk\"}"
"#,
        )],
        &["test"],
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok && text.contains("1 passed"), "{text}");
    // The library's own tests stay the library's, which is the rule `stdlib.rs` holds for every
    // other module and which a macro-carrying one must not break.
    assert!(
        !text.contains("2 passed"),
        "importing `json` brought the library's own tests into the program:\n{text}"
    );
}
