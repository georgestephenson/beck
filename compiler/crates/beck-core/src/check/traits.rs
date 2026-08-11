//! `trait` and `impl`, checked.
//!
//! `docs/03-type-and-effect-system.md` §3.1 asks for "traits/typeclasses with coherence (orphan
//! rule)", and `docs/11-language-tour.md` §11.3 writes what they look like. Until now the parser
//! read both forms and the checker warned that it did nothing with them — the oldest unpaid debt in
//! the project, named by four reports and by the one refusal file left in `sicp/refusals/`.
//!
//! # The shape of it
//!
//! A **trait** is a set of signatures over an abstract `Self`:
//!
//! ```text
//! trait Show:
//!     def show(self) -> Str
//!     def tagged(self, prefix: Str) -> Str uses log
//! ```
//!
//! An **impl** supplies the bodies, and writes no types at all:
//!
//! ```text
//! impl[T] Show for Tree[T]:
//!     def show(self):
//!         return "a tree"
//! ```
//!
//! The impl's method writes *names*; the trait already wrote the types, the return type and the
//! effect row, and repeating them would be a second place for them to disagree. So an annotation in
//! an impl is refused rather than checked (`B0362`), and the note says where the signature lives.
//!
//! # How it is compiled
//!
//! An impl is **desugared into ordinary top-level definitions** before anything is checked. Each
//! method becomes a `def` whose name is [`mangle`]d — `Show::show@Tree`, which no source identifier
//! can collide with — whose parameter types come from the trait with `Self` replaced by the impl's
//! target, and whose type parameters are the impl's. From there it is a definition like any other:
//! `collect_signatures` gives it a scheme, `check_items` checks its body, placement places it, the
//! effect row is inferred and bounded by the trait's declared one, and the evaluator calls it
//! through `CoreKind::Global`.
//!
//! That is why this pass adds **no IR node and no evaluator case**. Dispatch is static: a call
//! `p.show()` resolves at check time from the type of `p` to exactly one mangled global.
//!
//! # Bounds, and the dictionary that is not a data structure
//!
//! `def largest[T: Ord](xs: list[T]) -> Option[T]` carries a **bound**, and it is lowered by the
//! same trick: the definition gains one ordinary parameter per method of each bound, named exactly
//! as an impl method is named but with the *type parameter* as the target — `Ord::before@T`. Inside
//! the body, `a.before(b)` resolves `Ord::before@T` and finds a **local**; at a call site with
//! `T := Int` the caller passes `Ord::before@Int`, which is a **global**. One name scheme, two kinds
//! of binding, and one resolution rule that reads both:
//!
//! ```text
//! def largest[T: Ord](xs: list[T]) -> Option[T]
//!   ⇒ def largest[T](xs: list[T], Ord::before@T: (T, T) -> Bool) -> Option[T]
//!
//! largest([3, 1])   ⇒   largest([3, 1], Ord::before@Int)
//! ```
//!
//! A dictionary is therefore not a record and not a runtime value of its own — it is a function
//! argument — so bounds add no IR node either. A bounded definition calling another passes its own
//! parameter straight through, which is what makes the recursion terminate.
//!
//! # What it is not
//!
//! **A trait does not cross a module boundary.** A `.becki` publishes neither traits nor impls, and
//! `Interface::of` drops both the mangled definitions and any bounded one rather than publishing a
//! signature whose dictionary parameters no source could name.
//!
//! **A bounded definition cannot be passed as a value.** Its dictionaries are supplied at the call
//! site, and a reference that is never called has no call site to supply them.

use std::collections::BTreeSet;
use std::sync::Arc;

use beck_diag::{Diagnostic, Span};
use beck_syntax::{sym, Node, ScopeSet, Symbol};

use super::{BindKind, Binding, Checker};
use crate::core::{Const, Core, CoreKind};
use crate::ty::{ImplSig, MethodSig, Row, Scheme, TraitSig, Ty, TyDecl};

/// The separator that makes a desugared impl method unnameable from source.
///
/// `::` and `@` are not identifier characters in either surface, so `Show::show@Tree` cannot
/// collide with anything a program declares, and a stray one in a diagnostic is recognisable as
/// compiler-generated rather than as something the author wrote.
pub(crate) fn mangle(trait_name: &str, method: &str, target: &str) -> Arc<str> {
    Arc::from(format!("{trait_name}::{method}@{target}"))
}

/// Is this the name of a desugared impl method rather than something a program wrote?
pub fn is_impl_method(name: &str) -> bool {
    name.contains("::") && name.contains('@')
}

/// A `trait` declaration: the signatures it requires, as written.
///
/// The signature is kept as **syntax** rather than as a `Ty` because that is what an impl needs:
/// desugaring splices the trait's `(params …)`, `(returns …)` and `(uses …)` into the impl's `def`
/// with `Self` rewritten, and a `Ty` would have to be rendered back to a node to do it.
#[derive(Clone, Debug)]
pub(super) struct TraitDecl {
    pub methods: Vec<TraitMethod>,
    /// The same declaration as types — what a `.becki` publishes and what `--wire-compat`
    /// compares. Built once, here, so that a locally-declared trait and an imported one are the
    /// same thing to everything downstream.
    pub sig: TraitSig,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub(super) struct TraitMethod {
    pub name: Arc<str>,
    pub params: Node,
    pub returns: Node,
    pub uses: Node,
    pub span: Span,
}

/// One `impl Trait for Type`, keyed elsewhere by the pair.
#[derive(Clone, Debug)]
pub(super) struct ImplDecl {
    /// The head constructor of the target: `Tree` for `Tree[T]`. Dispatch keys on this, so
    /// `Tree[Int]` and `Tree[Str]` share one impl and coherence has one entry to check.
    pub target: Arc<str>,
    /// The published form — the header an importing module reads.
    pub sig: ImplSig,
    pub span: Span,
}

/// `Self`, the name a trait's signatures are written in terms of.
const SELF: &str = "Self";

/// The head of a written function type, as the parser produces it.
const FN_TYPE: &str = "fn-type";

/// One dictionary parameter of a bounded definition, in the order it was appended.
#[derive(Clone, Debug)]
pub(super) struct DictParam {
    /// The type parameter the bound is on: `T` in `[T: Ord]`.
    pub param: Arc<str>,
    pub trait_name: Arc<str>,
    pub method: Arc<str>,
}

/// The name of one entry in a `(typarams …)` list, bounded or not.
pub(super) fn typaram_name(p: &Node) -> Option<Arc<str>> {
    if p.is_form(sym::ANNOT) {
        return p
            .args
            .first()
            .and_then(|n| n.as_var())
            .map(|s| s.name.clone());
    }
    p.as_var().map(|s| s.name.clone())
}

/// Every **bounded** parameter of a `(typarams …)` node, with its traits, in written order.
pub(super) fn bounds_of(typarams: &Node) -> Vec<(Arc<str>, Vec<Arc<str>>)> {
    if !typarams.is_form(sym::TYPARAMS) {
        return Vec::new();
    }
    typarams
        .args
        .iter()
        .filter(|p| p.is_form(sym::ANNOT) && p.args.len() >= 2)
        .filter_map(|p| {
            let name = typaram_name(p)?;
            let traits: Vec<Arc<str>> = p.args[1..]
                .iter()
                .filter_map(|b| b.as_var().map(|s| s.name.clone()))
                .collect();
            Some((name, traits))
        })
        .collect()
}

impl Checker<'_> {
    /// Collect every `trait` declaration, before any impl is expanded and before any signature is
    /// read.
    pub(super) fn collect_traits(&mut self, items: &[&Node]) {
        for item in items {
            let (item, _) = self.undecorate(item);
            if !item.is_form(sym::TRAIT) || item.args.is_empty() {
                continue;
            }
            let Some(name) = item.args[0].as_var().map(|s| s.name.clone()) else {
                continue;
            };
            if self.types.contains_key(&name) {
                self.error(
                    "B0380",
                    format!("`{name}` is already a type, so it cannot also be a trait"),
                    item.span(),
                );
                continue;
            }
            let mut methods: Vec<TraitMethod> = Vec::new();
            for m in &item.args[1..] {
                let Some(method) = self.trait_method(m, &name) else {
                    continue;
                };
                if methods.iter().any(|x| x.name == method.name) {
                    self.error(
                        "B0381",
                        format!("`{name}` declares `{}` twice", method.name),
                        method.span,
                    );
                    continue;
                }
                methods.push(method);
            }
            if methods.is_empty() {
                self.diags.push(
                    Diagnostic::error(
                        "B0381",
                        format!("`{name}` declares no methods"),
                        item.span(),
                    )
                    .with_note(
                        "a trait with nothing in it can be implemented and never used, which is a \
                         marker rather than an abstraction; Beck has no marker traits because \
                         placement and effects are already properties of the signature",
                    ),
                );
                continue;
            }
            let sig = self.trait_sig(&name, &methods);
            let decl = TraitDecl {
                methods,
                sig,
                span: item.span(),
            };
            for m in &decl.methods {
                // One method name to one trait. Two traits declaring `show` would make `x.show()`
                // ambiguous at every call site, and resolving it by which impls exist would make
                // adding an impl change what an unrelated call means.
                if let Some(other) = self.trait_methods.get(&m.name) {
                    self.error(
                        "B0381",
                        format!("`{}` is already a method of trait `{other}`", m.name),
                        m.span,
                    );
                    continue;
                }
                if self.schemes.contains_key(&m.name) || self.prims.contains_key(&m.name) {
                    self.error(
                        "B0381",
                        format!(
                            "`{}` is already a definition, so `{name}` cannot declare it",
                            m.name
                        ),
                        m.span,
                    );
                    continue;
                }
                self.trait_methods.insert(m.name.clone(), name.clone());
                self.globals.push(Binding {
                    name: m.name.clone(),
                    scopes: ScopeSet::empty(),
                    kind: BindKind::TraitMethod(m.name.clone()),
                });
            }
            self.own_traits.push(name.clone());
            if self.traits.insert(name.clone(), decl).is_some() {
                self.error(
                    "B0380",
                    format!("trait `{name}` is declared twice"),
                    item.span(),
                );
            }
        }
    }

