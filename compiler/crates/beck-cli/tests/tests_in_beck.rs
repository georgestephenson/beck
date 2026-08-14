//! The harness for §21.2's `test` construct — the tests about tests.
//!
//! `docs/21-tests-in-beck-and-proof.md` is the design; this asserts the parts of it that a claim
//! could otherwise be made about without evidence:
//!
//! * the sketch's own tests pass, running the roles the runtime drives;
//! * a test cannot construct a state the program could not reach;
//! * §21.3's default really is "everything stubbed", and really does say what it did;
//! * a stub replaces the definition that *performs* an effect, not every caller that inherits it;
//! * the compile-time assertions answer without executing anything;
//! * a `property` is reproducible, and its counterexample is shrunk;
//! * and a `test` block leaves no trace in the published interface, the placement or the bundle.

mod support;

use std::sync::Arc;

use beck_core::Placed;
use beck_rt::testing::{Options, Outcome};

fn run(placed: &Placed) -> beck_rt::testing::Report {
    let backend = beck_eval::backend(placed);
    beck_rt::testing::run(placed, backend, &Options::default())
}

/// `Options` for a program that lives in `examples/`.
///
/// `base_dir` is where `snapshots/` and a `wire_compatible_with` path are resolved from, so a
/// harness running the sketch's own tests in-process has to say where the sketch is. `Default`
/// leaves it empty, which resolves against the *harness's* working directory — fine for a program
/// built from a string, wrong for one read off disk (`docs/22` §22.10).
fn examples_options() -> Options {
    Options {
        base_dir: std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples")),
        ..Default::default()
    }
}

fn compile(src: &str) -> Placed {
    let (placed, d, m) = beck_core::compile_str("t.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&m));
    placed.expect("this program compiles")
}

fn case<'r>(report: &'r beck_rt::testing::Report, name: &str) -> &'r beck_rt::testing::Case {
    report
        .cases
        .iter()
        .find(|c| c.name.contains(name))
        .unwrap_or_else(|| panic!("no test named `{name}` in {:?}", report.cases))
}

fn outcome(report: &beck_rt::testing::Report, name: &str) -> Outcome {
    case(report, name).outcome.clone()
}

fn why(report: &beck_rt::testing::Report, name: &str) -> String {
    match outcome(report, name) {
        Outcome::Failed { why } => why,
        other => panic!("`{name}` was expected to fail, and {other:?}"),
    }
}

/// The example the whole project is about, with the tests §21.2 writes.
const TODO: &str = include_str!("../../../examples/todo.beck");

// ---------------------------------------------------------------------------------------------
// 1. The sketch's own tests
// ---------------------------------------------------------------------------------------------

#[test]
fn the_sketchs_tests_pass_and_there_are_some() {
    let placed = support::todo_program();
    let backend = beck_eval::backend(&placed);
    let report = beck_rt::testing::run(&placed, backend, &examples_options());
    assert!(
        report.cases.len() >= 8,
        "the example is the acceptance case and has to carry real tests"
    );
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
    assert_eq!(report.skipped(), 0);
}

#[test]
fn a_cross_boundary_test_needs_no_network_and_no_fixture() {
    // §21.2's headline: "one client's command reaches another client's page" is three lines,
    // because the boundary is a placement of one graph rather than a seam between two programs.
    let placed = compile(&format!(
        "{TODO}\ntest \"ana's command reaches bo's page\":\n\
         \x20   given []\n\
         \x20   when session(\"ana\") sends Add(id=Id(\"1\"), text=\"milk\")\n\
         \x20   expect page(session(\"ana\")) contains \"milk\"\n\
         \x20   expect page(session(\"bo\")) contains \"remaining\"\n"
    ));
    let report = run(&placed);
    assert!(outcome(&report, "ana's command").is_pass());
}

