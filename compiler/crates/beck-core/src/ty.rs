//! Types, effect rows, unification, and the tier lattice.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../../docs/03-type-and-effect-system.md) §3.1:
//! "Hindley–Milner inference with bidirectional checking. Full inference inside bodies; mandatory
//! annotations on public signatures."
//!
//! Phase 2's change is §3.2: **every function type carries an effect row**, and the row is
//! inferred. `Ty::Fun` therefore has three components, not two, and `Subst` unifies rows alongside
//! types. The rows themselves live in [`crate::row`]; this module is where they meet the type
//! system.
//!
//! One deliberate omission remains, and it is named rather than implied: **row polymorphism on
//! records** (§3.1). Models are nominal. Effect rows are polymorphic; record rows are not.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

pub use crate::row::{Ambient, Effect, Row, RowVarId};

/// Where code runs. §3.3's table of what each tier can discharge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Pure and therefore *unplaced*: legal on every tier, compiled to each tier that needs it.
    /// "That duplication is the payoff, not waste" (§3.3).
    Any,
    Client,
    Server,
    /// The fold/view engine — pure computation over streams and signals, plus `durable`.
    Data,
}

/// The tiers a program can actually be placed on, in the order `beck explain place` reports them.
pub const CONCRETE_TIERS: [Tier; 3] = [Tier::Client, Tier::Server, Tier::Data];

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Any => "any",
            Tier::Client => "client",
            Tier::Server => "server",
            Tier::Data => "data",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        Some(match s {
            "any" => Tier::Any,
            "client" => Tier::Client,
            "server" => Tier::Server,
            "data" => Tier::Data,
            _ => return None,
        })
    }

    /// Can this tier discharge that effect? §3.3's table.
    ///
    /// `Tier::Any` is the **intersection**: unplaced means legal everywhere, so `any` discharges
    /// exactly what all three concrete tiers discharge. That is why an ambient effect never forces
    /// a placement and `durable` always does, without either being a special case.
    pub fn discharges(self, e: &Effect) -> bool {
        match self {
            Tier::Any => CONCRETE_TIERS.iter().all(|t| t.discharges(e)),
            Tier::Client => match e {
                Effect::Dom
                | Effect::Nondet
                | Effect::Partial
                | Effect::Raises(_)
                | Effect::Ambient(_) => true,
                // §3.3: `net.out(own-origin)` — and only that. A browser cannot reach an arbitrary
                // host, so a `net.out(payments.example.com)` on the client is a placement error and
                // not a CORS bug discovered in production.
                Effect::NetOut(host) => host.as_ref() == "origin",
                _ => false,
            },
            // "server: discharges ingress, durable, net.*, fs, env, spawn, cap.*; cannot: dom".
            Tier::Server => !matches!(e, Effect::Dom),
            // "data (the fold/view engine): pure computation over streams/signals; durable.
            //  cannot: dom, net, ambient time/rand".
            //
            // `partial` is discharged: a fold that diverges aborts the process, which is the same
            // failure mode as any other failed append (§18.5 item 6) and does not make replay a
            // different function of the log. `nondet` is not, and that is §3.7's rule.
            // `raises` is discharged everywhere, including here: failing is *control flow* and not
            // a resource. A fold that raises is a fold that produced no state from that event,
            // which is the same shape as `partial` and is a function of the log either way.
            Tier::Data => matches!(
                e,
                Effect::Durable | Effect::Partial | Effect::Raises(_) | Effect::Ambient(_)
            ),
        }
    }

    /// Every tier that can discharge this whole row, in report order. An open row is treated as its
    /// known atoms: a row variable stands for a caller's effects, which the caller must place.
    pub fn candidates(row: &Row) -> Vec<Tier> {
        CONCRETE_TIERS
            .into_iter()
            .filter(|t| row.atoms.iter().all(|e| t.discharges(e)))
            .collect()
    }
}

impl Effect {
    /// Does this effect make a fold a different function of the log? §3.7's replay-purity rule,
    /// stated as a property of the atom rather than as "the row is empty".
    ///
    /// `log` and `metrics` do not (they are write-only observations, §19.8), `partial` does not
    /// (a fold that aborts produces no state, rather than a different one), and `raises` does not
    /// (the same value raised for the same input, every replay). Everything else does.
    pub fn breaks_replay(&self) -> bool {
        !matches!(
            self,
            Effect::Ambient(_) | Effect::Partial | Effect::Raises(_)
        )
    }
}

pub type TyVarId = u32;

/// A unification variable's binding, shared so that unifying in one place is visible everywhere.
#[derive(Clone, Debug, Default)]
pub struct Subst {
    bindings: Rc<RefCell<BTreeMap<TyVarId, Ty>>>,
    rows: Rc<RefCell<BTreeMap<RowVarId, Row>>>,
    next: Rc<RefCell<TyVarId>>,
    next_row: Rc<RefCell<RowVarId>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Var(TyVarId),
    /// A named type constructor applied to arguments: `Int`, `list[T]`, `Map[K,V]`, `Signal[T]`,
    /// `secret[T]`, and every user `model`/`union`/`newtype`.
    Con(Arc<str>, Vec<Ty>),
    /// A function, with the effect row it performs when applied. §3.2's `(A) -> B ! e`.
    Fun(Vec<Ty>, Box<Ty>, Row),
}

