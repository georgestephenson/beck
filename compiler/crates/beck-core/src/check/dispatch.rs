//! Calling an impl whose own type parameters carry bounds.
//!
//! [`traits`](super::traits) has two ways to reach an implementation and they are deliberately the
//! same shape: `Checker::dictionary` answers with the impl's own global when the receiver is a
//! concrete type, and with a dictionary parameter when it is a bounded type parameter. Neither
//! answer says anything about the *implementation* needing a dictionary of its own — and
//! `impl[T: Ord] Ranked for list[T]` is exactly that. Its method is lowered by
//! `Checker::expand_bounds` like any other bounded definition, so `Ranked::rising@list` takes the
//! list **and** an `Ord::less@T`, and a call site that applied it to the receiver alone was told it
//! had passed one argument to something that takes two.
//!
//! # Why this is dispatch rather than a repair
//!
//! A bounded `def` knows what its dictionaries are for: `check/traits.rs`'s `apply_bounded` reads
//! the call's own type arguments back out of `Subst::instantiate_named` and looks an impl up at
//! each. A bounded *impl* has no such call. What the call site names is a trait method, and which
//! implementation runs is decided by the receiver — so learning what `T` is means **matching the
//! impl's target against the receiver's type**, which is a dispatch question and is why this is a
//! module rather than a line.
//!
//! Once `T` is known the rest is `apply_bounded`'s: one dictionary per method of each bound,
//! resolved through the same `Checker::dictionary` every other bound goes through, so an impl
//! bounded on a trait implemented by another bounded impl composes without a second rule
//! ([`Checker::dictionary_value`] is that case, and is the one that needs a closure).
//!
//! # A new file rather than four more screens of `check/mod.rs`
//!
//! [`docs/08`](../../../../../../docs/08-roadmap.md) §8.5.5 keeps Lane A serial because two language
//! features rewrite `check/mod.rs` and `ty.rs` together, and names the mitigation that works:
//! traits went into `check/traits.rs` rather than into `check/mod.rs`, and bounds then grew *that*
//! file. This is the same move again — nothing here needs to be in either of the two files
//! everybody else has to touch.

use std::sync::Arc;

use beck_diag::Span;

use super::traits::{mangle, DictParam};
use super::Checker;
use crate::core::{Core, CoreKind, VarId};
use crate::ty::{ImplSig, Scheme, Ty};
use beck_syntax::Node;

/// How deep a dictionary may be built out of other dictionaries.
///
/// `impl[T: ToJson] ToJson for list[T]` at `list[list[list[Int]]]` builds three, one inside the
/// next, and each step strips a constructor off a type that was written down — so the bound is a
/// property of the *source* and not of anything the checker invents. It is here anyway, because a
/// guard that costs one comparison is cheaper than trusting that argument to keep holding: the
/// stack this recursion runs on is the host's.
const MAX_DICTIONARY_DEPTH: usize = 32;