#[test]
fn the_state_a_test_arranges_went_through_the_real_fold_and_the_real_validate() {
    // The claim that makes `given` better than a factory: "a fixture can build an impossible
    // object; a log cannot". A `Toggle` of a todo that was never added produces no event, so the
    // state after it is the state before it — which a factory would have let a test fake.
    let placed = compile(&format!(
        "{TODO}\ntest \"a toggle of nothing changes nothing\":\n\
         \x20   given []\n\
         \x20   when Toggle(id=Id(\"missing\"))\n\
         \x20   expect Err(error=NoSuchTodo)\n\
         \x20   expect list_len(events) == 0\n\
         \x20   expect state == fold_of []\n"
    ));
    assert!(outcome(&run(&placed), "toggle of nothing").is_pass());
}

// ---------------------------------------------------------------------------------------------
// 2. §21.3 — mocks nobody writes
// ---------------------------------------------------------------------------------------------

/// A program with one genuinely external call, which is the whole subject of §21.3.
const ORDERS: &str = r#"
type Sku = newtype[Str]

model Order:
    sku: Sku
    qty: Int

model State:
    orders: Map[Sku, Order]

union Command:
    Place(sku: Sku, qty: Int)

union Event:
    Placed(sku: Sku, qty: Int)

union Rejection:
    PaymentDeclined
    Empty

union Answer:
    Approved
    Declined

def charge(qty: Int) -> Answer uses net.out(payments.example.com):
    return Approved

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Placed(sku, qty):
            return s.with(orders=map_insert(s.orders, sku, Order(sku=sku, qty=qty)))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Place(sku, qty):
            if qty <= 0:
                return Err(error=Empty)
            match charge(qty):
                case Approved:
                    return Ok(value=[Placed(sku=sku, qty=qty)])
                case Declined:
                    return Err(error=PaymentDeclined)

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            footer: (str(map_len(s.orders)) + " orders")

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, orders, validate)

@on(data)
orders: Signal[State] = durable(fold(apply_event, State(orders={}), events))

@on(client)
page: Signal[Html] = per_session(orders, view)
"#;

#[test]
fn a_test_that_mentions_no_effect_still_performs_none() {
    // §21.3 rule 1: "the tedious case is the one you do not care about, so it should cost nothing".
    // The test below says nothing about payments and does not reach the payment provider.
    let placed = compile(&format!(
        "{ORDERS}\ntest \"an order is recorded\":\n\
         \x20   when Place(sku=Sku(\"milk\"), qty=2)\n\
         \x20   expect events == [Placed(sku=Sku(\"milk\"), qty=2)]\n"
    ));
    let report = run(&placed);
    assert!(
        outcome(&report, "an order is recorded").is_pass(),
        "{}",
        beck_rt::testing::render(&report, true)
    );

    // …and the hidden default says what it did, which is the price of being hidden.
    let c = case(&report, "an order is recorded");
    assert_eq!(c.stubbed.len(), 1);
    assert_eq!(c.stubbed[0].atom, "net.out(payments.example.com)");
    assert_eq!(c.stubbed[0].def.as_ref(), "charge");
    assert_eq!(c.stubbed[0].calls, 1);
    assert!(!c.stubbed[0].explicit);
    // The canonical inhabitant of `Answer` is its first variant, and nothing had to say so.
    assert_eq!(
        c.stubbed[0].returned.as_ref().and_then(|v| v.variant()),
        Some("Approved")
    );
}

#[test]
fn naming_the_effect_is_the_whole_stub() {
    // §21.3 rule 2: "One line. No method name, because the effect atom *is* the identity."
    let placed = compile(&format!(
        "{ORDERS}\ntest \"a declined charge rejects the order\":\n\
         \x20   stub net.out(payments.example.com): Declined\n\
         \x20   when Place(sku=Sku(\"milk\"), qty=2)\n\
         \x20   expect Err(error=PaymentDeclined)\n"
    ));
    let report = run(&placed);
    assert!(
        outcome(&report, "declined charge").is_pass(),
        "{}",
        beck_rt::testing::render(&report, true)
    );
    assert!(case(&report, "declined charge").stubbed[0].explicit);
}

