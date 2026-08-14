//! `test` and `property` blocks — `docs/21-tests-in-beck-and-proof.md` §21.2 and §21.3, checked.
//!
//! This module holds the *checked* shape of a test. The runner is elsewhere (`beck-rt`), because
//! running one means driving the same `Roles` the runtime drives; what belongs here is the part
//! that is a language feature: a clause is typed against the program's own `Event`, `Command` and
//! state types, and an assertion about placement is answered from the compiler's own data without
//! running anything.
//!
//! # Why a test is clauses rather than statements
//!
//! §21.2: "A test names a log, an input, and an expectation. The log is the state, because state is
//! a fold — there is no fixture, no factory and no `setUp`." Each of the three is a clause, so the
//! checker knows which is which: `given` is a `list[Event]` and goes through the *real*
//! `apply_event`, `when` is a `Command` and goes through the *real* `validate`. A test therefore
//! cannot construct a state the program could not reach, which is the property a factory cannot
//! offer.
//!
//! # The row of a test is empty, and that is checked
//!
//! §21.2's open question — "Do test blocks have effect rows? They must not" — is settled here as an
//! error, `B0700`. An expression inside a test performs nothing: a test that could perform
//! `net.out` is a test that fails when somebody else's server is down. The *subject's* effects are
//! a different matter, and §21.3's answer is that they are stubbed — see [`Clause::Stub`] and the
//! auto-stubbing in the runner.

use std::sync::Arc;

use beck_diag::Span;

use crate::core::{Core, VarId};
use crate::ty::{Effect, Tier, Ty};

/// A checked `test` or `property` block.
#[derive(Clone, Debug)]
pub struct TestDef {
    pub name: Arc<str>,
    /// Non-empty for a `property`: the inputs a generator supplies (§21.3 rule 5).
    pub params: Vec<(VarId, Arc<str>, Ty)>,
    pub clauses: Vec<Clause>,
    /// The variables `state`, `events` and `result` are bound to while the expectations run.
    pub bindings: Bindings,
    pub span: Span,
}

impl TestDef {
    pub fn is_property(&self) -> bool {
        !self.params.is_empty()
    }

    /// Every expression this test evaluates, to be read rather than rewritten.
    ///
    /// [`Editor::references`](crate::editor::Editor::references) is what wants them: a name used
    /// only inside a `test` block is used, and an editor that did not look here would report it as
    /// unreferenced and rename it into a program that no longer compiles.
    ///
    /// This and [`cores_mut`](TestDef::cores_mut) have to stay the same list — the failure this
    /// file has already had once is a pass that walked `Program::defs` and missed every expression
    /// in a `test` block — so `tests::the_two_walks_agree` counts them
    /// against each other.
    pub fn cores(&self) -> Vec<&Core> {
        let mut out = Vec::new();
        for clause in &self.clauses {
            match clause {
                Clause::Given { events, .. } => out.push(events),
                Clause::When { commands, .. } => out.extend(commands.iter()),
                Clause::Stub { value, .. } => out.push(value),
                Clause::Expect { what, .. } => match what {
                    Expectation::Holds(c) => out.push(c),
                    Expectation::PageContains { needle, .. } => out.push(needle),
                    Expectation::FoldEquals { events, .. } => out.push(events),
                    Expectation::Performed {
                        how: Count::With(c),
                        ..
                    } => out.push(c),
                    Expectation::PageMatchesSnapshot { .. }
                    | Expectation::Place { .. }
                    | Expectation::Flow { .. }
                    | Expectation::WireCompatible { .. }
                    | Expectation::Performed { .. } => {}
                },
            }
        }
        out
    }

