//! How many bindings a function body makes, so that a call can reserve room for them.
//!
//! A `let` used to allocate a scope of its own — a vector for the one binding, an `Arc` around it
//! and an `Arc` around a clone of the enclosing environment, three allocations for one name. At
//! about 126 ns each that is most of what a function call costs, paid again per binding, and a
//! body of two dozen `let`s spent more on scopes than on the work
//! ([`76`](../../../../../docs/76-the-record-and-the-read-report.md) is where the number came
//! from).
//!
//! The count here is what removes them. A lambda's `Core::locals` is the number of bindings its
//! body can make, so [`crate::core::Env::call_frame`] sizes one frame for the parameters and all
//! of them together and a `let` writes into a slot that already exists.
//!
//! Two properties make that sound, and both are why this counts the way it does:
//!
//! * **Every binding that can be live at once gets its own slot.** The count sums what runs in
//!   sequence and takes the *maximum* over what cannot: two arms of a `match` are exclusive, so
//!   they share a reservation, and a slot is still never written twice within one call. That is
//!   what makes it safe for a closure to hold the frame — nothing it captured can change
//!   underneath it.
//! * **A nested lambda contributes nothing.** Its body runs in a frame of its own, made when it is
//!   called, so counting it here would reserve slots nothing writes.
//!
//! Miscounting is safe in one direction and merely slow in the other: too few slots and the
//! evaluator falls back to chaining a scope, exactly as before this existed. That is also what
//! happens to any program built by something that never runs this pass — a synthesised test body,
//! a splitter's generated module — which is why the fallback is kept rather than asserted away.

use crate::core::{children_mut, Arm, Core, CoreKind};

/// Count and record every lambda's local bindings, across a whole program.
pub fn reserve_program(program: &mut crate::check::Program) {
    for def in program.defs.values_mut() {
        reserve(&mut def.body);
    }
    for test in program.tests.iter_mut() {
        for c in test.cores_mut() {
            reserve(c);
        }
    }
}

/// How many bindings this expression makes before control leaves the frame it runs in.
///
/// The count [`reserve`] writes onto a lambda, for a caller holding an expression that is not one
/// yet: the test runner wraps a clause in a lambda of its own at the moment it evaluates it, and
/// this is how that lambda gets sized. `docs/79`.
pub fn locals_of(c: &Core) -> u32 {
    locals(c)
}

/// The same for one expression, whether or not it is a lambda.
pub fn reserve(c: &mut Core) {
    if let CoreKind::Lam { body, .. } = &mut c.kind {
        let body = std::sync::Arc::make_mut(body);
        c.locals = locals(body);
        reserve(body);
        return;
    }
    for child in children_mut(c) {
        reserve(child);
    }
}

/// How many bindings this expression makes before control leaves the frame it is running in.
fn locals(c: &Core) -> u32 {
    match &c.kind {
        // A lambda's bindings belong to the frame its own call makes.
        CoreKind::Lam { .. } => 0,
        CoreKind::Let { value, body, .. } => 1 + locals(value) + locals(body),
        // Exclusive: whichever branch runs, the other's slots are never written.
        CoreKind::If { cond, then, alt } => locals(cond) + locals(then).max(locals(alt)),
        CoreKind::Match { scrutinee, arms } => {
            locals(scrutinee)
                + arms
                    .iter()
                    .map(|a| a.pattern.binders().len() as u32 + locals(&a.body))
                    .max()
                    .unwrap_or(0)
        }
        _ => children(c).into_iter().map(locals).sum(),
    }
}

fn children(c: &Core) -> Vec<&Core> {
    match &c.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => Vec::new(),
        CoreKind::Lam { body, .. } => vec![body],
        CoreKind::App { func, args } => std::iter::once(&**func).chain(args).collect(),
        CoreKind::Let { value, body, .. } => vec![value, body],
        CoreKind::If { cond, then, alt } => vec![&**cond, &**then, &**alt],
        CoreKind::Match { scrutinee, arms } => std::iter::once(&**scrutinee)
            .chain(arms.iter().map(|a: &Arm| &a.body))
            .collect(),
        CoreKind::Prim { args, .. } => args.iter().collect(),
        CoreKind::Make { fields, .. } => fields.iter().map(|(_, f)| f).collect(),
        CoreKind::Field { base, .. } => vec![base],
        CoreKind::With { base, fields } => std::iter::once(&**base)
            .chain(fields.iter().map(|(_, f)| f))
            .collect(),
        CoreKind::ListLit(items) => items.iter().collect(),
        CoreKind::MapLit(kvs) => kvs.iter().flat_map(|(k, v)| [k, v]).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Const, Pattern, VarId};
    use crate::ty::Ty;
    use beck_diag::Span;

    fn int(n: i64) -> Core {
        Core::new(CoreKind::Const(Const::Int(n)), Ty::int(), Span::NONE)
    }

    fn var(v: VarId) -> Core {
        Core::new(CoreKind::Var(v), Ty::int(), Span::NONE)
    }

    fn lam(params: Vec<VarId>, body: Core) -> Core {
        Core::new(
            CoreKind::Lam {
                params: params.into(),
                body: std::sync::Arc::new(body),
            },
            Ty::int(),
            Span::NONE,
        )
    }

    fn let_(v: VarId, body: Core) -> Core {
        Core::new(
            CoreKind::Let {
                var: v,
                value: Box::new(int(1)),
                body: Box::new(body),
            },
            Ty::int(),
            Span::NONE,
        )
    }

    #[test]
    fn a_chain_of_lets_reserves_one_slot_each() {
        let mut f = lam(vec![0], let_(1, let_(2, let_(3, var(3)))));
        reserve(&mut f);
        assert_eq!(f.locals, 3);
    }

    /// The arms share a reservation, because only one of them can run — but the widest arm's
    /// bindings all fit, which is what keeps a slot from being written twice in one call.
    #[test]
    fn the_arms_of_a_match_share_the_widest_reservation() {
        let arms = vec![
            Arm {
                pattern: Pattern::Bind(1),
                body: let_(2, var(2)),
                span: Span::NONE,
            },
            Arm {
                pattern: Pattern::Ctor {
                    variant: "Node".into(),
                    binds: vec![("l".into(), 3), ("r".into(), 4)],
                },
                body: var(3),
                span: Span::NONE,
            },
        ];
        let mut f = lam(
            vec![0],
            Core::new(
                CoreKind::Match {
                    scrutinee: Box::new(var(0)),
                    arms,
                },
                Ty::int(),
                Span::NONE,
            ),
        );
        reserve(&mut f);
        assert_eq!(f.locals, 2);
    }

    /// The inner lambda's `let` belongs to the inner lambda's own frame, and to no other.
    #[test]
    fn a_nested_lambda_counts_for_itself_and_not_for_its_parent() {
        let inner = lam(vec![9], let_(8, var(8)));
        let mut outer = lam(vec![0], let_(1, inner));
        reserve(&mut outer);
        assert_eq!(outer.locals, 1);
        let CoreKind::Lam { body, .. } = &outer.kind else {
            unreachable!()
        };
        let CoreKind::Let { body: inner, .. } = &body.kind else {
            unreachable!()
        };
        assert_eq!(inner.locals, 1);
    }
}
