//! Which read of a local is its **last** one, so a backend may move the value instead of copying it.
//!
//! # Why this exists
//!
//! Beck has no mutable sequence, so every loop that builds one is a tail-recursive accumulator:
//!
//! ```text
//! def add_from(a, b, i, carry, done):
//!     …
//!     return add_from(a, b, i + 1, total / base(), list_append(done, total % base()))
//! ```
//!
//! `list_append` cannot push into `done` because the caller's frame still binds it, so it copies —
//! and the idiom is therefore quadratic in time. [`69`](../../../../../docs/69-standard-library-imports-report.md)
//! §69.7 is the measurement, and the fix is knowing that this read of `done` is its last: the frame
//! can hand the value over rather than lend it, and the append can push into a list nobody else
//! holds.
//!
//! It is the same idea as Koka's Perceus and Roc's opportunistic mutation, and it is *why* a
//! language can be pure and still write in place. It is computed here rather than in a backend
//! because it is a fact about the program: [`19`](../../../../../docs/19-phase-1-report.md) §19.4's
//! rule that a copied accumulator is "a semantic defect, not a backend one" cuts both ways.
//!
//! # What the flag promises
//!
//! `last_use` on a [`CoreKind::Var`] means: **on every path that evaluates this node, no later
//! evaluation in this function body reads that binding.** It says nothing about other frames, other
//! calls or the heap — a value may still be shared, and a backend must check that separately.
//!
//! `false` is always safe, and everything not understood here is left `false`.
//!
//! # The three rules that make it sound
//!
//! 1. **Branches are alternatives.** A read in the `then` arm is a last use if the variable is not
//!    read after the whole `if`, whatever `alt` does, because only one arm runs.
//! 2. **A `lam` body is not analysed against the enclosing frame.** A closure captures its
//!    environment and may be called any number of times later, so every variable free in it stays
//!    live, and nothing inside it is marked.
//! 3. **Evaluation order is left to right**, which is the order the evaluator uses for arguments,
//!    fields and list elements. Walking backwards over that order is what makes "later" mean
//!    anything.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::core::{Core, CoreKind, VarId};

/// Mark every definition and test in a checked program.
///
/// Run once, where the program is finished and before any backend sees it, so that "which read is
/// the last" is a property of the compiled program rather than something one backend worked out
/// for itself.
pub fn mark_program(program: &mut crate::check::Program) {
    for def in program.defs.values_mut() {
        mark(&mut def.body);
    }
    for signal in program.signals.iter_mut() {
        mark(&mut signal.expr);
    }
    // A test's expressions are code, and the runner wraps each one in a lambda whose frame is
    // built and dropped by a single call — so a last read inside one is exactly as safe to move as
    // a last read inside a definition. `docs/79`.
    for test in program.tests.iter_mut() {
        for c in test.cores_mut() {
            mark(c);
        }
    }
}

/// Mark every last read in `body`, given the parameters bound around it.
///
/// Idempotent, and safe to run on a body that has been marked already: the flag is recomputed from
/// scratch rather than accumulated.
pub fn mark(body: &mut Core) {
    // A definition *is* a lambda — [`crate::check::Def::body`] is "the whole definition as a
    // lambda, so evaluating the name yields a callable value" — and that outermost one is not a
    // closure: its frame is built fresh by each call and dies with it, so its parameters are
    // exactly the bindings worth moving. Looking through it is the difference between this pass
    // marking every function and marking nothing at all.
    if let CoreKind::Lam {
        params,
        body: inner,
    } = &mut body.kind
    {
        let params: Vec<VarId> = params.to_vec();
        // `make_mut` rather than a clone: this runs once, on a program nothing else holds yet, so
        // the copy-on-write never copies. The `Arc` is there for the *evaluator*, which shares one
        // body across every closure built from it (`docs/73` §73.1).
        mark_frame(&params, Arc::make_mut(inner));
    } else {
        // An expression that is not a definition: a signal, or a clause of a `test` block, which
        // the runner wraps in a lambda of its own when it evaluates it. Every variable it reads is
        // bound by that frame — its own bindings, or the wrapper's parameters — so all of them are
        // this frame's to hand over.
        let mut own = BTreeSet::new();
        reads(body, &mut own);
        mark_scope(body, own);
    }
}

