//! The `parallel:` scope: what it runs, what it refuses, and what it publishes.
//!
//! `docs/38` §38.4's prescription for structured concurrency is "a scope owns its children, and
//! errors and cancellation join at the scope", built as a *handler* rather than a mechanism.
//! `docs/80` is that built, and the claim it rests on is narrow enough to test:
//!
//! > The scope's answer does not depend on the order its children ran in.
//!
//! Two rules hold it up and each is a diagnostic rather than a convention — no child can name
//! another (`B0398`), and no child may perform an effect another child could observe (`B0399`).
//! The tests below are about answers and refusals, not about speed: nothing here runs two children
//! at the same time, and `docs/80` §80.5 says what would have to change for something to.

use beck_rt::testing::{Options, Outcome};

fn compile(src: &str) -> beck_core::split::Placed {
    let (placed, d, m) = beck_core::compile_or_library_str("concurrency.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&m));
    placed.expect("this program compiles")
}

fn cases(src: &str) -> Vec<(std::sync::Arc<str>, Outcome)> {
    let placed = compile(src);
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

/// Every diagnostic code a program raises, in the order the compiler emitted them.
fn codes(src: &str) -> Vec<String> {
    let (_, d, _) = beck_core::compile_or_library_str("concurrency.beck", src);
    d.iter().map(|x| x.code.to_string()).collect()
}

/// The published signature of one definition, as `beck iface` renders it.
fn signature(src: &str, name: &str) -> String {
    let (program, d, m) = beck_core::check_str("concurrency.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&m));
    let iface = beck_core::iface::Interface::of(&program);
    iface
        .items
        .iter()
        .find(|i| i.name.as_ref() == name)
        .map(|i| beck_core::iface::render_item(i).trim().to_string())
        .unwrap_or_else(|| panic!("`{name}` is published: {:?}", iface.items))
}

/// The tier the solver put a definition on.
fn tier(src: &str, name: &str) -> beck_core::Tier {
    let (program, d, m) = beck_core::check_str("concurrency.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&m));
    beck_core::place::solve(&program, None)
        .tiers
        .into_iter()
        .find(|(k, _)| k.to_string() == name || k.to_string() == format!("def/{name}"))
        .map(|(_, t)| t)
        .unwrap_or_else(|| panic!("`{name}` is placed"))
}

// ---------------------------------------------------------------- what the scope means

/// The tail runs after the join, with every child's result in scope.
#[test]
fn the_tail_sees_every_child() {
    all_pass(
        "\
def double(n: Int) -> Int:
    return n * 2

def square(n: Int) -> Int:
    return n * n

def both(n: Int) -> Int:
    return parallel:
        d = double(n)
        s = square(n)
        d + s

test \"the scope joins its children's answers\":
    expect both(5) == 35
    expect both(0) == 0
",
    );
}

/// A scope is an expression, so it composes where an expression does — including inside another.
///
/// The inner scope is a *child* of the outer one, which is the case that would break if the
/// checker's sibling list were global rather than saved and restored around each scope.
#[test]
fn a_scope_nests_inside_a_scope() {
    all_pass(
        "\
def inner(n: Int) -> Int:
    return parallel:
        a = n + 1
        b = n + 2
        a + b

def outer(n: Int) -> Int:
    return parallel:
        x = inner(n)
        y = inner(n * 10)
        x + y

test \"the inner scope's answer reaches the outer scope's tail\":
    expect inner(1) == 5
    expect outer(1) == 28
",
    );
}

/// Children bound but never read still run, because a child is a child by being a binding.
#[test]
fn a_child_whose_result_the_tail_ignores_still_runs() {
    all_pass(
        "\
def note(n: Int) -> Int:
    return n

def only_one(n: Int) -> Int:
    return parallel:
        a = note(n)
        b = note(n * 100)
        a

test \"the tail may drop a child's answer\":
    expect only_one(3) == 3
",
    );
}

// ---------------------------------------------------------------- failure crosses the scope

/// `docs/38` §38.4's "cancellation is the error row crossing the scope", with the ordered join
/// deciding which failure when two children could raise.
///
/// The third case is the one worth having: `bad(2)` and `bad(1)` raise *different* errors, and the
/// answer is the earliest child **in the order they are written** rather than whichever a
/// scheduler happened to reach first. That is what makes the failure a function of the program.
#[test]
fn a_childs_failure_joins_at_the_scope_and_the_earliest_child_wins() {
    all_pass(
        "\
union Refusal:
    First
    Second

def bad(tag: Int) -> Int:
    if tag == 1:
        raise First
    if tag == 2:
        raise Second
    return tag

def both(x: Int, y: Int) -> Result[Int, Refusal]:
    return try:
        parallel:
            a = bad(x)
            b = bad(y)
            a + b

test \"two children that do not fail join their answers\":
    expect both(3, 4) == Ok(7)

test \"a child's failure crosses the scope\":
    expect both(3, 1) == Err(First)

test \"the earliest child in the order written is the failure that wins\":
    expect both(2, 1) == Err(Second)
",
    );
}

// ---------------------------------------------------------------- what it publishes

/// A scope performs `spawn`, so its enclosing definition publishes it and lands on a tier that can
/// discharge it — and neither is written in the program.
#[test]
fn a_scope_publishes_spawn_and_places_itself_on_the_server() {
    let src = "\
def double(n: Int) -> Int:
    return n * 2

def both(n: Int) -> Int:
    return parallel:
        a = double(n)
        b = double(n + 1)
        a + b
";
    assert_eq!(signature(src, "both"), "def both(n: Int) -> Int uses spawn");
    assert_eq!(tier(src, "both"), beck_core::Tier::Server);
    // The children are pure, and stay so. `spawn` belongs to the scope, not to what it runs.
    assert_eq!(signature(src, "double"), "def double(n: Int) -> Int");
    assert_eq!(tier(src, "double"), beck_core::Tier::Any);
}

/// A child's row is the scope's row, so a fallible child makes the whole scope fallible.
#[test]
fn a_childs_row_is_the_scopes_row() {
    let src = "\
union Refusal:
    Nope

def bad(n: Int) -> Int:
    if n < 0:
        raise Nope
    return n

def both(n: Int) -> Int:
    return parallel:
        a = bad(n)
        b = bad(n + 1)
        a + b
";
    assert_eq!(
        signature(src, "both"),
        "def both(n: Int) -> Int uses spawn, raises(Refusal)"
    );
}

// ---------------------------------------------------------------- what it refuses

/// `B0397` — a scope with fewer than two children is claiming a concurrency it does not have.
///
/// Both directions: one binding is refused, and so is a scope whose work is all in the tail. The
/// second is the one somebody writes by accident.
#[test]
fn a_scope_needs_at_least_two_children() {
    let one = "\
def f(n: Int) -> Int:
    return parallel:
        a = n + 1
        a * 2
";
    assert!(codes(one).contains(&"B0397".into()), "{:?}", codes(one));

    let none = "\
def f(n: Int) -> Int:
    return parallel:
        n + 1
";
    assert!(codes(none).contains(&"B0397".into()), "{:?}", codes(none));
}

/// `B0398` — a child that could read a sibling would have to run second.
///
/// The point of the code is that the name is *absent for a reason*: without it this is `B0340`,
/// "cannot find `a` in this scope", which is true and tells the reader nothing.
#[test]
fn a_child_may_not_name_another_child() {
    let src = "\
def f(n: Int) -> Int:
    return parallel:
        a = n + 1
        b = a * 2
        a + b
";
    let got = codes(src);
    assert!(got.contains(&"B0398".into()), "{got:?}");
    assert!(!got.contains(&"B0340".into()), "{got:?}");
}

/// …and the same name **is** in scope in the tail, which is where a reader belongs.
#[test]
fn the_tail_may_name_every_child() {
    all_pass(
        "\
def f(n: Int) -> Int:
    return parallel:
        a = n + 1
        b = n + 2
        (a * b) + a

test \"the tail reads a child twice\":
    expect f(1) == 8
",
    );
}

/// `B0399` — an effect another child could observe.
///
/// `durable` is the sharpest case: two children appending to the log in the other order is a
/// different log, and §3.7 makes the log the only description of a program's history.
#[test]
fn a_child_may_not_perform_an_effect_another_child_could_observe() {
    let src = "\
def marked(s: Signal[Int]) -> Signal[Int]:
    return durable(s)

def f(s: Signal[Int]) -> Int:
    return parallel:
        a = str_len(str(marked(s)))
        b = str_len(\"x\")
        a + b
";
    assert!(codes(src).contains(&"B0399".into()), "{:?}", codes(src));
}

/// …and a *read* of the same kind of thing is not, which is what `docs/81` split `fs` for.
///
/// `docs/80` §80.2 had to refuse `fs(path)` whole, because one atom naming a resource cannot say
/// what is being done to it. Two children reading two files was a thing the form should allow and
/// could not. Both directions are asserted here, because the value of the split is precisely that
/// the two answers differ.
#[test]
fn two_children_may_read_files_and_may_not_write_them() {
    let reads = "\
def load_a(p: Str) -> Int uses fs.read(profiles):
    return str_len(p)

def load_b(p: Str) -> Int uses fs.read(settings):
    return str_len(p) * 2

def both(p: Str) -> Int:
    return parallel:
        a = load_a(p)
        b = load_b(p)
        a + b
";
    assert!(codes(reads).is_empty(), "{:?}", codes(reads));
    assert_eq!(
        signature(reads, "both"),
        "def both(p: Str) -> Int uses fs.read(profiles), fs.read(settings), spawn"
    );

    let writes = reads.replace("uses fs.read(profiles)", "uses fs.write(profiles)");
    assert!(
        codes(&writes).contains(&"B0399".into()),
        "{:?}",
        codes(&writes)
    );
}

/// The spelling that used to work says which of the two to write.
///
/// `fs(path)` was one atom until `docs/81`, so it is the spelling a reader arrives with — from §3.2
/// as it stood, or from habit. `B0305`'s bare "neither an effect nor a row" is true and useless.
#[test]
fn the_old_spelling_of_the_filesystem_atom_says_what_to_write_instead() {
    let src = "\
def load(p: Str) -> Int uses fs(profiles):
    return str_len(p)
";
    let (_, d, _) = beck_core::compile_or_library_str("concurrency.beck", src);
    let note = d
        .iter()
        .find(|x| x.code == "B0305")
        .map(|x| format!("{x:?}"))
        .unwrap_or_default();
    assert!(
        note.contains("fs.read(profiles)") && note.contains("fs.write(profiles)"),
        "the diagnostic should name both atoms: {note}"
    );
}

/// A scope pinned to the browser is a placement error, and nothing was written to make it one.
///
/// §3.3's table already said `server` discharges `spawn` and `client` does not. This is the third
/// feature running whose refusal is the effect system's rather than its own (`docs/37` §37.8,
/// `docs/39`), and the reason the row is charged in the checker rather than inferred from what the
/// children happen to do: a scope over two pure children still cannot run in a patch interpreter.
#[test]
fn a_scope_cannot_be_pinned_to_the_browser() {
    let src = "\
def double(n: Int) -> Int:
    return n * 2

@on(client)
def both(n: Int) -> Int:
    return parallel:
        a = double(n)
        b = double(n + 1)
        a + b
";
    assert!(codes(src).contains(&"B0401".into()), "{:?}", codes(src));
}

/// …and `net.out(host)` is deliberately *not* on that list.
///
/// A remote host's state was never Beck's to order, and two outbound calls are the case the form
/// exists for — a rule that refused them would leave it with nothing to do.
#[test]
fn two_outbound_calls_are_what_the_form_is_for() {
    let src = "\
def left(x: Str) -> Int uses net.out(a.example.com):
    return str_len(x)

def right(x: Str) -> Int uses net.out(b.example.com):
    return str_len(x) * 2

def both(x: Str) -> Int:
    return parallel:
        a = left(x)
        b = right(x)
        a + b
";
    assert!(codes(src).is_empty(), "{:?}", codes(src));
    assert_eq!(
        signature(src, "both"),
        "def both(x: Str) -> Int uses net.out(a.example.com), net.out(b.example.com), spawn"
    );
    assert_eq!(tier(src, "both"), beck_core::Tier::Server);
}

/// A function that starts spawning is a **breaking** change, in the sentence §4.3 wrote for
/// `net.out`.
///
/// Nothing was added to make this true either. `spawn` decides a placement, so a caller that could
/// run the old version anywhere may not be able to run the new one at all — which is the same
/// shape as "a library that starts phoning home cannot do so silently", and the reason the row is
/// on the published contract rather than in the body.
#[test]
fn a_function_that_starts_spawning_is_a_breaking_change() {
    let published = |src: &str| {
        let (program, d, m) = beck_core::check_str("concurrency.beck", src);
        assert!(!d.has_errors(), "{}", d.render(&m));
        beck_core::iface::Interface::of(&program)
    };
    let before = published(
        "\
def double(n: Int) -> Int:
    return n * 2

def both(n: Int) -> Int:
    return double(n) + double(n + 1)
",
    );
    let after = published(
        "\
def double(n: Int) -> Int:
    return n * 2

def both(n: Int) -> Int:
    return parallel:
        a = double(n)
        b = double(n + 1)
        a + b
",
    );
    let changes = beck_core::compat::compare(&before, &after);
    assert!(
        beck_core::compat::is_breaking(&changes),
        "widening a row with `spawn` has to be breaking: {changes:?}"
    );
}

// ---------------------------------------------------------------- the two surfaces

/// §2.2's round-trip, over the form this change adds.
///
/// `parallel:` carries an indented body, so — like `try:` and `ui:` — it has no call notation to
/// print as. The `x = parallel:` shape is the one a printer forgets, because it is neither a
/// statement head nor a `return`.
#[test]
fn a_scope_prints_back_as_what_it_was_written_as() {
    for src in [
        "\
def f(n: Int) -> Int:
    return parallel:
        a = n + 1
        b = n + 2
        a + b
",
        "\
def f(n: Int) -> Int:
    total = parallel:
        a = n + 1
        b = n + 2
        a + b
    return total * 2
",
    ] {
        let mut map = beck_diag::SourceMap::new();
        let file = map.add("t.beck", src);
        let mut d = beck_diag::Diagnostics::new();
        let node = beck_syntax::parser::parse_module(file, "t", src, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));

        let printed = beck_syntax::print::to_python(&node);
        assert!(
            printed.contains("parallel:"),
            "the form printed as a call:\n{printed}"
        );

        let mut d2 = beck_diag::Diagnostics::new();
        let file2 = map.add("t2.beck", &printed);
        let again = beck_syntax::parser::parse_module(file2, "t", &printed, &mut d2);
        assert!(!d2.has_errors(), "reprinting did not re-parse:\n{printed}");
        assert_eq!(
            beck_syntax::print::to_sexpr(&node),
            beck_syntax::print::to_sexpr(&again),
            "print(parse(src)) does not read back to the same tree"
        );
    }
}

// ---------------------------------------------------------------- what a test may do

/// A `test` block may run a `parallel:` scope, and `B0700` does not stop it.
///
/// `B0700`'s reason is that a test must not depend on anything outside itself — "a test that
/// performs a real `net.out` is a test that can fail because somebody else's server is down".
/// `spawn` crosses no boundary and reaches no host, so it is the one atom on §3.3's list the rule
/// does not apply to. It is also the one `beck_core::testing` will not stand in for: a stub would
/// delete the children rather than the thing they call.
#[test]
fn a_test_block_may_run_a_scope() {
    all_pass(
        "\
def both(n: Int) -> Int:
    return parallel:
        a = n + 1
        b = n + 2
        a + b

test \"a test may call a definition that spawns\":
    expect both(1) == 5
",
    );
    assert!(!beck_core::testing::is_stubbable(
        &beck_core::row::Effect::Spawn
    ));
}
