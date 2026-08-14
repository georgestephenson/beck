//! The `test` and `property` construct, checked.
//!
//! `docs/21-tests-in-beck-and-proof.md` §21.2–§21.3 and `docs/22-phase-3-report.md`: a program
//! asserts its own behaviour in Beck, and `beck test` runs those assertions through the same roles
//! the runtime drives, with no network and no fixture.
//!
//! This is a **deferred** pass, and that is what makes it separable. A test's clauses are typed
//! against the state and event types, which are only known once every signal has been checked —
//! `given` is a `list[Event]`, and `Event` is whatever the program's own `decide` node produces. So
//! [`super::Checker::check_module`] collects `test` and `property` items while walking, finishes
//! everything else, computes the four [`super::TestSubjects`], and only then comes here.
//!
//! A child module of [`super`] rather than a sibling, because it is one pass of the same checker
//! and not a separate one: it needs the substitution, the scopes and the diagnostics, and a private
//! field of `Checker` is visible to a descendant. `docs/22` §22.6 asked for the split.

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_diag::{Diagnostic, Span};
use beck_syntax::{sym, Lit, Node, ScopeSet};

use super::{BindKind, Binding, Checker, Def, SignalDecl, TestSubjects};
use crate::core::{CoreKind, Prim, VarId};
use crate::ty::{Effect, Tier, Ty};

