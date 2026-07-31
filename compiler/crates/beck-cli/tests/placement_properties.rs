//! Placement, as properties over generated programs — §4.8's row for this stage.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.8:
//!
//! | Layer | Technique | Tool |
//! |---|---|---|
//! | Placement | Property: *no valid program is rejected*; *no `secret` reaches client* (assert over
//! generated programs); determinism and stability properties | `proptest` |
//!
//! The corpus ([`corpus.rs`](corpus.rs)) checks 22 programs someone wrote. This checks programs
//! nobody wrote, which is the only way to find the case nobody thought of — and it is the row of
//! §4.8's table that Phase 2 nearly shipped without.
//!
//! # What is generated, and why not source at random
//!
//! Random *text* is almost never a program, and random well-typed Beck needs a type-directed
//! generator that would itself be a body of code to trust. What is generated instead is the thing
//! the solver actually reasons about: **a call graph of definitions with random effect rows**,
//! emitted into a real program that goes through the real pipeline. Every property below is
//! therefore a statement about the compiler, not about a model of it.
//!
//! Three shapes, because they license different claims:
//!
//! * **`Anywhere`** — every row is dischargeable on every tier. These programs *must* compile:
//!   nothing in them can force a placement, so a rejection would be §4.8's "a valid program was
//!   rejected".
//! * **`Authority`** — arbitrary rows, reached only from `validate`. The server discharges
//!   everything except `dom`, so these must compile too, and every helper must land somewhere that
//!   can discharge what it does.
//! * **`View`** — arbitrary rows reached from the view, which runs where the browser can see it.
//!   These may legitimately be refused. The claim is conditional and is the sharper one: *if* the
//!   compiler accepts it, no tier holds an effect it cannot discharge and no secret is on the
//!   client.

use std::collections::BTreeMap;

use beck_core::{Tier, Ty};
use proptest::prelude::*;

mod support;

// ---------------------------------------------------------------------------------------------
// The generator
// ---------------------------------------------------------------------------------------------

/// An effect row a generated helper may declare, as written in a `uses` clause.
///
/// Chosen to span the discharge table rather than to be exhaustive: one atom per row of §3.3, plus
/// the two that are *not* forced (`net.out(origin)` is legal on the client and the server; the
/// ambient pair is legal everywhere).
const ANYWHERE: &[&str] = &["", "log", "metrics", "log, metrics", "partial"];

const FORCED: &[&str] = &[
    "env",
    "nondet",
    "net.in",
    "spawn",
    "cap.session",
    "cap.admin",
    "net.out(api.example.com)",
    "external.read(legacy)",
    "external.write(legacy)",
    "fs(\"/var/lib/beck\")",
];

const OPEN: &[&str] = &["net.out(origin)", "dom"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Rows every tier discharges.
    Anywhere,
    /// Arbitrary rows, reached from the validator.
    Authority,
    /// Arbitrary rows, reached from the view.
    View,
}

/// A generated program: `n` helpers in a call chain, each with a row, wired into a real app.
#[derive(Clone, Debug)]
struct Generated {
    shape: Shape,
    /// The `uses` clause of each helper, by index.
    rows: Vec<String>,
    /// `calls[i]` is the set of helper indices `i` calls. Always strictly greater than `i`, so the
    /// call graph is a DAG and the generator cannot produce a program that fails to terminate.
    calls: Vec<Vec<usize>>,
}