    /// A trait's methods as *types*, for publication and comparison.
    ///
    /// `Self` is resolved to `Ty::con("Self")` by registering it as a type for exactly the length of
    /// this call. It is not a name any other pass looks up, and leaving it registered would let a
    /// declaration elsewhere mention a type that does not exist.
    fn trait_sig(&mut self, name: &Arc<str>, methods: &[TraitMethod]) -> TraitSig {
        let placeholder = TyDecl::Newtype {
            name: Arc::from(SELF),
            params: Vec::new(),
            inner: Ty::unit(),
        };
        self.types.insert(Arc::from(SELF), placeholder);
        let out = TraitSig {
            name: name.clone(),
            methods: methods
                .iter()
                .map(|m| MethodSig {
                    name: m.name.clone(),
                    params: m
                        .params
                        .args
                        .iter()
                        .map(|p| {
                            (
                                p.args[0]
                                    .as_var()
                                    .map(|s| s.name.clone())
                                    .unwrap_or_else(|| Arc::from("?")),
                                self.ty_from_node(&p.args[1]),
                            )
                        })
                        .collect(),
                    ret: self.ty_from_node(&m.returns.args[0]),
                    effects: self.declared_row(Some(&m.uses)).atoms.into_iter().collect(),
                })
                .collect(),
        };
        self.types.remove(SELF);
        out
    }

    /// One signature inside a `trait` body.
    fn trait_method(&mut self, m: &Node, trait_name: &str) -> Option<TraitMethod> {
        let (m, _) = self.undecorate(m);
        if !m.is_form(sym::DEF) || m.args.len() < 5 {
            self.error(
                "B0381",
                format!("`{trait_name}` may only contain `def` signatures"),
                m.span(),
            );
            return None;
        }
        let name = m.args[0].as_var()?.name.clone();
        if !m.args[1].args.is_empty() {
            self.error(
                "B0381",
                format!("`{name}` may not take type parameters of its own"),
                m.args[1].span(),
            );
            return None;
        }
        // A body in a trait would be a *default* method, which is a separate feature: it needs the
        // body checked once against an abstract `Self` rather than once per impl.
        if m.args.len() > 5 {
            self.diags.push(
                Diagnostic::error(
                    "B0381",
                    format!("`{name}` has a body, and a trait declares signatures"),
                    m.span(),
                )
                .with_note(
                    "a default method would have to be checked against an abstract `Self` rather \
                     than against each implementing type, which is not built",
                ),
            );
            return None;
        }
        let params = self.trait_params(&m.args[2], &name)?;
        if m.args[3].args.is_empty() {
            self.error(
                "B0381",
                format!("`{name}` needs a return type"),
                m.args[0].span(),
            );
            return None;
        }
        Some(TraitMethod {
            name,
            params,
            returns: m.args[3].clone(),
            uses: m.args[4].clone(),
            span: m.span(),
        })
    }

    /// The parameter list of a trait method, with a bare `self` given its implicit type.
    ///
    /// At least one parameter has to mention `Self`, because dispatch is by the receiver: a method
    /// nothing dispatches on could never be resolved from a call.
    fn trait_params(&mut self, params: &Node, method: &str) -> Option<Node> {
        let mut out = Vec::new();
        let mut mentions_self = false;
        for p in &params.args {
            let (name, ty) = if p.is_form(sym::ANNOT) && p.args.len() == 2 {
                (p.args[0].clone(), p.args[1].clone())
            } else if p.as_var().map(|s| s.name.as_ref() == "self") == Some(true) {
                // `def show(self) -> Str` — `self` alone means `self: Self`, which is the notation
                // `docs/11` §11.3 writes.
                (p.clone(), Node::sym(SELF, p.span()))
            } else {
                self.error(
                    "B0381",
                    format!("`{method}`'s parameters need types, and only `self` is implicit"),
                    p.span(),
                );
                return None;
            };
            if mentions(&ty, SELF) {
                mentions_self = true;
            }
            let span = name.span().to(ty.span());
            out.push(Node::form(sym::ANNOT, vec![name, ty], span));
        }
        if !mentions_self {
            self.diags.push(
                Diagnostic::error(
                    "B0381",
                    format!("`{method}` never mentions `Self`, so nothing dispatches on it"),
                    params.span(),
                )
                .with_note(
                    "a trait method is resolved from the type of an argument; one that mentions \
                     `Self` only in its return type would need the call site to say which impl it \
                     meant, and there is no notation for that",
                ),
            );
            return None;
        }
        Some(Node::form(sym::PARAMS, out, params.span()))
    }

    /// Turn every `impl` into ordinary `def` items, and register what each one implements.
    ///
    /// Returns the synthesised nodes; the caller keeps them alive and appends them to the item
    /// list, so that every later pass sees definitions rather than a form it has to know about.
    pub(super) fn expand_impls(&mut self, items: &[&Node]) -> Vec<Node> {
        let mut out = Vec::new();
        for item in items {
            let (item, _) = self.undecorate(item);
            if !item.is_form(sym::IMPL) || item.args.len() < 3 {
                continue;
            }
            self.expand_impl(item, &mut out);
        }
        out
    }