impl Ty {
    pub fn con(name: &str) -> Ty {
        Ty::Con(Arc::from(name), Vec::new())
    }

    pub fn app(name: &str, args: Vec<Ty>) -> Ty {
        Ty::Con(Arc::from(name), args)
    }

    /// A pure function — the common case, and the one worth being short.
    pub fn fun(params: Vec<Ty>, ret: Ty) -> Ty {
        Ty::Fun(params, Box::new(ret), Row::empty())
    }

    /// A function with an effect row.
    pub fn fun_eff(params: Vec<Ty>, ret: Ty, row: Row) -> Ty {
        Ty::Fun(params, Box::new(ret), row)
    }

    pub const INT: &'static str = "Int";
    pub const STR: &'static str = "Str";
    pub const BOOL: &'static str = "Bool";
    pub const FLOAT: &'static str = "Float";
    pub const UNIT: &'static str = "Unit";
    pub const HTML: &'static str = "Html";
    pub const ATTR: &'static str = "Attr";
    pub const LIST: &'static str = "list";
    pub const MAP: &'static str = "Map";
    pub const OPTION: &'static str = "Option";
    pub const RESULT: &'static str = "Result";
    pub const STREAM: &'static str = "Stream";
    pub const SIGNAL: &'static str = "Signal";
    pub const ENVELOPE: &'static str = "Envelope";
    /// §3.5: "`secret[T]` is not Sendable". The one type constructor whose whole purpose is to fail
    /// a boundary check.
    pub const SECRET: &'static str = "secret";
    /// The other half of that story, and the quadrant `secret[T]` alone leaves empty: **may be
    /// written to the log, may never cross a boundary**.
    ///
    /// An event-sourced system has to record what happened, including facts a client must never
    /// see — why an account was suspended, which rule fired, what an upstream vendor called the
    /// customer. `secret[T]` cannot express that, because a secret is *also* not storable (§3.7's
    /// F5: tokens must never be persisted into an immutable log). Without `internal[T]` the choice
    /// is between dropping the fact from the audit trail and trusting that no view ever renders it,
    /// and "trusting" is the word this language exists to delete.
    pub const INTERNAL: &'static str = "internal";

    pub fn int() -> Ty {
        Ty::con(Ty::INT)
    }
    pub fn str_() -> Ty {
        Ty::con(Ty::STR)
    }
    pub fn bool_() -> Ty {
        Ty::con(Ty::BOOL)
    }
    pub fn unit() -> Ty {
        Ty::con(Ty::UNIT)
    }
    pub fn html() -> Ty {
        Ty::con(Ty::HTML)
    }
    pub fn list(t: Ty) -> Ty {
        Ty::app(Ty::LIST, vec![t])
    }
    pub fn map(k: Ty, v: Ty) -> Ty {
        Ty::app(Ty::MAP, vec![k, v])
    }
    pub fn option(t: Ty) -> Ty {
        Ty::app(Ty::OPTION, vec![t])
    }
    pub fn signal(t: Ty) -> Ty {
        Ty::app(Ty::SIGNAL, vec![t])
    }
    pub fn stream(t: Ty) -> Ty {
        Ty::app(Ty::STREAM, vec![t])
    }
    pub fn secret(t: Ty) -> Ty {
        Ty::app(Ty::SECRET, vec![t])
    }
    pub fn internal(t: Ty) -> Ty {
        Ty::app(Ty::INTERNAL, vec![t])
    }

    pub fn con_name(&self) -> Option<&str> {
        match self {
            Ty::Con(n, _) => Some(n),
            _ => None,
        }
    }

    fn occurs(&self, v: TyVarId, s: &Subst) -> bool {
        match s.resolve_shallow(self) {
            Ty::Var(u) => u == v,
            Ty::Con(_, args) => args.iter().any(|a| a.occurs(v, s)),
            Ty::Fun(ps, r, _) => ps.iter().any(|a| a.occurs(v, s)) || r.occurs(v, s),
        }
    }
}

/// A type scheme: `forall vars rows. ty`. Let-polymorphism over *both* dimensions, which is what
/// §3.2 means by "effect polymorphism is what keeps one standard library".
#[derive(Clone, Debug)]
pub struct Scheme {
    pub vars: Vec<TyVarId>,
    pub row_vars: Vec<RowVarId>,
    /// The **named** type parameters of a user-written `def map[T, U](…)`.
    ///
    /// A prelude scheme quantifies over numbered variables because nobody reads its source; a
    /// user's quantifies over names, because the name is what the programmer wrote, what the body
    /// is checked against, what a diagnostic has to print, and what `beck iface` publishes. Inside
    /// the body each of these is a *rigid* `Ty::Con(name, [])` — an opaque type that unifies with
    /// itself and nothing else, which is exactly the property that makes the definition honest
    /// about being polymorphic. [`Subst::instantiate`] turns them back into fresh variables at
    /// every call site. `docs/27` §27.2.
    pub params: Vec<Arc<str>>,
    pub ty: Ty,
}