/// Mark a frame's body, given the parameters its call binds.
fn mark_frame(params: &[VarId], body: &mut Core) {
    // What this frame owns: its parameters, and everything bound *directly* in the body. A binding
    // made inside a nested lambda belongs to that lambda's own frame and is marked when this walk
    // reaches it.
    let mut own: BTreeSet<VarId> = params.iter().copied().collect();
    collect_own_binders(body, &mut own);
    mark_scope(body, own);
}

fn mark_scope(body: &mut Core, own: BTreeSet<VarId>) {
    // Rule 2, and it has to be a pre-pass rather than something the backward walk discovers. A
    // closure is created *before* the reads that follow it in the body, so walking backwards meets
    // those reads first and would call one of them the last — while the closure it already captured
    // is still holding the binding, to read whenever it is called. Every variable any lambda
    // touches is therefore excluded outright.
    let mut captured: BTreeSet<VarId> = BTreeSet::new();
    collect_captures(body, &mut captured, false);
    let mut live: BTreeSet<VarId> = BTreeSet::new();
    walk(body, &mut live, &captured, &own);
}

/// Every variable bound by *this* frame: a `let`, a match arm's binders, and a lambda's parameters
/// only when that lambda is this node. A nested lambda's bindings live in the frame its own call
/// makes, so the walk stops there.
fn collect_own_binders(c: &Core, out: &mut BTreeSet<VarId>) {
    match &c.kind {
        CoreKind::Lam { .. } => return,
        CoreKind::Let { var, .. } => {
            out.insert(*var);
        }
        CoreKind::Match { arms, .. } => {
            for arm in arms {
                out.extend(arm.pattern.binders());
            }
        }
        _ => {}
    }
    for child in children(c) {
        collect_own_binders(child, out);
    }
}

/// Every variable read anywhere inside, including inside nested lambdas.
fn reads(c: &Core, out: &mut BTreeSet<VarId>) {
    if let CoreKind::Var(v) = &c.kind {
        out.insert(*v);
    }
    for child in children(c) {
        reads(child, out);
    }
}

/// Every variable bound anywhere inside, nested lambdas included — so that subtracting it from
/// [`reads`] leaves exactly the variables a lambda takes from the scope around it.
fn binds(c: &Core, out: &mut BTreeSet<VarId>) {
    match &c.kind {
        CoreKind::Lam { params, .. } => out.extend(params.iter().copied()),
        CoreKind::Let { var, .. } => {
            out.insert(*var);
        }
        CoreKind::Match { arms, .. } => {
            for arm in arms {
                out.extend(arm.pattern.binders());
            }
        }
        _ => {}
    }
    for child in children(c) {
        binds(child, out);
    }
}

/// Every variable read anywhere inside a `lam`, over-approximated: a lambda's own parameters are
/// counted too, which costs a missed move and never an unsound one.
fn collect_captures(c: &Core, out: &mut BTreeSet<VarId>, inside: bool) {
    if let CoreKind::Var(v) = &c.kind {
        if inside {
            out.insert(*v);
        }
    }
    let inside = inside || matches!(c.kind, CoreKind::Lam { .. });
    for child in children(c) {
        collect_captures(child, out, inside);
    }
}

/// Every subexpression, in no particular order — the traversal `collect_captures` needs and the
/// only place in this module that does not care about evaluation order.
fn children(c: &Core) -> Vec<&Core> {
    match &c.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => Vec::new(),
        CoreKind::Lam { body, .. } => vec![&**body],
        CoreKind::App { func, args } => std::iter::once(&**func).chain(args.iter()).collect(),
        CoreKind::Prim { args, .. } => args.iter().collect(),
        CoreKind::Let { value, body, .. } => vec![&**value, &**body],
        CoreKind::If { cond, then, alt } => vec![&**cond, &**then, &**alt],
        CoreKind::Match { scrutinee, arms } => std::iter::once(&**scrutinee)
            .chain(arms.iter().map(|a| &a.body))
            .collect(),
        CoreKind::Make { fields, .. } => fields.iter().map(|(_, f)| f).collect(),
        CoreKind::Field { base, .. } => vec![&**base],
        CoreKind::With { base, fields } => std::iter::once(&**base)
            .chain(fields.iter().map(|(_, f)| f))
            .collect(),
        CoreKind::ListLit(xs) => xs.iter().collect(),
        CoreKind::MapLit(kvs) => kvs.iter().flat_map(|(k, v)| [k, v]).collect(),
    }
}