impl Checker<'_> {
    /// The implementation a **concrete** receiver selects, applied with the dictionaries the impl's
    /// own bounds ask for.
    ///
    /// `None` when this is not that case — an unbounded impl, a receiver that is still a type
    /// parameter, or no impl at all — and every one of those is somebody else's answer to give, so
    /// the caller falls through to the ordinary path rather than reporting anything here.
    pub(super) fn apply_bounded_impl(
        &mut self,
        trait_name: &Arc<str>,
        method: &Arc<str>,
        receiver: &Core,
        at: usize,
        args: &[Node],
        span: Span,
    ) -> Option<Core> {
        let ty = self.subst.resolve(&receiver.ty);
        let (name, specs, scheme, _) = self.bounded_impl(trait_name, method, &ty)?;
        let (fun, named) = self.subst.instantiate_named(&scheme);
        let Ty::Fun(param_tys, ret, latent) = fun else {
            return None;
        };
        self.perform(&latent);

        let ordinary = param_tys.len().saturating_sub(specs.len());
        if args.len() != ordinary {
            self.error(
                "B0351",
                format!("expected {ordinary} argument(s), got {}", args.len()),
                span,
            );
        }
        // The receiver is already checked — `trait_call` had to infer it to know which impl this
        // is — so it is spliced in rather than checked again, and unifying it is what tells the
        // instantiation what `T` is. Every dictionary below is resolved *after* that, for the
        // reason `apply_bounded` gives: until the receiver has been matched, `T` is a variable and
        // there is nothing to look an impl up by.
        let mut checked = Vec::with_capacity(param_tys.len());
        for (i, a) in args.iter().enumerate() {
            if i == at {
                if let Some(want) = param_tys.get(i) {
                    self.unify(&receiver.ty, want, receiver.span, "receiver");
                }
                checked.push(receiver.clone());
                continue;
            }
            checked.extend(self.check_args(std::slice::from_ref(a), &param_tys[i..]));
        }
        checked.extend(self.impl_dictionaries(&specs, &named, &param_tys[ordinary..], span, 0));

        let func = Core::new(
            CoreKind::Global(name),
            Ty::Fun(param_tys, ret.clone(), latent),
            span,
        );
        Some(Core::new(
            CoreKind::App {
                func: Box::new(func),
                args: checked,
            },
            *ret,
            span,
        ))
    }

    /// One implementation of one trait method at one type, as something **callable with one
    /// argument list** — the value a bounded definition is handed for its dictionary parameter.
    ///
    /// `Checker::dictionary` answers with the impl's global, which is right until that impl has
    /// bounds of its own: `ToJson::to_json@list` takes a list *and* an `Ord::to_json@T`, and a
    /// caller expecting `fn(list[Int]) -> Json` cannot be given it. So it is closed over — the
    /// dictionaries the impl needs are resolved here, at the type the caller asked for, and what
    /// comes back has the arity the bound published.
    ///
    /// `None` again means *not this case*, and the caller uses the plain global.
    pub(super) fn dictionary_value(
        &mut self,
        trait_name: &Arc<str>,
        method: &Arc<str>,
        ty: &Ty,
        span: Span,
        depth: usize,
    ) -> Option<Core> {
        if depth >= MAX_DICTIONARY_DEPTH {
            return None;
        }
        let (name, specs, scheme, sig) = self.bounded_impl(trait_name, method, ty)?;
        let (fun, named) = self.subst.instantiate_named(&scheme);
        let Ty::Fun(param_tys, ret, latent) = fun else {
            return None;
        };

        // The impl's *declared* target against the type the caller wants an implementation for —
        // `list[T]` against `list[Int]` — which is what says `T` is `Int`. Matched rather than
        // unified against a parameter position: which parameter carries the target is the trait's
        // business, and a method whose `Self` sits inside another constructor would have made a
        // guess wrong.
        let mut bound = std::collections::BTreeMap::new();
        match_target(&sig.target, ty, &sig.params, &mut bound);
        for (param, at) in &bound {
            if let Some(var) = named.get(param) {
                let _ = self.subst.unify(var, at);
            }
        }

        let ordinary = param_tys.len().saturating_sub(specs.len());
        let dicts = self.impl_dictionaries(&specs, &named, &param_tys[ordinary..], span, depth + 1);
        if dicts.len() != specs.len() {
            return None;
        }

        let mut params: Vec<VarId> = Vec::with_capacity(ordinary);
        let mut passed: Vec<Core> = Vec::with_capacity(param_tys.len());
        for t in &param_tys[..ordinary] {
            let v = self.fresh_var();
            params.push(v);
            passed.push(Core::new(CoreKind::Var(v), t.clone(), span));
        }
        passed.extend(dicts);

        let func = Core::new(
            CoreKind::Global(name),
            Ty::Fun(param_tys.clone(), ret.clone(), latent.clone()),
            span,
        );
        let body = Core::new(
            CoreKind::App {
                func: Box::new(func),
                args: passed,
            },
            (*ret).clone(),
            span,
        );
        Some(Core::new(
            CoreKind::Lam {
                params: Arc::from(params),
                body: Arc::new(body),
            },
            Ty::Fun(param_tys[..ordinary].to_vec(), ret, latent),
            span,
        ))
    }

    /// The impl a type selects, when that impl's own parameters carry bounds: its mangled name,
    /// what its bounds ask for, and the signature `expand_bounds` gave it.
    ///
    /// Keyed on the head constructor, which is what coherence keys on — `list[Int]` and
    /// `list[Str]` share one impl, so this is a lookup rather than a search.
    fn bounded_impl(
        &mut self,
        trait_name: &Arc<str>,
        method: &Arc<str>,
        ty: &Ty,
    ) -> Option<(Arc<str>, Vec<DictParam>, Scheme, ImplSig)> {
        let head = ty.con_name().map(Arc::<str>::from)?;
        // A bounded *parameter* is the other kind of answer entirely: the implementation arrived
        // as a dictionary and there is nothing to supply it with.
        if self.typarams.contains(&head) {
            return None;
        }
        let found = self.impls.get(&(trait_name.clone(), head))?;
        let sig = found.sig.clone();
        let name = mangle(trait_name, method, &found.target);
        let specs = self.dicts.get(&name)?.clone();
        let scheme = self.schemes.get(&name)?.clone();
        Some((name, specs, scheme, sig))
    }

    /// A dictionary at one type, closed over its own if the implementation is itself bounded.
    ///
    /// The one entry point a *bound* resolves through, and the reason the two answers stay one
    /// mechanism: what comes back always has the arity the bound published, whether the
    /// implementation needed anything to get there or not.
    pub(super) fn dictionary_at(
        &mut self,
        trait_name: &Arc<str>,
        method: &Arc<str>,
        ty: &Ty,
        span: Span,
        depth: usize,
    ) -> Option<Core> {
        match self.dictionary_value(trait_name, method, ty, span, depth) {
            Some(closed) => Some(closed),
            None => self.dictionary(trait_name, method, ty, span),
        }
    }

    /// One dictionary per method of each of the impl's bounds, at the types the receiver fixed.
    ///
    /// The same [`Checker::dictionary`] every bound resolves through, so an impl bounded on a
    /// trait whose implementation is *itself* bounded composes without a rule of its own — that
    /// case comes back here through [`Checker::dictionary_value`], one constructor shallower each
    /// time.
    fn impl_dictionaries(
        &mut self,
        specs: &[DictParam],
        named: &std::collections::BTreeMap<Arc<str>, Ty>,
        want: &[Ty],
        span: Span,
        depth: usize,
    ) -> Vec<Core> {
        let mut out = Vec::with_capacity(specs.len());
        for (i, spec) in specs.iter().enumerate() {
            let at = named
                .get(&spec.param)
                .map(|t| self.subst.resolve(t))
                .unwrap_or_else(|| self.subst.fresh());
            let Some(dict) = self.dictionary_at(&spec.trait_name, &spec.method, &at, span, depth)
            else {
                continue;
            };
            if let Some(w) = want.get(i) {
                self.unify(&dict.ty, w, span, "implementation");
            }
            out.push(dict);
        }
        out
    }
}

/// What an impl's parameters are, at a type its target matches: `list[T]` against `list[Int]`
/// binds `T` to `Int`.
///
/// A match rather than a unification, because the target is the *declaration's* — its parameters
/// are rigid `Ty::Con`s that would otherwise be unified with as though they were variables, and
/// the impl is one implementation covering every argument rather than a constraint on this call.
/// Nothing is reported when the shapes disagree: coherence keyed the lookup on the head
/// constructor, so a target that does not match here is a program the arity check has already
/// refused.
fn match_target(
    target: &Ty,
    actual: &Ty,
    params: &[Arc<str>],
    out: &mut std::collections::BTreeMap<Arc<str>, Ty>,
) {
    match (target, actual) {
        (Ty::Con(name, args), _) if args.is_empty() && params.contains(name) => {
            out.insert(name.clone(), actual.clone());
        }
        (Ty::Con(name, args), Ty::Con(other, actuals))
            if name == other && args.len() == actuals.len() =>
        {
            for (a, b) in args.iter().zip(actuals) {
                match_target(a, b, params, out);
            }
        }
        _ => {}
    }
}