#[test]
fn a_stub_replaces_what_performs_the_effect_not_everything_that_inherits_it() {
    // The rule that keeps §21.2's "goes through the *real* `validate`" true. An effect row
    // propagates, so `validate`'s row names the payment host too — and stubbing by *row* would
    // replace the authority chokepoint itself, leaving a test that exercises nothing while
    // reporting a pass.
    let placed = compile(&format!(
        "{ORDERS}\ntest \"authority is still exercised\":\n\
         \x20   stub net.out(payments.example.com): Approved\n\
         \x20   when Place(sku=Sku(\"milk\"), qty=0)\n\
         \x20   expect Err(error=Empty)\n"
    ));
    let report = run(&placed);
    assert!(
        outcome(&report, "authority is still").is_pass(),
        "{}",
        beck_rt::testing::render(&report, true)
    );
    // Exactly one definition was stubbed, and it is not the chokepoint.
    let stubbed: Vec<&str> = case(&report, "authority is still")
        .stubbed
        .iter()
        .map(|s| s.def.as_ref())
        .collect();
    assert_eq!(stubbed, vec!["charge"]);
}

#[test]
fn a_stub_can_answer_from_the_call_itself() {
    // §21.3 rule 3: "matching by value uses the language's own `match`, so there is no mock DSL".
    // The same stub, written once, declines one order and approves another — because it looks at
    // what it was called with.
    let src = format!(
        "{ORDERS}\n\
         test \"a large order is declined\":\n\
         \x20   stub net.out(payments.example.com):\n\
         \x20       case 1:\n\
         \x20           return Approved\n\
         \x20       case _:\n\
         \x20           return Declined\n\
         \x20   when Place(sku=Sku(\"yacht\"), qty=9)\n\
         \x20   expect Err(error=PaymentDeclined)\n\
         \ntest \"a small one is not\":\n\
         \x20   stub net.out(payments.example.com):\n\
         \x20       case 1:\n\
         \x20           return Approved\n\
         \x20       case _:\n\
         \x20           return Declined\n\
         \x20   when Place(sku=Sku(\"milk\"), qty=1)\n\
         \x20   expect list_len(events) == 1\n"
    );
    let placed = compile(&src);
    let report = run(&placed);
    assert!(
        outcome(&report, "large order").is_pass() && outcome(&report, "small one").is_pass(),
        "{}",
        beck_rt::testing::render(&report, true)
    );

    // The report says it answered from the call, and what it last answered — the only honest single
    // value a stub that varies can be described by.
    let c = case(&report, "large order");
    assert!(c.stubbed[0].from_the_call);
    assert!(c.stubbed[0].explicit);
    assert_eq!(
        c.stubbed[0].returned.as_ref().and_then(|v| v.variant()),
        Some("Declined")
    );
    assert_eq!(
        case(&report, "small one").stubbed[0]
            .returned
            .as_ref()
            .and_then(|v| v.variant()),
        Some("Approved")
    );
}

#[test]
fn the_general_form_of_a_stub_body_is_an_ordinary_expression() {
    // The `case` sugar is a case of this: the stubbed definition's parameters are in scope, and
    // every expression in the language works. A threshold reads as a threshold.
    let src = format!(
        "{ORDERS}\n\
         test \"a threshold reads as a threshold\":\n\
         \x20   stub net.out(payments.example.com):\n\
         \x20       return Declined if qty > 5 else Approved\n\
         \x20   when Place(sku=Sku(\"yacht\"), qty=9)\n\
         \x20   expect Err(error=PaymentDeclined)\n\
         \x20   expect net.out(payments.example.com) with 9\n"
    );
    assert!(
        outcome(&run(&compile(&src)), "threshold").is_pass(),
        "the parameters of the stubbed definition are in scope by name"
    );
}