impl Checker<'_> {
    pub(super) fn test_subjects(
        &mut self,
        signals: &[SignalDecl],
        defs: &BTreeMap<Arc<str>, Def>,
    ) -> TestSubjects {
        let find = |op: Prim| -> Option<&SignalDecl> {
            signals
                .iter()
                .find(|s| matches!(&s.expr.kind, CoreKind::Prim { op: o, .. } if *o == op))
        };
        // `state` is the *accumulator*, which is the program's own type when it declares one
        // `durable` fold and the fused record when it declares several — see
        // `docs/23-incremental-views-report.md` §23.3. The checker has to know which before a signal
        // graph exists, so both it and the slicer ask [`crate::signal::durables`].
        let folds = {
            let subst = &self.subst;
            crate::signal::durables(signals, &mut |t| subst.resolve(t))
        };
        let state = match folds.len() {
            0 => None,
            1 => Some(folds[0].1.clone()),
            _ => {
                self.types.insert(
                    Arc::from(crate::signal::FUSED_STATE),
                    crate::signal::fused_state_decl(&folds),
                );
                Some(Ty::con(crate::signal::FUSED_STATE))
            }
        };
        let decide = find(Prim::Decide);
        let event = decide.map(|s| match self.subst.resolve(&s.ty) {
            Ty::Con(n, args)
                if (n.as_ref() == Ty::STREAM || n.as_ref() == Ty::SIGNAL) && args.len() == 1 =>
            {
                args[0].clone()
            }
            other => other,
        });
        // `decide(proposals, state, validate)` — the third argument names the chokepoint, and its
        // return type is what `result` is.
        let result = decide
            .and_then(|s| match &s.expr.kind {
                CoreKind::Prim { args, .. } => args.get(2).cloned(),
                _ => None,
            })
            .and_then(|v| match &v.kind {
                CoreKind::Global(n) => defs.get(n).map(|d| self.subst.resolve(&d.ret)),
                _ => Some(self.subst.resolve(&v.ty)),
            });
        let command = self
            .types
            .contains_key("Command")
            .then(|| Ty::con("Command"));
        TestSubjects {
            state,
            event,
            result,
            command,
        }
    }

    pub(super) fn check_test(
        &mut self,
        item: &Node,
        subjects: &TestSubjects,
        defs: &BTreeMap<Arc<str>, Def>,
    ) -> Option<crate::testing::TestDef> {
        use crate::testing::{Clause, Count, Expectation, TestDef};

        let is_property = item.is_form(sym::PROPERTY);
        let name: Arc<str> = item.args.first()?.as_str_lit().map(Arc::from)?;
        let body = item.args.get(if is_property { 2 } else { 1 })?;
        let span = item.span();

        let before = self.locals.len();

        // A `property`'s parameters are generated (§21.3 rule 5), so they are ordinary bindings
        // with written types — the generator's contract is the type and nothing else.
        let mut params = Vec::new();
        if is_property {
            for p in &item.args[1].args {
                let (target, annot) = if p.is_form(sym::ANNOT) && p.args.len() == 2 {
                    (&p.args[0], Some(&p.args[1]))
                } else {
                    (p, None)
                };
                let Some(s) = target.as_var() else { continue };
                let Some(t) = annot else {
                    self.error(
                        "B0701",
                        format!("`{}` needs a type for the generator to work from", s.name),
                        p.span(),
                    );
                    continue;
                };
                let ty = self.ty_from_node(t);
                let id = self.fresh_var();
                params.push((id, s.name.clone(), ty.clone()));
                self.locals.push(Binding {
                    name: s.name.clone(),
                    scopes: s.scopes.clone(),
                    kind: BindKind::Local(id, ty),
                });
            }
            if params.is_empty() {
                self.error(
                    "B0701",
                    format!("`property {name}` generates nothing"),
                    span,
                );
            }
        }

        // `state`, `events` and `result` — plain data, bound around every expectation.
        let bindings = crate::testing::Bindings {
            state: self.fresh_var(),
            events: self.fresh_var(),
            result: self.fresh_var(),
        };
        let bind = |ck: &mut Self, name: &str, id: VarId, ty: Option<Ty>| {
            if let Some(ty) = ty {
                ck.locals.push(Binding {
                    name: Arc::from(name),
                    scopes: beck_syntax::ScopeSet::empty(),
                    kind: BindKind::Local(id, ty),
                });
            }
        };
        bind(self, "state", bindings.state, subjects.state.clone());
        bind(
            self,
            "events",
            bindings.events,
            subjects.event.clone().map(Ty::list),
        );
        bind(self, "result", bindings.result, subjects.result.clone());

        let (clauses, row) = self.in_scope(|ck| {
            let mut clauses = Vec::new();
            for stmt in &body.args {
                let cspan = stmt.span();
                let clause = match stmt.head_name() {
                    Some(sym::GIVEN) if !stmt.args.is_empty() => {
                        let want = require_subject(
                            ck,
                            subjects.event.clone().map(Ty::list),
                            "given",
                            "`list[Event]`",
                            cspan,
                        );
                        let events = ck.expr(&stmt.args[0], want.as_ref());
                        if let Some(w) = &want {
                            ck.unify(&events.ty, w, events.span, "`given`");
                        }
                        Clause::Given {
                            events,
                            actor: stmt.args.get(1).and_then(|a| a.as_str_lit()).map(Arc::from),
                            span: cspan,
                        }
                    }
                    Some(sym::WHEN) if stmt.args.len() >= 2 => {
                        let want = require_subject(
                            ck,
                            subjects.command.clone(),
                            "when",
                            "a `Command`",
                            cspan,
                        );
                        let commands = stmt.args[1..]
                            .iter()
                            .map(|c| {
                                let core = ck.expr(c, want.as_ref());
                                if let Some(w) = &want {
                                    ck.unify(&core.ty, w, core.span, "`when`");
                                }
                                core
                            })
                            .collect();
                        let (actor, route) = session_slot(&stmt.args[0]);
                        Clause::When {
                            actor,
                            route,
                            commands,
                            span: cspan,
                        }
                    }
                    Some(sym::STUB) if stmt.args.len() == 2 => ck.check_stub(stmt, defs, cspan)?,
                    Some(sym::EXPECT) if stmt.args.len() == 1 => {
                        let e = ck.expr(&stmt.args[0], Some(&Ty::bool_()));
                        ck.unify(&e.ty, &Ty::bool_(), e.span, "`expect`");
                        Clause::Expect {
                            what: Expectation::Holds(e),
                            span: cspan,
                        }
                    }
                    Some(sym::EXPECT_CONTAINS) if !stmt.args.is_empty() => {
                        let needle = ck.expr(&stmt.args[0], Some(&Ty::str_()));
                        ck.unify(&needle.ty, &Ty::str_(), needle.span, "`contains`");
                        let (actor, route) = stmt.args.get(1).map_or((None, None), session_slot);
                        Clause::Expect {
                            what: Expectation::PageContains {
                                needle,
                                actor,
                                route,
                            },
                            span: cspan,
                        }
                    }
                    Some(sym::EXPECT_SNAPSHOT) if stmt.args.len() == 2 => {
                        // Nothing to typecheck: both operands are literals the parser produced,
                        // and the page is the runtime's to render. What the checker owes this
                        // clause is the same thing it owes `expect page contains` — that it is in
                        // a `test` block at all, which the surrounding walk has already decided.
                        let (actor, route) = session_slot(&stmt.args[1]);
                        Clause::Expect {
                            what: Expectation::PageMatchesSnapshot {
                                name: stmt.args[0].as_str_lit().map(Arc::from),
                                actor,
                                route,
                            },
                            span: cspan,
                        }
                    }
                    Some(sym::EXPECT_FOLD) if !stmt.args.is_empty() => {
                        let want = require_subject(
                            ck,
                            subjects.event.clone().map(Ty::list),
                            "fold_of",
                            "`list[Event]`",
                            cspan,
                        );
                        let events = ck.expr(&stmt.args[0], want.as_ref());
                        if let Some(w) = &want {
                            ck.unify(&events.ty, w, events.span, "`fold_of`");
                        }
                        Clause::Expect {
                            what: Expectation::FoldEquals {
                                events,
                                actor: stmt.args.get(1).and_then(|a| a.as_str_lit()).map(Arc::from),
                            },
                            span: cspan,
                        }
                    }
                    Some(sym::EXPECT_PLACE) if stmt.args.len() == 2 => {
                        let what: Arc<str> = stmt.args[0].as_var()?.name.clone();
                        let what_span = stmt.args[0].span();
                        let tier = ck.test_tier(&stmt.args[1])?;
                        Clause::Expect {
                            what: Expectation::Place {
                                what,
                                what_span,
                                tier,
                            },
                            span: cspan,
                        }
                    }
                    Some(sym::EXPECT_FLOW) if stmt.args.len() == 2 => {
                        let ty: Arc<str> = stmt.args[0].as_var()?.name.clone();
                        let tier = ck.test_tier(&stmt.args[1])?;
                        Clause::Expect {
                            what: Expectation::Flow { ty, tier },
                            span: cspan,
                        }
                    }
                    Some(sym::EXPECT_WIRE) if stmt.args.len() == 1 => Clause::Expect {
                        what: Expectation::WireCompatible {
                            path: Arc::from(stmt.args[0].as_str_lit().unwrap_or_default()),
                        },
                        span: cspan,
                    },
                    Some(sym::EXPECT_EFFECT) if stmt.args.len() == 2 => {
                        let Some(atom) = ck.test_atom(&stmt.args[0], cspan) else {
                            continue;
                        };
                        let how = &stmt.args[1];
                        let how = match how.head_name() {
                            Some("times") if how.args.len() == 1 => match how.args[0].as_lit() {
                                Some(Lit::Int(n)) => Count::Times(*n),
                                _ => Count::Times(1),
                            },
                            Some("with") if how.args.len() == 1 => {
                                Count::With(ck.expr(&how.args[0], None))
                            }
                            _ => Count::Never,
                        };
                        Clause::Expect {
                            what: Expectation::Performed { atom, how },
                            span: cspan,
                        }
                    }
                    _ => {
                        ck.diags.push(
                            Diagnostic::error(
                                "B0705",
                                "only `given`, `when`, `stub` and `expect` may appear in a test",
                                cspan,
                            )
                            .with_note(
                                "§21.2: a test names a log, an input and an expectation — there is \
                                 no fixture to build and no `setUp` to write",
                            ),
                        );
                        continue;
                    }
                };
                clauses.push(clause);
            }
            Some(clauses)
        });
        self.locals.truncate(before);
        let clauses = clauses?;

        // §21.2's open question, settled as an error: "a test that performs a real `net.out` is a
        // test that can fail because somebody else's server is down".
        let leaked: Vec<Effect> = self
            .subst
            .resolve_row(&row)
            .atoms
            .iter()
            // `spawn` is not one of these. The rule's reason is that a test must not depend on
            // anything outside itself, and a `parallel:` scope is the program's own control flow —
            // it crosses no boundary, reaches no host, and is the one atom on §3.3's list that
            // `beck_core::testing` will not stand in for, because a stub would delete the
            // children rather than the thing they call.
            .filter(|e| !e.is_ambient() && **e != Effect::Spawn)
            .cloned()
            .collect();
        if !leaked.is_empty() {
            let names: Vec<String> = leaked.iter().map(|e| e.name()).collect();
            self.diags.push(
                Diagnostic::error(
                    "B0700",
                    format!("`test {name}` performs {}", names.join(", ")),
                    span,
                )
                .with_primary_label("a test block's own row must be empty")
                .with_note(
                    "an expectation is a pure question about a state, a log and a page; effects \
                     belong to the *subject*, and §21.3 stubs those",
                ),
            );
        }

        Some(TestDef {
            name,
            params,
            clauses,
            bindings,
            span,
        })
    }

    fn check_stub(
        &mut self,
        stmt: &Node,
        defs: &BTreeMap<Arc<str>, Def>,
        span: Span,
    ) -> Option<crate::testing::Clause> {
        let atom = self.test_atom(&stmt.args[0], span)?;
        if !crate::testing::is_stubbable(&atom) {
            self.diags.push(
                Diagnostic::error(
                    "B0703",
                    format!("`{}` is not something a stub can stand in for", atom.name()),
                    span,
                )
                .with_note(
                    "time, ids and persistence are not stubbed in Beck and there is nothing to \
                     write: the clock is data on the envelope, ids are minted at the edge, and the \
                     durable fold is real and in memory",
                ),
            );
            return None;
        }

        // The stub's type is the return type of what performs the effect — §21.3's whole claim:
        // "no parameter list, because parameters are not how the stub is selected".
        //
        // *Performs*, not *mentions*: a row propagates to callers, so `validate` inherits its
        // payment gateway's `net.out`. See [`crate::testing::performs_itself`] for why stubbing the
        // caller would be a bug rather than a broader match.
        let performers: Vec<&Def> = defs
            .values()
            .filter(|d| crate::testing::performs_itself(d, &atom))
            .collect();
        let mut returns: Vec<(&Arc<str>, Ty)> = Vec::new();
        for d in &performers {
            let ret = self.subst.resolve(&d.ret);
            if !returns.iter().any(|(_, t)| *t == ret) {
                returns.push((&d.name, ret));
            }
        }

        let body = &stmt.args[1];
        let answers_from_the_call = body.is_form(sym::STUB_ARMS) || body.is_form(sym::DO);

        if performers.is_empty() {
            self.diags.push(
                Diagnostic::error(
                    "B0704",
                    format!("nothing in this program performs `{}`", atom.name()),
                    span,
                )
                .with_primary_label("this stub would never be reached")
                .with_note(
                    "the complete list of what a program touches is its effect rows, and this \
                     atom is not among them",
                ),
            );
            let value = self.expr(body_expr_of(body), None);
            return Some(crate::testing::Clause::Stub {
                atom,
                params: Vec::new(),
                value,
                span,
            });
        }

        // §21.3 rule 3: a stub that answers *from* the call needs one call to answer from. Two
        // definitions performing one atom can share a *value*, because a value does not look at
        // anything; they cannot share a body, because the body names parameters and there is no
        // reason theirs agree. The fix is the one the effect vocabulary already offers — a second
        // host, a second store — and the diagnostic says so.
        if answers_from_the_call && performers.len() > 1 {
            let names: Vec<String> = performers.iter().map(|d| format!("`{}`", d.name)).collect();
            self.diags.push(
                Diagnostic::error(
                    "B0707",
                    format!(
                        "`{}` is performed by more than one definition, so a stub cannot answer \
                         from the call",
                        atom.name()
                    ),
                    span,
                )
                .with_primary_label(format!(
                    "{} {} perform it",
                    names.join(", "),
                    if performers.len() == 2 { "both" } else { "all" }
                ))
                .with_note(
                    "a stub that matches on arguments has to know whose arguments they are; a \
                     stub that is a plain value does not, and still works here",
                )
                .with_fix(
                    "give the one you mean its own atom — a second host or a second store — or \
                     stub a value instead of a block",
                ),
            );
            return None;
        }

        let want = if returns.len() == 1 {
            Some(returns[0].1.clone())
        } else {
            let names: Vec<String> = returns
                .iter()
                .map(|(n, t)| format!("`{n}` returns {t}"))
                .collect();
            self.diags.push(
                Diagnostic::error(
                    "B0704",
                    format!(
                        "`{}` is performed by definitions with different return types",
                        atom.name()
                    ),
                    span,
                )
                .with_primary_label(names.join("; "))
                .with_note(
                    "one stub is one value for one effect, so the effect has to have one \
                     answer — split the atom (a second host, a second store) or stub nothing \
                     and let the canonical inhabitant stand in",
                ),
            );
            None
        };

        if !answers_from_the_call {
            let value = self.expr(body, want.as_ref());
            if let Some(w) = &want {
                self.unify(&value.ty, w, value.span, "the stub's value");
            }
            return Some(crate::testing::Clause::Stub {
                atom,
                params: Vec::new(),
                value,
                span,
            });
        }

        // The block form. The stubbed definition's parameters come into scope under their own
        // names, so the stub is written the way the definition is read — and `match`, `if`, and
        // every other expression in the language work inside it without a mock DSL.
        let target = performers[0];
        let before = self.locals.len();
        let mut params = Vec::new();
        for (_, pname, pty) in &target.params {
            let id = self.fresh_var();
            let pty = self.subst.resolve(pty);
            params.push(id);
            self.locals.push(Binding {
                name: pname.clone(),
                scopes: ScopeSet::empty(),
                kind: BindKind::Local(id, pty),
            });
        }

        let value = if body.is_form(sym::STUB_ARMS) {
            // `case` arms with no scrutinee written: the scrutinee is the parameter, which only
            // the compiler knows. A definition with two of them has to say which.
            if target.params.len() != 1 {
                let names: Vec<String> = target
                    .params
                    .iter()
                    .map(|(_, n, t)| format!("`{n}: {t}`"))
                    .collect();
                self.diags.push(
                    Diagnostic::error(
                        "B0707",
                        format!(
                            "`{}` takes {} arguments, so bare `case` arms do not say what to \
                             match on",
                            target.name,
                            target.params.len()
                        ),
                        span,
                    )
                    .with_primary_label(if names.is_empty() {
                        "it takes none".to_string()
                    } else {
                        names.join(", ")
                    })
                    .with_fix("write the `match` out: `match <argument>:` inside the stub"),
                );
                self.locals.truncate(before);
                return None;
            }
            let scrutinee = Node::sym(target.params[0].1.as_ref(), span);
            let mut arms = vec![scrutinee];
            arms.extend(body.args.iter().cloned());
            let as_match = Node::form(sym::MATCH, arms, span);
            self.expr(&as_match, want.as_ref())
        } else {
            self.block(&body.args, want.as_ref())
        };
        self.locals.truncate(before);
        if let Some(w) = &want {
            self.unify(&value.ty, w, value.span, "the stub's value");
        }
        Some(crate::testing::Clause::Stub {
            atom,
            params,
            value,
            span,
        })
    }

    fn test_atom(&mut self, n: &Node, span: Span) -> Option<Effect> {
        let text = n.as_str_lit().unwrap_or_default();
        match Effect::parse(text) {
            Some(e) => Some(e),
            None => {
                self.error("B0702", format!("`{text}` is not an effect atom"), span);
                None
            }
        }
    }

    fn test_tier(&mut self, n: &Node) -> Option<Tier> {
        let name = n.as_var()?.name.clone();
        match Tier::parse(&name) {
            Some(t) => Some(t),
            None => {
                self.error("B0702", format!("`{name}` is not a tier"), n.span());
                None
            }
        }
    }
}