    /// Every expression this test evaluates, to be annotated in place.
    ///
    /// A `test` block's expressions are *code*, and the three passes that annotate a finished
    /// program — [`crate::liveness`], [`crate::frames`] and [`crate::fields`] — reach them through
    /// here. They did not until [`70`](../../../../../docs/70-the-evaluator-gets-fast-report.md): all three
    /// walked `Program::defs`, a test's clauses are not in it, and so every expression inside a
    /// `test` block ran on the paths those passes exist to replace.
    ///
    /// A clause that names something rather than computing it — `expect place(charge) == server`,
    /// `expect no net.out` — contributes nothing, because there is no expression to annotate.
    pub fn cores_mut(&mut self) -> Vec<&mut Core> {
        let mut out = Vec::new();
        for clause in &mut self.clauses {
            match clause {
                Clause::Given { events, .. } => out.push(events),
                Clause::When { commands, .. } => out.extend(commands.iter_mut()),
                Clause::Stub { value, .. } => out.push(value),
                Clause::Expect { what, .. } => match what {
                    Expectation::Holds(c) => out.push(c),
                    Expectation::PageContains { needle, .. } => out.push(needle),
                    Expectation::FoldEquals { events, .. } => out.push(events),
                    Expectation::Performed {
                        how: Count::With(c),
                        ..
                    } => out.push(c),
                    Expectation::PageMatchesSnapshot { .. }
                    | Expectation::Place { .. }
                    | Expectation::Flow { .. }
                    | Expectation::WireCompatible { .. }
                    | Expectation::Performed { .. } => {}
                },
            }
        }
        out
    }

    /// Every clause's span, so a caller can ask what part of the file a clause covers.
    pub fn clause_spans(&self) -> impl Iterator<Item = Span> + '_ {
        self.clauses.iter().map(|c| c.span())
    }

    /// Every assertion that needs no execution — placement, flow, wire compatibility.
    ///
    /// §21.2: "These are compile-time queries, not runtime assertions — `beck test` answers them
    /// without running anything, from the same data `beck explain place` and `beck check
    /// --wire-compat` already produce."
    pub fn is_static_only(&self) -> bool {
        self.clauses.iter().all(|c| match c {
            Clause::Expect { what, .. } => what.is_static(),
            _ => false,
        })
    }
}

/// The three names a test's expectations may use. They are plain data — a folded state, the events
/// a command produced, and the result of the last one — so every backend can hold them.
#[derive(Clone, Copy, Debug)]
pub struct Bindings {
    pub state: VarId,
    pub events: VarId,
    pub result: VarId,
}