#[test]
fn interaction_assertions_are_queries_over_what_happened() {
    // §21.3 rule 4: "Nothing had to be arranged for these to be answerable."
    let placed = compile(&format!(
        "{ORDERS}\ntest \"nothing left the process\":\n\
         \x20   when Place(sku=Sku(\"milk\"), qty=0)\n\
         \x20   expect no net.out(payments.example.com)\n\
         \ntest \"the charge happened once, with the quantity\":\n\
         \x20   when Place(sku=Sku(\"milk\"), qty=2)\n\
         \x20   expect net.out(payments.example.com) once\n\
         \x20   expect net.out(payments.example.com) with 2\n"
    ));
    let report = run(&placed);
    assert!(outcome(&report, "nothing left").is_pass());
    assert!(
        outcome(&report, "happened once").is_pass(),
        "{}",
        beck_rt::testing::render(&report, true)
    );

    // …and a wrong count fails, rather than being an expectation nobody checked.
    let placed = compile(&format!(
        "{ORDERS}\ntest \"a claim that is false\":\n\
         \x20   when Place(sku=Sku(\"milk\"), qty=2)\n\
         \x20   expect no net.out(payments.example.com)\n"
    ));
    assert!(why(&run(&placed), "false").contains("was performed 1 time"));
}

// ---------------------------------------------------------------------------------------------
// 3. Failure reporting
// ---------------------------------------------------------------------------------------------

#[test]
fn a_failing_comparison_reports_both_sides() {
    // §4.5: "for a language whose main feature is inference, error quality *is* the product." A
    // test runner is a diagnostic surface too, and "expected true, got false" is not an answer.
    let placed = compile(&format!(
        "{TODO}\ntest \"wrong on purpose\":\n\
         \x20   when Add(id=Id(\"1\"), text=\"milk\")\n\
         \x20   expect Err(error=BlankText)\n"
    ));
    let why = why(&run(&placed), "wrong on purpose");
    assert!(why.contains("are not equal"), "{why}");
    assert!(why.contains("BlankText"), "{why}");
    assert!(why.contains("Added"), "{why}");
}

#[test]
fn a_page_assertion_that_fails_prints_the_page() {
    let placed = compile(&format!(
        "{TODO}\ntest \"milk is not there\":\n\
         \x20   given []\n\
         \x20   expect page contains \"milk\"\n"
    ));
    let why = why(&run(&placed), "milk is not there");
    assert!(why.contains("does not contain"), "{why}");
    assert!(
        why.contains("<h1>todos</h1>"),
        "the page itself is in the report: {why}"
    );
}

// ---------------------------------------------------------------------------------------------
// 4. Compile-time assertions — §3.4's assertability guardrail, beside the code
// ---------------------------------------------------------------------------------------------

#[test]
fn placement_and_flow_are_answered_without_running_anything() {
    let placed = compile(&format!(
        "{TODO}\ntest \"the page is the browser's job\":\n\
         \x20   expect place(page) == client\n\
         \x20   expect place(todos) == data\n\
         \x20   expect flow(Rejection) reaches nothing on client\n"
    ));
    let report = run(&placed);
    assert!(
        outcome(&report, "browser's job").is_pass(),
        "{}",
        beck_rt::testing::render(&report, true)
    );
    // Nothing ran: no stub was installed and no input was generated.
    let c = case(&report, "browser's job");
    assert_eq!(c.runs, 0);
    assert!(c.stubbed.is_empty());
}

#[test]
fn a_placement_that_moves_fails_the_test_that_depended_on_it() {
    let placed = compile(&format!(
        "{TODO}\ntest \"the fold is on the client\":\n    expect place(todos) == client\n"
    ));
    let why = why(&run(&placed), "fold is on the client");
    assert!(why.contains("is placed on `data`"), "{why}");
}

#[test]
fn an_unplaced_definition_counts_as_reaching_every_tier() {
    // The subtle half of `flow`. `apply_event` is `Tier::Any` — compiled to whichever tier calls
    // it — so a type it mentions reaches the client. Counting `any` as "not the client" would make
    // this assertion pass for the most dangerous case there is.
    let placed = compile(&format!(
        "{TODO}\ntest \"events stay off the browser\":\n\
         \x20   expect flow(Event) reaches nothing on client\n"
    ));
    let why = why(&run(&placed), "events stay off");
    assert!(why.contains("apply_event"), "{why}");
    assert!(why.contains("(any)"), "{why}");
}