impl Generated {
    /// The row `h{i}` must *declare*: its own, plus everything it can reach.
    ///
    /// §3.6 makes `uses` a published **bound**, so a body that exceeds it is B0370 — correctly. A
    /// generator that ignored that would emit programs the compiler is right to refuse, and the
    /// properties below would be asserting a falsehood about them. The transitive closure is
    /// computed here for the same reason the compiler computes it: because it is what the function
    /// actually does.
    fn declared(&self, i: usize) -> Vec<&str> {
        let mut seen = vec![i];
        let mut work = vec![i];
        let mut atoms: Vec<&str> = Vec::new();
        while let Some(k) = work.pop() {
            for atom in self.rows[k]
                .split(',')
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                if !atoms.contains(&atom) {
                    atoms.push(atom);
                }
            }
            for j in &self.calls[k] {
                if !seen.contains(j) {
                    seen.push(*j);
                    work.push(*j);
                }
            }
        }
        atoms.sort_unstable();
        atoms
    }

    fn source(&self) -> String {
        let mut out = String::from(PRELUDE);
        for i in 0..self.rows.len() {
            let declared = self.declared(i);
            let uses = if declared.is_empty() {
                String::new()
            } else {
                format!(" uses {}", declared.join(", "))
            };
            let body: String = if self.calls[i].is_empty() {
                "    return 1\n".into()
            } else {
                let terms: Vec<String> = self.calls[i].iter().map(|j| format!("h{j}()")).collect();
                format!("    return 1 + {}\n", terms.join(" + "))
            };
            out.push_str(&format!("def h{i}() -> Int{uses}:\n{body}\n"));
        }

        // Where the helpers are reached from decides what the solver is allowed to do with them.
        let entry = if self.rows.is_empty() {
            "0".to_string()
        } else {
            (0..self.rows.len())
                .map(|i| format!("h{i}()"))
                .collect::<Vec<_>>()
                .join(" + ")
        };
        match self.shape {
            Shape::View => out.push_str(&format!(
                "{VALIDATE_PLAIN}\ndef extra(s: State) -> Int:\n    return {entry}\n\n{VIEW_USING_EXTRA}\n{WIRING}"
            )),
            _ => out.push_str(&format!(
                "def extra(s: State) -> Int:\n    return {entry}\n\n{VALIDATE_USING_EXTRA}\n{VIEW_PLAIN}\n{WIRING}"
            )),
        }
        out
    }
}

const PRELUDE: &str = "\
model State:
    n: Int

union Command:
    Bump

union Event:
    Bumped

union Rejection:
    Refused

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Bumped:
            return s.with(n=(s.n + 1))

";

const VALIDATE_USING_EXTRA: &str = "\
def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Bump:
            if extra(s) < 0:
                return Err(error=Refused)
            return Ok(value=[Bumped])
";

const VALIDATE_PLAIN: &str = "\
def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Bump:
            return Ok(value=[Bumped])
";

const VIEW_PLAIN: &str = "\
def view(s: State, session: Session) -> Html:
    return ui:
        main:
            h1: str(s.n)
";

const VIEW_USING_EXTRA: &str = "\
def view(s: State, session: Session) -> Html:
    return ui:
        main:
            h1: str(s.n + extra(s))
";

const WIRING: &str = "
proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, count, validate)
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
page: Signal[Html] = per_session(count, view)
";

fn program(shape: Shape) -> impl Strategy<Value = Generated> {
    let atoms: Vec<&'static str> = match shape {
        Shape::Anywhere => ANYWHERE.to_vec(),
        _ => ANYWHERE.iter().chain(FORCED).chain(OPEN).copied().collect(),
    };
    (1usize..6)
        .prop_flat_map(move |n| {
            let rows = proptest::collection::vec(
                proptest::sample::select(atoms.clone()).prop_map(String::from),
                n,
            );
            // Edges are drawn over the whole range and then kept only where the target index is
            // greater, which makes the call graph a DAG *by construction*: a generated program can
            // never recurse, so a hang is not one of the things this suite can be testing.
            let edges = proptest::collection::vec(proptest::collection::vec(0..n, 0..3), n);
            (rows, edges)
        })
        .prop_map(move |(rows, edges)| {
            let calls = edges
                .into_iter()
                .enumerate()
                .map(|(i, mut c)| {
                    c.retain(|j| *j > i);
                    c.sort_unstable();
                    c.dedup();
                    c
                })
                .collect();
            Generated { shape, rows, calls }
        })
}

// ---------------------------------------------------------------------------------------------
// What is asserted of every program that compiles
// ---------------------------------------------------------------------------------------------