    fn expand_impl(&mut self, item: &Node, out: &mut Vec<Node>) {
        let span = item.span();
        let Some(trait_name) = item.args[0].as_var().map(|s| s.name.clone()) else {
            return;
        };
        let Some(decl) = self.traits.get(&trait_name).cloned() else {
            self.error("B0383", format!("cannot find trait `{trait_name}`"), span);
            return;
        };
        let target_node = &item.args[2];
        let Some(target) = target_node.head_name().map(Arc::<str>::from) else {
            self.error("B0383", "expected a type to implement the trait for", span);
            return;
        };

        // The impl's own type parameters, bound rigidly for the duration: `impl[T] Show for
        // Tree[T]` is one impl covering every `T`, and each method it produces is a generic `def`
        // exactly as if it had been written by hand.
        let typarams = item.args[1].clone();
        let param_names = Self::typaram_names(item);

        if !self.types.contains_key(&target)
            && crate::prelude::builtin_arity(&target).is_none()
            && !param_names.contains(&target)
        {
            self.error(
                "B0383",
                format!("cannot find type `{target}`"),
                target_node.span(),
            );
            return;
        }
        if param_names.contains(&target) {
            self.diags.push(
                Diagnostic::error(
                    "B0384",
                    format!("`{target}` is a type parameter, so this impl covers every type"),
                    target_node.span(),
                )
                .with_note(
                    "a blanket impl makes coherence a search rather than a lookup, and Beck's \
                     orphan rule is written for one impl per trait per type constructor",
                ),
            );
            return;
        }
        // Coherence, half one: one impl per trait per type constructor. Keyed on the *head*, so
        // `Tree[Int]` and `Tree[Str]` cannot be given different behaviour and a call never has to
        // pick between two.
        let key = (trait_name.clone(), target.clone());
        if let Some(prev) = self.impls.get(&key) {
            self.diags.push(
                Diagnostic::error(
                    "B0384",
                    format!("`{trait_name}` is already implemented for `{target}`"),
                    span,
                )
                .with_label(prev.span, "the first implementation")
                .with_note(
                    "coherence: one impl per trait per type, so that what a call means never \
                     depends on which impls happen to be in scope",
                ),
            );
            return;
        }
        // Coherence, half two: the orphan rule. Implementing somebody else's trait for somebody
        // else's type is what makes two libraries able to conflict.
        // *Declared here*, not merely in scope. A trait that arrived from the prelude or from an
        // import is somebody else's, and implementing somebody else's trait for somebody else's
        // type is exactly what the rule refuses.
        let owns_trait = self.own_traits.contains(&trait_name);
        let owns_type = self.own_types.contains(&target);
        if !owns_trait && !owns_type {
            self.diags.push(
                Diagnostic::error(
                    "B0385",
                    format!("neither `{trait_name}` nor `{target}` is declared in this module"),
                    span,
                )
                .with_note(
                    "the orphan rule: an impl belongs with the trait or with the type, so that two \
                     modules cannot both supply one and disagree",
                ),
            );
            return;
        }

        // The published header: the target with the impl's own parameters rigid, so `Bundle[T]`
        // reads back as a type rather than as a name plus a promise.
        let sig = {
            let before = std::mem::take(&mut self.typarams);
            self.typarams = param_names.iter().cloned().collect();
            let target_ty = self.ty_from_node(target_node);
            self.typarams = before;
            ImplSig {
                trait_name: trait_name.clone(),
                params: param_names.clone(),
                target: target_ty,
                // Filled in after the bodies are checked — this is the header, and what its
                // methods perform is not known until they have been read.
                effects: Vec::new(),
            }
        };

        // An interface publishes the header and not the bodies, exactly as it publishes a `def`'s
        // signature and not its body. What it *does* publish about a method is its **row**, since
        // `docs/27` made that a property of the impl rather than of the trait: a caller in another
        // module has nowhere else to learn it.
        if self.mode == super::Mode::Interface {
            let mut sig = sig;
            for m in &item.args[3..] {
                let (m, _) = self.undecorate(m);
                // `def add uses raises(MoneyError)` — a bodyless `def` carrying only a row, which
                // is what `render_impl` writes.
                if !m.is_form(sym::DEF) || m.args.len() > 5 {
                    self.diags.push(
                        Diagnostic::error(
                            "B0382",
                            "an impl in a `.becki` publishes its methods' effects, not their bodies",
                            m.span(),
                        )
                        .with_note(
                            "the implementation stays in the module that wrote it; what crosses is \
                             that it exists and what it performs, which is what a call in another \
                             module needs to resolve",
                        ),
                    );
                    continue;
                }
                let Some(name) = m.args[0].as_var().map(|s| s.name.clone()) else {
                    continue;
                };
                let row = self.declared_row(m.args.get(4));
                if !row.atoms.is_empty() {
                    sig.effects.push((name, row.atoms.into_iter().collect()));
                }
            }
            self.register_impl(key, target.clone(), sig, span);
            return;
        }
        if item.args.len() == 3 {
            self.diags.push(
                Diagnostic::error("B0382", "this impl has no methods", span).with_note(
                    "a header with nothing behind it is a declaration, which is what a `.becki` \
                     interface is made of; an ordinary module has to implement what it claims",
                ),
            );
            return;
        }

        let mut seen: BTreeSet<Arc<str>> = BTreeSet::new();
        for m in &item.args[3..] {
            let (m, _) = self.undecorate(m);
            if !m.is_form(sym::DEF) || m.args.len() < 6 {
                self.error(
                    "B0382",
                    "an impl may only contain `def`s with bodies",
                    m.span(),
                );
                continue;
            }
            let Some(name) = m.args[0].as_var().map(|s| s.name.clone()) else {
                continue;
            };
            let Some(sig) = decl.methods.iter().find(|x| x.name == name) else {
                self.diags.push(
                    Diagnostic::error(
                        "B0382",
                        format!("`{trait_name}` has no method `{name}`"),
                        m.args[0].span(),
                    )
                    .with_label(decl.span, "the trait is declared here"),
                );
                continue;
            };
            if !seen.insert(name.clone()) {
                self.error(
                    "B0382",
                    format!("`{name}` is implemented twice for `{target}`"),
                    m.span(),
                );
                continue;
            }
            if let Some(def) =
                self.impl_method(m, sig, &trait_name, &target, target_node, &typarams)
            {
                if let Some(s) = def.args[0].as_var() {
                    self.impl_methods.insert(s.name.clone());
                }
                out.push(def);
            }
        }

        let missing: Vec<String> = decl
            .methods
            .iter()
            .filter(|m| !seen.contains(&m.name))
            .map(|m| m.name.to_string())
            .collect();
        if !missing.is_empty() {
            self.diags.push(
                Diagnostic::error(
                    "B0382",
                    format!("`{target}` does not implement all of `{trait_name}`"),
                    span,
                )
                .with_primary_label(format!("missing: {}", missing.join(", ")))
                .with_label(decl.span, "declared here"),
            );
        }
        self.register_impl(key, target, sig, span);
    }

    fn register_impl(
        &mut self,
        key: (Arc<str>, Arc<str>),
        target: Arc<str>,
        sig: ImplSig,
        span: Span,
    ) {
        self.own_impls.push(key.clone());
        self.impls.insert(key, ImplDecl { target, sig, span });
    }