// ---------------------------------------------------------------------------------------------
// 5. `property` — one generator, three features
// ---------------------------------------------------------------------------------------------

#[test]
fn a_property_runs_generated_logs_through_the_real_fold() {
    let placed = compile(&format!(
        "{TODO}\nproperty \"every log renders\"(log: list[Event]):\n\
         \x20   given log\n\
         \x20   expect page contains \"remaining\"\n"
    ));
    let report = run(&placed);
    assert!(
        outcome(&report, "every log renders").is_pass(),
        "{}",
        beck_rt::testing::render(&report, true)
    );
    assert_eq!(
        case(&report, "every log renders").runs,
        beck_rt::testing::DEFAULT_RUNS
    );
}

#[test]
fn a_property_that_fails_is_reproducible_and_shrunk() {
    // §21.2: "**A flaky Beck test should be impossible**, and if one appears it is a compiler
    // defect." The seed is the test's name and the run index, never a clock — so the same failing
    // input, and the same shrunk counterexample, come back on every machine.
    let src = format!(
        "{TODO}\nproperty \"no log is ever empty\"(log: list[Event]):\n\
         \x20   given log\n\
         \x20   expect list_len(log) > 0\n"
    );
    let placed = compile(&src);
    let first = why(&run(&placed), "no log is ever empty");
    let second = why(&run(&placed), "no log is ever empty");
    assert_eq!(first, second, "two runs of the same property must agree");
    // Shrunk all the way to the empty log, which is the smallest thing that fails.
    assert!(first.contains("log = []"), "{first}");
}

#[test]
fn a_generator_that_would_have_to_invent_a_secret_refuses() {
    // §21.3 rule 5's refusal, at the surface: "inventing a secret in a test is exactly the sort of
    // thing that should require somebody to type it out".
    let placed = compile(&format!(
        "{TODO}\nproperty \"keys\"(k: secret[Str]):\n    expect list_len(events) == 0\n"
    ));
    let why = why(&run(&placed), "keys");
    assert!(why.contains("written out by a person"), "{why}");
}

// ---------------------------------------------------------------------------------------------
// 6. A test leaves no trace in what ships
// ---------------------------------------------------------------------------------------------

#[test]
fn a_test_block_is_not_part_of_the_published_interface() {
    // §3.6: a `.becki` is what downstream modules compile against, and what `--wire-compat`
    // compares releases of. A test is neither, and it must not move the interface digest.
    let (with_tests, d, m) = beck_core::check_str("t.beck", TODO);
    assert!(!d.has_errors(), "{}", d.render(&m));
    let stripped: String = TODO
        .split("# ---------- Tests: a log, a command, an expectation ----------")
        .next()
        .expect("the example carries its tests at the end")
        .to_string();
    let (without, d, m) = beck_core::check_str("t.beck", &stripped);
    assert!(!d.has_errors(), "{}", d.render(&m));
    assert!(!with_tests.tests.is_empty());
    assert!(without.tests.is_empty());

    let a = beck_core::Interface::of(&with_tests);
    let b = beck_core::Interface::of(&without);
    assert_eq!(
        a.render(),
        b.render(),
        "a test changed the published interface"
    );
    assert_eq!(a.digest(), b.digest());
    for word in ["test", "given", "expect", "milk"] {
        assert!(
            !a.render().contains(word),
            "`{word}` reached the published interface"
        );
    }
}

