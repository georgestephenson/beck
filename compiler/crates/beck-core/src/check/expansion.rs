//! Expanding a typed macro, and what the module is charged for having done so.
//!
//! A `typed macro` is expanded here rather than in `beck_macro` because its body asks what its
//! arguments *are*, and until the checker has run there is no answer (§2.4). The order is the
//! whole feature — infer the arguments, hand the macro what they are, check the code it wrote —
//! and it is also where a nested call was being paid for twice.
//!
//! # The doubling, and which half of it is a defect
//!
//! `Checker::typed_macro` infers a call's arguments inside a **probe** that rolls itself back, and
//! then checks whatever the macro wrote — which contains those same arguments. So a call whose
//! argument is another typed-macro call expanded that argument twice, nesting `d` deep cost `2^d`
//! expansions, and **every one of them was charged** against F17's module-wide production budget.
//! The budget is a bound on what expansion *produces* ([`docs/42`](../../../../../../docs/42-security-assurance.md)
//! §42.6) and the probe's output is thrown away, so a fifteen-deep nest of a macro producing three
//! nodes — forty-five nodes of program — was refused for producing ninety-eight thousand.
//!
//! The exponent itself is not always wrong. A macro that writes `[$x, $x]` **really does** produce
//! `2^d` nodes, and really does owe them. What separates the two cases is not the call site but
//! how many places the expansion ends up in, so the charge is **per use** and the memo below
//! saves only the work of computing the same expansion again:
//!
//! - the same call site at the same argument types produces the same expansion, so it is expanded
//!   once and remembered;
//! - every time the checker is *handed* that expansion to walk, it is charged — a memo hit
//!   included, because a second copy in the program is a second copy;
//! - except inside a probe, where a hit answers with the type it answered with before and the
//!   expansion is not walked at all. There is nothing to charge, because nothing was produced.
//!
//! That last one is what turns the `2^d` into `2d`: a probe wants a *type*, and the type of this
//! call site at these argument types is already known. It is safe there and nowhere else — a probe
//! discards diagnostics, effects and bindings on purpose, which is exactly the difference between
//! reusing an inferred answer and reusing the `Core` that carried it.
//!
//! # A file of its own
//!
//! [`docs/08`](../../../../../../docs/08-roadmap.md) §8.5.5 keeps Lane A serial because two
//! language features rewrite `check/mod.rs` and `ty.rs` together, and names the mitigation that
//! works: put the new logic beside those files rather than in them. `typed_macro` and its probe
//! moved here whole, so `check/mod.rs` is *shorter* for having grown a memo.

use std::collections::BTreeMap;

use beck_diag::{Diagnostic, Span};
use beck_syntax::Node;

use super::{argument_expr, Checker};
use crate::core::{Const, Core, CoreKind};
use crate::ty::Ty;
use beck_macro::TyRepr;

/// One remembered expansion of one call.
struct Expansion {
    /// The call as written, and the reason a span is not the key on its own: **macro-generated
    /// code borrows the span it was expanded from**, so `json_of(here.$named)` in a template is
    /// one span and one call site no matter how many fields it is instantiated for. Two of those
    /// are different calls that happen to be written in the same place, and telling them apart is
    /// what [`Node`]'s equality is — structural, and comparing the hygiene scopes on every symbol,
    /// so two clones of one written call are the same call and two instantiations of one template
    /// are not.
    call: Node,
    /// What the body was told about **the syntax it was given** — the other half of the key. The
    /// spans inside the call and no others: a macro reads `node_ty` of a parameter and of what
    /// `node_args` reaches from one, all of which the call site wrote, and none of which is in the
    /// template it is about to instantiate. Held as pairs rather than as a hash because [`TyRepr`]
    /// is comparable and a call site has one entry in all but the generic case.
    types: Vec<(Span, TyRepr)>,
    /// What the macro wrote, after any ordinary macro inside it has expanded too.
    out: Node,
    /// The type checking that expansion arrived at, when it is a type a second check could not
    /// disagree with. `None` when it still holds variables: reusing those would let one call
    /// site's inference reach another's, and a probe that falls through to the ordinary path is
    /// only slower.
    ty: Option<Ty>,
    /// What the walk of that expansion told the enclosing probe about the call's own syntax, so a
    /// hit tells it the same thing. Resolved when it was stored, so replaying it cannot depend on
    /// what has been unified since. `None` when the walk did not happen inside a probe and there
    /// is nothing to replay — an entry made that way answers a later probe with nothing, so it
    /// does not answer one at all.
    recorded: Option<Vec<(Span, Ty)>>,
}