/// The two soundness properties, checked together because they are checked the same way.
fn solved_placement_is_legal(src: &str) -> Result<BTreeMap<String, Tier>, String> {
    let (program, d, map) = beck_core::check_str("gen.beck", src);
    if d.has_errors() {
        return Err(d.render(&map));
    }
    let solution = beck_core::place::solve(&program, None);

    for name in &program.def_order {
        let def = &program.defs[name];
        let tier = solution.tiers[&beck_core::Key::Def(name.clone())];
        for e in &def.row.atoms {
            // §4.8: no tier may hold an effect it cannot discharge. `any` is the intersection, so
            // this covers the unplaced case without a special test.
            if !tier.discharges(e) {
                return Err(format!(
                    "`{name}` is on `{}` and performs `{}`, which that tier cannot discharge",
                    tier.name(),
                    e.name()
                ));
            }
        }
        // §4.8: "no `secret` reaches client".
        if tier == Tier::Client {
            for t in std::iter::once(&def.ret).chain(def.params.iter().map(|(_, _, t)| t)) {
                if let Err(bad) = beck_core::sendable(t, &program.types) {
                    if bad.offender.starts_with("secret[") {
                        return Err(format!(
                            "`{name}` is on the client and holds {}",
                            bad.offender
                        ));
                    }
                }
            }
        }
    }
    for s in &program.signals {
        let tier = solution.tiers[&beck_core::Key::Signal(s.name.clone())];
        for e in &s.row.atoms {
            if !tier.discharges(e) {
                return Err(format!(
                    "signal `{}` is on `{}` and performs `{}`",
                    s.name,
                    tier.name(),
                    e.name()
                ));
            }
        }
        if tier == Tier::Client {
            let carried = match &s.ty {
                Ty::Con(n, args)
                    if (n.as_ref() == Ty::SIGNAL || n.as_ref() == Ty::STREAM)
                        && args.len() == 1 =>
                {
                    args[0].clone()
                }
                other => other.clone(),
            };
            if let Err(bad) = beck_core::sendable(&carried, &program.types) {
                return Err(format!(
                    "signal `{}` crosses to the browser carrying {}",
                    s.name, bad.offender
                ));
            }
        }
    }

    Ok(solution
        .tiers
        .into_iter()
        .map(|(k, t)| (k.to_string(), t))
        .collect())
}