    /// One impl method, rewritten into a top-level `def` with a mangled name.
    fn impl_method(
        &mut self,
        m: &Node,
        sig: &TraitMethod,
        trait_name: &str,
        target: &str,
        target_node: &Node,
        typarams: &Node,
    ) -> Option<Node> {
        if !m.args[1].args.is_empty() {
            self.error(
                "B0382",
                format!("`{}` takes its type parameters from the impl", sig.name),
                m.args[1].span(),
            );
            return None;
        }
        if !m.args[3].args.is_empty() || !m.args[4].args.is_empty() {
            self.diags.push(
                Diagnostic::error(
                    "B0382",
                    format!(
                        "`{}` may not restate its return type or its effects",
                        sig.name
                    ),
                    m.args[0].span(),
                )
                .with_label(sig.span, "the trait already said both")
                .with_note(
                    "an impl writes the body; the signature is the trait's, and a second copy of \
                     it is a second place for it to be wrong",
                ),
            );
            return None;
        }
        let written = &m.args[2].args;
        if written.len() != sig.params.args.len() {
            self.error(
                "B0382",
                format!(
                    "`{}` takes {} parameter(s), got {}",
                    sig.name,
                    sig.params.args.len(),
                    written.len()
                ),
                m.args[2].span(),
            );
            return None;
        }
        // The impl supplies names, the trait supplies types, and `Self` becomes the target.
        let mut params = Vec::new();
        for (w, s) in written.iter().zip(&sig.params.args) {
            if w.is_form(sym::ANNOT) {
                self.diags.push(
                    Diagnostic::error(
                        "B0382",
                        format!("`{}`'s parameter types come from the trait", sig.name),
                        w.span(),
                    )
                    .with_label(sig.span, "declared here"),
                );
                return None;
            }
            let Some(name) = w.as_var() else {
                self.error("B0382", "expected a parameter name", w.span());
                return None;
            };
            let ty = substitute_self(&s.args[1], target_node);
            let span = w.span().to(ty.span());
            params.push(Node::form(
                sym::ANNOT,
                vec![Node::sym(&name.name, w.span()), ty],
                span,
            ));
        }
        let name = mangle(trait_name, &sig.name, target);
        Some(Node::form(
            sym::DEF,
            vec![
                Node::sym(&name, m.args[0].span()),
                typarams.clone(),
                Node::form(sym::PARAMS, params, m.args[2].span()),
                substitute_self(&sig.returns, target_node),
                sig.uses.clone(),
                m.args[5].clone(),
            ],
            m.span(),
        ))
    }

    // ------------------------------------------------------------------------------- bounds

    /// Rewrite every bounded `def` so that its dictionaries are ordinary parameters.
    ///
    /// Returns the replacement for `item`, or `None` when it has no bounds and needs none. Run
    /// before `collect_signatures`, so every later pass sees a definition with one more argument
    /// and nothing else to know about.
    pub(super) fn expand_bounds(&mut self, item: &Node) -> Option<Node> {
        if item.is_form(sym::DECORATE) && item.args.len() == 2 {
            let inner = self.expand_bounds(&item.args[1])?;
            let mut out = item.clone();
            out.args[1] = inner;
            return Some(out);
        }
        if !item.is_form(sym::DEF) || item.args.len() < 5 {
            return None;
        }
        let bounds = bounds_of(&item.args[1]);
        if bounds.is_empty() {
            return None;
        }
        let name = item.args[0].as_var().map(|s| s.name.clone())?;
        let mut extra = Vec::new();
        let mut specs = Vec::new();
        for (param, traits) in &bounds {
            let param_node = Node::sym(param, item.args[1].span());
            for t in traits {
                let Some(decl) = self.traits.get(t).cloned() else {
                    self.error(
                        "B0383",
                        format!("cannot find trait `{t}`"),
                        item.args[1].span(),
                    );
                    continue;
                };
                for m in &decl.methods {
                    let dict = mangle(t, &m.name, param);
                    let span = item.args[1].span();
                    // The method's own signature with `Self` := the type parameter. Its row is left
                    // to `ty_from_node`, which mints a variable for a written function type — so a
                    // caller that supplies a pure impl stays pure (`docs/27` §27.3).
                    let mut fn_ty: Vec<Node> = m
                        .params
                        .args
                        .iter()
                        .map(|p| substitute_self(&p.args[1], &param_node))
                        .collect();
                    fn_ty.push(substitute_self(&m.returns.args[0], &param_node));
                    extra.push(Node::form(
                        sym::ANNOT,
                        vec![Node::sym(&dict, span), Node::form(FN_TYPE, fn_ty, span)],
                        span,
                    ));
                    specs.push(DictParam {
                        param: param.clone(),
                        trait_name: t.clone(),
                        method: m.name.clone(),
                    });
                }
            }
        }
        if specs.is_empty() {
            return None;
        }
        self.dicts.insert(name, specs);
        let mut out = item.clone();
        // The type-parameter list keeps only the names from here on: the bound has been spent, and
        // leaving it would make `bind_typarams` read a form it does not need to know about.
        out.args[1] = Node::form(
            sym::TYPARAMS,
            bounds_of(&item.args[1])
                .iter()
                .map(|(p, _)| Node::sym(p, item.args[1].span()))
                .chain(
                    item.args[1]
                        .args
                        .iter()
                        .filter(|p| !p.is_form(sym::ANNOT))
                        .cloned(),
                )
                .collect(),
            item.args[1].span(),
        );
        out.args[2].args.extend(extra);
        Some(out)
    }

    /// The bounds on a definition's type parameters, recovered from the dictionaries it was given.
    ///
    /// One entry per bounded parameter, in the order the parameters were written, with each
    /// parameter's traits in the order they were written — which is the order the dictionaries were
    /// appended in, so reading them back off the dictionaries cannot disagree with the signature.
    pub(super) fn bounds_of_def(&self, name: &Arc<str>) -> Vec<(Arc<str>, Vec<Arc<str>>)> {
        let Some(specs) = self.dicts.get(name) else {
            return Vec::new();
        };
        let mut out: Vec<(Arc<str>, Vec<Arc<str>>)> = Vec::new();
        for s in specs {
            match out.iter_mut().find(|(p, _)| *p == s.param) {
                Some((_, traits)) => {
                    if !traits.contains(&s.trait_name) {
                        traits.push(s.trait_name.clone());
                    }
                }
                None => out.push((s.param.clone(), vec![s.trait_name.clone()])),
            }
        }
        out
    }