/// The expression inside a stub whose atom nothing performs, so that a second error is not stacked
/// on the first. A block has no single expression, and `unit` is as good an answer as any when the
/// clause has already been refused.
fn body_expr_of(body: &Node) -> &Node {
    if body.is_form(sym::STUB_ARMS) || body.is_form(sym::DO) {
        body.args.first().unwrap_or(body)
    } else {
        body
    }
}

/// A clause that needs a type the program does not have — `given` in a program with no event
/// stream — is one error here rather than four confusing ones downstream.
fn require_subject(
    ck: &mut Checker<'_>,
    ty: Option<Ty>,
    clause: &str,
    what: &str,
    span: Span,
) -> Option<Ty> {
    if ty.is_none() {
        ck.diags.push(
            Diagnostic::error(
                "B0706",
                format!("`{clause}` needs {what}, and this program does not have one"),
                span,
            )
            .with_note(
                "the state a test arranges is a fold over the program's own event stream, so a \
                 program with no `merge_clients` → `decide` → `durable(fold(…))` has nothing for \
                 `given` and `when` to mean",
            ),
        );
    }
    ty
}

/// A test's session slot: who, and where.
///
/// One node with two shapes — a bare actor, or `(at "ana" "/done")` — and both ends of it are
/// optional, because `expect page contains` names neither. Reading it in one place is what keeps
/// the three clauses that have a session slot from disagreeing about what one is.
fn session_slot(n: &Node) -> (Option<Arc<str>>, Option<Arc<str>>) {
    if n.is_form(sym::AT) && n.args.len() == 2 {
        return (
            n.args[0].as_str_lit().map(Arc::from),
            n.args[1].as_str_lit().map(Arc::from),
        );
    }
    (n.as_str_lit().map(Arc::from), None)
}