/// Backwards over evaluation order. `live` is the set of variables read *after* this node, and
/// `captured` is rule 2's exclusion list.
fn walk(
    c: &mut Core,
    live: &mut BTreeSet<VarId>,
    captured: &BTreeSet<VarId>,
    own: &BTreeSet<VarId>,
) {
    match &mut c.kind {
        CoreKind::Var(v) => {
            let v = *v;
            // `own` is the third condition and the newest: a read may only be handed over by the
            // frame that binds it. Without it a lambda would mark a read of a variable it took
            // from the scope around it, and that binding outlives the call (`docs/79` §79.3).
            c.last_use = own.contains(&v) && !captured.contains(&v) && !live.contains(&v);
            live.insert(v);
        }
        CoreKind::Const(_) | CoreKind::Global(_) => {}

        CoreKind::Lam { params, body } => {
            // Two separate jobs, and conflating them is what made this pass mark nothing inside a
            // lambda for three reports.
            //
            // Outwards: every variable the lambda takes from the scope around it becomes live
            // here, because a closure that reads `xs` is a reader of `xs` for as long as it
            // exists — and it may be called any number of times, at any point later.
            //
            // Inwards: the body is a frame of its own, built by each call to the closure and
            // dropped with it, exactly as a definition's is. So it gets its own analysis, in which
            // the lambda's parameters and bindings are the ones worth handing over. `list_fold`'s
            // accumulator is a lambda's parameter and nothing else, which is why the fold form of
            // the accumulator idiom stayed quadratic after `docs/70` made the recursive form
            // linear.
            let params: Vec<VarId> = params.to_vec();
            let body = Arc::make_mut(body);
            let (mut read, mut bound) = (BTreeSet::new(), BTreeSet::new());
            reads(body, &mut read);
            bound.extend(params.iter().copied());
            binds(body, &mut bound);
            live.extend(read.difference(&bound).copied());
            mark_frame(&params, body);
        }

        CoreKind::App { func, args } => {
            // **The callee is evaluated after its arguments**, not before: `Interp::step`'s `App`
            // arm evaluates every operand and *then* the function, so that a stub can answer "with
            // what?" (§21.3 rule 4). Walking backwards therefore means the callee first. Getting
            // this the intuitive way round says the last read of `f` in `f(x, g(f))` is the inner
            // one, moves it there, and leaves the call itself with nothing to call — which is what
            // `sicp/ch1.beck`'s exercise 1.32 does, and what caught it.
            walk(func, live, captured, own);
            for a in args.iter_mut().rev() {
                walk(a, live, captured, own);
            }
        }
        CoreKind::Prim { op: _, args } => {
            for a in args.iter_mut().rev() {
                walk(a, live, captured, own);
            }
        }
        CoreKind::ListLit(items) => {
            for i in items.iter_mut().rev() {
                walk(i, live, captured, own);
            }
        }
        CoreKind::MapLit(kvs) => {
            for (k, v) in kvs.iter_mut().rev() {
                walk(v, live, captured, own);
                walk(k, live, captured, own);
            }
        }
        CoreKind::Make { fields, .. } => {
            for (_, f) in fields.iter_mut().rev() {
                walk(f, live, captured, own);
            }
        }
        CoreKind::Field { base, .. } => walk(base, live, captured, own),
        CoreKind::With { base, fields } => {
            for (_, f) in fields.iter_mut().rev() {
                walk(f, live, captured, own);
            }
            walk(base, live, captured, own);
        }

        CoreKind::Let { var, value, body } => {
            walk(body, live, captured, own);
            // The binding dies with its `let`: a read of `var` after this node cannot be this
            // `var`, because the name is out of scope there.
            let var = *var;
            live.remove(&var);
            walk(value, live, captured, own);
        }

        CoreKind::If { cond, then, alt } => {
            // Rule 1: each arm sees what is live after the whole `if`, not what the other arm reads.
            let mut then_live = live.clone();
            walk(then, &mut then_live, captured, own);
            let mut alt_live = std::mem::take(live);
            walk(alt, &mut alt_live, captured, own);
            *live = then_live;
            live.extend(alt_live);
            walk(cond, live, captured, own);
        }

        CoreKind::Match { scrutinee, arms } => {
            let after = std::mem::take(live);
            let mut union = BTreeSet::new();
            for arm in arms.iter_mut() {
                let mut arm_live = after.clone();
                walk(&mut arm.body, &mut arm_live, captured, own);
                // The arm's own binders die with the arm, the way a `let`'s does.
                for b in arm.pattern.binders() {
                    arm_live.remove(&b);
                }
                union.extend(arm_live);
            }
            *live = union;
            walk(scrutinee, live, captured, own);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::Ty;
    use beck_diag::Span;

    fn var(v: VarId) -> Core {
        Core::new(CoreKind::Var(v), Ty::int(), Span::NONE)
    }

    fn prim(op: crate::core::Prim, args: Vec<Core>) -> Core {
        Core::new(CoreKind::Prim { op, args }, Ty::int(), Span::NONE)
    }

    /// Every `Var` node in the tree, in a stable order, with its flag.
    fn marks(c: &Core) -> Vec<(VarId, bool)> {
        let mut out = Vec::new();
        fn go(c: &Core, out: &mut Vec<(VarId, bool)>) {
            if let CoreKind::Var(v) = &c.kind {
                out.push((*v, c.last_use));
            }
            for child in children(c) {
                go(child, out);
            }
        }
        fn children(c: &Core) -> Vec<&Core> {
            match &c.kind {
                CoreKind::Prim { args, .. } => args.iter().collect(),
                CoreKind::App { func, args } => {
                    let mut v: Vec<&Core> = vec![func];
                    v.extend(args.iter());
                    v
                }
                CoreKind::If { cond, then, alt } => vec![cond, then, alt],
                CoreKind::Let { value, body, .. } => vec![value, body],
                CoreKind::Lam { body, .. } => vec![body],
                CoreKind::ListLit(xs) => xs.iter().collect(),
                CoreKind::Field { base, .. } => vec![base],
                _ => Vec::new(),
            }
        }
        go(c, &mut out);
        out
    }

    #[test]
    fn the_only_read_of_a_variable_is_its_last() {
        let mut c = prim(crate::core::Prim::ListAppend, vec![var(1), var(2)]);
        mark(&mut c);
        assert_eq!(marks(&c), vec![(1, true), (2, true)]);
    }

    #[test]
    fn an_earlier_read_of_the_same_variable_is_not() {
        // `add(x, x)` — arguments run left to right, so the *second* one is the last read.
        let mut c = prim(crate::core::Prim::Add, vec![var(1), var(1)]);
        mark(&mut c);
        assert_eq!(marks(&c), vec![(1, false), (1, true)]);
    }

    /// Rule 1. Only one arm runs, so a read in each arm is that path's last read.
    #[test]
    fn a_read_in_each_branch_is_a_last_read_in_both() {
        let mut c = Core::new(
            CoreKind::If {
                cond: Box::new(var(9)),
                then: Box::new(var(1)),
                alt: Box::new(var(1)),
            },
            Ty::int(),
            Span::NONE,
        );
        mark(&mut c);
        assert_eq!(marks(&c), vec![(9, true), (1, true), (1, true)]);
    }

    /// …but not when the variable is read again after the branch.
    #[test]
    fn a_read_in_a_branch_is_not_a_last_read_when_the_value_outlives_the_branch() {
        let inner = Core::new(
            CoreKind::If {
                cond: Box::new(var(9)),
                then: Box::new(var(1)),
                alt: Box::new(var(2)),
            },
            Ty::int(),
            Span::NONE,
        );
        let mut c = prim(crate::core::Prim::Add, vec![inner, var(1)]);
        mark(&mut c);
        assert_eq!(
            marks(&c),
            vec![(9, true), (1, false), (2, true), (1, true)],
            "the `then` arm's read of 1 is followed by another read"
        );
    }

    /// Rule 2. A closure may be called twice, so it never gets to move what it **captured**.
    #[test]
    fn a_lambda_never_moves_what_it_took_from_the_scope_around_it() {
        let lam = Core::new(
            CoreKind::Lam {
                params: vec![7].into(),
                body: Arc::new(var(1)),
            },
            Ty::int(),
            Span::NONE,
        );
        // The captured read comes *first* in evaluation order, and the later direct read of the
        // same variable must not be treated as the last one either — the closure outlives it.
        let mut c = prim(crate::core::Prim::Add, vec![lam, var(1)]);
        mark(&mut c);
        assert_eq!(marks(&c), vec![(1, false), (1, false)]);
    }

    /// …but a variable the lambda **binds itself** is another matter: its frame is built by the
    /// call and dropped with it, exactly as a definition's is.
    ///
    /// This is `docs/79`, and `list_fold`'s accumulator is the case that matters — `acc` is a
    /// lambda's parameter and nothing else, so before this the fold form of the accumulator idiom
    /// copied where the recursive form moved.
    #[test]
    fn a_lambdas_own_parameter_is_moved_on_its_last_read() {
        let mut c = Core::new(
            CoreKind::Lam {
                params: vec![7, 8].into(),
                body: Arc::new(prim(crate::core::Prim::ListAppend, vec![var(7), var(8)])),
            },
            Ty::int(),
            Span::NONE,
        );
        mark(&mut c);
        assert_eq!(marks(&c), vec![(7, true), (8, true)]);
    }

    /// The line between the two, in one lambda: its own parameter is handed over and the variable
    /// it took from outside is not.
    ///
    /// The evaluator would refuse the second one anyway — a captured environment is shared, so
    /// `Env::read` cannot empty it — which is why this is asserted here, on the flag, rather than
    /// by a program that could not tell the difference (`docs/79` §79.6).
    #[test]
    fn a_lambda_moves_its_parameter_and_lends_its_capture() {
        let mut c = Core::new(
            CoreKind::Lam {
                params: vec![7].into(),
                body: Arc::new(prim(crate::core::Prim::ListAppend, vec![var(1), var(7)])),
            },
            Ty::int(),
            Span::NONE,
        );
        mark(&mut c);
        assert_eq!(
            marks(&c),
            vec![(1, false), (7, true)],
            "1 is free in the lambda and 7 is its parameter"
        );
    }

    /// And a parameter a *deeper* lambda reads goes back to being lent, by the same rule one level
    /// down.
    #[test]
    fn a_lambda_parameter_a_deeper_lambda_reads_is_not_moved() {
        let deeper = Core::new(
            CoreKind::Lam {
                params: vec![9].into(),
                body: Arc::new(var(7)),
            },
            Ty::int(),
            Span::NONE,
        );
        let mut c = Core::new(
            CoreKind::Lam {
                params: vec![7].into(),
                body: Arc::new(prim(crate::core::Prim::ListAppend, vec![var(7), deeper])),
            },
            Ty::int(),
            Span::NONE,
        );
        mark(&mut c);
        assert_eq!(marks(&c), vec![(7, false), (7, false)]);
    }

    #[test]
    fn a_let_bound_variable_dies_with_its_body() {
        // let x = y in x  — both reads are last reads.
        let mut c = Core::new(
            CoreKind::Let {
                var: 1,
                value: Box::new(var(2)),
                body: Box::new(var(1)),
            },
            Ty::int(),
            Span::NONE,
        );
        mark(&mut c);
        assert_eq!(marks(&c), vec![(2, true), (1, true)]);
    }

    /// `accumulate`'s shape: the combiner is the callee *and* an argument of the nested call.
    ///
    /// The evaluator runs the arguments first, so the callee position is the last read and the
    /// nested argument is not. Marking it the other way round is a program that loses its own
    /// function — `sicp/ch1.beck` exercise 1.32, which is where this came from.
    #[test]
    fn a_callee_is_read_after_its_arguments() {
        let inner = Core::new(
            CoreKind::App {
                func: Box::new(var(1)),
                args: vec![var(2)],
            },
            Ty::int(),
            Span::NONE,
        );
        let mut c = Core::new(
            CoreKind::App {
                func: Box::new(var(1)),
                args: vec![inner],
            },
            Ty::int(),
            Span::NONE,
        );
        mark(&mut c);
        // Outer callee (evaluated last) is the last read; the inner callee and argument are not.
        assert_eq!(marks(&c), vec![(1, true), (1, false), (2, true)]);
    }

    #[test]
    fn marking_twice_is_marking_once() {
        let mut a = prim(crate::core::Prim::Add, vec![var(1), var(1)]);
        mark(&mut a);
        let once = marks(&a);
        mark(&mut a);
        assert_eq!(marks(&a), once);
    }
}