    /// Apply a **bounded** definition, supplying one dictionary per method of each bound.
    ///
    /// The ordinary arguments are checked *and* the result is unified with what the context wants,
    /// both before any dictionary is resolved — because until then the call's `T` is a variable and
    /// there is nothing to look an impl up by. Consulting the expectation is what makes
    /// `def none_yet() -> Option[Int]: return largest([])` work: the element type is not in the
    /// argument, and it is in the return type.
    pub(super) fn apply_bounded(
        &mut self,
        name: &Arc<str>,
        specs: &[DictParam],
        args: &[Node],
        expected: Option<&Ty>,
        span: Span,
    ) -> Core {
        let Some(scheme) = self.schemes.get(name).cloned() else {
            return Core::new(CoreKind::Const(Const::Unit), self.subst.fresh(), span);
        };
        let (ty, named) = self.subst.instantiate_named(&scheme);
        let func = Core::new(CoreKind::Global(name.clone()), ty.clone(), span);
        let Ty::Fun(param_tys, ret, latent) = ty else {
            return self.apply_fn(func, args, span);
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
        let mut checked = self.check_args(args, &param_tys[..ordinary]);
        if let Some(want) = expected {
            // Deliberately not reported as a mismatch here: the caller unifies the result again
            // when it has a label for what went wrong, and a second message would be noise.
            let _ = self.subst.unify(&ret, want);
        }
        for (i, spec) in specs.iter().enumerate() {
            let at = named
                .get(&spec.param)
                .map(|t| self.subst.resolve(t))
                .unwrap_or_else(|| self.subst.fresh());
            let Some(dict) = self.dictionary(&spec.trait_name, &spec.method, &at, span) else {
                continue;
            };
            if let Some(want) = param_tys.get(ordinary + i) {
                self.unify(&dict.ty, want, span, "implementation");
            }
            checked.push(dict);
        }
        Core::new(
            CoreKind::App {
                func: Box::new(func),
                args: checked,
            },
            *ret,
            span,
        )
    }

    /// The implementation of one trait method at one type, as something callable.
    ///
    /// Two kinds of answer, and the whole design is that they are found the same way. If the type
    /// is a **type parameter** of the definition being checked, the implementation arrived as a
    /// dictionary parameter and this is a local. Otherwise it is a concrete type and this is the
    /// impl's own global. Both are named `Trait::method@Target`.
    pub(super) fn dictionary(
        &mut self,
        trait_name: &Arc<str>,
        method: &Arc<str>,
        ty: &Ty,
        span: Span,
    ) -> Option<Core> {
        let head = ty.con_name().map(Arc::<str>::from);
        if let Some(head) = &head {
            if self.typarams.contains(head) {
                let want = mangle(trait_name, method, head);
                if let Some(BindKind::Local(id, t)) =
                    self.resolve(&Symbol::new(&want)).map(|b| b.kind.clone())
                {
                    return Some(Core::new(CoreKind::Var(id), t, span));
                }
                self.diags.push(
                    Diagnostic::error(
                        "B0386",
                        format!("`{head}` is not known to implement `{trait_name}`"),
                        span,
                    )
                    .with_primary_label(format!("`{method}` needs it"))
                    .with_fix(format!("bound it: `[{head}: {trait_name}]`")),
                );
                return None;
            }
        }
        let Some(head) = head else {
            self.diags.push(
                Diagnostic::error(
                    "B0386",
                    format!("cannot tell which type `{method}` dispatches on here"),
                    span,
                )
                .with_primary_label("the type is not determined at this call")
                .with_fix("annotate it, or pass an argument that fixes it")
                .with_note(
                    "an implementation is chosen from a concrete type or from a bound on a type \
                     parameter; this is neither yet, and the choice is made where the call is \
                     written rather than after the whole body has been read",
                ),
            );
            return None;
        };
        let Some(found) = self.impls.get(&(trait_name.clone(), head.clone())) else {
            let decl = self.traits.get(trait_name).map(|d| d.span);
            let mut d = Diagnostic::error(
                "B0387",
                format!("`{head}` does not implement `{trait_name}`"),
                span,
            )
            .with_primary_label(format!(
                "`{method}` needs an `impl {trait_name} for {head}`"
            ));
            if let Some(at) = decl {
                d = d.with_label(at, "the trait is declared here");
            }
            self.diags.push(d);
            return None;
        };
        let name = mangle(trait_name, method, &found.target);
        let ty = self
            .schemes
            .get(&name)
            .map(|sc| self.subst.instantiate(sc))?;
        Some(Core::new(CoreKind::Global(name), ty, span))
    }

    /// Register an imported module's traits and impls.
    ///
    /// An imported trait is turned back into the syntax a local one is kept as, so that everything
    /// downstream — dispatch, an impl for a local type, a bound on a local definition — cannot tell
    /// the difference. The impl methods it names are registered as *signatures*: the bodies stayed
    /// in the module that wrote them, and what crosses is that they exist and what they promise.
    pub(super) fn import_traits(&mut self, traits: &[TraitSig], impls: &[ImplSig]) {
        for t in traits {
            let methods: Vec<TraitMethod> = t
                .methods
                .iter()
                .map(|m| TraitMethod {
                    name: m.name.clone(),
                    params: Node::form(
                        sym::PARAMS,
                        m.params
                            .iter()
                            .map(|(n, ty)| {
                                Node::form(
                                    sym::ANNOT,
                                    vec![Node::sym(n, Span::NONE), ty_to_node(ty)],
                                    Span::NONE,
                                )
                            })
                            .collect(),
                        Span::NONE,
                    ),
                    returns: Node::form(sym::RETURNS, vec![ty_to_node(&m.ret)], Span::NONE),
                    uses: Node::form(
                        "uses",
                        m.effects
                            .iter()
                            .map(|e| Node::sym(e.name(), Span::NONE))
                            .collect(),
                        Span::NONE,
                    ),
                    span: Span::NONE,
                })
                .collect();
            for m in &methods {
                self.trait_methods.insert(m.name.clone(), t.name.clone());
                self.globals.push(Binding {
                    name: m.name.clone(),
                    scopes: ScopeSet::empty(),
                    kind: BindKind::TraitMethod(m.name.clone()),
                });
            }
            self.traits.insert(
                t.name.clone(),
                TraitDecl {
                    methods,
                    sig: t.clone(),
                    span: Span::NONE,
                },
            );
        }
        for i in impls {
            let head = i.head();
            let Some(decl) = self.traits.get(&i.trait_name).cloned() else {
                continue;
            };
            for m in &decl.sig.methods {
                // The signature the importing module will call through: the trait's shape, with
                // `Self` replaced by this impl's target, and **this impl's** row rather than the
                // trait's. `docs/27` inverted that: a trait's row is a floor and an impl may be
                // more effectful, so taking the row off the trait here would let a fallible method
                // arrive in another module looking pure.
                let name = mangle(&i.trait_name, &m.name, &head);
                let row = i
                    .effects
                    .iter()
                    .find(|(n, _)| *n == m.name)
                    .map(|(_, r)| Row::of(r.iter().cloned()))
                    .unwrap_or_else(|| Row::of(m.effects.iter().cloned()));
                let params: Vec<Ty> = m
                    .params
                    .iter()
                    .map(|(_, t)| substitute_self_ty(t, &i.target))
                    .collect();
                let ret = substitute_self_ty(&m.ret, &i.target);
                let ty = Ty::fun_eff(params, ret, row);
                self.schemes
                    .insert(name.clone(), Scheme::generic(i.params.clone(), ty));
            }
            self.impls.insert(
                (i.trait_name.clone(), head.clone()),
                ImplDecl {
                    target: head,
                    sig: i.clone(),
                    span: Span::NONE,
                },
            );
        }
    }

    /// Rebuild an imported definition's dictionary parameters from its published bound.
    ///
    /// The mirror of [`Checker::expand_bounds`], and it has to produce exactly the same parameters
    /// in exactly the same order — a `.becki` publishes `def total[T: Priced](xs: list[T]) -> Int`
    /// and the module that wrote it lowered that to a two-parameter function. Working from types
    /// rather than from syntax, because an imported name arrives as a scheme.
    pub(super) fn import_bounded(
        &mut self,
        name: &Arc<str>,
        bounds: &[(Arc<str>, Vec<Arc<str>>)],
        scheme: Scheme,
    ) -> Scheme {
        let Ty::Fun(mut params, ret, row) = scheme.ty.clone() else {
            return scheme;
        };
        let mut specs = Vec::new();
        for (param, traits) in bounds {
            let at = Ty::con(param);
            for t in traits {
                let Some(decl) = self.traits.get(t).cloned() else {
                    continue;
                };
                for m in &decl.sig.methods {
                    // A **fresh row variable**, matching what `expand_bounds` mints locally: a
                    // bounded definition is effect-polymorphic in its bounds, so a caller that
                    // supplies a pure impl stays pure and one that supplies a fallible impl
                    // inherits exactly its failure (`docs/27` §27.3, `docs/27` §27.7).
                    let row = self.subst.fresh_row();
                    params.push(Ty::fun_eff(
                        m.params
                            .iter()
                            .map(|(_, ty)| substitute_self_ty(ty, &at))
                            .collect(),
                        substitute_self_ty(&m.ret, &at),
                        row,
                    ));
                    specs.push(DictParam {
                        param: param.clone(),
                        trait_name: t.clone(),
                        method: m.name.clone(),
                    });
                }
            }
        }
        if specs.is_empty() {
            return scheme;
        }
        self.dicts.insert(name.clone(), specs);
        Scheme::generic(scheme.params.clone(), Ty::Fun(params, ret, row))
    }

    /// A call to a trait method, resolved from the type of the argument that carries `Self`.
    pub(super) fn trait_call(&mut self, method: &Arc<str>, args: &[Node], span: Span) -> Core {
        let unit = || Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span);
        let Some(trait_name) = self.trait_methods.get(method).cloned() else {
            return unit();
        };
        let Some(decl) = self.traits.get(&trait_name).cloned() else {
            return unit();
        };
        let Some(sig) = decl.methods.iter().find(|m| &m.name == method) else {
            return unit();
        };
        // Which parameter carries `Self` — the first one that mentions it. `trait_params` refused
        // the method if none did, so this is a position and not a search that can fail.
        let at = sig
            .params
            .args
            .iter()
            .position(|p| mentions(&p.args[1], SELF))
            .unwrap_or(0);
        if args.len() <= at {
            self.error(
                "B0351",
                format!(
                    "`{method}` takes {} argument(s), got {}",
                    sig.params.args.len(),
                    args.len()
                ),
                span,
            );
            return unit();
        }
        let receiver = self.expr(&args[at], None);
        let ty = self.subst.resolve(&receiver.ty);
        // One rule for both kinds of answer: a concrete receiver finds the impl's own global, and a
        // bounded type parameter finds the dictionary its definition was handed.
        let Some(func) = self.dictionary(&trait_name, method, &ty, args[at].span()) else {
            return unit();
        };
        self.apply_fn_with(func, receiver, at, args, span)
    }
}