#[test]
fn a_test_block_is_not_placed_and_does_not_move_the_wire_id() {
    // Both compiled under the same module name: the operation id hashes the module (§4.3), so
    // comparing `examples/todo.beck` against `t.beck` would be comparing two different programs.
    let with_tests = compile(TODO);
    let stripped: String = TODO
        .split("# ---------- Tests: a log, a command, an expectation ----------")
        .next()
        .expect("the example carries its tests at the end")
        .to_string();
    let without = compile(&stripped);

    // Nothing in a test is a placeable node…
    assert_eq!(
        with_tests.placement.tiers.len(),
        without.placement.tiers.len(),
        "a test added a node to the placement problem"
    );
    // …and the operation id is derived from the *wire*, which a test is not part of.
    assert_eq!(with_tests.wire_id, without.wire_id);
}

#[test]
fn no_test_reaches_the_client_bundle() {
    // The template is `security.rs`'s bundle search: "by construction" is a claim until something
    // checks the bytes, and the bytes are what ships.
    let client = beck_rt::THIN_CLIENT;
    for word in ["milk", "expect", "fold_of", "BlankText"] {
        assert!(
            !client.contains(word),
            "`{word}` came from a test and must not be in the client bundle"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 7. The backend seam stays load-bearing
// ---------------------------------------------------------------------------------------------

/// A backend that executes but cannot install stubs — the default of
/// [`beck_core::backend::Backend::intercepting`].
struct NoStubs(Arc<dyn beck_core::Backend>);

impl beck_core::Backend for NoStubs {
    fn name(&self) -> &'static str {
        "no-stubs"
    }
    fn constant(&self, code: &beck_core::Core) -> Result<beck_core::Value, beck_core::ExecError> {
        self.0.constant(code)
    }
    fn function(
        &self,
        code: &beck_core::Core,
    ) -> Result<beck_core::backend::Callable, beck_core::ExecError> {
        self.0.function(code)
    }
}

#[test]
fn a_backend_that_cannot_stub_skips_rather_than_running_the_real_thing() {
    // The alternative would be to run the payment call for real and report a pass, which is the one
    // outcome a test harness must never produce.
    let placed = compile(&format!(
        "{ORDERS}\ntest \"an order is recorded\":\n\
         \x20   when Place(sku=Sku(\"milk\"), qty=2)\n\
         \x20   expect list_len(events) == 1\n"
    ));
    let inner = beck_eval::backend(&placed);
    let backend: Arc<dyn beck_core::Backend> = Arc::new(NoStubs(inner));
    let report = beck_rt::testing::run(&placed, backend, &Options::default());
    match outcome(&report, "an order is recorded") {
        Outcome::Skipped(why) => assert!(why.contains("cannot install stubs"), "{why}"),
        other => panic!("a backend with no stubs must not silently pass: {other:?}"),
    }
    assert!(report.ok(), "a skip is not a failure");
    assert_eq!(report.passed(), 0, "…and it is not a pass either");
}

#[test]
fn a_program_with_no_effects_needs_no_interceptor_at_all() {
    // The other half: the todo sketch touches nothing external, so it runs on a backend that
    // cannot stub — which is what keeps the seam an addition rather than a requirement.
    let placed = support::todo_program();
    let inner = beck_eval::backend(&placed);
    let backend: Arc<dyn beck_core::Backend> = Arc::new(NoStubs(inner));
    let report = beck_rt::testing::run(&placed, backend, &examples_options());
    assert_eq!(report.skipped(), 0);
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
}

// ---------------------------------------------------------------------------------------------
// 8. Filtering
// ---------------------------------------------------------------------------------------------

#[test]
fn a_filter_selects_by_name() {
    let placed = support::todo_program();
    let backend = beck_eval::backend(&placed);
    let report = beck_rt::testing::run(
        &placed,
        backend,
        &Options {
            filter: Some("empty todo".into()),
            ..Default::default()
        },
    );
    assert_eq!(report.cases.len(), 1);
    assert!(report.cases[0].name.contains("empty todo"));
}

// ---------------------------------------------------------------------------------------------
// Page snapshots — §21.2's golden assertion, and `beck test --update`
// ---------------------------------------------------------------------------------------------

/// `expect page matches snapshot`, through the binary, in all four states it can be in.
///
/// Through the binary rather than in-process because `--update` is a *flag*, and the property that
/// matters is that nothing writes a snapshot without one. A test that called the runtime with
/// `update_snapshots: true` would assert the writing and not the policy.
#[test]
fn a_page_snapshot_is_recorded_only_when_asked_and_compared_every_other_time() {
    let dir = std::env::temp_dir().join("beck-snapshot-flow");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let file = dir.join("todo.beck");

    let mut src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/todo.beck"
    ))
    .expect("the sketch is checked in");
    src.push_str(
        "\ntest \"a snapshotted page\":\n    \
         given [Added(id=Id(\"1\"), text=\"milk\")] by \"ana\"\n    \
         expect page(session(\"ana\")) matches snapshot\n",
    );
    std::fs::write(&file, &src).expect("written");

    let beck = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
            .args(args)
            .output()
            .expect("the compiler is built");
        (
            out.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let path = file.to_str().expect("a path").to_string();
    let snapshot = dir.join("snapshots").join("a-snapshotted-page@ana.html");

    // 1. Nothing recorded is a *failure*, not a silent write. A first run that quietly passed would
    //    be a test that has never compared anything.
    let (ok, text) = beck(&["test", &path, "--filter", "snapshotted"]);
    assert!(!ok, "an unrecorded snapshot has to fail:\n{text}");
    assert!(text.contains("no snapshot recorded"), "{text}");
    assert!(
        text.contains("--update"),
        "and say how to record it:\n{text}"
    );
    assert!(!snapshot.exists(), "and must not have written one:\n{text}");

    // 2. `--update` records it, and the file is the page.
    let (ok, text) = beck(&["test", &path, "--filter", "snapshotted", "--update"]);
    assert!(ok, "{text}");
    let recorded = std::fs::read_to_string(&snapshot).expect("--update wrote it");
    assert!(
        recorded.contains("milk"),
        "the snapshot is the page: {recorded}"
    );

    // 3. It now passes, against the file rather than against anything in memory.
    let (ok, text) = beck(&["test", &path, "--filter", "snapshotted"]);
    assert!(ok, "{text}");

    // 4. A page that changed fails, and the message says *where* — a rendered page is one long
    //    line, so a diff that elided from the start would show two identical prefixes.
    std::fs::write(&snapshot, recorded.replace("milk", "bread")).expect("written");
    let (ok, text) = beck(&["test", &path, "--filter", "snapshotted"]);
    assert!(!ok, "a changed page has to fail:\n{text}");
    assert!(text.contains("does not match"), "{text}");
    assert!(
        text.contains("column"),
        "the failure has to name the column, or it is a diff the reader has to do:\n{text}"
    );
    assert!(
        text.contains("bread") && text.contains("milk"),
        "and show both sides at the difference rather than at the start:\n{text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The assertion round-trips through both surfaces, named and unnamed, with and without an actor.
///
/// `docs/02` §2.2's property: the two surfaces are one language, so a form the parser accepts and
/// the printer cannot write is a form that would be lost by `beck fmt`.
#[test]
fn a_snapshot_assertion_survives_being_printed_and_read_back() {
    for clause in [
        "expect page matches snapshot",
        "expect page matches snapshot \"after checkout\"",
        "expect page(session(\"ana\")) matches snapshot",
        "expect page(session(\"ana\")) matches snapshot \"after checkout\"",
    ] {
        let src = format!("test \"a page\":\n    given []\n    {clause}\n");
        let mut map = beck_diag::SourceMap::new();
        let file = map.add("snap.beck", &src);
        let mut diags = beck_diag::Diagnostics::new();
        let parsed = beck_syntax::parse_file(file, "snap.beck", &src, &mut diags);
        assert!(!diags.has_errors(), "{clause}:\n{}", diags.render(&map));
        let printed = beck_syntax::print::to_python(&parsed);
        assert!(
            printed.contains(clause),
            "`{clause}` printed back as:\n{printed}"
        );
    }
}