impl Scheme {
    pub fn mono(ty: Ty) -> Scheme {
        Scheme {
            vars: Vec::new(),
            row_vars: Vec::new(),
            params: Vec::new(),
            ty,
        }
    }

    /// A scheme over named type parameters — what a `def` with a `[T, U]` list gets.
    pub fn generic(params: Vec<Arc<str>>, ty: Ty) -> Scheme {
        Scheme {
            vars: Vec::new(),
            row_vars: Vec::new(),
            params,
            ty,
        }
    }
}

impl Subst {
    pub fn new() -> Subst {
        Subst::default()
    }

    pub fn fresh(&self) -> Ty {
        let mut n = self.next.borrow_mut();
        let id = *n;
        *n += 1;
        Ty::Var(id)
    }

    pub fn fresh_row_var(&self) -> RowVarId {
        let mut n = self.next_row.borrow_mut();
        let id = *n;
        *n += 1;
        id
    }

    pub fn fresh_row(&self) -> Row {
        Row::var(self.fresh_row_var())
    }

    fn resolve_shallow(&self, t: &Ty) -> Ty {
        let mut cur = t.clone();
        loop {
            match cur {
                Ty::Var(v) => match self.bindings.borrow().get(&v) {
                    Some(next) => cur = next.clone(),
                    None => return Ty::Var(v),
                },
                other => return other,
            }
        }
    }

    /// Fully apply the substitution — what diagnostics and the emitted `Core` see.
    pub fn resolve(&self, t: &Ty) -> Ty {
        match self.resolve_shallow(t) {
            Ty::Var(v) => Ty::Var(v),
            Ty::Con(n, args) => Ty::Con(n, args.iter().map(|a| self.resolve(a)).collect()),
            Ty::Fun(ps, r, row) => Ty::Fun(
                ps.iter().map(|p| self.resolve(p)).collect(),
                Box::new(self.resolve(&r)),
                self.resolve_row(&row),
            ),
        }
    }

    /// Expand a row through its variable bindings, to a fixed point.
    ///
    /// The `seen` set is not a safety net; it is the semantics. A definition's row variable can be
    /// bound to a row that mentions itself — that is what mutual recursion between two effectful
    /// functions *is* — and because a row is a union, stopping at an already-expanded variable
    /// computes exactly the least fixed point rather than diverging.
    pub fn resolve_row(&self, r: &Row) -> Row {
        let mut out = Row {
            atoms: r.atoms.clone(),
            tails: BTreeSet::new(),
        };
        let mut seen: BTreeSet<RowVarId> = BTreeSet::new();
        let mut work: Vec<RowVarId> = r.tails.iter().copied().collect();
        while let Some(v) = work.pop() {
            if !seen.insert(v) {
                continue;
            }
            match self.rows.borrow().get(&v) {
                Some(next) => {
                    out.atoms.extend(next.atoms.iter().cloned());
                    work.extend(next.tails.iter().copied());
                }
                None => {
                    out.tails.insert(v);
                }
            }
        }
        out
    }

    pub fn bind_row(&self, v: RowVarId, r: Row) {
        self.rows.borrow_mut().insert(v, r);
    }

    /// Instantiate a scheme with fresh type and row variables.
    pub fn instantiate(&self, s: &Scheme) -> Ty {
        self.instantiate_named(s).0
    }

    /// [`Subst::instantiate`], and the fresh variable each **named** parameter became.
    ///
    /// A bounded definition needs the map: `def sort[T: Ord](xs: list[T])` is lowered with a
    /// dictionary parameter per method of `Ord`, and the call site can only say which impl to pass
    /// once it knows what this call's `T` turned out to be. `docs/27` §27.5.
    pub fn instantiate_named(&self, s: &Scheme) -> (Ty, BTreeMap<Arc<str>, Ty>) {
        if s.vars.is_empty() && s.row_vars.is_empty() && s.params.is_empty() {
            return (s.ty.clone(), BTreeMap::new());
        }
        let tys: BTreeMap<TyVarId, Ty> = s.vars.iter().map(|v| (*v, self.fresh())).collect();
        let rows: BTreeMap<RowVarId, RowVarId> = s
            .row_vars
            .iter()
            .map(|v| (*v, self.fresh_row_var()))
            .collect();
        let ty = subst_vars(&s.ty, &tys, &rows);
        if s.params.is_empty() {
            return (ty, BTreeMap::new());
        }
        // A fresh variable per named parameter, per use — which is what makes two calls of the same
        // `map` at two element types two different types rather than one over-constrained one.
        let named: BTreeMap<Arc<str>, Ty> =
            s.params.iter().map(|p| (p.clone(), self.fresh())).collect();
        (subst_named(&ty, &named), named)
    }