/// A type as a type *expression*, so a published signature can be spliced like a written one.
///
/// The inverse of `ty_from_node`, and the reason an imported trait behaves exactly like a local
/// one: desugaring an impl or a bound is a syntax rewrite, so an imported trait has to arrive as
/// syntax. Every span is [`Span::NONE`] — "macro-generated code that chose not to borrow one" —
/// because the nodes describe a declaration in a file this module does not own.
fn ty_to_node(t: &Ty) -> Node {
    let span = Span::NONE;
    match t {
        Ty::Con(n, args) if args.is_empty() => Node::sym(n, span),
        Ty::Con(n, args) => Node::form_sym(
            beck_syntax::Symbol::new(n),
            args.iter().map(ty_to_node).collect(),
            span,
        ),
        Ty::Fun(ps, r, _) => {
            let mut parts: Vec<Node> = ps.iter().map(ty_to_node).collect();
            parts.push(ty_to_node(r));
            Node::form(FN_TYPE, parts, span)
        }
        // A published row is closed and a published signature has no free variables, so this is
        // unreachable for anything `Interface` carries. `Unit` rather than a panic: a malformed
        // `.becki` should be a diagnostic somewhere, never a crash here.
        Ty::Var(_) => Node::sym(Ty::UNIT, span),
    }
}

/// Does this type expression mention `name` anywhere?
fn mentions(n: &Node, name: &str) -> bool {
    n.head_name() == Some(name) || n.args.iter().any(|a| mentions(a, name))
}

/// Replace `Self` with the impl's target throughout a *type*.
fn substitute_self_ty(t: &Ty, target: &Ty) -> Ty {
    match t {
        Ty::Con(n, args) if n.as_ref() == SELF && args.is_empty() => target.clone(),
        Ty::Con(n, args) => Ty::Con(
            n.clone(),
            args.iter().map(|a| substitute_self_ty(a, target)).collect(),
        ),
        Ty::Fun(ps, r, row) => Ty::Fun(
            ps.iter().map(|p| substitute_self_ty(p, target)).collect(),
            Box::new(substitute_self_ty(r, target)),
            row.clone(),
        ),
        Ty::Var(_) => t.clone(),
    }
}

/// Replace `Self` with the impl's target throughout a type expression.
fn substitute_self(n: &Node, target: &Node) -> Node {
    if n.head_name() == Some(SELF) && n.args.is_empty() {
        let mut t = target.clone();
        t.meta = n.meta.clone();
        return t;
    }
    let mut out = n.clone();
    out.args = n.args.iter().map(|a| substitute_self(a, target)).collect();
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::check_str;

    fn codes(src: &str) -> Vec<&'static str> {
        let (_, d, _) = check_str("t.beck", src);
        d.iter().map(|x| x.code).collect()
    }

    fn errors(src: &str) -> String {
        let (_, d, map) = check_str("t.beck", src);
        d.render(&map)
    }

    const SHOW: &str = "\
trait Show:
    def show(self) -> Str

model Point:
    x: Int

impl Show for Point:
    def show(self):
        return str(self.x)