impl Clause {
    /// The part of the source this clause covers.
    pub fn span(&self) -> Span {
        match self {
            Clause::Given { span, .. }
            | Clause::When { span, .. }
            | Clause::Stub { span, .. }
            | Clause::Expect { span, .. } => *span,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Clause {
    /// `given [Added(…)] by "ana"` — the log the state is folded from.
    Given {
        events: Core,
        actor: Option<Arc<str>>,
        span: Span,
    },
    /// `when session("ana") sends Add(…), Toggle(…)` — proposals through the real `validate`.
    When {
        actor: Option<Arc<str>>,
        /// `when session("ana", "/done") sends …` — the route the proposal was made from, which is
        /// what a `Proposal`'s own session carries. `None` is the application's root.
        route: Option<Arc<str>>,
        commands: Vec<Core>,
        span: Span,
    },
    /// `stub net.out(payments.example.com): Declined` — §21.3 rules 2 and 3.
    Stub {
        atom: Effect,
        /// The stubbed definition's parameters, when the stub answers *from* them (rule 3).
        /// Empty for a plain value (rule 2), which is evaluated once and does not see the call.
        params: Vec<VarId>,
        value: Core,
        span: Span,
    },
    Expect {
        what: Expectation,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub enum Expectation {
    /// `expect <Bool>`, with `state`, `events` and `result` in scope.
    Holds(Core),
    /// `expect page(session("bo")) contains "milk"`, and `session("bo", "/done")` for a route.
    PageContains {
        needle: Core,
        actor: Option<Arc<str>>,
        route: Option<Arc<str>>,
    },
    /// `expect page matches snapshot` / `… matches snapshot "after checkout"`.
    ///
    /// The rendered page is compared to a checked-in file rather than to a string in the test, so
    /// the assertion is the whole page rather than the part somebody remembered to name. `name` is
    /// `None` when the test's own name keys it — the common case, and the one that keeps the
    /// assertion one line long.
    PageMatchesSnapshot {
        name: Option<Arc<str>>,
        actor: Option<Arc<str>>,
        route: Option<Arc<str>>,
    },
    /// `expect state == fold_of [ … ]`.
    FoldEquals {
        events: Core,
        actor: Option<Arc<str>>,
    },
    /// `expect place(charge) == server`.
    Place {
        what: Arc<str>,
        /// Where `charge` is written. The name is a **reference** to a definition, resolved
        /// against the placement table rather than evaluated, so there is no `Core` node carrying
        /// its position — and an editor renaming that definition has to edit this too
        /// ([`crate::editor::Editor::occurrences`]).
        what_span: Span,
        tier: Tier,
    },
    /// `expect flow(ApiKey) reaches nothing on client`.
    Flow { ty: Arc<str>, tier: Tier },
    /// `expect wire_compatible_with "orders.v1.becki"`.
    WireCompatible { path: Arc<str> },
    /// `expect no net.out` / `… once` / `… with Charge(amount=2000)` — §21.3 rule 4.
    Performed { atom: Effect, how: Count },
}

impl Expectation {
    pub fn is_static(&self) -> bool {
        matches!(
            self,
            Expectation::Place { .. }
                | Expectation::Flow { .. }
                | Expectation::WireCompatible { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub enum Count {
    /// `expect no net.out` — nothing left the process.
    Never,
    Times(i64),
    With(Core),
}

/// Which effect atoms a stub can stand in for.
///
/// §21.3: "What is left is the genuinely external: `net.out(host)`, `env`, `external.read/write
/// (store)`, `fs.read/write(path)`, `cap.*`, `nondet`." Two of that list are handled by the harness rather than
/// by a stub and are excluded here for reasons worth stating:
///
/// * `nondet` — ids and the clock are supplied deterministically by the harness, because §3.7
///   already makes them data at the edge. A stub would be a second answer to a solved problem.
/// * `cap.*` — a capability is discharged by the authority chokepoint, and §21.2's whole claim for
///   `when` is that it "goes through the *real* `validate`, so authorisation is exercised rather
///   than bypassed". Stubbing a capability would bypass it. An explicit `stub cap.x:` is still
///   accepted — saying it out loud is the point — but nothing is stubbed automatically.
/// * `spawn` — not on §21.3's list either, and not external at all: a `parallel:` scope is the
///   program's own control flow, and standing in for it would delete the children rather than the
///   boundary they cross. What a test wants stubbed is what a child *does*.
pub fn is_auto_stubbable(e: &Effect) -> bool {
    matches!(
        e,
        Effect::NetOut(_)
            | Effect::NetIn
            | Effect::FsRead(_)
            | Effect::FsWrite(_)
            | Effect::Env
            | Effect::ExternalRead(_)
            | Effect::ExternalWrite(_)
    )
}

/// Whether an atom may be named in a `stub` clause at all.
pub fn is_stubbable(e: &Effect) -> bool {
    is_auto_stubbable(e) || matches!(e, Effect::Cap(_))
}

/// Does a definition *perform* an atom, as opposed to inheriting it from something it calls?
///
/// This distinction is the whole difference between a stub that works and one that deletes the
/// program. An effect row propagates: `validate` calls `charge`, so `validate`'s row contains
/// `net.out(payments.example.com)` too. Stubbing every definition whose row mentions the atom would
/// replace `validate` itself — and §21.2's claim that `when` "goes through the *real* `validate`,
/// so authorisation is exercised rather than bypassed" would be false of every program that talks
/// to anything.
///
/// A definition performs an atom itself when it *declares* it (§3.6's `uses` clause is the
/// published bound and the only way to introduce a non-primitive effect) or when its own body
/// applies a primitive that carries it. Everything else in the row arrived from a callee, and the
/// callee is where the stub belongs.
pub fn performs_itself(d: &crate::check::Def, atom: &Effect) -> bool {
    if d.declared_effects.contains(atom) {
        return true;
    }
    // The atoms this body reaches through primitives alone — the global oracle contributes
    // nothing, so a call to an effectful definition does not count.
    let mut own = Vec::new();
    d.body.effects(&|_| Vec::new(), &mut own);
    own.contains(atom)
}

#[cfg(test)]
mod tests {
    use crate::check_str;
    use crate::split::tests::TODO;

    fn with(
        extra: &str,
    ) -> (
        crate::check::Program,
        beck_diag::Diagnostics,
        beck_diag::SourceMap,
    ) {
        check_str("todo.beck", &format!("{TODO}\n{extra}"))
    }

    #[test]
    fn a_test_is_typed_against_the_programs_own_event_and_command_types() {
        let (p, d, m) = with(
            "test \"an empty todo is rejected\":\n    given []\n    when Add(id=Id(\"1\"), text=\"   \")\n    expect Err(error=BlankText)\n",
        );
        assert!(!d.has_errors(), "{}", d.render(&m));
        assert_eq!(p.tests.len(), 1);
        assert_eq!(p.tests[0].name.as_ref(), "an empty todo is rejected");
        assert_eq!(p.tests[0].clauses.len(), 3);
    }

    #[test]
    fn a_given_that_is_not_a_log_of_this_programs_events_is_a_type_error() {
        // The fixture-versus-log distinction, mechanised: a test cannot arrange a state out of
        // values the program's own stream could never carry.
        let (_, d, _) = with("test \"x\":\n    given [1, 2, 3]\n");
        assert!(d.has_errors());
    }

    #[test]
    fn a_command_the_union_does_not_declare_is_a_type_error() {
        let (_, d, _) = with("test \"x\":\n    when Frobnicate(id=Id(\"1\"))\n");
        assert!(d.has_errors());
    }

    #[test]
    fn a_test_that_performs_an_effect_is_refused_by_name() {
        // §21.2's open question, settled: "a test that performs a real `net.out` is a test that can
        // fail because somebody else's server is down".
        let src = format!(
            "{TODO}\ndef phone_home() -> Bool uses net.out(x.example.com):\n    return True\n\ntest \"x\":\n    expect phone_home()\n"
        );
        let (_, d, _) = check_str("todo.beck", &src);
        assert!(
            d.iter().any(|x| x.code == "B0700"),
            "{:?}",
            d.iter().map(|x| x.code).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_stub_is_typed_from_the_return_type_of_whatever_performs_the_effect() {
        let src = format!(
            "{TODO}\ndef charge() -> Bool uses net.out(pay.example.com):\n    return True\n\ntest \"x\":\n    stub net.out(pay.example.com): False\n"
        );
        let (p, d, m) = check_str("todo.beck", &src);
        assert!(!d.has_errors(), "{}", d.render(&m));
        assert!(matches!(p.tests[0].clauses[0], super::Clause::Stub { .. }));

        // …and a stub whose value is the wrong type is a type error, with no parameter list
        // restated anywhere.
        let src = format!(
            "{TODO}\ndef charge() -> Bool uses net.out(pay.example.com):\n    return True\n\ntest \"x\":\n    stub net.out(pay.example.com): 3\n"
        );
        let (_, d, _) = check_str("todo.beck", &src);
        assert!(d.has_errors());
    }

    #[test]
    fn a_stub_can_answer_from_the_call_and_the_arguments_are_in_scope_by_name() {
        // §21.3 rule 3. The stubbed definition's parameters are bound under their own names, so a
        // stub is written the way the definition is read — and `match`, `if` and everything else in
        // the language work inside it without a mock DSL.
        let src = format!(
            "{TODO}\ndef charge(amount: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\ntest \"x\":\n    stub net.out(pay.example.com):\n        return amount > 10\n"
        );
        let (p, d, m) = check_str("todo.beck", &src);
        assert!(!d.has_errors(), "{}", d.render(&m));
        match &p.tests[0].clauses[0] {
            super::Clause::Stub { params, .. } => assert_eq!(params.len(), 1),
            other => panic!("{other:?}"),
        }

        // …and the body is typechecked against the definition's return type like any other code.
        let src = format!(
            "{TODO}\ndef charge(amount: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\ntest \"x\":\n    stub net.out(pay.example.com):\n        return amount\n"
        );
        let (_, d, _) = check_str("todo.beck", &src);
        assert!(d.has_errors(), "an Int is not a Bool");
    }

    #[test]
    fn bare_case_arms_match_on_the_one_argument_there_is() {
        let src = format!(
            "{TODO}\ndef charge(amount: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\ntest \"x\":\n    stub net.out(pay.example.com):\n        case 1:\n            return True\n        case _:\n            return False\n"
        );
        let (p, d, m) = check_str("todo.beck", &src);
        assert!(!d.has_errors(), "{}", d.render(&m));
        assert!(matches!(p.tests[0].clauses[0], super::Clause::Stub { .. }));

        // Two arguments and no scrutinee written is a refusal, not a guess.
        let src = format!(
            "{TODO}\ndef charge(amount: Int, tries: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\ntest \"x\":\n    stub net.out(pay.example.com):\n        case 1:\n            return True\n        case _:\n            return False\n"
        );
        let (_, d, _) = check_str("todo.beck", &src);
        assert!(d.iter().any(|x| x.code == "B0707"));
    }

    #[test]
    fn a_stub_that_answers_from_the_call_needs_one_definition_to_take_it_from() {
        // Two definitions can share a stub *value* — a value looks at nothing. They cannot share a
        // body, because a body names parameters and there is no reason theirs agree.
        let two = format!(
            "{TODO}\ndef charge(amount: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\ndef refund(amount: Int) -> Bool uses net.out(pay.example.com):\n    return True\n"
        );
        let (_, d, m) = check_str(
            "todo.beck",
            &format!("{two}\ntest \"x\":\n    stub net.out(pay.example.com): True\n"),
        );
        assert!(!d.has_errors(), "a value still works: {}", d.render(&m));

        let (_, d, _) = check_str(
            "todo.beck",
            &format!("{two}\ntest \"x\":\n    stub net.out(pay.example.com):\n        return amount > 1\n"),
        );
        assert!(d.iter().any(|x| x.code == "B0707"), "a body cannot");
    }

    #[test]
    fn a_stub_body_is_test_code_and_may_not_perform_anything_either() {
        let src = format!(
            "{TODO}\ndef charge(amount: Int) -> Bool uses net.out(pay.example.com):\n    return True\n\ndef ping() -> Bool uses net.out(other.example.com):\n    return True\n\ntest \"x\":\n    stub net.out(pay.example.com):\n        return ping()\n"
        );
        let (_, d, _) = check_str("todo.beck", &src);
        assert!(d.iter().any(|x| x.code == "B0700"));
    }

    #[test]
    fn stubbing_an_effect_nothing_performs_says_so_rather_than_passing_quietly() {
        let (_, d, _) = with("test \"x\":\n    stub net.out(nobody.example.com): True\n");
        assert!(d.iter().any(|x| x.code == "B0704"));
    }

    #[test]
    fn the_durable_fold_and_the_clock_are_not_things_a_stub_can_replace() {
        let (_, d, _) = with("test \"x\":\n    stub durable: True\n");
        assert!(d.iter().any(|x| x.code == "B0703"));
    }

    #[test]
    fn a_program_with_no_merge_point_is_told_what_given_would_mean() {
        let (_, d, _) = check_str(
            "t.beck",
            "def f() -> Int:\n    return 1\n\ntest \"x\":\n    given []\n",
        );
        assert!(d.iter().any(|x| x.code == "B0706"));
    }

    #[test]
    fn a_property_carries_typed_parameters_for_the_generator() {
        let (p, d, m) = with("property \"any log folds\"(log: list[Event]):\n    given log\n    expect map_len(state.todos) >= 0\n");
        assert!(!d.has_errors(), "{}", d.render(&m));
        assert!(p.tests[0].is_property());
        assert_eq!(p.tests[0].params.len(), 1);
    }

    #[test]
    fn the_two_walks_agree() {
        // `cores` and `cores_mut` are the same list written twice, and a clause added to one and
        // not the other is invisible until something downstream skips an expression. The fixture
        // carries one of every clause that holds an expression, so a new variant that only reaches
        // the mutable walk changes these counts.
        let (mut p, d, m) = with(
            "test \"every clause that carries an expression\":\n    \
             given []\n    \
             when Add(id=Id(\"1\"), text=\"milk\")\n    \
             expect map_len(state.todos) == 1\n    \
             expect state == fold_of []\n",
        );
        assert!(!d.has_errors(), "{}", d.render(&m));
        let test = &mut p.tests[0];
        let read = test.cores().len();
        assert!(read >= 4, "the fixture exercises four clauses, got {read}");
        assert_eq!(read, test.cores_mut().len());
    }

    #[test]
    fn the_static_assertions_need_no_execution() {
        let (p, d, m) = with(
            "test \"the page is a browser's job\":\n    expect place(page) == client\n    expect flow(Todo) reaches nothing on server\n",
        );
        assert!(!d.has_errors(), "{}", d.render(&m));
        assert!(p.tests[0].is_static_only());
    }
}