    /// Unify two types that are **alternatives** rather than actual-and-expected, and return the
    /// type of whichever one runs.
    ///
    /// [`Subst::unify`] is asymmetric on purpose: its first argument is the actual type and its
    /// second the expected one, and [`Subst::subsume_row`] leans on that so a function which does
    /// less than its context allows is accepted. The two branches of an `if` are neither. Making one
    /// of them the "expected" type of the other says that a branch returning `identity` — inferred
    /// pure, so its row is closed — is the standard the other branch has to meet, and the other
    /// branch returning a call's result carries a row *variable*. A variable is not a subset of the
    /// empty row, so the two are reported as a conflict, with nothing missing to name:
    ///
    /// ```text
    /// error[B0320]: the two branches may not perform {} here
    /// ```
    ///
    /// which is `docs/25-benchmarks-and-expressiveness.md` §25.6 item 6, and what exercise 1.43
    /// costs. The answer is the one every row-typed language reaches: the alternatives do not meet
    /// each other, they both flow into a **fresh row**, and the result performs whatever either of
    /// them might. Sound in the direction §3.2 requires — the join contains both branches' atoms, so
    /// an effect can never be lost — and it leaves a free tail, exactly as a written function type
    /// does, so a later context may widen it again.
    pub fn unify_join(&self, a: &Ty, b: &Ty) -> Result<Ty, Mismatch> {
        let (ra, rb) = (self.resolve_shallow(a), self.resolve_shallow(b));
        match (&ra, &rb) {
            // A variable on either side: nothing to join yet, and binding it is what `unify`
            // already does correctly.
            (Ty::Var(_), _) | (_, Ty::Var(_)) => {
                self.unify(&ra, &rb)?;
                Ok(self.resolve_shallow(&ra))
            }
            (Ty::Con(n1, a1), Ty::Con(n2, a2)) if n1 == n2 && a1.len() == a2.len() => {
                let mut args = Vec::with_capacity(a1.len());
                for (x, y) in a1.iter().zip(a2) {
                    args.push(self.unify_join(x, y)?);
                }
                Ok(Ty::Con(n1.clone(), args))
            }
            (Ty::Fun(p1, r1, e1), Ty::Fun(p2, r2, e2)) => {
                if p1.len() != p2.len() {
                    return Err(Mismatch::Arity(p1.len(), p2.len()));
                }
                // Parameters are contravariant, so joining them would be unsound in the other
                // direction. Two alternatives must accept the same arguments: ordinary unification.
                for (x, y) in p1.iter().zip(p2) {
                    self.unify(x, y)?;
                }
                let ret = self.unify_join(r1, r2)?;
                let row = Row::var(self.fresh_row_var());
                self.subsume_row(e1, &row)?;
                self.subsume_row(e2, &row)?;
                Ok(Ty::Fun(p1.clone(), Box::new(ret), row))
            }
            _ => {
                self.unify(&ra, &rb)?;
                Ok(self.resolve_shallow(&ra))
            }
        }
    }

    pub fn unify(&self, a: &Ty, b: &Ty) -> Result<(), Mismatch> {
        let (ra, rb) = (self.resolve_shallow(a), self.resolve_shallow(b));
        match (&ra, &rb) {
            (Ty::Var(x), Ty::Var(y)) if x == y => Ok(()),
            (Ty::Var(v), other) | (other, Ty::Var(v)) => {
                if other.occurs(*v, self) {
                    return Err(Mismatch::Infinite);
                }
                self.bindings.borrow_mut().insert(*v, other.clone());
                Ok(())
            }
            (Ty::Con(n1, a1), Ty::Con(n2, a2)) => {
                if n1 != n2 || a1.len() != a2.len() {
                    return Err(Mismatch::different(self.resolve(&ra), self.resolve(&rb)));
                }
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Ty::Fun(p1, r1, e1), Ty::Fun(p2, r2, e2)) => {
                if p1.len() != p2.len() {
                    return Err(Mismatch::Arity(p1.len(), p2.len()));
                }
                for (x, y) in p1.iter().zip(p2) {
                    self.unify(x, y)?;
                }
                self.unify(r1, r2)?;
                // Rows *subsume*, they do not equate: §3.1 permits "no subtyping beyond
                // effect-row subsumption", and this is that one exception. The first argument is
                // the actual type and the second the expected one throughout the checker, so a
                // function that does less than its context allows is accepted — which is the whole
                // reason a pure `lambda t: t.done` can be passed where `(a -> b ! e)` is wanted.
                self.subsume_row(e1, e2)
            }
            _ => Err(Mismatch::different(self.resolve(&ra), self.resolve(&rb))),
        }
    }