";

    #[test]
    fn a_trait_and_an_impl_check() {
        assert_eq!(codes(SHOW), Vec::<&str>::new());
    }

    #[test]
    fn a_call_resolves_to_the_impl_for_the_receivers_type() {
        let src = format!(
            "{SHOW}
def label(p: Point) -> Str:
    return p.show()

def same(p: Point) -> Str:
    return show(p)
"
        );
        assert_eq!(codes(&src), Vec::<&str>::new());

        // …and the desugared definition is what the call names, so nothing downstream of the
        // checker has to know a trait was involved.
        let (program, _, _) = check_str("t.beck", &src);
        assert!(
            program.defs.contains_key("Show::show@Point"),
            "{:?}",
            program.defs.keys().collect::<Vec<_>>()
        );
        assert!(super::is_impl_method("Show::show@Point"));
        assert!(!super::is_impl_method("label"));
    }

    #[test]
    fn one_impl_covers_every_argument_of_a_parameterised_type() {
        // The feature docs/27 built, meeting the feature this one does: `impl[T] Show for Tree[T]`
        // is one impl, and a call at `Tree[Int]` and a call at `Tree[Str]` both find it.
        let src = "\
trait Show:
    def show(self) -> Str

union Tree[T]:
    Leaf(value: T)

impl[T] Show for Tree[T]:
    def show(self):
        return \"leaf\"

def a() -> Str:
    return Leaf(value=1).show()

def b() -> Str:
    return Leaf(value=\"x\").show()
";
        assert_eq!(codes(src), Vec::<&str>::new());
    }

    #[test]
    fn a_type_with_no_impl_is_refused_by_name() {
        let src = format!(
            "{SHOW}
model Other:
    y: Int

def f(o: Other) -> Str:
    return o.show()
"
        );
        let text = errors(&src);
        assert!(text.contains("B0387"), "{text}");
        assert!(text.contains("impl Show for Other"), "{text}");
    }

    #[test]
    fn coherence_is_one_impl_per_trait_per_type() {
        let dup =
            format!("{SHOW}\nimpl Show for Point:\n    def show(self):\n        return \"\"\n");
        assert!(codes(&dup).contains(&"B0384"), "{:?}", codes(&dup));

        // …and no blanket impl, because that would make coherence a search.
        let blanket = "\
trait Show:
    def show(self) -> Str

impl[T] Show for T:
    def show(self):
        return \"\"
";
        assert!(codes(blanket).contains(&"B0384"), "{:?}", codes(blanket));
    }

    #[test]
    fn the_orphan_rule_needs_the_trait_or_the_type() {
        // Neither is declared here, so this impl belongs in whichever module owns one of them.
        let src = "\
impl Show for Int:
    def show(self):
        return \"\"
";
        // The trait is not declared either, so the first thing reported is that.
        assert!(codes(src).contains(&"B0383"), "{:?}", codes(src));

        // With the trait local and the type foreign, it is allowed — that is the rule, not a
        // blanket ban on implementing for a builtin.
        let owns_trait = "\
trait Show:
    def show(self) -> Str

impl Show for Int:
    def show(self):
        return str(self)
";
        assert_eq!(codes(owns_trait), Vec::<&str>::new());
    }

    #[test]
    fn an_impl_must_be_complete_and_no_more() {
        let two = "\
trait Show:
    def show(self) -> Str
    def tag(self) -> Str

model Point:
    x: Int

impl Show for Point:
    def show(self):
        return \"\"
";
        let text = errors(two);
        assert!(text.contains("B0382"), "{text}");
        assert!(text.contains("missing: tag"), "{text}");

        // An extra method inside the same impl, rather than a second impl — which would be
        // B0384's business and not this test's.
        let extra = SHOW.replace(
            "    def show(self):\n        return str(self.x)\n",
            "    def show(self):\n        return str(self.x)\n\n    def nope(self):\n        return \"\"\n",
        );
        let text = errors(&extra);
        assert!(text.contains("B0382"), "{text}");
        assert!(text.contains("has no method `nope`"), "{text}");
    }

    #[test]
    fn an_impl_writes_the_body_and_the_trait_writes_the_signature() {
        for (src, why) in [
            (
                "    def show(self: Point):\n        return \"\"\n",
                "a parameter type",
            ),
            (
                "    def show(self) -> Str:\n        return \"\"\n",
                "a return type",
            ),
            (
                "    def show(self) uses log:\n        return \"\"\n",
                "an effect row",
            ),
        ] {
            let program = SHOW.replace("    def show(self):\n        return str(self.x)\n", src);
            assert!(
                codes(&program).contains(&"B0382"),
                "{why}: {:?}",
                codes(&program)
            );
        }
    }

    /// An impl may perform more than its trait declares, and the caller inherits it.
    ///
    /// This **reverses** what `docs/27` §27.7 built, and `docs/27` says why: a trait's row as a
    /// ceiling meant a fallible operation could not be a trait method, so `Money` could not have
    /// `+`. The row is now inferred per impl. Nothing is lost, because what a caller sees is what
    /// the impl does rather than what the trait guessed.
    #[test]
    fn an_impl_may_perform_more_than_its_trait_declares_and_the_caller_inherits_it() {
        let src = "\
trait Show:
    def show(self) -> Str

model Point:
    x: Int

impl Show for Point:
    def show(self):
        return str(uuid())

def label(p: Point) -> Str:
    return p.show()
";
        let (program, d, map) = crate::check_str("t.beck", src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let row: Vec<String> = program
            .defs
            .get("label")
            .expect("label")
            .effects
            .iter()
            .map(|e| e.name())
            .collect();
        assert_eq!(
            row,
            vec!["nondet"],
            "a caller of a trait method performs what the *impl* performs"
        );
    }

    /// And a bounded caller is polymorphic in it: the same generic definition is pure with a pure
    /// impl and effectful with an effectful one, which is `docs/27`'s property applied to a bound.
    #[test]
    fn a_bounded_definition_inherits_the_row_of_whichever_impl_it_is_given() {
        let src = "\
trait Show:
    def show(self) -> Str

model Quiet:
    x: Int

model Loud:
    x: Int

impl Show for Quiet:
    def show(self):
        return str(self.x)

impl Show for Loud:
    def show(self):
        return str(uuid())

def label[T: Show](x: T) -> Str:
    return x.show()

def quiet(q: Quiet) -> Str:
    return label(q)

def loud(l: Loud) -> Str:
    return label(l)
";
        let (program, d, map) = crate::check_str("t.beck", src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let row = |name: &str| -> Vec<String> {
            program
                .defs
                .get(name)
                .unwrap_or_else(|| panic!("no `{name}`"))
                .effects
                .iter()
                .map(|e| e.name())
                .collect()
        };
        assert!(
            row("quiet").is_empty(),
            "a pure impl leaves its caller pure: {:?}",
            row("quiet")
        );
        assert_eq!(row("loud"), vec!["nondet"]);
    }

    #[test]
    fn an_unbounded_type_parameter_cannot_call_a_trait_method() {
        // The distinction the diagnostic has to make: `T` is not a type with no impl, it is a type
        // nobody said anything about — so the fix is a bound and not an impl.
        let generic = format!(
            "{SHOW}
def twice[T](x: T) -> Str:
    return x.show()
"
        );
        let text = errors(&generic);
        assert!(text.contains("B0386"), "{text}");
        assert!(text.contains("not known to implement"), "{text}");
        assert!(text.contains("[T: Show]"), "the fix names itself:\n{text}");
    }

    #[test]
    fn a_bound_lets_a_generic_body_call_a_trait_method() {
        let src = format!(
            "{SHOW}
def label[T: Show](x: T) -> Str:
    return \"<\" + x.show() + \">\"

def a() -> Str:
    return label(Point(x=1))
"
        );
        assert_eq!(codes(&src), Vec::<&str>::new());

        // The dictionary is an ordinary parameter, so the lowered definition has one more of them
        // than the source wrote — which is the whole implementation, visible.
        let (program, _, _) = check_str("t.beck", &src);
        let label = &program.defs["label"];
        assert_eq!(label.params.len(), 2, "{:?}", label.params);
        assert_eq!(label.params[1].1.as_ref(), "Show::show@T");
        assert_eq!(
            label.bounds,
            vec![(Arc::<str>::from("T"), vec![Arc::<str>::from("Show")])]
        );
    }

    #[test]
    fn a_bounded_definition_passes_its_own_dictionary_through() {
        // The case that makes bounds compose rather than bottom out: `outer` has no idea what `U`
        // is, and hands `inner` the implementation it was handed itself.
        let src = format!(
            "{SHOW}
def inner[T: Show](x: T) -> Str:
    return x.show()

def outer[U: Show](x: U) -> Str:
    return inner(x)

def used() -> Str:
    return outer(Point(x=1))
"
        );
        assert_eq!(codes(&src), Vec::<&str>::new());
    }

    #[test]
    fn a_call_takes_its_implementation_from_the_context_when_the_arguments_do_not_say() {
        let src = format!(
            "{SHOW}
def none_of[T: Show](xs: list[T]) -> Option[T]:
    return None

def nothing() -> Option[Point]:
    return none_of([])
"
        );
        assert_eq!(
            codes(&src),
            Vec::<&str>::new(),
            "the element type is in the return type, not in the argument"
        );
    }

    #[test]
    fn a_call_whose_type_is_undetermined_says_so() {
        let src = format!(
            "{SHOW}
def none_of[T: Show](xs: list[T]) -> Option[T]:
    return None

def nothing() -> Int:
    return list_len([none_of([])])
"
        );
        let text = errors(&src);
        assert!(text.contains("B0386"), "{text}");
        assert!(text.contains("not determined at this call"), "{text}");
    }

    #[test]
    fn a_bound_names_a_trait_and_nothing_else() {
        let src = format!(
            "{SHOW}
def label[T: Nope](x: T) -> Str:
    return \"\"
"
        );
        assert!(codes(&src).contains(&"B0383"), "{:?}", codes(&src));
    }

    #[test]
    fn neither_a_trait_method_nor_a_bounded_definition_is_a_value() {
        let method = format!(
            "{SHOW}
def all(ps: list[Point]) -> list[Str]:
    return map_list(ps, show)
"
        );
        let text = errors(&method);
        assert!(text.contains("B0386"), "{text}");
        assert!(text.contains("cannot be used as a value"), "{text}");

        // The same for a definition that carries a bound: its implementations arrive at the call
        // site, and a reference has no call site.
        let bounded = format!(
            "{SHOW}
def label[T: Show](x: T) -> Str:
    return x.show()

def all(ps: list[Point]) -> list[Str]:
    return map_list(ps, label)
"
        );
        let text = errors(&bounded);
        assert!(text.contains("B0386"), "{text}");
        assert!(text.contains("has a bound"), "{text}");
    }

    #[test]
    fn a_bounded_definition_publishes_its_bound_and_not_its_dictionaries() {
        // The wall docs/38 §38.6 named, from the other side: a library can publish the interesting
        // half of itself. What crosses is the *bound*; the parameters it was lowered with are named
        // `Show::show@T` and belong to the lowering rather than to the contract.
        let src = format!(
            "{SHOW}
def label[T: Show](x: T) -> Str:
    return x.show()
"
        );
        let (placed, d, map) = crate::compile_or_library_str("t.beck", &src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let iface = crate::iface::Interface::of(&placed.expect("compiles").program);
        let text = iface.render();
        assert!(text.contains("trait Show:"), "{text}");
        assert!(text.contains("    def show(self) -> Str"), "{text}");
        assert!(text.contains("impl Show for Point"), "{text}");
        assert!(text.contains("def label[T: Show](x: T) -> Str"), "{text}");
        assert!(
            !text.contains("Show::show@"),
            "a dictionary parameter is not part of the contract:\n{text}"
        );
    }

    #[test]
    fn a_declaration_cannot_bound_its_type_parameter() {
        // A bound says what a body may call, and a `model` has no body. Refused rather than
        // accepted and ignored, which is what it was before docs/27.
        let src = "trait Show:\n    def show(self) -> Str\n\nmodel Box[T: Show]:\n    held: T\n";
        let text = errors(src);
        assert!(text.contains("B0316"), "{text}");
        assert!(text.contains("has no body"), "{text}");
    }

    #[test]
    fn a_trait_an_impl_and_a_bound_cross_a_becki() {
        // The gap docs/38 §38.6 named. What the exporting module publishes is the trait, the impl
        // *header* and the bound; the bodies and the dictionary parameters stay behind.
        let lib = format!(
            "{SHOW}
def label[T: Show](x: T) -> Str:
    return x.show()
"
        );
        let (placed, d, map) = crate::compile_or_library_str("lib.beck", &lib);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let published = crate::iface::Interface::of(&placed.expect("compiles").program);

        // Through the file form, because that is what an importing module actually reads.
        let text = published.render();
        let mut m = beck_diag::SourceMap::new();
        let mut d = beck_diag::Diagnostics::new();
        let reread = crate::iface::Interface::parse("lib", &text, &mut m, &mut d);
        assert!(!d.has_errors(), "{}\n---\n{text}", d.render(&m));
        assert_eq!(published.digest(), reread.digest(), "rendered:\n{text}");
        assert_eq!(reread.traits.len(), 1);
        assert_eq!(reread.impls.len(), 1);

        // And an importing module resolves through it: a trait method on an imported type, and a
        // bounded definition whose dictionary it has to rebuild from the published bound.
        let app = "\
import lib

def one() -> Str:
    return Point(x=1).show()

def two() -> Str:
    return label(Point(x=2))
";
        let node = {
            let mut map = beck_diag::SourceMap::new();
            let file = map.add("app.beck", app);
            let mut d = beck_diag::Diagnostics::new();
            let n = beck_syntax::parse_file(file, "app", app, &mut d);
            assert!(!d.has_errors(), "{}", d.render(&map));
            n
        };
        let mut d = beck_diag::Diagnostics::new();
        let imports = vec![("lib".to_string(), reread)];
        let mut map = beck_diag::SourceMap::new();
        map.add("app.beck", app);
        crate::check::check_module_with(&node, crate::check::Mode::Module, &imports, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));
    }

    // ------------------------------------------------------------------- generic arithmetic

    const RATIONAL: &str = "\
model Rational:
    numer: Int
    denom: Int

impl Num for Rational:
    def add(self, other):
        return Rational(numer=self.numer + other.numer, denom=self.denom)

    def sub(self, other):
        return self

    def mul(self, other):
        return self

    def div(self, other):
        return self
";

    #[test]
    fn a_user_type_joins_the_numeric_tower_through_num() {
        let src = format!(
            "{RATIONAL}
def sum(a: Rational, b: Rational) -> Rational:
    return a + b

def rest(a: Rational, b: Rational) -> Rational:
    return (a - b) * (a / b)
"
        );
        assert_eq!(codes(&src), Vec::<&str>::new());

        // `+` on a `Rational` is a call to the impl, not a primitive — which is what makes the
        // tower open rather than a list inside the compiler.
        let (program, _, _) = check_str("t.beck", &src);
        assert!(program.defs.contains_key("Num::add@Rational"));
    }

    #[test]
    fn num_is_the_preludes_and_a_module_may_not_implement_it_for_a_type_it_does_not_own() {
        // `Num` arrives from the prelude, so `own_traits` does not contain it: the orphan rule's
        // "the trait or the type is declared here" leaves only the type, and `Int` is not.
        let src = "\
impl Num for Int:
    def add(self, other):
        return self

    def sub(self, other):
        return self

    def mul(self, other):
        return self

    def div(self, other):
        return self
";
        assert!(codes(src).contains(&"B0385"), "{:?}", codes(src));
    }

    #[test]
    fn a_declared_type_with_no_num_impl_is_told_how_to_join() {
        let src = "\
model Money:
    pence: Int

def sum(a: Money, b: Money) -> Money:
    return a + b
";
        let text = errors(src);
        assert!(text.contains("B0387"), "{text}");
        assert!(text.contains("impl Num for Money"), "{text}");
    }

    #[test]
    fn the_numeric_rule_is_unchanged_where_it_already_had_an_answer() {
        // The whole point of dispatching only when there is something to dispatch to. `1 + true`
        // is a mismatch and not a lecture about traits, and `1 + 1.0` still has no answer —
        // docs/27 §27.2's refusal to coerce is untouched.
        for (src, want) in [
            (
                "def f(n: Int, b: Bool) -> Int:\n    return n + b\n",
                "found `Bool`",
            ),
            (
                "def f(n: Int, x: Float) -> Float:\n    return n + x\n",
                "found `Float`",
            ),
        ] {
            let text = errors(src);
            assert!(text.contains("B0320"), "{text}");
            assert!(text.contains(want), "{text}");
        }

        // And a `Str` still concatenates rather than looking for an impl.
        let ok = "def f(a: Str, b: Str) -> Str:\n    return a + b\n";
        assert_eq!(codes(ok), Vec::<&str>::new());
    }

    #[test]
    fn a_bounded_type_parameter_may_use_the_operators() {
        // The two features meeting: `Num` is a trait like any other, so a bound on it hands the
        // body a dictionary and `a + b` inside a generic definition resolves to it.
        let src = format!(
            "{RATIONAL}
def twice[T: Num](x: T) -> T:
    return x + x

def used(r: Rational) -> Rational:
    return twice(r)
"
        );
        assert_eq!(codes(&src), Vec::<&str>::new());
    }

    #[test]
    fn a_method_name_belongs_to_one_trait() {
        let src = "\
trait Show:
    def show(self) -> Str

trait Other:
    def show(self) -> Str
";
        assert!(codes(src).contains(&"B0381"), "{:?}", codes(src));
    }

    #[test]
    fn a_trait_method_has_to_mention_self() {
        let src = "trait Show:\n    def show(n: Int) -> Str\n";
        let text = errors(src);
        assert!(text.contains("B0381"), "{text}");
        assert!(text.contains("nothing dispatches on it"), "{text}");
    }

    #[test]
    fn a_trait_declares_signatures_and_not_bodies() {
        let src = "trait Show:\n    def show(self) -> Str:\n        return \"\"\n";
        let text = errors(src);
        assert!(text.contains("B0381"), "{text}");
        assert!(text.contains("has a body"), "{text}");
    }
}