/// Every expansion a module has produced, by the call site that produced it.
#[derive(Default)]
pub(super) struct Expansions {
    by_call: BTreeMap<Span, Vec<Expansion>>,
}

impl Expansions {
    /// The expansion this call produced at these argument types, if it has.
    ///
    /// Bucketed by span so that the structural comparison is made against the handful of calls
    /// written in one place rather than against every call in the module.
    fn get(&self, at: Span, call: &Node, types: &[(Span, TyRepr)]) -> Option<&Expansion> {
        self.by_call
            .get(&at)?
            .iter()
            .find(|e| e.types == types && &e.call == call)
    }

    fn insert(&mut self, at: Span, entry: Expansion) {
        self.by_call.entry(at).or_default().push(entry);
    }
}

impl Checker<'_> {
    /// Expand one `typed macro` call and check what it produced.
    ///
    /// The order is the whole feature: infer the arguments, hand the macro what they are, check
    /// the code it wrote. The expansion is checked with the *caller's* expectation, so a typed
    /// macro is an expression like any other and inference flows through it.
    pub(super) fn typed_macro(&mut self, n: &Node, expected: Option<&Ty>, span: Span) -> Core {
        let unit = |ck: &mut Self| Core::new(CoreKind::Const(Const::Unit), ck.subst.fresh(), span);
        if self.typed_depth >= beck_macro::MAX_DEPTH {
            self.diags.push(
                Diagnostic::error("B0201", "macro expansion did not terminate", span)
                    .with_primary_label("expanded past the depth limit")
                    .with_note(format!(
                        "the limit is {} nested expansions, and a typed macro's nest through the \
                         checker rather than through the expander: each one emits a call the check \
                         of its own answer meets",
                        beck_macro::MAX_DEPTH
                    )),
            );
            return unit(self);
        }
        // A budget that has run out has already been reported, and everything after it expands to
        // nothing. Walking further would be spending the rest of the compile on a refused program.
        if self.typed.exhausted() {
            return unit(self);
        }
        let types = self.probe(&n.args, span);

        // Inside a probe, the answer to "what is this call's type" is the answer this call site
        // gave last time it was asked with these argument types. Nothing is produced and nothing
        // is walked, which is what stops a nested call from costing `2^d`.
        if self.probe.is_some() {
            if let Some(known) = self.remembered(span, n, &types) {
                return known;
            }
        }

        // Three disjoint fields either way, so the expander is moved out for the call rather than
        // borrowed beside them.
        let mut typed = std::mem::take(&mut self.typed);
        let (node, remember) = match self.expansions.get(span, n, &types) {
            // Produced before, and going into the program again. The body is not run again; the
            // nodes are charged again, because two copies of an expansion are two copies.
            Some(entry) => {
                let out = entry.out.clone();
                let fits = typed.charge_expansion(&out, span, self.diags);
                (fits.then_some(out), false)
            }
            None => (typed.expand(n, &self.typed_env, self.diags), true),
        };
        self.typed = typed;
        let Some(node) = node else {
            return unit(self);
        };

        let mark = self.probe.as_ref().map_or(0, Vec::len);
        self.typed_depth += 1;
        let core = self.expr(&node, expected);
        self.typed_depth -= 1;
        if remember {
            let recorded = self.probe.as_ref().map(|rec| {
                // By span, and the *last* record for one wins, exactly as the walk itself resolves
                // it: an expansion that contains two copies of a call records the same spans
                // twice, and keeping both would make what one hit replays twice what the hit
                // below it replayed — the compounding this memo exists to remove, moved from the
                // expansions to the notes about them.
                let kept: BTreeMap<Span, Ty> = rec[mark..]
                    .iter()
                    .filter(|(s, _)| within(*s, span))
                    .map(|(s, t)| (*s, self.subst.resolve(t)))
                    .collect();
                kept.into_iter().collect::<Vec<_>>()
            });
            let ty = self.subst.resolve(&core.ty);
            let ty = is_settled(&ty).then_some(ty);
            self.expansions.insert(
                span,
                Expansion {
                    call: n.clone(),
                    types,
                    out: node,
                    ty,
                    recorded,
                },
            );
        }
        core
    }

    /// This call site's own answer, replayed for a probe that has asked for it again.
    ///
    /// The enclosing probe is told what the walk told it the first time, so a macro body reading
    /// `node_ty` of anything inside this call sees what it would have seen. `None` when the
    /// expansion has not been produced yet, or when its type still holds variables — those are the
    /// cases where answering from memory would be answering a different question.
    fn remembered(&mut self, span: Span, call: &Node, types: &[(Span, TyRepr)]) -> Option<Core> {
        let entry = self.expansions.get(span, call, types)?;
        let ty = entry.ty.clone()?;
        let recorded = entry.recorded.clone()?;
        if let Some(rec) = &mut self.probe {
            rec.extend(recorded);
        }
        // The `Core` is the one thing a probe does not keep: it is inferred and thrown away, and
        // what the caller unifies against is the type. Building the real one would mean walking the
        // expansion, which is the work this exists to skip.
        Some(Core::new(CoreKind::Const(Const::Unit), ty, span))
    }

    /// Infer a typed macro call's arguments, and forget everything about having done so.
    ///
    /// Everything except the types: the diagnostics, the effects and the bindings are rolled back,
    /// because the arguments are checked again — inside whatever the macro wrote — and a macro that
    /// *discards* an argument must not leave that argument's effects on the definition's row.
    /// Inference itself is not rolled back, and cannot be: a unification the arguments force is one
    /// the real check would force too.
    ///
    /// Returns what the body will be told, which is also half the key an expansion is remembered
    /// by: the same call site told the same thing writes the same code.
    fn probe(&mut self, args: &[Node], call: Span) -> Vec<(Span, TyRepr)> {
        let outer = self.probe.replace(Vec::new());
        let exhausted_before = self.typed.exhausted();
        let diags_mark = self.diags.len();
        let row_before = self.row.clone();
        let locals_before = self.locals.len();
        let siblings_before = self.parallel_siblings.len();
        for a in args {
            let _ = self.expr(argument_expr(a), None);
        }
        let recorded = std::mem::replace(&mut self.probe, outer).unwrap_or_default();
        self.parallel_siblings.truncate(siblings_before);
        self.locals.truncate(locals_before);
        self.row = row_before;
        self.diags.truncate(diags_mark);
        // One report does **not** roll back. An argument that is itself a typed macro call expands
        // here, and expansion draws on a module-wide budget that is spent once and refused once —
        // so discarding that refusal would leave the only report there will ever be deleted, every
        // expansion afterwards producing nothing, and a program silently checked as `unit`.
        if !exhausted_before && self.typed.exhausted() {
            let typed = std::mem::take(&mut self.typed);
            typed.report_exhaustion(call, self.diags);
            self.typed = typed;
        }

        self.typed_env.clear_nodes();
        // Keyed by span rather than accumulated in visit order: the map is what a body reads, a
        // later record for a span replaces an earlier one exactly as [`beck_macro::TypeEnv`] does,
        // and two walks that visited the same syntax in a different order have to agree or the
        // memo they key is not a memo.
        let mut types: BTreeMap<Span, TyRepr> = BTreeMap::new();
        for (span, ty) in recorded {
            let ty = self.subst.resolve(&ty);
            let repr = self.repr(&ty);
            self.typed_env.record(span, repr.clone());
            if within(span, call) {
                types.insert(span, repr);
            }
        }
        types.into_iter().collect()
    }
}

/// Is this span inside that one — part of the syntax the call was written with?
///
/// What a macro body may ask about, and the reason the key is small: a walk of an expansion
/// records the template's spans too, and those are in the macro's own definition rather than in
/// anything the call site handed over.
fn within(inner: Span, outer: Span) -> bool {
    !inner.is_none()
        && !outer.is_none()
        && inner.file == outer.file
        && inner.start >= outer.start
        && inner.end <= outer.end
}

/// Is this a type a second check could not disagree with?
///
/// A variable is the one thing that could: it may be bound by the time the same expansion is asked
/// about again, and answering with the unbound form would carry one call site's inference into
/// another's. Every other type is the same answer whenever it is given.
fn is_settled(t: &Ty) -> bool {
    match t {
        Ty::Var(_) => false,
        Ty::Con(_, args) => args.iter().all(is_settled),
        Ty::Fun(params, ret, _) => params.iter().all(is_settled) && is_settled(ret),
    }
}