    /// Require `actual ⊆ expected` — the effect-row subsumption §3.1 permits and nothing else.
    ///
    /// Rows are sets, so this is: whatever the actual row does, the expected row must already
    /// allow, or must have a variable free to absorb it. Concretely, three cases and no more:
    ///
    /// * everything the actual side does is already named on the expected side — nothing to do;
    /// * the expected side has a free row variable — bind it to the difference, leaving a fresh
    ///   variable behind so a *later* call site can widen it again. This is what makes
    ///   `(a -> b ! e)` accept a pure function at one call and an effectful one at the next;
    /// * the expected side is closed and lacks something — the rows genuinely differ, and that is
    ///   the error.
    ///
    /// **The direction is the design.** Equality would make a pure lambda fail to match
    /// `(a -> b ! e)` unless `e` were solved first, and would make the same higher-order function
    /// unusable at two call sites with different arguments. Subsumption over-approximates in one
    /// direction only: a definition's inferred row may be a superset of what one call actually
    /// performs, which can cost a placement candidate but can never lose an effect.
    pub fn subsume_row(&self, actual: &Row, expected: &Row) -> Result<(), Mismatch> {
        let a = self.resolve_row(actual);
        let e = self.resolve_row(expected);

        let missing: BTreeSet<Effect> = a.atoms.difference(&e.atoms).cloned().collect();
        let extra_tails: BTreeSet<RowVarId> = a.tails.difference(&e.tails).copied().collect();
        if missing.is_empty() && extra_tails.is_empty() {
            return Ok(());
        }
        // Deterministically the lowest-numbered free variable, so the same program always produces
        // the same solution — §3.4's determinism guardrail starts here, not at the solver.
        let Some(v) = e.tails.iter().next().copied() else {
            // Naming what is missing is only possible when something *is*. When the actual side's
            // extra is a row **variable** — an unknown row, from a call whose effects are not
            // decided here — the honest report is that the expected side is closed, not a list of
            // nothing. `Effects("")` used to render as "may not perform {}", which fails §4.5 on
            // its own terms: no user can act on it (docs/25 §25.6 item 6).
            if missing.is_empty() {
                return Err(Mismatch::UnknownEffects);
            }
            return Err(Mismatch::Effects(
                missing
                    .iter()
                    .map(|x| x.name())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        };
        let rest = self.fresh_row_var();
        let mut tails = extra_tails;
        tails.insert(rest);
        self.bind_row(
            v,
            Row {
                atoms: missing,
                tails,
            },
        );
        Ok(())
    }

    /// Two rows denote the same set of effects. Used where a signature is *compared* rather than
    /// checked — `.becki` agreement and `--wire-compat`.
    pub fn rows_equal(&self, a: &Row, b: &Row) -> bool {
        self.resolve_row(a) == self.resolve_row(b)
    }

    /// The free variables of a resolved type.
    pub fn free_vars(&self, t: &Ty, out: &mut Vec<TyVarId>) {
        match self.resolve_shallow(t) {
            Ty::Var(v) => {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            Ty::Con(_, args) => {
                for a in &args {
                    self.free_vars(a, out);
                }
            }
            Ty::Fun(ps, r, _) => {
                for p in &ps {
                    self.free_vars(p, out);
                }
                self.free_vars(&r, out);
            }
        }
    }

    /// The free *row* variables of a resolved type — what a definition generalises over.
    pub fn free_row_vars(&self, t: &Ty, out: &mut Vec<RowVarId>) {
        match self.resolve_shallow(t) {
            Ty::Var(_) => {}
            Ty::Con(_, args) => {
                for a in &args {
                    self.free_row_vars(a, out);
                }
            }
            Ty::Fun(ps, r, row) => {
                for p in &ps {
                    self.free_row_vars(p, out);
                }
                self.free_row_vars(&r, out);
                for v in self.resolve_row(&row).tails {
                    if !out.contains(&v) {
                        out.push(v);
                    }
                }
            }
        }
    }
}

fn subst_vars(t: &Ty, m: &BTreeMap<TyVarId, Ty>, rows: &BTreeMap<RowVarId, RowVarId>) -> Ty {
    match t {
        Ty::Var(v) => m.get(v).cloned().unwrap_or(Ty::Var(*v)),
        Ty::Con(n, args) => Ty::Con(
            n.clone(),
            args.iter().map(|a| subst_vars(a, m, rows)).collect(),
        ),
        Ty::Fun(ps, r, row) => Ty::Fun(
            ps.iter().map(|p| subst_vars(p, m, rows)).collect(),
            Box::new(subst_vars(r, m, rows)),
            Row {
                atoms: row.atoms.clone(),
                tails: row
                    .tails
                    .iter()
                    .map(|v| rows.get(v).copied().unwrap_or(*v))
                    .collect(),
            },
        ),
    }
}

/// Replace each rigid type parameter with whatever it was instantiated to.
///
/// A parameter is a nullary `Con`, so this is a leaf substitution: `list[T]` becomes `list[?7]` and
/// a `T` that happens to be applied to arguments is left alone, because a type parameter cannot be
/// a type *constructor* — §27.10 records that as a limit rather than working round it.
fn subst_named(t: &Ty, m: &BTreeMap<Arc<str>, Ty>) -> Ty {
    match t {
        Ty::Var(v) => Ty::Var(*v),
        Ty::Con(n, args) if args.is_empty() => m.get(n).cloned().unwrap_or_else(|| t.clone()),
        Ty::Con(n, args) => Ty::Con(n.clone(), args.iter().map(|a| subst_named(a, m)).collect()),
        Ty::Fun(ps, r, row) => Ty::Fun(
            ps.iter().map(|p| subst_named(p, m)).collect(),
            Box::new(subst_named(r, m)),
            row.clone(),
        ),
    }
}

#[derive(Clone, Debug)]
pub enum Mismatch {
    /// Boxed because a `Ty` is a tree and this is the *error* path: making every successful
    /// unification carry the size of a failed one is the wrong trade.
    Different(Box<(Ty, Ty)>),
    Arity(usize, usize),
    Infinite,
    /// Two function types agree on their arguments and result but not on what they do.
    Effects(String),
    /// The same, where what the actual side may do is not yet known — an unsolved row variable
    /// against a context whose row is closed. Distinct from [`Mismatch::Effects`] because there is
    /// no effect to name, and a message that names none is one no user can act on.
    UnknownEffects,
}

impl Mismatch {
    fn different(a: Ty, b: Ty) -> Mismatch {
        Mismatch::Different(Box::new((a, b)))
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Var(v) => write!(f, "?{v}"),
            Ty::Con(n, args) if args.is_empty() => write!(f, "{n}"),
            Ty::Con(n, args) => {
                write!(f, "{n}[")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, "]")
            }
            Ty::Fun(ps, r, row) => {
                write!(f, "(")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {r}")?;
                // A pure function prints without a row: `! {}` on every signature would be noise,
                // and §3.2 elides the ambient set for the same reason.
                let visible: Vec<String> = row.visible().iter().map(|e| e.name()).collect();
                if visible.is_empty() && row.tails.is_empty() {
                    Ok(())
                } else {
                    let tails: Vec<String> = row.tails.iter().map(|v| format!("e{v}")).collect();
                    let parts = if tails.is_empty() {
                        visible.join(", ")
                    } else if visible.is_empty() {
                        tails.join(" | ")
                    } else {
                        format!("{} | {}", visible.join(", "), tails.join(" | "))
                    };
                    write!(f, " ! {{{parts}}}")
                }
            }
        }
    }
}

/// The base a declaration's type parameters are numbered from.
///
/// The *n*th parameter of a declaration is `Ty::Var(SCHEME_BASE + n)` wherever it appears in that
/// declaration's field types, so instantiating `Tree[Str]` is an index rather than a search. The
/// base is far above any unification variable the checker will mint, which is what lets one `Ty`
/// carry both without a tag.
pub const SCHEME_BASE: u32 = 1_000_000;

/// A user-declared type: a `model` (record), a `union` (ADT), a `newtype`, or an alias.
///
/// Comparable because a `.becki` interface is compared (§3.6, §4.3): two builds agree on a module's
/// contract exactly when their type declarations are equal.
///
/// `params` is the declaration's type-parameter *names*, in order — `union Tree[T]` has `["T"]`.
/// The names are what a `.becki` renders and what a doc page shows; the field types refer to the
/// parameters positionally through [`SCHEME_BASE`], so a rename is a rename and nothing more.
/// Arity is `params.len()`, declared rather than inferred from use: a parameter no field mentions
/// is still a parameter, and `Phantom[Int]` and `Phantom[Str]` are still different types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TyDecl {
    Model {
        name: Arc<str>,
        params: Vec<Arc<str>>,
        fields: Vec<(Arc<str>, Ty)>,
    },
    Union {
        name: Arc<str>,
        params: Vec<Arc<str>>,
        variants: Vec<Variant>,
    },
    /// §3.1's "zero-cost nominal newtype": ids of different entities must not be interchangeable.
    Newtype {
        name: Arc<str>,
        params: Vec<Arc<str>>,
        inner: Ty,
    },
    Alias {
        name: Arc<str>,
        params: Vec<Arc<str>>,
        ty: Ty,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: Arc<str>,
    pub fields: Vec<(Arc<str>, Ty)>,
}

impl TyDecl {
    pub fn name(&self) -> &Arc<str> {
        match self {
            TyDecl::Model { name, .. }
            | TyDecl::Union { name, .. }
            | TyDecl::Newtype { name, .. }
            | TyDecl::Alias { name, .. } => name,
        }
    }

