//! Types, unification, and the tier lattice.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../docs/03-type-and-effect-system.md) §3.1:
//! "Hindley–Milner inference with bidirectional checking. Full inference inside bodies; mandatory
//! annotations on public signatures." §3.10 stage 1 is what Phase 1 builds: "HM + ADTs + traits;
//! `Stream`/`Signal`/`fold`/`durable` typed but **placement fully manual** (`@on`), matching the
//! original sketch exactly."
//!
//! Two deliberate omissions, both Phase 2 by the staging in §3.10, and both named in the Phase 1
//! report rather than quietly skipped: **row polymorphism** on records (models are nominal here)
//! and **inferred effect rows** (effects are declared, and collected from what a body calls).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

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

    /// Can this tier discharge that effect? §3.3's table, verbatim.
    pub fn discharges(self, e: &Effect) -> bool {
        match (self, e) {
            (Tier::Any, _) => false,
            (Tier::Client, Effect::Dom) => true,
            (Tier::Client, _) => false,
            (Tier::Server, Effect::Dom) => false,
            (Tier::Server, _) => true,
            (Tier::Data, Effect::Durable) => true,
            (Tier::Data, _) => false,
        }
    }
}

/// The effect atoms Phase 1 knows about.
///
/// §3.2 lists many more (`net.out(host)`, `fs(path)`, `cap.X`, …). Phase 1 carries the ones the
/// walking skeleton actually needs to *decide* something — placement legality and fold purity —
/// because a checker that pretends to know about `net.out` without inference would be a lie in the
/// signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Effect {
    /// A merge point: arbitrary interleaving. "there is exactly one of these."
    Ingress,
    /// A persistent accumulator.
    Durable,
    /// Touches the document.
    Dom,
    /// Reads a clock, a random source, or mints an id — the three things §3.7 forbids inside a
    /// fold, which is what makes replay bit-identical.
    Nondeterministic,
}

impl Effect {
    pub fn name(self) -> &'static str {
        match self {
            Effect::Ingress => "ingress",
            Effect::Durable => "durable",
            Effect::Dom => "dom",
            Effect::Nondeterministic => "nondet",
        }
    }

    pub fn parse(s: &str) -> Option<Effect> {
        Some(match s {
            "ingress" => Effect::Ingress,
            "durable" => Effect::Durable,
            "dom" => Effect::Dom,
            "nondet" | "nondeterministic" => Effect::Nondeterministic,
            _ => return None,
        })
    }
}

/// A set of effect atoms. Phase 1 has no row variables — see the module docs.
pub type Effects = std::collections::BTreeSet<Effect>;

pub type TyVarId = u32;

/// A unification variable's binding, shared so that unifying in one place is visible everywhere.
#[derive(Clone, Debug, Default)]
pub struct Subst {
    bindings: Rc<RefCell<BTreeMap<TyVarId, Ty>>>,
    next: Rc<RefCell<TyVarId>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Var(TyVarId),
    /// A named type constructor applied to arguments: `Int`, `list[T]`, `Map[K,V]`, `Signal[T]`,
    /// and every user `model`/`union`/`newtype`.
    Con(Arc<str>, Vec<Ty>),
    Fun(Vec<Ty>, Box<Ty>),
}

impl Ty {
    pub fn con(name: &str) -> Ty {
        Ty::Con(Arc::from(name), Vec::new())
    }

    pub fn app(name: &str, args: Vec<Ty>) -> Ty {
        Ty::Con(Arc::from(name), args)
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
            Ty::Fun(ps, r) => ps.iter().any(|a| a.occurs(v, s)) || r.occurs(v, s),
        }
    }
}

/// A type scheme: `forall vars. ty`. Let-polymorphism, and the reason the standard library can be
/// one library rather than one per tier.
#[derive(Clone, Debug)]
pub struct Scheme {
    pub vars: Vec<TyVarId>,
    pub ty: Ty,
}

impl Scheme {
    pub fn mono(ty: Ty) -> Scheme {
        Scheme {
            vars: Vec::new(),
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
            Ty::Fun(ps, r) => Ty::Fun(
                ps.iter().map(|p| self.resolve(p)).collect(),
                Box::new(self.resolve(&r)),
            ),
        }
    }

    /// Instantiate a scheme with fresh variables.
    pub fn instantiate(&self, s: &Scheme) -> Ty {
        if s.vars.is_empty() {
            return s.ty.clone();
        }
        let mapping: BTreeMap<TyVarId, Ty> = s.vars.iter().map(|v| (*v, self.fresh())).collect();
        subst_vars(&s.ty, &mapping)
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
                    return Err(Mismatch::Different(self.resolve(&ra), self.resolve(&rb)));
                }
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Ty::Fun(p1, r1), Ty::Fun(p2, r2)) => {
                if p1.len() != p2.len() {
                    return Err(Mismatch::Arity(p1.len(), p2.len()));
                }
                for (x, y) in p1.iter().zip(p2) {
                    self.unify(x, y)?;
                }
                self.unify(r1, r2)
            }
            _ => Err(Mismatch::Different(self.resolve(&ra), self.resolve(&rb))),
        }
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
            Ty::Fun(ps, r) => {
                for p in &ps {
                    self.free_vars(p, out);
                }
                self.free_vars(&r, out);
            }
        }
    }
}

fn subst_vars(t: &Ty, m: &BTreeMap<TyVarId, Ty>) -> Ty {
    match t {
        Ty::Var(v) => m.get(v).cloned().unwrap_or(Ty::Var(*v)),
        Ty::Con(n, args) => Ty::Con(n.clone(), args.iter().map(|a| subst_vars(a, m)).collect()),
        Ty::Fun(ps, r) => Ty::Fun(
            ps.iter().map(|p| subst_vars(p, m)).collect(),
            Box::new(subst_vars(r, m)),
        ),
    }
}

#[derive(Clone, Debug)]
pub enum Mismatch {
    Different(Ty, Ty),
    Arity(usize, usize),
    Infinite,
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
            Ty::Fun(ps, r) => {
                write!(f, "(")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {r}")
            }
        }
    }
}

/// A user-declared type: a `model` (record), a `union` (ADT), a `newtype`, or an alias.
#[derive(Clone, Debug)]
pub enum TyDecl {
    Model {
        name: Arc<str>,
        fields: Vec<(Arc<str>, Ty)>,
    },
    Union {
        name: Arc<str>,
        variants: Vec<Variant>,
    },
    /// §3.1's "zero-cost nominal newtype": ids of different entities must not be interchangeable.
    Newtype {
        name: Arc<str>,
        inner: Ty,
    },
    Alias {
        name: Arc<str>,
        ty: Ty,
    },
}

#[derive(Clone, Debug)]
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
            vars: vec![v],
            ty: Ty::Fun(vec![Ty::Var(v)], Box::new(Ty::Var(v))),
        };
        let one = s.instantiate(&scheme);
        let two = s.instantiate(&scheme);
        // Using the first at Int must not constrain the second.
        if let Ty::Fun(ps, _) = &one {
            assert!(s.unify(&ps[0], &Ty::int()).is_ok());
        }
        if let Ty::Fun(ps, _) = &two {
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
        // `any` means unplaced-pure: it discharges nothing, so anything with an effect must be
        // placed somewhere concrete.
        assert!(!Tier::Any.discharges(&Effect::Durable));
    }
}
