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
//! # What it is not
//!
//! **There are no bounds.** `def f[T: Show](x: T)` is not writable, so no *generic* code can call a
//! trait method — resolution needs a concrete receiver. That is the half of the design that needs
//! dictionary passing, and building it badly would be worse than not building it. `B0366` says so
//! by name where a program tries.
//!
//! **A trait does not cross a module boundary.** A `.becki` publishes neither traits nor impls, and
//! `Interface::of` drops the mangled definitions rather than publishing names no parser could read
//! back.

use std::collections::BTreeSet;
use std::sync::Arc;

use beck_diag::{Diagnostic, Span};
use beck_syntax::{sym, Node, ScopeSet};

use super::{BindKind, Binding, Checker};
use crate::core::{Const, Core, CoreKind};
use crate::ty::Ty;

/// The separator that makes a desugared impl method unnameable from source.
///
/// `::` and `@` are not identifier characters in either surface, so `Show::show@Tree` cannot
/// collide with anything a program declares, and a stray one in a diagnostic is recognisable as
/// compiler-generated rather than as something the author wrote.
pub(super) fn mangle(trait_name: &str, method: &str, target: &str) -> Arc<str> {
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
    pub span: Span,
}

/// `Self`, the name a trait's signatures are written in terms of.
const SELF: &str = "Self";

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
            if self.mode == super::Mode::Interface {
                self.diags.push(
                    Diagnostic::error(
                        "B0380",
                        "a `.becki` interface cannot declare a trait",
                        item.span(),
                    )
                    .with_note(
                        "a trait does not cross a module boundary yet: `beck iface` publishes \
                         neither traits nor impls, so an interface holding one would promise \
                         something no importing module could use",
                    ),
                );
                continue;
            }
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
            let decl = TraitDecl {
                methods,
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
            if self.traits.insert(name.clone(), decl).is_some() {
                self.error(
                    "B0380",
                    format!("trait `{name}` is declared twice"),
                    item.span(),
                );
            }
        }
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
        if self.mode == super::Mode::Interface {
            self.error("B0380", "a `.becki` interface cannot contain an impl", span);
            return;
        }
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
        let owns_trait = self.traits.contains_key(&trait_name);
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
        self.impls.insert(
            key,
            ImplDecl {
                target: target.clone(),
                span,
            },
        );
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
        // A rigid type parameter *has* a head — `Ty::Con("T", [])` — so it would otherwise be
        // reported as a type with no impl, which is the wrong answer to the right question. What is
        // missing is a bound saying `T` implements the trait, not an impl for a type called `T`.
        let is_typaram = ty
            .con_name()
            .map(|n| self.typarams.contains(n))
            .unwrap_or(false);
        let Some(head) = ty.con_name().map(Arc::<str>::from).filter(|_| !is_typaram) else {
            self.diags.push(
                Diagnostic::error(
                    "B0386",
                    format!("cannot tell which type `{method}` dispatches on here"),
                    args[at].span(),
                )
                .with_primary_label(format!("this is `{ty}`"))
                .with_note(
                    "a trait method is resolved from a concrete receiver, and there are no bounds \
                     on a type parameter yet — so a generic definition cannot call one",
                ),
            );
            return unit();
        };
        let Some(found) = self.impls.get(&(trait_name.clone(), head.clone())) else {
            self.diags.push(
                Diagnostic::error(
                    "B0387",
                    format!("`{head}` does not implement `{trait_name}`"),
                    args[at].span(),
                )
                .with_primary_label(format!(
                    "`{method}` needs an `impl {trait_name} for {head}`"
                ))
                .with_label(decl.span, "the trait is declared here"),
            );
            return unit();
        };
        let name = mangle(&trait_name, method, &found.target);
        let ty = match self.schemes.get(&name) {
            Some(sc) => self.subst.instantiate(sc),
            None => return unit(), // the impl itself failed to check; it has said so already
        };
        let func = Core::new(CoreKind::Global(name), ty, span);
        self.apply_fn_with(func, receiver, at, args, span)
    }
}

/// Does this type expression mention `name` anywhere?
fn mentions(n: &Node, name: &str) -> bool {
    n.head_name() == Some(name) || n.args.iter().any(|a| mentions(a, name))
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
        // The feature docs/36 built, meeting the feature this one does: `impl[T] Show for Tree[T]`
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

    #[test]
    fn an_impl_may_not_perform_more_than_the_trait_declares() {
        // The effect row is the trait's, and B0370's "a `uses` clause is the published bound"
        // applies to it exactly as it applies to a hand-written signature.
        let src = "\
trait Show:
    def show(self) -> Str

model Point:
    x: Int

impl Show for Point:
    def show(self):
        return uuid()
";
        let text = errors(src);
        assert!(text.contains("B0370"), "{text}");
        assert!(text.contains("nondet"), "{text}");
    }

    #[test]
    fn a_trait_method_needs_a_concrete_receiver() {
        // The limit this step stops at, asserted so that it starts failing the day bounds land.
        let generic = format!(
            "{SHOW}
def twice[T](x: T) -> Str:
    return x.show()
"
        );
        assert!(codes(&generic).contains(&"B0386"), "{:?}", codes(&generic));

        let as_value = format!(
            "{SHOW}
def all(ps: list[Point]) -> list[Str]:
    return map_list(ps, show)
"
        );
        let text = errors(&as_value);
        assert!(text.contains("B0386"), "{text}");
        assert!(text.contains("cannot be used as a value"), "{text}");
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