    pub fn params(&self) -> &[Arc<str>] {
        match self {
            TyDecl::Model { params, .. }
            | TyDecl::Union { params, .. }
            | TyDecl::Newtype { params, .. }
            | TyDecl::Alias { params, .. } => params,
        }
    }

    /// How many type arguments a mention of this name must carry.
    pub fn arity(&self) -> usize {
        self.params().len()
    }

    /// `[T, U]`, or the empty string when there is nothing to quantify.
    pub fn param_brackets(&self) -> String {
        if self.params().is_empty() {
            return String::new();
        }
        format!(
            "[{}]",
            self.params()
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    /// One of this declaration's field types as the declaration wrote it: the positional
    /// parameters put back under the names they were given.
    ///
    /// Everything downstream of a `TyDecl` holds its parameters positionally, which is what makes
    /// instantiation an index. Rendering is the one place that has to undo it — a `.becki` is
    /// *source*, and `value: ?1000000` is not something the parser can read back.
    pub fn as_written(&self, t: &Ty) -> Ty {
        if self.params().is_empty() {
            return t.clone();
        }
        let args: Vec<Ty> = self.params().iter().map(|p| Ty::con(p)).collect();
        instantiate_decl(t, &args)
    }
}

/// A published `trait`: the signatures it requires, over an abstract `Self`.
///
/// The checker keeps a trait's methods as **syntax** while it is desugaring impls and bounds, because
/// splicing is what that pass does. This is the other half: the same declaration as *types*, which
/// is what a `.becki` compares, what `--wire-compat` classifies, and what an importing module reads.
/// A trait that crossed a boundary as syntax would carry spans into a file that does not own them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraitSig {
    pub name: Arc<str>,
    pub methods: Vec<MethodSig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodSig {
    pub name: Arc<str>,
    /// Parameter names and types, with the abstract receiver as `Ty::con("Self")`.
    pub params: Vec<(Arc<str>, Ty)>,
    pub ret: Ty,
    /// The declared row — the bound every implementation is held to (`docs/27` §27.7), and
    /// therefore what a caller in another module may assume.
    pub effects: Vec<Effect>,
}

/// A published `impl Trait for Type`.
///
/// There is no body here and there never will be: an importing module needs to know *that* the
/// implementation exists and what its signature is, and the implementation itself stays where it
/// was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplSig {
    pub trait_name: Arc<str>,
    /// The impl's own type parameters: `["T"]` for `impl[T] Priced for Bundle[T]`.
    pub params: Vec<Arc<str>>,
    /// The target, with those parameters as rigid names — `Bundle[T]`.
    pub target: Ty,
    /// What each method actually performs, by name, for the methods that perform anything.
    ///
    /// The trait declares an abstract signature; this impl's methods may be **more** effectful than
    /// it (`docs/27`), so a caller in another module cannot take the row off the trait. It has to
    /// be published with the impl, and this is where it crosses. Empty rows are omitted: most
    /// impls are pure and a `.becki` full of `uses` clauses saying nothing is a `.becki` nobody
    /// reviews.
    pub effects: Vec<(Arc<str>, Vec<Effect>)>,
}

impl ImplSig {
    /// The head constructor dispatch keys on.
    pub fn head(&self) -> Arc<str> {
        self.target
            .con_name()
            .map(Arc::from)
            .unwrap_or_else(|| Arc::from("?"))
    }
}

/// Replace the positional parameters of a declaration with `args`.
///
/// A field type of `Some(value: ?1000000)` under `Option[Int]` is `value: Int`, and every pass that
/// reads a declaration's fields against a concrete type goes through here.
pub fn instantiate_decl(t: &Ty, args: &[Ty]) -> Ty {
    match t {
        Ty::Var(v) if *v >= SCHEME_BASE => args
            .get((*v - SCHEME_BASE) as usize)
            .cloned()
            .unwrap_or_else(|| t.clone()),
        Ty::Var(_) => t.clone(),
        Ty::Con(n, xs) => Ty::Con(
            n.clone(),
            xs.iter().map(|x| instantiate_decl(x, args)).collect(),
        ),
        Ty::Fun(ps, r, row) => Ty::Fun(
            ps.iter().map(|x| instantiate_decl(x, args)).collect(),
            Box::new(instantiate_decl(r, args)),
            row.clone(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unification_binds_and_propagates() {
        let s = Subst::new();
        let a = s.fresh();
        assert!(s.unify(&a, &Ty::int()).is_ok());
        assert_eq!(s.resolve(&a), Ty::int());
        assert!(s.unify(&a, &Ty::str_()).is_err());
    }

    #[test]
    fn structural_unification_descends() {
        let s = Subst::new();
        let a = s.fresh();
        let b = s.fresh();
        assert!(s
            .unify(
                &Ty::map(a.clone(), b.clone()),
                &Ty::map(Ty::int(), Ty::str_())
            )
            .is_ok());
        assert_eq!(s.resolve(&a), Ty::int());
        assert_eq!(s.resolve(&b), Ty::str_());
    }

    #[test]
    fn the_occurs_check_rejects_infinite_types() {
        let s = Subst::new();
        let a = s.fresh();
        assert!(matches!(
            s.unify(&a, &Ty::list(a.clone())),
            Err(Mismatch::Infinite)
        ));
    }

    #[test]
    fn instantiation_is_fresh_per_use() {
        let s = Subst::new();
        let v = 0;
        let scheme = Scheme {
            params: Vec::new(),
            vars: vec![v],
            row_vars: Vec::new(),
            ty: Ty::fun(vec![Ty::Var(v)], Ty::Var(v)),
        };
        let one = s.instantiate(&scheme);
        let two = s.instantiate(&scheme);
        if let Ty::Fun(ps, _, _) = &one {
            assert!(s.unify(&ps[0], &Ty::int()).is_ok());
        }
        if let Ty::Fun(ps, _, _) = &two {
            assert!(s.unify(&ps[0], &Ty::str_()).is_ok());
        }
    }

    #[test]
    fn the_tier_table_matches_the_design() {
        // §3.3: client cannot discharge `ingress` or `durable`; server cannot discharge `dom`.
        assert!(!Tier::Client.discharges(&Effect::Ingress));
        assert!(!Tier::Client.discharges(&Effect::Durable));
        assert!(Tier::Client.discharges(&Effect::Dom));
        assert!(Tier::Server.discharges(&Effect::Ingress));
        assert!(Tier::Server.discharges(&Effect::Durable));
        assert!(!Tier::Server.discharges(&Effect::Dom));
        assert!(Tier::Data.discharges(&Effect::Durable));
        assert!(!Tier::Data.discharges(&Effect::Nondet));
        // `any` means unplaced-pure: it discharges exactly the intersection, so anything a tier
        // refuses forces a placement, and an ambient effect never does.
        assert!(!Tier::Any.discharges(&Effect::Durable));
        assert!(Tier::Any.discharges(&Effect::Ambient(Ambient::Log)));
    }

    #[test]
    fn only_the_own_origin_is_reachable_from_a_browser() {
        // §3.3's table says `net.out(own-origin)` and means it: a client that could name any host
        // would be a placement decision made by CORS at runtime.
        assert!(Tier::Client.discharges(&Effect::NetOut(Arc::from("origin"))));
        assert!(!Tier::Client.discharges(&Effect::NetOut(Arc::from("payments.example.com"))));
        assert!(Tier::Server.discharges(&Effect::NetOut(Arc::from("payments.example.com"))));
    }

    #[test]
    fn a_closed_row_accepts_less_and_refuses_more() {
        let s = Subst::new();
        assert!(s
            .subsume_row(&Row::of([Effect::Dom]), &Row::of([Effect::Dom]))
            .is_ok());
        // Doing less than the context allows is fine — that is subsumption.
        assert!(s
            .subsume_row(&Row::empty(), &Row::of([Effect::Dom]))
            .is_ok());
        // Doing something the context does not allow is the error.
        assert!(s
            .subsume_row(&Row::of([Effect::Dom]), &Row::of([Effect::Durable]))
            .is_err());
    }

    #[test]
    fn an_open_row_absorbs_what_is_passed_to_it_twice() {
        // The everyday case: `map_list`'s `(a -> b ! e)` meets a lambda that touches the dom, and
        // then — at another call site through the same monomorphic parameter — a pure one.
        let s = Subst::new();
        let e = s.fresh_row();
        assert!(s.subsume_row(&Row::of([Effect::Dom]), &e).is_ok());
        assert!(s.subsume_row(&Row::empty(), &e).is_ok());
        assert!(s.subsume_row(&Row::of([Effect::Durable]), &e).is_ok());
        assert_eq!(
            s.resolve_row(&e).atoms,
            BTreeSet::from([Effect::Dom, Effect::Durable]),
            "a row variable widened at two call sites holds the union, never a contradiction"
        );
    }

    #[test]
    fn effect_polymorphism_carries_a_callers_row_to_the_result() {
        // `map_list : (list[a], (a -> b ! e)) -> list[b] ! e`, applied to an effectful function,
        // must make the *application* effectful. That is the whole point of the row variable.
        let s = Subst::new();
        let e = s.fresh_row_var();
        let scheme = Scheme {
            params: Vec::new(),
            vars: vec![],
            row_vars: vec![e],
            ty: Ty::fun_eff(
                vec![Ty::fun_eff(vec![Ty::int()], Ty::int(), Row::var(e))],
                Ty::int(),
                Row::var(e),
            ),
        };
        let Ty::Fun(params, _, latent) = s.instantiate(&scheme) else {
            panic!("a function");
        };
        // Pass something that mints ids. The actual is the argument, the expected the parameter.
        assert!(s
            .unify(
                &Ty::fun_eff(vec![Ty::int()], Ty::int(), Row::of([Effect::Nondet])),
                &params[0],
            )
            .is_ok());
        // The trailing variable is deliberate: subsumption leaves room for a *later* call site to
        // widen the same row. What matters is that the atom arrived.
        assert_eq!(
            s.resolve_row(&latent).atoms,
            BTreeSet::from([Effect::Nondet])
        );
    }

    #[test]
    fn a_recursive_row_resolves_to_its_least_fixed_point_rather_than_diverging() {
        // Two mutually recursive effectful functions: `r_f = {dom} ∪ r_g`, `r_g = {durable} ∪ r_f`.
        let s = Subst::new();
        let (f, g) = (s.fresh_row_var(), s.fresh_row_var());
        s.bind_row(f, Row::of([Effect::Dom]).union(&Row::var(g)));
        s.bind_row(g, Row::of([Effect::Durable]).union(&Row::var(f)));
        let resolved = s.resolve_row(&Row::var(f));
        assert_eq!(resolved, Row::of([Effect::Dom, Effect::Durable]));
        assert!(resolved.is_closed());
    }

    #[test]
    fn a_pure_function_prints_without_a_row_and_an_effectful_one_with_it() {
        assert_eq!(
            Ty::fun(vec![Ty::int()], Ty::int()).to_string(),
            "(Int) -> Int"
        );
        assert_eq!(
            Ty::fun_eff(vec![], Ty::unit(), Row::of([Effect::Durable])).to_string(),
            "() -> Unit ! {durable}"
        );
        // Ambient effects are elided from the signature, exactly as §3.2 says.
        assert_eq!(
            Ty::fun_eff(
                vec![],
                Ty::unit(),
                Row::of([Effect::Ambient(Ambient::Log), Effect::Dom])
            )
            .to_string(),
            "() -> Unit ! {dom}"
        );
    }
}