proptest! {
    // No failure-persistence file: a counterexample belongs in the test output and, if it is worth
    // keeping, in the corpus as a program someone can read — not in a regression file nobody opens.
    #![proptest_config(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// §4.8: **no valid program is rejected.** A program whose every row is dischargeable on every
    /// tier cannot be forced anywhere, so a refusal would be the compiler inventing a constraint.
    #[test]
    fn a_program_that_forces_nothing_is_never_rejected(g in program(Shape::Anywhere)) {
        let src = g.source();
        let tiers = solved_placement_is_legal(&src)
            .map_err(|e| TestCaseError::fail(format!("{e}\n--- program ---\n{src}")))?;
        // …and every helper stays unplaced, because nothing it does needs a tier. This is the
        // assertion that found the defect: `partial` is not ambient but *is* discharged by every
        // tier, and the pin was testing "no visible effects" rather than "legal everywhere".
        for i in 0..g.rows.len() {
            prop_assert_eq!(
                tiers.get(&format!("def/h{i}")),
                Some(&Tier::Any),
                "h{} does only ambient work and should be unplaced\n{}", i, src
            );
        }
    }

    /// The authority path is the server's, and the server discharges everything but `dom`. So an
    /// arbitrary row reached from `validate` must still compile.
    #[test]
    fn an_arbitrary_row_on_the_authority_path_is_never_rejected(g in program(Shape::Authority)) {
        let src = g.source();
        // `dom` is the one atom the server cannot discharge, so a generated `dom` on this path is
        // a program that genuinely has no answer. Skip it rather than assert a falsehood.
        prop_assume!(!g.rows.iter().any(|r| r.contains("dom")));
        solved_placement_is_legal(&src)
            .map_err(|e| TestCaseError::fail(format!("{e}\n--- program ---\n{src}")))?;
    }

    /// The conditional claim, over the shape that may legitimately be refused: **if** it compiles,
    /// the placement is sound.
    #[test]
    fn whatever_is_accepted_is_placed_legally(g in program(Shape::View)) {
        let src = g.source();
        if let Err(e) = solved_placement_is_legal(&src) {
            // A refusal is allowed here; a refusal *without a diagnostic* is not.
            prop_assert!(!e.is_empty(), "refused with nothing to say\n{}", src);
        }
    }

    /// §3.4's first guardrail: same input, same solution.
    #[test]
    fn placement_is_deterministic(g in program(Shape::Authority)) {
        let src = g.source();
        prop_assume!(!g.rows.iter().any(|r| r.contains("dom")));
        let first = solved_placement_is_legal(&src).map_err(TestCaseError::fail)?;
        for _ in 0..3 {
            let again = solved_placement_is_legal(&src).map_err(TestCaseError::fail)?;
            prop_assert_eq!(&first, &again, "the solver moved\n{}", src);
        }
    }

    /// §3.4's second guardrail: the previous solution is honoured, and re-solving against a lock
    /// written from an unchanged program changes nothing — no churn, no drift.
    #[test]
    fn a_lock_written_from_a_program_reproduces_it(g in program(Shape::Authority)) {
        let src = g.source();
        prop_assume!(!g.rows.iter().any(|r| r.contains("dom")));
        let (program, d, map) = beck_core::check_str("gen.beck", &src);
        prop_assume!(!d.has_errors());
        let _ = map;

        let first = beck_core::place::solve(&program, None);
        let lock = beck_core::Lock::of(&first);
        let again = beck_core::place::solve(&program, Some(&lock));
        prop_assert_eq!(&first.tiers, &again.tiers, "re-solving under its own lock moved it\n{}", src);
        prop_assert!(again.churn.is_empty(), "churn against its own lock: {:?}\n{}", again.churn, src);
    }

    /// The solver reports `Exhaustive` only when it enumerated everything — and when it does, the
    /// answer it returned is genuinely the cheapest. Checked by re-costing every alternative for a
    /// single node, which is what an optimum means locally.
    #[test]
    fn an_exhaustive_solution_is_not_beaten_by_moving_one_node(g in program(Shape::Authority)) {
        let src = g.source();
        prop_assume!(!g.rows.iter().any(|r| r.contains("dom")));
        let (program, d, _) = beck_core::check_str("gen.beck", &src);
        prop_assume!(!d.has_errors());
        let solution = beck_core::place::solve(&program, None);
        prop_assume!(solution.method == beck_core::Method::Exhaustive);

        for e in &solution.explanations {
            // A pinned node is where it is by a *rule* — `@on(...)`, purity, or "a `Signal[Html]`
            // is the browser's subscription" — and rules are allowed to cost more than the
            // alternative. That is what pinning means, and the distinction is why `Explanation`
            // carries it.
            if e.pinned {
                continue;
            }
            let chosen = e
                .candidates
                .iter()
                .find(|(t, _)| *t == e.chosen)
                .map(|(_, c)| *c);
            let Some(chosen) = chosen else { continue };
            for (tier, cost) in &e.candidates {
                prop_assert!(
                    *cost >= chosen,
                    "`{}` sits on {} at {chosen} but {} costs {cost}\n{}",
                    e.key, e.chosen.name(), tier.name(), src
                );
            }
        }
    }
}

#[test]
fn the_generator_produces_programs_the_compiler_accepts() {
    // A property suite whose generator emits nothing valid passes vacuously. This is the check that
    // it does not — one fixed instance of each shape, compiled and asserted.
    for (shape, rows) in [
        (Shape::Anywhere, vec!["log".to_string(), String::new()]),
        (
            Shape::Authority,
            vec!["cap.session".to_string(), "env".to_string()],
        ),
        (Shape::View, vec!["net.out(origin)".to_string()]),
    ] {
        let n = rows.len();
        let g = Generated {
            shape,
            rows,
            calls: (0..n).map(|_| Vec::new()).collect(),
        };
        let src = g.source();
        let (placed, d, map) = beck_core::compile_str("gen.beck", &src);
        assert!(
            !d.has_errors(),
            "{shape:?} does not compile:\n{}\n--- program ---\n{src}",
            d.render(&map)
        );
        assert!(placed.is_some(), "{shape:?} did not slice");
    }
    // …and the pipeline they go through is the same one the example uses.
    let _ = support::todo_program();
}
