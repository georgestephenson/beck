//! Resolution and typechecking, elaborating straight into `Core`.
//!
//! Stages 4 and 5 of [`docs/04-compiler-architecture.md`](../../../../../docs/04-compiler-architecture.md)
//! §4.1 — "modules, imports, name binding, hygiene scopes → resolved AST" and "HM + rows + effect
//! rows + capabilities → typed AST" — run as one pass that emits stage 6's `Core` directly. §4.2
//! allows exactly three IRs; a separate resolved-but-untyped tree would be a fourth.
//!
//! # Resolution is hygiene-aware
//!
//! A reference resolves to a binding when `binding.scopes ⊆ reference.scopes`, innermost first.
//! That one rule is what makes the macro expander's work mean something: a binding a macro
//! introduced carries a scope the call site does not have, so the call site cannot see it.
//!
//! # What Phase 1 checks, and what it does not
//!
//! Checked: HM inference with unification and let-polymorphism, ADTs (`union`), records (`model`),
//! nominal newtypes, `match` exhaustiveness, mandatory annotations on top-level signatures, and
//! the `Stream`/`Signal`/`fold`/`durable` types of §3.7.
//!
//! Phase 2 adds §3.2's **effect inference**. Every definition gets a row *variable* before any body
//! is checked; checking a body accumulates the latent row of everything it applies; the variable is
//! then bound to what accumulated. A mere *reference* to a function performs nothing — only
//! applying one does — which is the difference between inference and Phase 1's syntactic collection,
//! and the reason `fold(apply_event, …)` is not itself effectful.
//!
//! Mutual recursion needs no ordering: `f`'s row may be bound to a row mentioning `g`'s variable and
//! vice versa, and [`Subst::resolve_row`] computes the least fixed point because a row is a union.
//!
//! Not checked, and named rather than implied: row polymorphism on records, trait constraints on
//! type variables, and `var` mutability (a `var` binding is checked as an ordinary immutable one).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::depth::Nesting;
use beck_diag::{Diagnostic, Diagnostics, Span};
use beck_syntax::{sym, Lit, Node, ScopeSet, Symbol};

use crate::core::{Arm, Const, Core, CoreKind, Pattern, Prim, VarId};
use crate::iface::Interface;
use crate::prelude;
use crate::ty::{self, Effect, Mismatch, Row, RowVarId, Scheme, Subst, Tier, Ty, TyDecl, Variant};

/// A checked module: everything the placement checker, the splitter and the runtime need.
#[derive(Clone, Debug)]
pub struct Program {
    pub name: String,
    pub types: BTreeMap<Arc<str>, TyDecl>,
    /// The `trait` declarations this module owns, in declaration order.
    pub traits: Vec<ty::TraitSig>,
    /// The `impl` headers this module owns, in declaration order. Both cross a `.becki`: a call in
    /// another module cannot resolve `item.pence()` without knowing the trait *and* that the impl
    /// exists.
    pub impls: Vec<ty::ImplSig>,
    /// The types *this* module declares, in declaration order. An imported type is usable here and
    /// published by the module that owns it, never by this one (§3.6).
    pub own_types: Vec<Arc<str>>,
    /// The interfaces this module was checked against.
    pub imports: Vec<String>,
    pub defs: BTreeMap<Arc<str>, Def>,
    /// Source order, so diagnostics and `beck explain` are stable.
    pub def_order: Vec<Arc<str>>,
    pub signals: Vec<SignalDecl>,
    pub tests: Vec<crate::testing::TestDef>,
    /// The `##` doc comment attached to each declaration, keyed by the name it documents:
    /// `todos` for a definition or signal, `Todo` for a type, `Todo.text` for a model field and
    /// `Event.Toggled` for a union variant ([`beck_syntax::doc`]).
    ///
    /// A side table rather than a field on [`Def`], because documentation is not something the
    /// checker, the solver or the runtime may read: nothing downstream of here may behave
    /// differently because a definition is documented.
    pub docs: BTreeMap<Arc<str>, Arc<str>>,
}

#[derive(Clone, Debug)]
pub struct Def {
    pub name: Arc<str>,
    /// The names in `def map[T, U](…)`, in the order written.
    ///
    /// Carried on the `Def` rather than left in the scheme because `beck iface` publishes it: a
    /// `.becki` line that dropped the `[T, U]` would read back as a signature mentioning two types
    /// nobody declared (`docs/32` §32.8).
    pub typarams: Vec<Arc<str>>,
    pub params: Vec<(VarId, Arc<str>, Ty)>,
    pub ret: Ty,
    /// The whole definition as a lambda, so evaluating the name yields a callable value.
    pub body: Core,
    pub tier: Tier,
    /// The inferred row's atoms, resolved and sorted — what placement and the infrastructure
    /// derivation read.
    pub effects: Vec<Effect>,
    /// The inferred row itself, variables and all. This is the signature §3.6 publishes.
    pub row: Row,
    /// What the signature declared with `uses`, if anything.
    pub declared_effects: Vec<Effect>,
    /// The trait bounds on this definition's type parameters, in written order.
    ///
    /// Empty for almost everything. A bounded definition is **not published**: its dictionary
    /// parameters carry names no source could write, and a trait does not cross a module boundary,
    /// so `beck iface` drops it rather than publishing a signature nobody could call.
    pub bounds: Vec<(Arc<str>, Vec<Arc<str>>)>,
    /// True when the signature **stated** its row — so an empty one is a bound of "performs
    /// nothing" rather than an absent declaration.
    ///
    /// A hand-written `def` with no `uses` has this false, because writing nothing means "infer
    /// it". A trait method's row is stated by the trait, empty or not, so every impl method has it
    /// true: otherwise `def show(self) -> Str` would let one implementation reach for a clock and
    /// every caller of `show` would inherit it silently.
    pub row_is_declared: bool,
    /// True when the placement was written by hand rather than solved for (§3.4).
    pub tier_is_annotated: bool,
    /// A signature with nothing behind it: a line of a `.becki` interface, or a trait's method.
    pub is_declaration: bool,
    /// `@signal` — a declaration that publishes a signal rather than a function (§3.6).
    pub declares_signal: bool,
    pub span: Span,
    pub tier_span: Span,
}

/// A top-level signal or stream declaration — the wiring of the program.
#[derive(Clone, Debug)]
pub struct SignalDecl {
    pub name: Arc<str>,
    pub ty: Ty,
    pub expr: Core,
    pub tier: Tier,
    pub effects: Vec<Effect>,
    pub row: Row,
    pub tier_is_annotated: bool,
    pub span: Span,
    pub tier_span: Span,
}

/// The four types a test's clauses are checked against — see
/// [`Checker::test_subjects`](Checker::test_subjects).
#[derive(Clone, Debug, Default)]
struct TestSubjects {
    state: Option<Ty>,
    event: Option<Ty>,
    result: Option<Ty>,
    command: Option<Ty>,
}

#[derive(Clone, Debug)]
enum BindKind {
    Local(VarId, Ty),
    Global(Arc<str>),
    Prim(Prim),
    /// A trait method, resolved to an impl from the type of its receiver at each call site.
    TraitMethod(Arc<str>),
    /// A union variant; carries the union it belongs to.
    Ctor(Arc<str>, Arc<str>),
    /// A model, used as a constructor: `Todo(id=…, text=…)`.
    Model(Arc<str>),
}

#[derive(Clone, Debug)]
struct Binding {
    name: Arc<str>,
    scopes: ScopeSet,
    kind: BindKind,
}

pub struct Checker<'a> {
    diags: &'a mut Diagnostics,
    subst: Subst,
    types: BTreeMap<Arc<str>, TyDecl>,
    schemes: BTreeMap<Arc<str>, Scheme>,
    prims: BTreeMap<Arc<str>, (Prim, Scheme)>,
    /// Innermost last. Resolution walks it backwards.
    locals: Vec<Binding>,
    globals: Vec<Binding>,
    /// The row each definition's signature declares with `uses`.
    declared: BTreeMap<Arc<str>, Row>,
    /// `row Failure = raises(FormError), log` — a name for a bundle, expanded wherever it is used.
    ///
    /// Module-local by design. A `.becki` renders the expanded atoms, because a published contract
    /// that referred to a name the reader has to look up somewhere else would not be a contract.
    row_aliases: BTreeMap<Arc<str>, Row>,
    /// Types declared in this module, in source order.
    own_types: Vec<Arc<str>>,
    /// The row *variable* standing for each definition's inferred row, minted before any body is
    /// checked so that callers can name it and mutual recursion needs no ordering.
    def_row: BTreeMap<Arc<str>, RowVarId>,
    /// The row variables each definition's scheme quantifies over — §3.2's `e`, for a definition a
    /// user wrote rather than one the prelude declares.
    generic_rows: BTreeMap<Arc<str>, Vec<RowVarId>>,
    /// What the body currently being checked has been seen to perform.
    row: Row,
    next_var: VarId,
    /// Set while checking a fold's function, so §3.7's determinism rule can be enforced.
    in_fold: bool,
    /// The type parameters of the `def` whose signature or body is being read. Empty everywhere
    /// else, which is why a monomorphic program cannot accidentally see one (`docs/32` §32.7).
    typarams: BTreeSet<Arc<str>>,
    /// The type parameters of the `model`, `union`, `newtype` or `type` whose fields are being
    /// read, mapped to their position. Empty everywhere else, and never in scope at the same time
    /// as `typarams`: a declaration has no body and a definition has no fields.
    decl_typarams: BTreeMap<Arc<str>, u32>,
    /// Every `trait` this module declares, by name.
    traits: BTreeMap<Arc<str>, traits::TraitDecl>,
    /// Which trait a method name belongs to. One entry per method, because a name may belong to
    /// only one trait — see [`Checker::collect_traits`].
    trait_methods: BTreeMap<Arc<str>, Arc<str>>,
    /// The impls, keyed by trait and by the *head* constructor of the target type.
    impls: BTreeMap<(Arc<str>, Arc<str>), traits::ImplDecl>,
    /// The traits and impls *this* module declares, in order — an imported one is published by the
    /// module that owns it, and republishing would make two modules claim the same contract.
    own_traits: Vec<Arc<str>>,
    own_impls: Vec<(Arc<str>, Arc<str>)>,
    /// The mangled names `expand_impls` produced, so that a definition standing in for a trait
    /// method can be told from one somebody wrote.
    impl_methods: BTreeSet<Arc<str>>,
    /// The dictionary parameters `expand_bounds` appended, per definition, in order. A call site
    /// reads this to know how many of the callee's parameters it has to supply itself.
    dicts: BTreeMap<Arc<str>, Vec<traits::DictParam>>,
    /// How deep this pass is inside an expression or a type, against the ceiling the reader
    /// counts against. The reader's bound does not cover this one: a macro can expand into a tree
    /// deeper than the one that was written, and the checker is the first pass to see it.
    nesting: Nesting,
    mode: Mode,
}

/// What kind of file is being checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// An ordinary `.beck` module: every `def` needs a body.
    Module,
    /// A `.becki` interface (§3.6): every `def` is a signature, and none has a body.
    Interface,
}

/// Check a module that macro expansion has already run over.
pub fn check_module(module: &Node, diags: &mut Diagnostics) -> Program {
    check_module_with(module, Mode::Module, &[], diags)
}

/// Check a module against the interfaces it imports — §3.6's separate compilation.
///
/// The importing module sees signatures and nothing else: types, parameter and result types,
/// effect rows and placements. It never sees a body, which is exactly why editing one downstream
/// costs nothing here.
pub fn check_module_with(
    module: &Node,
    mode: Mode,
    imports: &[(String, Interface)],
    diags: &mut Diagnostics,
) -> Program {
    let name = module
        .args
        .first()
        .and_then(|n| n.as_var())
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "main".into());

    let mut ck = Checker {
        diags,
        subst: Subst::new(),
        types: prelude::types(),
        schemes: BTreeMap::new(),
        prims: BTreeMap::new(),
        locals: Vec::new(),
        globals: Vec::new(),
        declared: BTreeMap::new(),
        row_aliases: BTreeMap::new(),
        own_types: Vec::new(),
        def_row: BTreeMap::new(),
        row: Row::empty(),
        next_var: 0,
        in_fold: false,
        typarams: BTreeSet::new(),
        decl_typarams: BTreeMap::new(),
        traits: BTreeMap::new(),
        trait_methods: BTreeMap::new(),
        impls: BTreeMap::new(),
        own_traits: Vec::new(),
        own_impls: Vec::new(),
        impl_methods: BTreeSet::new(),
        dicts: BTreeMap::new(),
        generic_rows: BTreeMap::new(),
        nesting: Nesting::new(),
        mode,
    };
    for (name, prim, scheme) in prelude::prims() {
        ck.prims.insert(Arc::from(name), (prim, scheme));
        ck.globals.push(Binding {
            name: Arc::from(name),
            scopes: ScopeSet::empty(),
            kind: BindKind::Prim(prim),
        });
    }

    // The language's own traits, before anything local is read. `Num` is what `+`, `-`, `*` and `/`
    // resolve through for a type that is neither `Int` nor `Float` nor `Str`, and it arrives by the
    // same door an imported trait does — so nothing downstream has a special case for it.
    ck.import_traits(&prelude::traits(), &[]);

    // Imported names arrive before anything local is collected, so a local definition may shadow
    // one and the diagnostic points at the local.
    for (module_name, iface) in imports {
        let (types, names) = iface.exports();
        for (n, d) in types {
            ck.types.insert(n, d);
        }
        ck.import_traits(&iface.traits, &iface.impls);
        for (n, e) in names {
            // A bounded import is given back the dictionary parameters the exporting module lowered
            // it with, so a call site here supplies exactly what a call site there would.
            let scheme = if e.bounds.is_empty() {
                e.scheme
            } else {
                ck.import_bounded(&n, &e.bounds, e.scheme)
            };
            ck.schemes.insert(n.clone(), scheme);
            ck.declared.insert(n.clone(), e.row);
            ck.globals.push(Binding {
                name: n.clone(),
                scopes: ScopeSet::empty(),
                kind: BindKind::Global(n),
            });
        }
        let _ = module_name;
    }

    let items: Vec<&Node> = module.args.iter().skip(1).collect();
    // Three passes over the declarations, and the split is what lets a type mention itself or
    // anything declared later (docs/27 §27.3): names, then aliases in dependency order, then every
    // declaration's field types against the complete set of names.
    ck.declare_type_names(&items);
    ck.collect_aliases(&items);
    ck.collect_types(&items);
    ck.register_type_constructors();
    // Traits before impls, and impls before any signature: an `impl` is *desugared* into ordinary
    // definitions, so by the time `collect_signatures` runs there is nothing trait-shaped left for
    // it — or for placement, or for the splitter, or for the evaluator — to know about.
    // Row aliases before anything reads a `uses` clause, and after the types so a `raises(E)`
    // names a type that exists.
    ck.collect_row_aliases(&items);
    ck.collect_traits(&items);
    let expanded = ck.expand_impls(&items);
    // A bounded `def` is rewritten in place, so what follows sees a definition with one more
    // parameter and no bound. The rewrites are owned here because they replace items rather than
    // adding to them.
    let bounded: Vec<(usize, Node)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| ck.expand_bounds(it).map(|n| (i, n)))
        .collect();
    // The impl's methods go **first**, and the order is load-bearing rather than tidy. A row is
    // solved as its definition is checked, so a `try:` in a caller can only see what has already
    // been decided — and a trait method is the one thing every operator call in the module goes
    // through. Checking `Num::add@Money` before the definitions that write `a + b` is what lets a
    // handler discharge the failure that impl performs rather than carrying it as an unresolved
    // tail (`docs/47` §47.4).
    let mut items: Vec<&Node> = expanded.iter().chain(items).collect();
    for (i, node) in &bounded {
        items[expanded.len() + *i] = node;
    }
    ck.collect_signatures(&items);
    ck.collect_signal_names(&items);
    let mut program = ck.check_items(&items, name);
    program.imports = imports.iter().map(|(n, _)| n.clone()).collect();
    program
}

mod tests_in_beck;
mod traits;

pub use traits::is_impl_method;

impl<'a> Checker<'a> {
    fn error(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.diags.push(Diagnostic::error(code, msg, span));
    }

    fn fresh_var(&mut self) -> VarId {
        let v = self.next_var;
        self.next_var += 1;
        v
    }

    /// Record that the body being checked performs this row.
    fn perform(&mut self, row: &Row) {
        let acc = std::mem::take(&mut self.row);
        self.row = acc.union(row);
    }

    /// Check a sub-expression in its own effect scope, returning what it performed. Used for a
    /// lambda body (whose effects belong to the lambda's *type*, not to its enclosing function) and
    /// for each top-level item.
    fn in_scope<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> (T, Row) {
        let outer = std::mem::take(&mut self.row);
        let out = f(self);
        let inner = std::mem::replace(&mut self.row, outer);
        (out, inner)
    }

    // ------------------------------------------------------------------ declarations

    /// Strip `@on(...)` decorators, returning the inner item and the tier it names.
    fn undecorate<'n>(&mut self, item: &'n Node) -> (&'n Node, Option<(Tier, Span)>) {
        self.undecorate_full(item).0
    }

    /// The same, also reporting whether `@signal` was present — §3.6's marker for a published
    /// signal, which is a declaration of a *value* rather than of a function.
    #[allow(clippy::type_complexity)]
    fn undecorate_full<'n>(&mut self, item: &'n Node) -> ((&'n Node, Option<(Tier, Span)>), bool) {
        let mut inner = item;
        let mut tier = None;
        let mut is_signal = false;
        while inner.is_form(sym::DECORATE) && inner.args.len() == 2 {
            let deco = &inner.args[0];
            if deco.head_name() == Some("signal") && deco.args.is_empty() {
                is_signal = true;
            } else if deco.has_head(sym::ON) && deco.args.len() == 1 {
                let span = deco.span();
                match deco.args[0].as_var().and_then(|s| Tier::parse(s.as_str())) {
                    Some(t) => tier = Some((t, span)),
                    None => self.error(
                        "B0300",
                        format!(
                            "`{}` is not a tier",
                            deco.args[0].as_var().map(|s| s.as_str()).unwrap_or("?")
                        ),
                        span,
                    ),
                }
            } else {
                self.diags.push(
                    Diagnostic::warning("B0301", "unsupported decorator", deco.span())
                        .with_note("Phase 1 understands `@on(client|server|data|any)`"),
                );
            }
            inner = &inner.args[1];
        }
        ((inner, tier), is_signal)
    }

    /// Register every declared type's *name* before resolving any declaration's field types.
    ///
    /// [`Checker::collect_types`] used to resolve each declaration as it walked the file, so a type
    /// could only mention types declared above it, and could never mention itself:
    ///
    /// ```text
    /// union Tree:
    ///     Leaf(value: Int)
    ///     Node(left: Tree, right: Tree)   error[B0310]: cannot find type `Tree`
    /// ```
    ///
    /// which is `docs/25-benchmarks-and-expressiveness.md` §25.6 item 2 — §2.2 of SICP *is* "the
    /// closure property", so this ended chapter 2 at §2.2 and took chapters 4 and 5 with it. It is
    /// also the reason no Beck program could describe a tree, a comment thread or an expression.
    ///
    /// The fix is the one [`Checker::collect_signatures`] already made for definitions — "register
    /// every top-level `def`'s signature before checking any body, so definitions may refer to each
    /// other in any order" — applied one layer down. Names first, bodies second.
    ///
    /// Aliases are the exception and are resolved in between (see [`Checker::collect_aliases`]),
    /// because they are *transparent*: `ty_from_node` replaces an alias with its target, so the
    /// target has to be known before any declaration that mentions it is resolved.
    fn declare_type_names(&mut self, items: &[&Node]) {
        for item in items {
            let (item, _) = self.undecorate(item);
            let Some(name) = item
                .args
                .first()
                .and_then(|n| n.as_var())
                .map(|s| s.name.clone())
            else {
                continue;
            };
            // The placeholder carries the declaration's *parameters*, unlike its fields: a
            // recursive mention resolved while the real declaration is still being built —
            // `Node(kids: list[Tree[T]])` — is arity-checked against this, so the placeholder has
            // to know that `Tree` takes one argument even though it does not yet know what a
            // `Tree` contains.
            let params = Self::typaram_names(item);
            // Otherwise a placeholder is never observed: `collect_types` overwrites every one of
            // them, and a name that reaches a later pass unfilled would be a name no declaration
            // produced.
            let placeholder = if item.is_form(sym::MODEL) {
                TyDecl::Model {
                    name: name.clone(),
                    params,
                    fields: Vec::new(),
                }
            } else if item.is_form(sym::UNION) {
                TyDecl::Union {
                    name: name.clone(),
                    params,
                    variants: Vec::new(),
                }
            } else if item.is_form(sym::NEWTYPE) {
                TyDecl::Newtype {
                    name: name.clone(),
                    params,
                    inner: Ty::unit(),
                }
            } else {
                // An alias is deliberately *not* registered here. `ty_from_node` expands an alias
                // the moment it sees one, so registering a placeholder would expand every mention
                // of it to the placeholder's target. `collect_aliases` fills them in next.
                continue;
            };
            if self.types.insert(name.clone(), placeholder).is_some() {
                self.error(
                    "B0302",
                    format!("type `{name}` is declared twice"),
                    item.span(),
                );
            }
            self.own_types.push(name.clone());
        }
    }

    /// The names in a declaration's `(typarams …)` list, as written.
    ///
    /// Unvalidated on purpose: this runs while the set of type names is still being built, so it
    /// cannot yet tell a parameter that shadows a type from one that does not.
    /// [`Checker::bind_decl_typarams`] does that once, when every name is known.
    fn typaram_names(item: &Node) -> Vec<Arc<str>> {
        item.args
            .get(1)
            .filter(|n| n.is_form(sym::TYPARAMS))
            .map(|n| {
                n.args
                    .iter()
                    .filter_map(|p| p.as_var().map(|s| s.name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Put a *declaration's* type parameters in scope, numbered from [`ty::SCHEME_BASE`].
    ///
    /// This is the difference between a declaration and a definition. A `def`'s parameter is
    /// **rigid** — `Ty::con("T")`, which unifies with itself and nothing else, so the body is
    /// forced to work for every `T` (`docs/32` §32.7). A declaration has no body to constrain, and
    /// its parameter has to survive into the stored `TyDecl` so that every later mention of
    /// `Tree[Str]` can substitute for it. A positional variable does both: it cannot be unified
    /// with by accident, because the checker's own variables are numbered from zero, and it is an
    /// index into the arguments of whatever type mentions the name.
    fn bind_decl_typarams(&mut self, item: &Node, decl_name: &str) -> Vec<Arc<str>> {
        self.decl_typarams.clear();
        let mut out: Vec<Arc<str>> = Vec::new();
        let Some(list) = item.args.get(1).filter(|n| n.is_form(sym::TYPARAMS)) else {
            return out;
        };
        for p in &list.args {
            let Some(s) = p.as_var() else { continue };
            let name = s.name.clone();
            if self.types.contains_key(&name) || prelude::builtin_arity(&name).is_some() {
                self.diags.push(
                    Diagnostic::error(
                        "B0314",
                        format!(
                            "`{name}` is already a type, so `{decl_name}` cannot take it as a \
                             parameter"
                        ),
                        p.span(),
                    )
                    .with_primary_label("this name already names a type")
                    .with_note(
                        "a type parameter is a name the declaration invents, and one that shadowed \
                         an existing type would make its fields read as though they mentioned that \
                         type",
                    ),
                );
                continue;
            }
            if out.contains(&name) {
                self.error(
                    "B0315",
                    format!("`{name}` is repeated in `{decl_name}`'s type parameters"),
                    p.span(),
                );
                continue;
            }
            self.decl_typarams
                .insert(name.clone(), ty::SCHEME_BASE + out.len() as u32);
            out.push(name);
        }
        out
    }

    /// Resolve `type` aliases, in dependency order, before anything else reads a type.
    ///
    /// An alias is transparent, so it must be *expanded* rather than referenced, and expanding it
    /// needs its target. Ordering that by hand would put the burden back on the source order this
    /// pass exists to remove, so instead each alias is resolved on demand and the ones it names are
    /// resolved first.
    ///
    /// A cycle is the one case that cannot be resolved rather than merely reordered: `type A = B`
    /// and `type B = A` describe no type at all, and `type Chain = list[Chain]` is an infinitely
    /// large one. A *union* may be recursive because its variants are a finite tag plus fields; an
    /// alias has no such boundary, which is why the two are different passes and only one of them
    /// refuses a cycle.
    fn collect_aliases(&mut self, items: &[&Node]) {
        let mut pending: BTreeMap<Arc<str>, (Node, Span)> = BTreeMap::new();
        let mut order: Vec<Arc<str>> = Vec::new();
        for item in items {
            let (item, _) = self.undecorate(item);
            if !item.is_form(sym::TYPE) || item.args.len() < 3 {
                continue;
            }
            let Some(name) = item.args[0].as_var().map(|s| s.name.clone()) else {
                continue;
            };
            if self.types.contains_key(&name) || pending.contains_key(&name) {
                self.error(
                    "B0302",
                    format!("type `{name}` is declared twice"),
                    item.span(),
                );
                continue;
            }
            // The whole item, not just its target: an alias may be parameterised, and its
            // parameters have to be in scope when the target is read.
            pending.insert(name.clone(), (item.clone(), item.span()));
            order.push(name);
        }
        let mut resolving: Vec<Arc<str>> = Vec::new();
        for name in order {
            self.resolve_alias(&name, &pending, &mut resolving);
        }
    }

    fn resolve_alias(
        &mut self,
        name: &Arc<str>,
        pending: &BTreeMap<Arc<str>, (Node, Span)>,
        resolving: &mut Vec<Arc<str>>,
    ) {
        if self.types.contains_key(name) {
            return;
        }
        let Some((item, span)) = pending.get(name) else {
            return;
        };
        let (item, span) = (item.clone(), *span);
        let node = &item.args[2];
        if resolving.contains(name) {
            self.error(
                "B0312",
                format!(
                    "type alias `{name}` is defined in terms of itself — an alias is transparent, \
                     so this describes no type; a `union` may be recursive, an alias may not"
                ),
                span,
            );
            // Registered as an alias for a fresh variable, so that every *other* mention of it
            // reports nothing further: one cycle is one diagnostic.
            let ty = self.subst.fresh();
            self.types.insert(
                name.clone(),
                TyDecl::Alias {
                    name: name.clone(),
                    params: Self::typaram_names(&item),
                    ty,
                },
            );
            return;
        }
        resolving.push(name.clone());
        for referenced in Self::type_names_in(node) {
            if pending.contains_key(&referenced) {
                self.resolve_alias(&referenced, pending, resolving);
            }
        }
        resolving.pop();
        if self.types.contains_key(name) {
            return; // the cycle branch above already filled it in
        }
        // Bound *after* the aliases this one names are resolved, because resolving them rebinds
        // the same scope.
        let params = self.bind_decl_typarams(&item, name);
        let ty = self.ty_from_node(node);
        self.decl_typarams.clear();
        self.types.insert(
            name.clone(),
            TyDecl::Alias {
                name: name.clone(),
                params,
                ty,
            },
        );
        self.own_types.push(name.clone());
    }

    /// Every type name a type expression mentions, so an alias can be resolved after the aliases it
    /// names.
    ///
    /// Deliberately over-approximate: it reports the head of every application, including builtins like
    /// `list`, and the caller keeps only the ones that are pending aliases. A name it missed would be an
    /// alias resolved too early, which is why it errs the other way.
    fn type_names_in(n: &Node) -> Vec<Arc<str>> {
        let mut out = Vec::new();
        fn walk(n: &Node, out: &mut Vec<Arc<str>>) {
            if let Some(name) = n.head_name() {
                out.push(Arc::from(name));
            }
            for a in &n.args {
                walk(a, out);
            }
        }
        walk(n, &mut out);
        out
    }

    fn collect_types(&mut self, items: &[&Node]) {
        for item in items {
            let (item, _) = self.undecorate(item);
            let Some(name) = item
                .args
                .first()
                .and_then(|n| n.as_var())
                .map(|s| s.name.clone())
            else {
                continue;
            };
            if !item.is_form(sym::MODEL) && !item.is_form(sym::UNION) && !item.is_form(sym::NEWTYPE)
            {
                // `type` aliases were resolved by `collect_aliases`, and everything else is not a
                // type declaration.
                continue;
            }
            // A declaration has no body, so a bound on one is a promise with no reader: nothing
            // inside a `model` or a `union` can call a method. Refused rather than ignored.
            for (p, _) in traits::bounds_of(&item.args[1]) {
                self.diags.push(
                    Diagnostic::error(
                        "B0316",
                        format!("`{name}` cannot bound its type parameter `{p}`"),
                        item.args[1].span(),
                    )
                    .with_note(
                        "a bound says what a body may call, and a declaration has no body; the \
                         definitions that take this type apart are where the bound belongs",
                    ),
                );
            }
            // In scope for every field type below, and only for those: a parameter belongs to the
            // declaration that introduced it.
            let params = self.bind_decl_typarams(item, &name);
            let decl = if item.is_form(sym::MODEL) {
                let fields = item.args[2..]
                    .iter()
                    .filter_map(|f| self.field_decl(f))
                    .collect();
                TyDecl::Model {
                    name: name.clone(),
                    params,
                    fields,
                }
            } else if item.is_form(sym::UNION) {
                let variants = item.args[2..]
                    .iter()
                    .map(|vn| Variant {
                        name: vn
                            .args
                            .first()
                            .and_then(|n| n.as_var())
                            .map(|s| s.name.clone())
                            .unwrap_or_else(|| Arc::from("?")),
                        fields: vn.args[1..]
                            .iter()
                            .filter_map(|f| self.field_decl(f))
                            .collect(),
                    })
                    .collect();
                TyDecl::Union {
                    name: name.clone(),
                    params,
                    variants,
                }
            } else {
                TyDecl::Newtype {
                    name: name.clone(),
                    params,
                    inner: self.ty_from_node(&item.args[2]),
                }
            };
            self.decl_typarams.clear();
            // Overwrites the placeholder `declare_type_names` left. Duplicates were reported there,
            // where both declarations are still in view; reporting them again here would double
            // every message.
            self.types.insert(name.clone(), decl);
        }
    }

    fn field_decl(&mut self, f: &Node) -> Option<(Arc<str>, Ty)> {
        if !f.is_form(sym::FIELD) || f.args.len() != 2 {
            return None;
        }
        let name = f.args[0].as_var()?.name.clone();
        Some((name, self.ty_from_node(&f.args[1])))
    }

    fn register_type_constructors(&mut self) {
        let decls: Vec<TyDecl> = self.types.values().cloned().collect();
        for d in decls {
            match &d {
                TyDecl::Union { name, variants, .. } => {
                    for v in variants {
                        self.globals.push(Binding {
                            name: v.name.clone(),
                            scopes: ScopeSet::empty(),
                            kind: BindKind::Ctor(name.clone(), v.name.clone()),
                        });
                    }
                }
                TyDecl::Model { name, .. } | TyDecl::Newtype { name, .. } => {
                    self.globals.push(Binding {
                        name: name.clone(),
                        scopes: ScopeSet::empty(),
                        kind: BindKind::Model(name.clone()),
                    });
                }
                TyDecl::Alias { .. } => {}
            }
        }
    }

    /// Register every top-level `def`'s signature before checking any body, so definitions may
    /// refer to each other in any order.
    fn collect_signatures(&mut self, items: &[&Node]) {
        for item in items {
            let (item, _) = self.undecorate(item);
            if !item.is_form(sym::DEF) || item.args.len() < 5 {
                continue;
            }
            let Some(name) = item.args[0].as_var().map(|s| s.name.clone()) else {
                continue;
            };
            // The type parameters go into scope *before* the signature is read, so that `T` in
            // `xs: list[T]` resolves to the rigid `T` rather than to `cannot find type`.
            let typarams = self.bind_typarams(&item.args[1], &name);
            let params: Vec<Ty> = item.args[2]
                .args
                .iter()
                .map(|p| {
                    if p.is_form(sym::ANNOT) && p.args.len() == 2 {
                        self.ty_from_node(&p.args[1])
                    } else {
                        self.error(
                            "B0303",
                            "a top-level parameter needs a type annotation",
                            p.span(),
                        );
                        self.subst.fresh()
                    }
                })
                .collect();
            let ret = match item.args[3].args.first() {
                Some(t) => self.ty_from_node(t),
                None => {
                    self.error(
                        "B0304",
                        format!("`{name}` needs a return type"),
                        item.args[0].span(),
                    );
                    self.subst.fresh()
                }
            };
            self.typarams.clear();
            // `record(x)` is a record literal however `record` is bound, so a definition with one
            // of these names would compile and never be reachable. Better a message than a mystery.
            if sym::RESERVED_FORMS.contains(&name.as_ref()) {
                self.diags.push(
                    Diagnostic::error(
                        "B0312",
                        format!("`{name}` is a form of the language, so nothing can be named it"),
                        item.args[0].span(),
                    )
                    .with_primary_label("this name is matched as syntax before it is resolved")
                    .with_note(
                        "the checker recognises these heads structurally, so a definition with one \
                         of their names would be shadowed by the form and never called",
                    ),
                );
            }
            let declared = self.declared_row(item.args.get(4));

            // The definition's latent row is a *variable*, bound once its body has been checked.
            // Minting it here is what lets any definition call any other in any order.
            let rv = self.subst.fresh_row_var();
            // §3.2's `map : (list[a], (a -> b ! e)) -> list[b] ! e`, for a definition a *user*
            // wrote. The row variables the signature's function-typed parameters carry are
            // quantified, so each call site gets its own — without which one caller passing an
            // effectful function makes every other caller effectful too (`docs/33` §33.2).
            let generic_rows = self.generalisable_rows(&params, &ret);
            let mut latent = Row::var(rv);
            latent.tails.extend(generic_rows.iter().copied());
            self.schemes.insert(
                name.clone(),
                Scheme {
                    vars: Vec::new(),
                    row_vars: generic_rows.clone(),
                    params: typarams,
                    ty: Ty::fun_eff(params, ret, latent),
                },
            );
            self.def_row.insert(name.clone(), rv);
            self.generic_rows.insert(name.clone(), generic_rows);
            self.declared.insert(name.clone(), declared);
            self.globals.push(Binding {
                name: name.clone(),
                scopes: ScopeSet::empty(),
                kind: BindKind::Global(name.clone()),
            });
        }
    }

    /// The row variables a definition's signature may quantify over.
    ///
    /// Those written into its *parameters*, and only when the return type carries none of its own.
    /// The restriction is not conservatism for its own sake: a variable quantified in the scheme is
    /// renamed by `instantiate` wherever it appears **syntactically**, and a return type whose row
    /// is bound — through the substitution — to a parameter's would keep the generic variable on
    /// one side of the call and the fresh one on the other. `docs/33` §33.3 says what that costs
    /// and what would lift it.
    fn generalisable_rows(&self, params: &[Ty], ret: &Ty) -> Vec<RowVarId> {
        let mut in_ret = Vec::new();
        row_vars_of(ret, &mut in_ret);
        if !in_ret.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for p in params {
            row_vars_of(p, &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Put a `def`'s `[T, U]` into scope, and answer with the names in order.
    ///
    /// Order matters because it is the order [`Subst::instantiate`] and `beck iface` both use, and
    /// a set would not have one. Shadowing is refused rather than resolved: a type parameter named
    /// after a `model` in the same module is far more likely to be a mistake than an intention, and
    /// there is no syntax to disambiguate it afterwards.
    fn bind_typarams(&mut self, node: &Node, def_name: &str) -> Vec<Arc<str>> {
        self.typarams.clear();
        let mut out: Vec<Arc<str>> = Vec::new();
        for p in &node.args {
            let Some(name) = traits::typaram_name(p) else {
                continue;
            };
            if self.types.contains_key(&name) || prelude::builtin_arity(&name).is_some() {
                self.diags.push(
                    Diagnostic::error(
                        "B0314",
                        format!("`{name}` is already a type, so `{def_name}` cannot take it as a parameter"),
                        p.span(),
                    )
                    .with_primary_label("this name already names a type")
                    .with_note(
                        "a type parameter is a name the definition invents, and one that shadowed \
                         an existing type would make its signature read as though it mentioned that \
                         type",
                    ),
                );
                continue;
            }
            if out.contains(&name) {
                self.error(
                    "B0315",
                    format!("`{name}` is repeated in `{def_name}`'s type parameters"),
                    p.span(),
                );
                continue;
            }
            out.push(name.clone());
            self.typarams.insert(name);
        }
        out
    }

    /// The row a `uses` clause declares. §3.2's atoms are written as they print:
    /// `durable`, `net.out(api.example.com)`, `cap.session`.
    fn declared_row(&mut self, uses: Option<&Node>) -> Row {
        let mut row = Row::empty();
        let Some(u) = uses else { return row };
        for e in &u.args {
            // `net.out(host)` and `cap.session` are ordinary dotted syntax by the time they reach
            // here, so the atom is reassembled from what was written rather than pattern-matched
            // per shape — which is why adding an atom to §3.2's list costs one line in `row.rs`.
            let text = written_form(e).unwrap_or_default();
            // A name that is not an atom may be a row alias. Tried second, so an alias cannot
            // shadow an effect: `row durable = ...` would otherwise silently change what every
            // signature in the module means.
            if let Some(atom) = Effect::parse(&text) {
                row.add(atom);
            } else if let Some(alias) = self.row_aliases.get(text.as_str()).cloned() {
                row = row.union(&alias);
            } else {
                self.error(
                    "B0305",
                    format!(
                        "`{}` is neither an effect nor a row",
                        if text.is_empty() { "?" } else { &text }
                    ),
                    e.span(),
                );
            }
        }
        row
    }

    /// Collect `row Name = …` declarations, before any signature mentions one.
    ///
    /// An alias may name an alias declared earlier in the file. It may not name one declared later,
    /// and that is the one place this differs from types — which may mention anything, in any order
    /// (`docs/27` §27.3). The reason is that a row is a *set* being built here rather than a
    /// declaration being resolved later, and a forward reference would mean a fixpoint over
    /// something a reader cannot see the end of. A cycle is refused for the same reason.
    fn collect_row_aliases(&mut self, items: &[&Node]) {
        for item in items {
            let (item, _) = self.undecorate(item);
            if !item.is_form(sym::ROW) || item.args.len() < 2 {
                continue;
            }
            let Some(name) = item.args[0].as_var().map(|s| s.name.clone()) else {
                continue;
            };
            if self.row_aliases.contains_key(&name) {
                self.error(
                    "B0394",
                    format!("row `{name}` is declared twice"),
                    item.span(),
                );
                continue;
            }
            let body = Node::form("uses", item.args[1..].to_vec(), item.span());
            let row = self.declared_row(Some(&body));
            self.row_aliases.insert(name, row);
        }
    }

    /// Register every top-level signal before checking any of them.
    ///
    /// The signal graph is legitimately *cyclic*: `events` is decided from `todos`, and `todos` is
    /// folded from `events`. §3.7 makes that sound — validation reads the accumulator under the
    /// same lock as the append — so the checker must not require a topological order the program
    /// does not have.
    fn collect_signal_names(&mut self, items: &[&Node]) {
        for item in items {
            let (item, _) = self.undecorate(item);
            if !(item.is_form(sym::LET) || item.is_form(sym::VAR)) || item.args.len() != 2 {
                continue;
            }
            let target = &item.args[0];
            let (name_node, annot) = if target.is_form(sym::ANNOT) && target.args.len() == 2 {
                (&target.args[0], Some(&target.args[1]))
            } else {
                (target, None)
            };
            let Some(s) = name_node.as_var() else {
                continue;
            };
            let ty = match annot {
                Some(t) => self.ty_from_node(t),
                None => self.subst.fresh(),
            };
            self.schemes.insert(s.name.clone(), Scheme::mono(ty));
            self.globals.push(Binding {
                name: s.name.clone(),
                scopes: s.scopes.clone(),
                kind: BindKind::Global(s.name.clone()),
            });
        }
    }

    fn check_items(mut self, items: &[&Node], name: String) -> Program {
        let docs = crate::docgen::collect_docs(items);
        let mut defs = BTreeMap::new();
        let mut def_order = Vec::new();
        let mut signals = Vec::new();
        let mut test_items: Vec<&Node> = Vec::new();

        for item in items {
            let ((inner, annotated), declares_signal) = self.undecorate_full(item);
            let tier_is_annotated = annotated.is_some();
            let (tier, tier_span) = annotated.unwrap_or((Tier::Any, inner.span()));

            if inner.is_form(sym::DEF) {
                if let Some(def) =
                    self.check_def(inner, tier, tier_span, tier_is_annotated, declares_signal)
                {
                    def_order.push(def.name.clone());
                    defs.insert(def.name.clone(), def);
                }
            } else if inner.is_form(sym::LET) || inner.is_form(sym::VAR) {
                if let Some(s) = self.check_signal(inner, tier, tier_span, tier_is_annotated) {
                    signals.push(s);
                }
            } else if inner.is_form(sym::TEST) || inner.is_form(sym::PROPERTY) {
                // Deferred: a test's clauses are typed against the state and event types, which are
                // only known once every signal has been checked. §21.2's "the log is the state" is
                // exactly why — a `given` is a `list[Event]`, and `Event` is whatever the program's
                // own `decide` node produces.
                test_items.push(inner);
            } else if inner.is_form(sym::MODEL)
                || inner.is_form(sym::UNION)
                || inner.is_form(sym::TYPE)
                || inner.is_form(sym::NEWTYPE)
                || inner.is_form(sym::IMPORT)
                || inner.is_form(sym::TRAIT)
                || inner.is_form(sym::IMPL)
                || inner.is_form(sym::ROW)
            {
                // Declarations, all of them already collected. A `trait` was read by
                // `collect_traits` and an `impl` was expanded into the `def`s this loop is
                // checking, so neither has anything left to do here.
            } else {
                self.error("B0307", "unsupported top-level item", inner.span());
            }
        }

        // §21.2's `test` and `property` blocks, now that every signal and definition has been seen.
        //
        // A program that declares signals but has no `decide`/`durable(fold(…))` is one the
        // splitter refuses by name (B0500–B0504), and that refusal is the diagnostic worth reading.
        // Type-checking its tests first would bury it under one error per clause, so the tests are
        // dropped here and the later stage speaks. A module with *no* signals is a different case —
        // a library, which the project pipeline checks on purpose — and there B0706 is the answer.
        let subjects = self.test_subjects(&signals, &defs);
        let broken_topology =
            !signals.is_empty() && (subjects.state.is_none() || subjects.event.is_none());
        let mut tests = Vec::new();
        if !broken_topology {
            for item in test_items {
                if let Some(t) = self.check_test(item, &subjects, &defs) {
                    tests.push(t);
                }
            }
        }

        // Resolve every recorded type through the substitution so that what leaves the checker is
        // ground wherever inference succeeded. Rows resolve here too, and only here: a row bound
        // during one body may mention a variable another body binds later, so nothing is final
        // until every body has been seen.
        for def in defs.values_mut() {
            def.ret = self.subst.resolve(&def.ret);
            for p in &mut def.params {
                p.2 = self.subst.resolve(&p.2);
            }
            resolve_types(&mut def.body, &self.subst);
            def.row = self.subst.resolve_row(&def.row);
            // Close the row variables no caller can ever bind.
            //
            // A free tail means "plus whatever the caller's function argument does", which is only
            // a real quantity when the definition *takes* a function. `mine(s: State, session:
            // Session)` calls `sort_by`, whose scheme is row-polymorphic, and subsumption leaves a
            // fresh trailing variable behind so a later call site could widen it. For a definition
            // with no function parameter there is no later call site: the variable is vacuous, and
            // printing `{e9 | e10}` where the truth is `{}` would make every pure function in
            // `beck explain place` look effectful.
            let mut bindable = Vec::new();
            for (_, _, t) in &def.params {
                self.subst.free_row_vars(t, &mut bindable);
            }
            def.row.tails.retain(|v| bindable.contains(v));
            def.effects = def.row.atoms.iter().cloned().collect();
        }
        for t in &mut tests {
            for clause in &mut t.clauses {
                for c in clause_cores_mut(clause) {
                    resolve_types(c, &self.subst);
                }
            }
            for p in &mut t.params {
                p.2 = self.subst.resolve(&p.2);
            }
        }
        for s in &mut signals {
            s.ty = self.subst.resolve(&s.ty);
            resolve_types(&mut s.expr, &self.subst);
            s.row = self.subst.resolve_row(&s.row);
            // A signal takes no arguments, so nothing can widen its row: every free variable in it
            // is vacuous.
            s.row.tails.clear();
            s.effects = s.row.atoms.iter().cloned().collect();
        }

        // §3.6: "effect widening is a breaking API change". A `uses` clause is therefore a *bound*,
        // and a body that exceeds it is an error rather than a silent widening of the signature —
        // which is the property that makes "a library that starts phoning home cannot do so
        // silently" true of Beck rather than aspirational.
        for name in &def_order {
            let Some(def) = defs.get(name) else { continue };
            if !def.row_is_declared {
                continue;
            }
            let undeclared: Vec<Effect> = def
                .effects
                .iter()
                .filter(|e| !e.is_ambient() && !def.declared_effects.contains(e))
                .cloned()
                .collect();
            if undeclared.is_empty() {
                continue;
            }
            let names: Vec<String> = undeclared.iter().map(|e| e.name()).collect();
            self.diags.push(
                Diagnostic::error(
                    "B0370",
                    format!("`{name}` performs more than its signature declares"),
                    def.span,
                )
                .with_primary_label(format!("undeclared: {}", names.join(", ")))
                .with_note(
                    "a `uses` clause is the published bound, and widening it is a breaking API \
                     change — so the compiler will not widen it for you",
                )
                .with_fix(format!(
                    "declare it: `uses {}`",
                    def.effects
                        .iter()
                        .filter(|e| !e.is_ambient())
                        .map(|e| e.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            );
        }

        // Declaration order, so a rendered `.becki` is stable and a trait a later impl names has
        // already been read by the time the file reaches it again.
        let traits: Vec<ty::TraitSig> = self
            .own_traits
            .iter()
            .filter_map(|n| self.traits.get(n).map(|d| d.sig.clone()))
            .collect();
        // The header, plus what each of its methods turned out to perform. An impl's row is
        // inferred rather than taken from the trait (`docs/47`), so a module that publishes an
        // impl has to publish the rows too — a caller in another module has nowhere else to get
        // them, and taking them off the trait is exactly the unsoundness this closes.
        let impls: Vec<ty::ImplSig> = self
            .own_impls
            .iter()
            .filter_map(|k| self.impls.get(k).map(|d| d.sig.clone()))
            .map(|mut sig| {
                let head = sig.head();
                if let Some(decl) = self.traits.get(&sig.trait_name) {
                    for m in &decl.sig.methods {
                        let mangled = traits::mangle(&sig.trait_name, &m.name, &head);
                        let Some(def) = defs.get(&mangled) else {
                            continue;
                        };
                        let row: Vec<Effect> = def
                            .effects
                            .iter()
                            .filter(|e| !e.is_ambient())
                            .cloned()
                            .collect();
                        if !row.is_empty() {
                            sig.effects.push((m.name.clone(), row));
                        }
                    }
                }
                sig
            })
            .collect();

        Program {
            name,
            types: self.types,
            traits,
            impls,
            own_types: self.own_types,
            imports: Vec::new(),
            defs,
            def_order,
            signals,
            tests,
            docs,
        }
    }

    // ------------------------------------------------------------------ §21.2's test construct

    /// The four types a test's clauses are checked against, read off the program's own signal graph.
    ///
    /// Nothing here is a convention a test author has to know: `given` is a `list[Event]` because
    /// the fold's stream is a `Stream[Event]`, and `result` is `validate`'s return type because
    /// `when` goes through `validate`. A program with no merge point has none of them, and saying
    /// so once here is better than four confusing type errors later.
    fn check_def(
        &mut self,
        item: &Node,
        tier: Tier,
        tier_span: Span,
        tier_is_annotated: bool,
        declares_signal: bool,
    ) -> Option<Def> {
        let name = item.args[0].as_var()?.name.clone();
        let scheme = self.schemes.get(&name)?.clone();
        let Ty::Fun(param_tys, ret, latent) = scheme.ty.clone() else {
            return None;
        };
        // The same rigid names the signature was read with, so an annotation *inside* the body may
        // mention them too — and so that a diagnostic about one prints `T` and not `?7`.
        self.typarams = scheme.params.iter().cloned().collect();

        let before = self.locals.len();
        let mut params = Vec::new();
        for (p, ty) in item.args[2].args.iter().zip(&param_tys) {
            let target = if p.is_form(sym::ANNOT) { &p.args[0] } else { p };
            let Some(s) = target.as_var() else { continue };
            let id = self.fresh_var();
            params.push((id, s.name.clone(), ty.clone()));
            self.locals.push(Binding {
                name: s.name.clone(),
                scopes: s.scopes.clone(),
                kind: BindKind::Local(id, ty.clone()),
            });
        }

        let body_node = item.args.get(5);
        let span = item.span();
        // A `def` with no body is a signature. That is the whole content of a `.becki` (§3.6), and
        // it is a promise nobody keeps in an ordinary module.
        if body_node.is_none() && self.mode == Mode::Module {
            self.diags.push(
                Diagnostic::error("B0335", format!("`{name}` has no body"), span)
                    .with_primary_label("a signature with nothing behind it")
                    .with_note(
                        "a bodyless `def` is a declaration, which is what a `.becki` interface file \
                         is made of; an ordinary module has to define what it declares",
                    ),
            );
        }
        let (body, performed) = self.in_scope(|ck| match body_node {
            Some(b) => ck.block(&b.args, Some(&ret)),
            // A declaration has no body to check against its result type — that is what makes it a
            // declaration. Standing in a `unit` here and unifying it would report every line of a
            // `.becki` as a type error.
            None => Core::new(CoreKind::Const(Const::Unit), ret.as_ref().clone(), span),
        });
        if body_node.is_some() {
            self.unify(&body.ty, &ret, body.span, "return type");
        }
        self.locals.truncate(before);
        self.typarams.clear();

        let declared = self.declared.get(&name).cloned().unwrap_or_default();
        // A declared effect is part of the signature whether or not the body reaches it: a stub
        // that will phone home later must say so today, or its callers would be re-placed by the
        // edit that fills the body in.
        let inferred = performed.union(&declared);
        if let Some(rv) = self.def_row.get(&name).copied() {
            // What the body performs *itself*, with the quantified tails taken out — they are
            // already in the scheme's latent row, and leaving them in `rv` as well would put the
            // *generic* variable into every instantiated call rather than the call's own copy of
            // it. Resolved first, because a tail reached through `map_list`'s own row variable is
            // still that tail (`docs/33` §33.2).
            let generic: &[RowVarId] = self
                .generic_rows
                .get(&name)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let mut own = self.subst.resolve_row(&inferred);
            own.tails.retain(|t| !generic.contains(t));
            self.subst.bind_row(rv, own);
        }

        let lam = Core {
            kind: CoreKind::Lam {
                params: params.iter().map(|(id, _, _)| *id).collect(),
                body: Arc::new(body),
            },
            ty: Ty::Fun(param_tys, ret.clone(), latent),
            tier,
            span,
            last_use: false,
        };

        let mut declared_effects: Vec<Effect> = declared.atoms.iter().cloned().collect();
        declared_effects.sort();
        // An impl method's row is **inferred**, not bounded by the trait's.
        //
        // It was bounded until `docs/46` §46.5: a trait's declared row was a ceiling every impl was
        // held to, which meant a fallible operation could not be a trait method and `Money` could
        // not have `+`. A trait's row is now a floor and a piece of documentation — what a caller
        // of an *unknown* impl may assume — and what a caller of a known one performs is what that
        // impl performs. `.becki` publishes it per impl, so the boundary is not where this
        // becomes untrue.
        let row_is_declared = !declared_effects.is_empty();
        let bounds = self.bounds_of_def(&name);
        Some(Def {
            name,
            typarams: scheme.params.clone(),
            params,
            ret: *ret,
            body: lam,
            tier,
            effects: Vec::new(),
            row: inferred,
            declared_effects,
            bounds,
            row_is_declared,
            tier_is_annotated,
            is_declaration: body_node.is_none(),
            declares_signal,
            span,
            tier_span,
        })
    }

    fn check_signal(
        &mut self,
        item: &Node,
        tier: Tier,
        tier_span: Span,
        tier_is_annotated: bool,
    ) -> Option<SignalDecl> {
        let target = &item.args[0];
        let (name_node, annot) = if target.is_form(sym::ANNOT) && target.args.len() == 2 {
            (&target.args[0], Some(&target.args[1]))
        } else {
            (target, None)
        };
        let name = name_node.as_var()?.name.clone();
        let expected = annot.map(|t| self.ty_from_node(t));

        // A signal is a node in a graph, not a function: its row is what *evaluating its defining
        // expression* performs. Naming another signal contributes nothing — the dependency is an
        // edge, and the edge is what placement reasons about.
        let (expr, row) = self.in_scope(|ck| ck.expr(&item.args[1], expected.as_ref()));
        if let Some(e) = &expected {
            self.unify(&expr.ty, e, expr.span, "declared type");
        }

        // The name was pre-registered so the graph could be cyclic; tie the placeholder to what
        // the expression actually produced.
        if let Some(pre) = self.schemes.get(&name).cloned() {
            self.unify(&expr.ty, &pre.ty, expr.span, "declared type");
        }

        Some(SignalDecl {
            name,
            ty: expr.ty.clone(),
            expr,
            tier,
            effects: Vec::new(),
            row,
            tier_is_annotated,
            span: item.span(),
            tier_span,
        })
    }

    // ------------------------------------------------------------------ types from syntax

    fn ty_from_node(&mut self, n: &Node) -> Ty {
        if !self.enter(n.span()) {
            return self.subst.fresh();
        }
        let out = self.ty_from_node_inner(n);
        self.nesting.leave();
        out
    }

    /// Descend one level of the tree, or refuse.
    ///
    /// `false` means the ceiling is reached: the caller returns whatever it returns for an
    /// expression it could not check — a fresh variable, which unifies with anything and so raises
    /// no second error — without recursing and without leaving.
    fn enter(&mut self, span: Span) -> bool {
        if self.nesting.enter() {
            return true;
        }
        if self.nesting.should_report() {
            let note = self.nesting.note();
            self.diags.push(
                Diagnostic::error("B0390", "the expression nests too deep to check", span)
                    .with_primary_label("the checker gave up here")
                    .with_note(note),
            );
        }
        false
    }

    fn ty_from_node_inner(&mut self, n: &Node) -> Ty {
        let span = n.span();
        // One argument is a **nullary** function type: `() -> T`, which the parser builds with the
        // return type alone. It was `>= 2` here, so `() -> Int` parsed and then reported "cannot
        // find type `fn-type`" — and the one thing that needs it is a thunk, which is what
        // Felleisen's `delay` expands to (`docs/63` §63.3).
        if n.has_head("fn-type") && !n.args.is_empty() {
            let params: Vec<Ty> = n.args[..n.args.len() - 1]
                .iter()
                .map(|a| self.ty_from_node(a))
                .collect();
            let ret = self.ty_from_node(&n.args[n.args.len() - 1]);
            // A written function type says nothing about what the function does, so its row is a
            // variable: `(Todo) -> Bool` accepts a pure predicate and an effectful one alike, and
            // the enclosing definition inherits whichever it is handed.
            return Ty::fun_eff(params, ret, self.subst.fresh_row());
        }
        let Some(name) = n.head_name() else {
            self.error("B0308", "expected a type", span);
            return self.subst.fresh();
        };
        // A type parameter of the definition being read. It is rigid — `Ty::Con(name, [])` unifies
        // with itself and nothing else — which is what makes the body of `def first[T](xs: list[T])
        // -> T` provably work for every `T` rather than for whichever one the body happened to
        // force (`docs/32` §32.7).
        if self.typarams.contains(name) {
            if !n.args.is_empty() {
                self.error(
                    "B0313",
                    format!("`{name}` is a type parameter, so it takes no type arguments"),
                    span,
                );
            }
            return Ty::con(name);
        }
        // A type parameter of the *declaration* being read — positional rather than rigid, because
        // it has to survive into the stored `TyDecl` and be substituted for at every mention of the
        // declaration. See [`Checker::bind_decl_typarams`].
        if let Some(v) = self.decl_typarams.get(name).copied() {
            if !n.args.is_empty() {
                self.error(
                    "B0313",
                    format!("`{name}` is a type parameter, so it takes no type arguments"),
                    span,
                );
            }
            return Ty::Var(v);
        }

        let args: Vec<Ty> = n.args.iter().map(|a| self.ty_from_node(a)).collect();

        // Aliases are transparent; newtypes are not — that is what "ids of different entities must
        // not be interchangeable" (§3.1) means. A parameterised alias is expanded *and* applied:
        // `type Pairs[A] = list[Pair[A, A]]` names no type of its own, so `Pairs[Int]` has to be
        // `list[Pair[Int, Int]]` by the time anything else sees it.
        if let Some(TyDecl::Alias { ty, params, .. }) = self.types.get(name) {
            let (ty, params) = (ty.clone(), params.clone());
            if !self.check_arity(name, &params, args.len(), span) {
                return self.subst.fresh();
            }
            return ty::instantiate_decl(&ty, &args);
        }

        let params = match prelude::builtin_arity(name) {
            Some(a) => letters(a),
            None => match self.types.get(name) {
                Some(d) => d.params().to_vec(),
                None => {
                    self.error("B0310", format!("cannot find type `{name}`"), span);
                    return self.subst.fresh();
                }
            },
        };
        if !self.check_arity(name, &params, args.len(), span) {
            return self.subst.fresh();
        }
        Ty::Con(Arc::from(name), args)
    }

    /// A mention of a type carries exactly as many arguments as the declaration has parameters.
    ///
    /// Reported here rather than left to unification, because `Tree` with its argument missing
    /// would otherwise unify with `Tree[Int]` and the error would surface as a mismatch somewhere
    /// downstream of the line that is actually wrong.
    ///
    /// `params` is the declaration's own parameter names, so the suggestion is a program: there is
    /// no wildcard type in this language, and every argument is either concrete or a parameter
    /// bound where the mention is — including by an `impl` head, which binds its own.
    fn check_arity(&mut self, name: &str, params: &[Arc<str>], got: usize, span: Span) -> bool {
        let arity = params.len();
        if arity == got {
            return true;
        }
        let d = Diagnostic::error(
            "B0311",
            format!("`{name}` takes {arity} type argument(s), got {got}"),
            span,
        );
        self.diags.push(if arity == 0 {
            d.with_primary_label("this type takes no arguments")
        } else {
            let written = params
                .iter()
                .map(|p| p.as_ref())
                .collect::<Vec<_>>()
                .join(", ");
            let one = params[0].as_ref();
            let d = d.with_primary_label(format!("write `{name}[{written}]`"));
            if got < arity {
                // Only when something is *missing*: the reader has to get a type into the
                // brackets, and every way of doing that either names one or binds one.
                d.with_note(format!(
                    "each argument is a concrete type, or a parameter bound where this mention \
                     is — `def f[{one}]`, `model M[{one}]`, or an `impl[{one}]` head"
                ))
            } else {
                d
            }
        });
        false
    }

    /// The type of two alternatives, neither of which is the other's expectation.
    ///
    /// The branches of an `if` are not actual-and-expected, and typing them as though they were is
    /// what refused exercise 1.43 (docs/25 §25.6 item 6): a branch whose row is closed became the
    /// standard a branch whose row is still a variable had to meet. [`crate::ty::Subst::unify_join`]
    /// is the join; this is where its failure becomes a diagnostic.
    fn join(&mut self, then: &Ty, alt: &Ty, span: Span) -> Ty {
        match self.subst.unify_join(then, alt) {
            Ok(ty) => ty,
            Err(e) => {
                let msg = self.mismatch(e, "the two branches");
                self.error("B0320", msg, span);
                then.clone()
            }
        }
    }

    fn unify(&mut self, actual: &Ty, expected: &Ty, span: Span, what: &str) {
        if let Err(e) = self.subst.unify(actual, expected) {
            let msg = self.mismatch(e, what);
            self.error("B0320", msg, span);
        }
    }

    fn mismatch(&self, e: Mismatch, what: &str) -> String {
        match e {
            Mismatch::Different(pair) => {
                let (a, b) = *pair;
                format!("{what} mismatch: expected `{b}`, found `{a}`")
            }
            Mismatch::Arity(a, b) => {
                format!("{what} takes {b} argument(s), got {a}")
            }
            Mismatch::Infinite => format!("{what} would be an infinite type"),
            Mismatch::Effects(e) => {
                format!("{what} may not perform {{{e}}} here")
            }
            Mismatch::UnknownEffects => {
                format!(
                    "{what} may perform effects this context does not allow: one side's effects \
                     are not decided here, and the other's are fixed and empty"
                )
            }
        }
    }

    // ------------------------------------------------------------------ resolution

    /// §2.4's hygiene rule, mechanised: a binding is a candidate for a reference exactly when its
    /// scope set is a subset of the reference's, and the innermost such binding wins.
    fn resolve(&self, s: &Symbol) -> Option<&Binding> {
        self.locals
            .iter()
            .rev()
            .chain(self.globals.iter().rev())
            .find(|b| b.name == s.name && b.scopes.is_subset_of(&s.scopes))
    }

    // ------------------------------------------------------------------ statements

    fn block(&mut self, stmts: &[Node], expected: Option<&Ty>) -> Core {
        let span = stmts.first().map(|s| s.span()).unwrap_or(Span::NONE);
        self.block_from(stmts, expected, span)
    }

    fn block_from(&mut self, stmts: &[Node], expected: Option<&Ty>, span: Span) -> Core {
        let Some((first, rest)) = stmts.split_first() else {
            return Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span);
        };

        if first.is_form(sym::RETURN) {
            if !rest.is_empty() {
                self.diags.push(Diagnostic::warning(
                    "B0330",
                    "statements after `return` are unreachable",
                    rest[0].span(),
                ));
            }
            return match first.args.first() {
                Some(e) => self.expr(e, expected),
                None => Core::new(CoreKind::Const(Const::Unit), Ty::unit(), first.span()),
            };
        }

        if (first.is_form(sym::LET) || first.is_form(sym::VAR)) && first.args.len() == 2 {
            let target = &first.args[0];
            let (name_node, annot) = if target.is_form(sym::ANNOT) && target.args.len() == 2 {
                (&target.args[0], Some(&target.args[1]))
            } else {
                (target, None)
            };
            let want = annot.map(|t| self.ty_from_node(t));
            let value = self.expr(&first.args[1], want.as_ref());
            if let Some(w) = &want {
                self.unify(&value.ty, w, value.span, "declared type");
            }
            let id = self.fresh_var();
            if let Some(s) = name_node.as_var() {
                self.locals.push(Binding {
                    name: s.name.clone(),
                    scopes: s.scopes.clone(),
                    kind: BindKind::Local(id, value.ty.clone()),
                });
            }
            let body = self.block_from(rest, expected, span);
            self.locals.pop();
            let ty = body.ty.clone();
            return Core::new(
                CoreKind::Let {
                    var: id,
                    value: Box::new(value),
                    body: Box::new(body),
                },
                ty,
                first.span(),
            );
        }

        if first.is_form(sym::FOR) || first.is_form(sym::WHILE) {
            self.diags.push(
                Diagnostic::error("B0331", "loops are not available in Phase 1", first.span())
                    .with_primary_label("no statement-level iteration yet")
                    .with_note(
                        "everything is an expression and `var` is not yet mutable, so a loop has \
                     nothing to accumulate into",
                    )
                    .with_fix("use `map_list`, `filter_list` or `fold`"),
            );
            return Core::new(CoreKind::Const(Const::Unit), Ty::unit(), first.span());
        }

        // A guard clause: `if blank: return Err(…)` followed by the rest of the body. In an
        // expression language the rest *is* the else branch — §2.6's "everything is an expression"
        // is what makes early return work without a control-flow graph.
        if first.is_form(sym::IF) && !rest.is_empty() && first.args.len() >= 2 {
            let cond = self.expr(&first.args[0], Some(&Ty::bool_()));
            self.unify(&cond.ty, &Ty::bool_(), cond.span, "condition");
            let then = self.body_expr(&first.args[1], expected);
            let alt = match first.args.get(2) {
                Some(explicit) => {
                    self.diags.push(Diagnostic::warning(
                        "B0330",
                        "statements after an `if`/`else` that both return are unreachable",
                        rest[0].span(),
                    ));
                    self.body_expr(explicit, expected)
                }
                None => self.block_from(rest, expected, span),
            };
            let ty = self.join(&then.ty, &alt.ty, then.span);
            return Core::new(
                CoreKind::If {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    alt: Box::new(alt),
                },
                ty,
                first.span(),
            );
        }

        // The last statement is the block's value; anything before it is sequenced.
        if rest.is_empty() {
            return self.expr(first, expected);
        }
        let value = self.expr(first, None);
        let body = self.block_from(rest, expected, span);
        let id = self.fresh_var();
        let ty = body.ty.clone();
        Core::new(
            CoreKind::Let {
                var: id,
                value: Box::new(value),
                body: Box::new(body),
            },
            ty,
            first.span(),
        )
    }

    /// A `do` block used where an expression is wanted.
    fn body_expr(&mut self, n: &Node, expected: Option<&Ty>) -> Core {
        if n.is_form(sym::DO) {
            let before = self.locals.len();
            let out = self.block(&n.args, expected);
            self.locals.truncate(before);
            out
        } else {
            self.expr(n, expected)
        }
    }

    // ------------------------------------------------------------------ expressions

    fn expr(&mut self, n: &Node, expected: Option<&Ty>) -> Core {
        if !self.enter(n.span()) {
            return Core::new(CoreKind::Const(Const::Unit), self.subst.fresh(), n.span());
        }
        let out = self.expr_inner(n, expected);
        self.nesting.leave();
        out
    }

    fn expr_inner(&mut self, n: &Node, expected: Option<&Ty>) -> Core {
        let span = n.span();

        if let Some(l) = n.as_lit() {
            return match l {
                Lit::Int(i) => Core::new(CoreKind::Const(Const::Int(*i)), Ty::int(), span),
                Lit::Float(f) => {
                    Core::new(CoreKind::Const(Const::Float(*f)), Ty::con(Ty::FLOAT), span)
                }
                Lit::Bool(b) => Core::new(CoreKind::Const(Const::Bool(*b)), Ty::bool_(), span),
                Lit::Str(s) => Core::new(CoreKind::Const(Const::Str(s.clone())), Ty::str_(), span),
                Lit::Keyword(k) => {
                    Core::new(CoreKind::Const(Const::Str(k.clone())), Ty::str_(), span)
                }
            };
        }

        if let Some(s) = n.as_var() {
            if s.as_str() == "unit" {
                return Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span);
            }
            return self.var_ref(s, span);
        }

        let head = n.head_name().unwrap_or("");
        match head {
            sym::DO => self.body_expr(n, expected),
            sym::IF if n.args.len() >= 2 => {
                let cond = self.expr(&n.args[0], Some(&Ty::bool_()));
                self.unify(&cond.ty, &Ty::bool_(), cond.span, "condition");
                let then = self.body_expr(&n.args[1], expected);
                let alt = match n.args.get(2) {
                    Some(a) => self.body_expr(a, expected),
                    None => Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span),
                };
                let ty = self.join(&then.ty, &alt.ty, alt.span);
                Core::new(
                    CoreKind::If {
                        cond: Box::new(cond),
                        then: Box::new(then),
                        alt: Box::new(alt),
                    },
                    ty,
                    span,
                )
            }
            sym::FN if n.args.len() == 2 => self.lambda(n, expected, span),
            sym::RAISE if n.args.len() == 1 => self.raise_expr(&n.args[0], span),
            sym::TRY if n.args.len() == 1 => self.try_expr(&n.args[0], expected, span),
            sym::MATCH if !n.args.is_empty() => self.match_expr(n, expected, span),
            sym::LIST => {
                let elem = expected
                    .and_then(|t| match t {
                        Ty::Con(c, args) if c.as_ref() == Ty::LIST && args.len() == 1 => {
                            Some(args[0].clone())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| self.subst.fresh());
                let items: Vec<Core> = n
                    .args
                    .iter()
                    .map(|a| {
                        let c = self.expr(a, Some(&elem));
                        self.unify(&c.ty, &elem, c.span, "list element");
                        c
                    })
                    .collect();
                Core::new(CoreKind::ListLit(items), Ty::list(elem), span)
            }
            sym::MAP => {
                let k = self.subst.fresh();
                let v = self.subst.fresh();
                let mut pairs = Vec::new();
                for pair in n.args.chunks(2) {
                    if pair.len() != 2 {
                        break;
                    }
                    let kc = self.expr(&pair[0], Some(&k));
                    self.unify(&kc.ty, &k, kc.span, "map key");
                    let vc = self.expr(&pair[1], Some(&v));
                    self.unify(&vc.ty, &v, vc.span, "map value");
                    pairs.push((kc, vc));
                }
                Core::new(CoreKind::MapLit(pairs), Ty::map(k, v), span)
            }
            sym::RECORD => self.record_lit(n, expected, span),
            sym::DOT if n.args.len() >= 2 => self.dot(n, span),
            "index" if n.args.len() == 2 => {
                let base = self.expr(&n.args[0], None);
                let key = self.expr(&n.args[1], None);
                let v = self.subst.fresh();
                self.unify(
                    &base.ty,
                    &Ty::map(key.ty.clone(), v.clone()),
                    span,
                    "indexing",
                );
                Core::new(
                    CoreKind::Prim {
                        op: Prim::MapGet,
                        args: vec![base, key],
                    },
                    Ty::option(v),
                    span,
                )
            }
            "+" | "-" | "*" | "/" if n.args.len() == 2 => {
                let op = match head {
                    "+" => Prim::Add,
                    "-" => Prim::Sub,
                    "*" => Prim::Mul,
                    _ => Prim::Div,
                };
                self.arith(op, &n.args[0], &n.args[1], expected, span)
            }
            "negate" if n.args.len() == 1 => {
                let arg = self.expr(&n.args[0], expected);
                let want = self.numeric_of(&arg.ty, expected).unwrap_or_else(Ty::int);
                self.unify(&arg.ty, &want, arg.span, "operand of `-`");
                Core::new(
                    CoreKind::Prim {
                        op: Prim::Neg,
                        args: vec![arg],
                    },
                    want,
                    span,
                )
            }
            "abs" if n.args.len() == 1 && n.applied => {
                // The one *named* member of the tower that is resolved rather than declared. SICP
                // writes `abs` at both tiers, and a scheme cannot say "Int or Float" without a
                // numeric class; `docs/32` §32.3 argues why the class is not worth it yet.
                let arg = self.expr(&n.args[0], expected);
                let want = match self.numeric_of(&arg.ty, expected) {
                    Some(t) => t,
                    None => Ty::int(),
                };
                self.unify(&arg.ty, &want, arg.span, "operand of `abs`");
                Core::new(
                    CoreKind::Prim {
                        op: Prim::Abs,
                        args: vec![arg],
                    },
                    want,
                    span,
                )
            }
            sym::QUOTE => {
                self.error("B0332", "a `quote` survived macro expansion", span);
                Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span)
            }
            sym::KW_ARG => {
                self.error("B0333", "a keyword argument outside a call", span);
                Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span)
            }
            _ if n.applied => self.call(n, expected, span),
            _ => {
                self.error("B0334", "unsupported expression", span);
                Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span)
            }
        }
    }

    /// The arithmetic operators, resolved from their operands rather than from a type class.
    ///
    /// Phase 1 gave `+` this treatment already, for `Str` concatenation: "a `(a, a) -> a` scheme
    /// would let `Bool + Bool` typecheck". The numeric tower needs the same answer for the same
    /// reason and one more tier — a real — and `docs/32` §32.3 sets out why an ad-hoc resolution is
    /// the honest thing to build before traits exist rather than a stand-in for them.
    ///
    /// The rule is: whichever of the two operands and the expectation *first* resolves to a numeric
    /// type decides, and `Int` is what an expression with nothing known about it defaults to — so
    /// every program written before reals existed still means what it meant.
    fn arith(
        &mut self,
        op: Prim,
        lhs_node: &Node,
        rhs_node: &Node,
        expected: Option<&Ty>,
        span: Span,
    ) -> Core {
        let lhs = self.expr(lhs_node, None);
        let rhs = self.expr(rhs_node, None);
        // `+` alone also concatenates, which is what the sketch's footer wants.
        let is_str = op == Prim::Add
            && (self.subst.resolve(&lhs.ty).con_name() == Some(Ty::STR)
                || self.subst.resolve(&rhs.ty).con_name() == Some(Ty::STR)
                || expected
                    .map(|t| t.con_name() == Some(Ty::STR))
                    .unwrap_or(false));
        let numeric = if is_str {
            Some(Ty::str_())
        } else {
            self.numeric_of(&lhs.ty, None)
                .or_else(|| self.numeric_of(&rhs.ty, None))
                .or_else(|| expected.and_then(|t| self.numeric_of(t, None)))
        };
        // Neither operand is a number and neither is a string: the third floor of the tower, which
        // a user's type joins by implementing `Num` (`docs/41` §41.2). Only when there is an
        // implementation to dispatch to — otherwise the old rule runs and says what it always said,
        // so `1 + true` is still a mismatch rather than a lecture about traits.
        if numeric.is_none() {
            if let Some(core) = self.arith_through_num(op, &lhs, &rhs, span) {
                return core;
            }
        }
        let want = numeric.unwrap_or_else(Ty::int);
        let label = format!("operand of `{}`", op.name());
        self.unify(&lhs.ty, &want, lhs.span, &label);
        self.unify(&rhs.ty, &want, rhs.span, &label);
        Core::new(
            CoreKind::Prim {
                op,
                args: vec![lhs, rhs],
            },
            want,
            span,
        )
    }

    /// `a + b` where `a` is neither a number nor a string, resolved through `Num`.
    ///
    /// SICP §2.5.1's generic arithmetic, and `docs/32` §32.3's deferred decision taken: the four
    /// operators are the four methods of one prelude trait, so a `Rational` joins the tower the way
    /// the book joins it — by implementing the operations, not by being added to a list inside the
    /// compiler.
    ///
    /// Returns `None` when there is nothing to dispatch to, and the caller falls back to the
    /// numeric rule unchanged. The failure this *does* report is the one worth reporting: an
    /// operand whose type is a declared one with no implementation, where "expected `Int`, found
    /// `Rational`" names the symptom and `impl Num for Rational` is the cure.
    fn arith_through_num(&mut self, op: Prim, lhs: &Core, rhs: &Core, span: Span) -> Option<Core> {
        let method: Arc<str> = Arc::from(prelude::num_method(op)?);
        let num: Arc<str> = Arc::from(prelude::NUM);
        let ty = [&lhs.ty, &rhs.ty]
            .into_iter()
            .map(|t| self.subst.resolve(t))
            .find(|t| self.joins_the_tower(t))?;
        let head = ty.con_name().map(Arc::<str>::from)?;
        let known = self.impls.contains_key(&(num.clone(), head.clone()))
            || self
                .resolve(&Symbol::new(traits::mangle(&num, &method, &head)))
                .is_some();
        if !known {
            // A declared type with no implementation. Reported here rather than left to the numeric
            // rule, because "this type is not in the tower, and here is how to put it there" is a
            // different sentence from "this is not an `Int`".
            if self.types.contains_key(&head) {
                self.diags.push(
                    Diagnostic::error(
                        "B0387",
                        format!("`{head}` does not implement `{num}`"),
                        span,
                    )
                    .with_primary_label(format!("`{}` resolves through it", op.name()))
                    .with_fix(format!("write `impl {num} for {head}`")),
                );
                return Some(Core::new(CoreKind::Const(Const::Unit), ty, span));
            }
            return None;
        }
        let func = self.dictionary(&num, &method, &ty, span)?;
        let Ty::Fun(params, ret, row) = self.subst.resolve(&func.ty) else {
            return None;
        };
        self.perform(&row);
        let label = format!("operand of `{}`", op.name());
        self.unify(&lhs.ty, &params[0], lhs.span, &label);
        self.unify(&rhs.ty, &params[1], rhs.span, &label);
        Some(Core::new(
            CoreKind::App {
                func: Box::new(func),
                args: vec![lhs.clone(), rhs.clone()],
            },
            *ret,
            span,
        ))
    }

    /// Could this type be a floor of the numeric tower a user built?
    ///
    /// Everything with a name except the two the primitives already handle and the one `+` also
    /// concatenates. A unification variable is not: an expression nothing has pinned down yet still
    /// defaults to `Int`, which is what keeps every program written before this compiling.
    fn joins_the_tower(&self, t: &Ty) -> bool {
        !matches!(
            t.con_name(),
            None | Some(Ty::INT) | Some(Ty::FLOAT) | Some(Ty::STR)
        )
    }

    /// `Int` or `Float` if either is what this type already is, otherwise nothing.
    ///
    /// "Otherwise nothing" rather than "otherwise Int" matters: an unresolved variable must not
    /// commit the expression, or `abs(x)` inside a `Float -> Float` definition would fix `x` to
    /// `Int` before the parameter's annotation had been consulted.
    fn numeric_of(&mut self, ty: &Ty, expected: Option<&Ty>) -> Option<Ty> {
        for candidate in [Some(ty), expected].into_iter().flatten() {
            match self.subst.resolve(candidate).con_name() {
                Some(Ty::INT) => return Some(Ty::int()),
                Some(Ty::FLOAT) => return Some(Ty::con(Ty::FLOAT)),
                _ => {}
            }
        }
        None
    }

    fn var_ref(&mut self, s: &Symbol, span: Span) -> Core {
        let Some(b) = self.resolve(s).cloned() else {
            self.error("B0340", format!("cannot find `{s}` in this scope"), span);
            let t = self.subst.fresh();
            return Core::new(CoreKind::Const(Const::Unit), t, span);
        };
        match b.kind {
            BindKind::Local(id, ty) => Core::new(CoreKind::Var(id), ty, span),
            // A trait method is resolved from the type of its receiver, so there is nothing to
            // hand over until it is applied. `map_list(xs, show)` would need a dictionary.
            BindKind::TraitMethod(m) => {
                let owner = self.trait_methods.get(&m).cloned();
                self.diags.push(
                    Diagnostic::error(
                        "B0386",
                        format!("`{m}` is a trait method and cannot be used as a value"),
                        span,
                    )
                    .with_primary_label(match &owner {
                        Some(t) => format!("declared by trait `{t}`"),
                        None => "a trait method".into(),
                    })
                    .with_note(
                        "which implementation it means is decided by the type of its receiver, so \
                         it has to be called rather than passed; passing one needs bounds on a type \
                         parameter, which is not built",
                    ),
                );
                Core::new(CoreKind::Const(Const::Unit), self.subst.fresh(), span)
            }
            BindKind::Global(name) => {
                if self.dicts.contains_key(&name) {
                    self.diags.push(
                        Diagnostic::error(
                            "B0386",
                            format!("`{name}` has a bound, so it cannot be used as a value"),
                            span,
                        )
                        .with_note(
                            "a bounded definition is handed its implementations at the call site, \
                             and a reference that is never called has no call site to hand them \
                             over",
                        ),
                    );
                }
                let ty = self
                    .schemes
                    .get(&name)
                    .map(|sc| self.subst.instantiate(sc))
                    .unwrap_or_else(|| self.subst.fresh());
                Core::new(CoreKind::Global(name), ty, span)
            }
            BindKind::Prim(p) => {
                // A primitive used as a value becomes a lambda wrapping it, so it can be passed to
                // `map_list` like any other function.
                let (_, scheme) = self.prims.get(p.name()).cloned().expect("prim registered");
                let ty = self.subst.instantiate(&scheme);
                // Referencing a function performs nothing; the row rides on the *type* and is
                // charged to whoever applies it.
                let Ty::Fun(params, ret, latent) = ty.clone() else {
                    return Core::new(
                        CoreKind::Prim {
                            op: p,
                            args: vec![],
                        },
                        ty,
                        span,
                    );
                };
                let ids: Vec<VarId> = params.iter().map(|_| self.fresh_var()).collect();
                let args: Vec<Core> = ids
                    .iter()
                    .zip(&params)
                    .map(|(id, t)| Core::new(CoreKind::Var(*id), t.clone(), span))
                    .collect();
                Core::new(
                    CoreKind::Lam {
                        params: ids,
                        body: Arc::new(Core::new(
                            CoreKind::Prim { op: p, args },
                            *ret.clone(),
                            span,
                        )),
                    },
                    Ty::Fun(params, ret, latent),
                    span,
                )
            }
            BindKind::Ctor(union, variant) => self.make(&union, Some(&variant), &[], span),
            BindKind::Model(model) => self.make(&model, None, &[], span),
        }
    }

    // ---------------------------------------------------------- failure, as a row label

    /// `raise e` — perform `raises(T)`, and have no type of its own.
    ///
    /// The result is a fresh variable rather than `never`, for the reason `docs/38` §38.4 gives for
    /// the whole shape: a raise is an *effect*, so the expression it stands in for is whatever the
    /// context wanted. `if text == "": raise Blank else: text` is a `Str`.
    fn raise_expr(&mut self, arg: &Node, span: Span) -> Core {
        let value = self.expr(arg, None);
        let ty = self.subst.resolve(&value.ty);
        let Some(name) = error_ty_name(&ty) else {
            self.error(
                "B0391",
                format!("a raised value must have a declared type, and this one is `{ty}`"),
                value.span,
            );
            return Core::new(CoreKind::Const(Const::Unit), self.subst.fresh(), span);
        };
        // The atom names the type, so a handler can say what it catches. This is why `Raise` is the
        // one primitive whose row `Prim::effects` cannot state: it is a function of the argument.
        self.perform(&Row::of([Effect::Raises(name)]));
        Core::new(
            CoreKind::Prim {
                op: Prim::Raise,
                args: vec![value],
            },
            self.subst.fresh(),
            span,
        )
    }

    /// `try: block` — run the block, and reify one failure as a `Result[T, E]`.
    ///
    /// This is the handler, and it is a *form*: lexically scoped by construction, with no dynamic
    /// search for who handles what (POPL 2019's result, `docs/38` §38.4).
    ///
    /// **It catches one error type and lets every other failure travel**, which is what makes it
    /// composable rather than a barrier. `E` comes from the expectation where there is one — a
    /// `try:` almost always flows into something whose type says `Result[T, E]` — and from the
    /// block's own row where there is not. Taking it from the expectation is not a convenience: a
    /// row is decided lazily, so a call to a definition declared *later* in the file contributes a
    /// row *variable* at this point and a handler that could only read atoms would be wrong about
    /// exactly the forward references a program is made of.
    ///
    /// Whatever is not caught stays in the enclosing row — other `raises` atoms, every other
    /// effect, and the row variables, which may hide a failure this handler has no type for. That
    /// last point is why the primitive is given the name of what it catches: the runtime compares.
    fn try_expr(&mut self, body: &Node, expected: Option<&Ty>, span: Span) -> Core {
        // The `Result[T, E]` a caller expects tells the block both halves: what its value type
        // should be, and which failure this handler is for.
        let (inner_expected, expected_error) = match expected.map(|t| self.subst.resolve(t)) {
            Some(Ty::Con(c, args)) if c.as_ref() == Ty::RESULT && args.len() == 2 => (
                Some(args[0].clone()),
                match self.subst.resolve(&args[1]) {
                    Ty::Con(e, es) if es.is_empty() => Some(e),
                    _ => None,
                },
            ),
            _ => (None, None),
        };

        let outer = std::mem::take(&mut self.row);
        let before = self.locals.len();
        let core = self.body_expr(body, inner_expected.as_ref());
        self.locals.truncate(before);
        // Resolved, not raw: a call to something whose row is still a variable contributes a tail,
        // and the atoms behind it are only visible once the substitution has caught up.
        let inner = self
            .subst
            .resolve_row(&std::mem::replace(&mut self.row, outer));

        let mut raised: Vec<Arc<str>> = Vec::new();
        for atom in &inner.atoms {
            if let Effect::Raises(t) = atom {
                if !raised.contains(t) {
                    raised.push(t.clone());
                }
            }
        }
        raised.sort();

        let error = match expected_error {
            Some(e) => e,
            None => match raised.len() {
                1 => raised[0].clone(),
                0 => {
                    self.error(
                        "B0392",
                        "nothing here can fail, and nothing says what this would catch",
                        span,
                    );
                    return core;
                }
                _ => {
                    let names: Vec<String> = raised.iter().map(|t| format!("`{t}`")).collect();
                    self.error(
                        "B0393",
                        format!(
                            "this block can fail in {} ways ({}), so say which one to catch — a \
                             `Result[T, E]` on the enclosing signature is how",
                            raised.len(),
                            names.join(", ")
                        ),
                        span,
                    );
                    raised[0].clone()
                }
            },
        };

        // Everything except the failure being caught is still performed by the enclosing
        // definition. A handler catches one failure; it does not launder a `durable`, and it does
        // not silently swallow a second error type.
        let mut rest = Row::empty();
        rest.tails = inner.tails.clone();
        for atom in &inner.atoms {
            if !matches!(atom, Effect::Raises(t) if *t == error) {
                rest.atoms.insert(atom.clone());
            }
        }
        self.perform(&rest);

        let value_ty = core.ty.clone();
        let result_ty = Ty::app(Ty::RESULT, vec![value_ty, Ty::con(&error)]);
        let thunk = Core::new(
            CoreKind::Lam {
                params: Vec::new(),
                body: Arc::new(core),
            },
            self.subst.fresh(),
            span,
        );
        Core::new(
            CoreKind::Prim {
                op: Prim::Try,
                args: vec![
                    thunk,
                    Core::new(CoreKind::Const(Const::Str(error.clone())), Ty::str_(), span),
                ],
            },
            result_ty,
            span,
        )
    }

    fn lambda(&mut self, n: &Node, expected: Option<&Ty>, span: Span) -> Core {
        let want: Option<(Vec<Ty>, Ty)> = expected.and_then(|t| match self.subst.resolve(t) {
            Ty::Fun(ps, r, _) => Some((ps, *r)),
            _ => None,
        });
        let before = self.locals.len();
        let mut ids = Vec::new();
        let mut tys = Vec::new();
        for (i, p) in n.args[0].args.iter().enumerate() {
            let (target, annot) = if p.is_form(sym::ANNOT) && p.args.len() == 2 {
                (&p.args[0], Some(&p.args[1]))
            } else {
                (p, None)
            };
            let ty = match annot {
                Some(t) => self.ty_from_node(t),
                None => want
                    .as_ref()
                    .and_then(|(ps, _)| ps.get(i).cloned())
                    .unwrap_or_else(|| self.subst.fresh()),
            };
            let id = self.fresh_var();
            if let Some(s) = target.as_var() {
                self.locals.push(Binding {
                    name: s.name.clone(),
                    scopes: s.scopes.clone(),
                    kind: BindKind::Local(id, ty.clone()),
                });
            }
            ids.push(id);
            tys.push(ty);
        }
        let ret_want = want.as_ref().map(|(_, r)| r.clone());
        // What a lambda's body does is what the *lambda* does when called, not what the enclosing
        // definition does by writing it down. `sort_by(xs, lambda t: t.text)` performs nothing.
        let (body, row) = self.in_scope(|ck| ck.body_expr(&n.args[1], ret_want.as_ref()));
        self.locals.truncate(before);
        let ret = body.ty.clone();
        Core::new(
            CoreKind::Lam {
                params: ids,
                body: Arc::new(body),
            },
            Ty::fun_eff(tys, ret, row),
            span,
        )
    }

    fn match_expr(&mut self, n: &Node, expected: Option<&Ty>, span: Span) -> Core {
        let scrutinee = self.expr(&n.args[0], None);
        let scrut_ty = self.subst.resolve(&scrutinee.ty);
        let result = expected.cloned().unwrap_or_else(|| self.subst.fresh());

        let mut arms = Vec::new();
        let mut covered: BTreeSet<Arc<str>> = BTreeSet::new();
        let mut irrefutable = false;

        for arm in &n.args[1..] {
            if !arm.is_form(sym::CASE) || arm.args.len() != 2 {
                continue;
            }
            let before = self.locals.len();
            let pattern = self.pattern(&arm.args[0], &scrut_ty, &mut covered, &mut irrefutable);
            let body = self.body_expr(&arm.args[1], Some(&result));
            self.unify(&body.ty, &result, body.span, "match arm");
            self.locals.truncate(before);
            arms.push(Arm {
                pattern,
                body,
                span: arm.span(),
            });
        }

        // §3.1: "a fold over a `union Event` that misses a case is a compile error — this single
        // check carries the migration story" (§3.9).
        // A `match` on a list is exhaustive when it covers the empty list and a non-empty one.
        // Two cases, because the pattern language has two shapes that partition a list — which is
        // the whole reason it has exactly those two shapes (`docs/33` §33.5).
        if !irrefutable && scrut_ty.con_name() == Some(Ty::LIST) {
            let has_empty = covered.contains(LIST_EMPTY);
            let has_tail = covered.contains(LIST_NONEMPTY);
            if !(has_empty && has_tail) {
                let missing = if has_empty {
                    "a list with elements — `case [first, *rest]`"
                } else if has_tail {
                    "the empty list — `case []`"
                } else {
                    "the empty list and a list with elements — `case []` and `case [first, *rest]`"
                };
                self.diags.push(
                    Diagnostic::error("B0341", "match is not exhaustive", span)
                        .with_primary_label(format!("missing: {missing}"))
                        .with_note(
                            "a list is empty or it is not, and a fold that handles only one of \
                             those is a fold that fails on the input nobody tested",
                        ),
                );
            }
        }

        if !irrefutable {
            if let Some(TyDecl::Union { variants, .. }) =
                scrut_ty.con_name().and_then(|c| self.types.get(c))
            {
                let missing: Vec<String> = variants
                    .iter()
                    .filter(|v| !covered.contains(&v.name))
                    .map(|v| v.name.to_string())
                    .collect();
                if !missing.is_empty() {
                    self.diags.push(
                        Diagnostic::error("B0341", "match is not exhaustive", span)
                            .with_primary_label(format!("missing: {}", missing.join(", ")))
                            .with_note(
                                "adding a variant must break every fold that consumes it — that \
                                 is what makes a missed migration a compile error rather than a \
                                 3 a.m. page",
                            ),
                    );
                }
            }
        }

        Core::new(
            CoreKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            result,
            span,
        )
    }

    fn pattern(
        &mut self,
        p: &Node,
        scrut: &Ty,
        covered: &mut BTreeSet<Arc<str>>,
        irrefutable: &mut bool,
    ) -> Pattern {
        let span = p.span();
        if let Some(l) = p.as_lit() {
            return Pattern::Const(match l {
                Lit::Int(i) => Const::Int(*i),
                Lit::Float(f) => Const::Float(*f),
                Lit::Bool(b) => Const::Bool(*b),
                Lit::Str(s) | Lit::Keyword(s) => Const::Str(s.clone()),
            });
        }
        if let Some(s) = p.as_var() {
            if s.as_str() == sym::WILDCARD {
                *irrefutable = true;
                return Pattern::Wildcard;
            }
            // A bare name that is a nullary constructor matches that variant; anything else binds.
            if let Some(Binding {
                kind: BindKind::Ctor(_, variant),
                ..
            }) = self.resolve(s).cloned()
            {
                covered.insert(variant.clone());
                return Pattern::Ctor {
                    variant,
                    binds: Vec::new(),
                };
            }
            *irrefutable = true;
            let id = self.fresh_var();
            self.locals.push(Binding {
                name: s.name.clone(),
                scopes: s.scopes.clone(),
                kind: BindKind::Local(id, scrut.clone()),
            });
            return Pattern::Bind(id);
        }

        // `[]`, `[x]`, `[first, *rest]` — a list taken apart. The scrutinee decides the element
        // type, so the binders need no annotation (`docs/33` §33.5).
        if p.is_form(sym::LIST) {
            let elem = self.subst.fresh();
            self.unify(
                scrut,
                &Ty::list(elem.clone()),
                span,
                "a list pattern matches a list",
            );
            let mut binds = Vec::new();
            let mut rest = None;
            for (i, item) in p.args.iter().enumerate() {
                let is_rest = item.is_form(sym::REST) && item.args.len() == 1;
                let target = if is_rest { &item.args[0] } else { item };
                if is_rest && i + 1 != p.args.len() {
                    self.error(
                        "B0346",
                        "`*rest` has to be the last element of a list pattern",
                        item.span(),
                    );
                    continue;
                }
                let ty = if is_rest {
                    Ty::list(elem.clone())
                } else {
                    elem.clone()
                };
                let bound = match target.as_var() {
                    Some(s) if s.as_str() == sym::WILDCARD => None,
                    Some(s) => {
                        let id = self.fresh_var();
                        self.locals.push(Binding {
                            name: s.name.clone(),
                            scopes: s.scopes.clone(),
                            kind: BindKind::Local(id, ty),
                        });
                        Some(id)
                    }
                    None => {
                        self.error(
                            "B0345",
                            "nested patterns are not available in Phase 1",
                            target.span(),
                        );
                        None
                    }
                };
                if is_rest {
                    rest = Some(bound);
                } else {
                    binds.push(bound);
                }
            }
            // `[*rest]` alone matches every list, and so does a bare binder. Nothing else does, so
            // nothing else makes the match irrefutable.
            if binds.is_empty() && rest.is_some() {
                *irrefutable = true;
            }
            covered.insert(Arc::from(if rest.is_some() {
                LIST_NONEMPTY
            } else if binds.is_empty() {
                LIST_EMPTY
            } else {
                LIST_FIXED
            }));
            return Pattern::List { binds, rest };
        }

        if p.is_form(sym::REST) {
            self.error(
                "B0347",
                "`*name` is only meaningful inside a list pattern",
                span,
            );
            return Pattern::Wildcard;
        }

        let Some(head) = p.head_sym().cloned() else {
            self.error("B0342", "unsupported pattern", span);
            return Pattern::Wildcard;
        };
        let Some(Binding {
            kind: BindKind::Ctor(union, variant),
            ..
        }) = self.resolve(&head).cloned()
        else {
            self.error("B0343", format!("`{head}` is not a constructor"), span);
            return Pattern::Wildcard;
        };
        covered.insert(variant.clone());

        let fields = match self.types.get(&union) {
            Some(TyDecl::Union { variants, .. }) => variants
                .iter()
                .find(|v| v.name == variant)
                .map(|v| v.fields.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let field_tys = self.variant_field_types(scrut, &union, &fields);

        let mut binds = Vec::new();
        for (i, arg) in p.args.iter().enumerate() {
            // `Added(id, text)` binds by position; `Added(text=t)` by name.
            let (field_name, target) = if arg.is_form(sym::KW_ARG) && arg.args.len() == 2 {
                (arg.args[0].as_var().map(|s| s.name.clone()), &arg.args[1])
            } else {
                (fields.get(i).map(|(n, _)| n.clone()), arg)
            };
            let Some(field_name) = field_name else {
                self.error("B0344", "cannot tell which field this binds", arg.span());
                continue;
            };
            let Some(s) = target.as_var() else {
                self.error(
                    "B0345",
                    "nested patterns are not available in Phase 1",
                    arg.span(),
                );
                continue;
            };
            let ty = field_tys
                .get(&field_name)
                .cloned()
                .unwrap_or_else(|| self.subst.fresh());
            let id = self.fresh_var();
            self.locals.push(Binding {
                name: s.name.clone(),
                scopes: s.scopes.clone(),
                kind: BindKind::Local(id, ty),
            });
            binds.push((field_name, id));
        }
        Pattern::Ctor { variant, binds }
    }

    /// Instantiate a variant's declared field types against the scrutinee's type arguments.
    fn variant_field_types(
        &mut self,
        scrut: &Ty,
        union: &str,
        fields: &[(Arc<str>, Ty)],
    ) -> BTreeMap<Arc<str>, Ty> {
        // The scrutinee's own arguments: matching `Leaf(v)` against a `Tree[Str]` binds `v: Str`.
        let mut args: Vec<Ty> = Vec::new();
        if let Ty::Con(name, xs) = self.subst.resolve(scrut) {
            if name.as_ref() == union {
                args = xs;
            }
        }
        fields
            .iter()
            .map(|(n, t)| (n.clone(), ty::instantiate_decl(t, &args)))
            .collect()
    }

    fn record_lit(&mut self, n: &Node, expected: Option<&Ty>, span: Span) -> Core {
        // `{}` and `{k: v}` are a *map* when that is what the context wants — `State(todos={})`
        // builds an empty `Map[Id, Todo]`, not a record with no fields.
        if let Some(Ty::Con(name, args)) = expected.map(|t| self.subst.resolve(t)) {
            if name.as_ref() == Ty::MAP && args.len() == 2 {
                let mut pairs = Vec::new();
                for pair in n.args.chunks(2) {
                    if pair.len() != 2 {
                        break;
                    }
                    let kc = self.expr(&pair[0], Some(&args[0]));
                    self.unify(&kc.ty, &args[0], kc.span, "map key");
                    let vc = self.expr(&pair[1], Some(&args[1]));
                    self.unify(&vc.ty, &args[1], vc.span, "map value");
                    pairs.push((kc, vc));
                }
                return Core::new(
                    CoreKind::MapLit(pairs),
                    Ty::map(args[0].clone(), args[1].clone()),
                    span,
                );
            }
        }
        let Some(model) = expected
            .map(|t| self.subst.resolve(t))
            .and_then(|t| t.con_name().map(Arc::<str>::from))
        else {
            self.error("B0346", "cannot tell which model this record builds", span);
            return Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span);
        };
        let mut args: Vec<Node> = Vec::new();
        for pair in n.args.chunks(2) {
            if pair.len() != 2 {
                break;
            }
            let key = pair[0].as_keyword().unwrap_or("?");
            args.push(Node::form(
                sym::KW_ARG,
                vec![Node::sym(key, pair[0].span()), pair[1].clone()],
                pair[1].span(),
            ));
        }
        self.make(&model, None, &args, span)
    }

    fn dot(&mut self, n: &Node, span: Span) -> Core {
        let base = self.expr(&n.args[0], None);
        let Some(name) = n.args[1].as_var().map(|s| s.name.clone()) else {
            self.error("B0347", "expected a field or method name", span);
            return Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span);
        };
        let rest = &n.args[2..];

        // `t.with(done=…)` — functional record update.
        if name.as_ref() == "with" {
            let base_ty = self.subst.resolve(&base.ty);
            let field_tys = self.model_fields(&base_ty);
            let mut fields = Vec::new();
            for a in rest {
                if !a.is_form(sym::KW_ARG) || a.args.len() != 2 {
                    self.error("B0348", "`with` takes named fields", a.span());
                    continue;
                }
                let Some(fname) = a.args[0].as_var().map(|s| s.name.clone()) else {
                    continue;
                };
                let want = field_tys.get(&fname).cloned();
                let value = self.expr(&a.args[1], want.as_ref());
                match want {
                    Some(w) => self.unify(&value.ty, &w, value.span, &format!("field `{fname}`")),
                    None => self.error(
                        "B0349",
                        format!("no field `{fname}` on `{base_ty}`"),
                        a.span(),
                    ),
                }
                fields.push((fname, value));
            }
            let ty = base.ty.clone();
            return Core::new(
                CoreKind::With {
                    base: Box::new(base),
                    fields,
                },
                ty,
                span,
            );
        }

        // A plain field read.
        if rest.is_empty() {
            let base_ty = self.subst.resolve(&base.ty);
            if let Some(ty) = self.model_fields(&base_ty).get(&name).cloned() {
                return Core::new(
                    CoreKind::Field {
                        base: Box::new(base),
                        name,
                    },
                    ty,
                    span,
                );
            }
        }

        // Otherwise it is uniform function-call syntax: `xs.map_list(f)` is `map_list(xs, f)`.
        let mut call_args = vec![n.args[0].clone()];
        call_args.extend(rest.iter().cloned());
        let call = Node::form_sym(
            n.args[1]
                .head_sym()
                .cloned()
                .unwrap_or_else(|| Symbol::new(&name)),
            call_args,
            span,
        );
        if self.resolve(&Symbol::new(&name)).is_some() {
            return self.call(&call, None, span);
        }
        let base_ty = self.subst.resolve(&base.ty);
        self.error(
            "B0350",
            format!("no field or function `{name}` for `{base_ty}`"),
            span,
        );
        Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span)
    }

    fn model_fields(&self, ty: &Ty) -> BTreeMap<Arc<str>, Ty> {
        let Some(name) = ty.con_name() else {
            return BTreeMap::new();
        };
        match self.types.get(name) {
            Some(TyDecl::Model { fields, .. }) => {
                let args: &[Ty] = match ty {
                    Ty::Con(_, args) => args,
                    _ => &[],
                };
                fields
                    .iter()
                    .map(|(n, t)| (n.clone(), ty::instantiate_decl(t, args)))
                    .collect()
            }
            Some(TyDecl::Newtype { inner, .. }) => {
                BTreeMap::from([(Arc::from("value"), inner.clone())])
            }
            _ => BTreeMap::new(),
        }
    }

    fn call(&mut self, n: &Node, expected: Option<&Ty>, span: Span) -> Core {
        let head = n.head_sym().cloned().unwrap_or_else(|| Symbol::new("?"));

        // `(call callee args...)` — a computed callee.
        if head.as_str() == sym::CALL && !n.args.is_empty() {
            let func = self.expr(&n.args[0], None);
            return self.apply_fn(func, &n.args[1..], span);
        }

        match self.resolve(&head).cloned().map(|b| b.kind) {
            Some(BindKind::Prim(p)) => self.prim_call(p, &n.args, expected, span),
            Some(BindKind::TraitMethod(m)) => self.trait_call(&m, &n.args, span),
            Some(BindKind::Ctor(union, variant)) => {
                self.make(&union, Some(&variant), &n.args, span)
            }
            Some(BindKind::Model(model)) => self.make(&model, None, &n.args, span),
            Some(BindKind::Global(name)) => {
                if let Some(specs) = self.dicts.get(&name).cloned() {
                    return self.apply_bounded(&name, &specs, &n.args, expected, span);
                }
                let ty = self
                    .schemes
                    .get(&name)
                    .map(|sc| self.subst.instantiate(sc))
                    .unwrap_or_else(|| self.subst.fresh());
                let func = Core::new(CoreKind::Global(name), ty, span);
                self.apply_fn(func, &n.args, span)
            }
            Some(BindKind::Local(id, ty)) => {
                let func = Core::new(CoreKind::Var(id), ty, span);
                self.apply_fn(func, &n.args, span)
            }
            None => {
                self.error("B0340", format!("cannot find `{head}` in this scope"), span);
                Core::new(CoreKind::Const(Const::Unit), self.subst.fresh(), span)
            }
        }
    }

    /// [`Checker::apply_fn`] where one argument has already been checked.
    ///
    /// A trait call has to type its receiver *before* it can tell which function is being called,
    /// so by the time there is a callee that argument is a `Core` and not a `Node`. Re-checking it
    /// would report anything wrong with it twice.
    fn apply_fn_with(
        &mut self,
        func: Core,
        done: Core,
        at: usize,
        args: &[Node],
        span: Span,
    ) -> Core {
        let ftype = self.subst.resolve(&func.ty);
        let Ty::Fun(param_tys, ret, latent) = ftype else {
            return self.apply_fn(func, args, span);
        };
        self.perform(&latent);
        if args.len() != param_tys.len() {
            self.error(
                "B0351",
                format!(
                    "expected {} argument(s), got {}",
                    param_tys.len(),
                    args.len()
                ),
                span,
            );
        }
        if let Some(want) = param_tys.get(at) {
            self.unify(&done.ty, want, done.span, "receiver");
        }
        let mut checked = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            if i == at {
                checked.push(done.clone());
                continue;
            }
            let one = self.check_args(std::slice::from_ref(a), &param_tys[i..]);
            checked.extend(one);
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

    fn apply_fn(&mut self, func: Core, args: &[Node], span: Span) -> Core {
        let ftype = self.subst.resolve(&func.ty);
        let (param_tys, ret, latent) = match &ftype {
            Ty::Fun(ps, r, row) => (ps.clone(), (**r).clone(), row.clone()),
            _ => {
                let ps: Vec<Ty> = args.iter().map(|_| self.subst.fresh()).collect();
                let r = self.subst.fresh();
                let row = self.subst.fresh_row();
                self.unify(
                    &func.ty,
                    &Ty::fun_eff(ps.clone(), r.clone(), row.clone()),
                    span,
                    "callee",
                );
                (ps, r, row)
            }
        };
        // §3.2's inference, in one line: applying a function performs its row.
        self.perform(&latent);
        if args.len() != param_tys.len() {
            self.error(
                "B0351",
                format!(
                    "expected {} argument(s), got {}",
                    param_tys.len(),
                    args.len()
                ),
                span,
            );
        }
        let checked = self.check_args(args, &param_tys);
        Core::new(
            CoreKind::App {
                func: Box::new(func),
                args: checked,
            },
            ret,
            span,
        )
    }

    fn check_args(&mut self, args: &[Node], param_tys: &[Ty]) -> Vec<Core> {
        args.iter()
            .enumerate()
            .map(|(i, a)| {
                let a = if a.is_form(sym::KW_ARG) && a.args.len() == 2 {
                    &a.args[1]
                } else {
                    a
                };
                let want = param_tys.get(i).cloned();
                let c = self.expr(a, want.as_ref());
                if let Some(w) = want {
                    self.unify(&c.ty, &w, c.span, "argument");
                }
                c
            })
            .collect()
    }

    fn prim_call(&mut self, p: Prim, args: &[Node], _expected: Option<&Ty>, span: Span) -> Core {
        let (_, scheme) = self.prims.get(p.name()).cloned().expect("prim registered");
        let ty = self.subst.instantiate(&scheme);
        let Ty::Fun(param_tys, ret, latent) = ty else {
            self.error("B0352", format!("`{}` is not callable", p.name()), span);
            return Core::new(CoreKind::Const(Const::Unit), Ty::unit(), span);
        };
        if args.len() != param_tys.len() {
            self.error(
                "B0351",
                format!(
                    "`{}` takes {} argument(s), got {}",
                    p.name(),
                    param_tys.len(),
                    args.len()
                ),
                span,
            );
        }

        // §3.7's determinism rule: "the checker therefore rejects `now()`, `rand()`, `uuid()` and
        // any I/O **inside a fold** — time is data on the envelope".
        if matches!(p, Prim::NewUuid | Prim::Now) && self.in_fold {
            self.diags.push(
                Diagnostic::error(
                    "B0360",
                    format!("`{}()` cannot be called inside a fold", p.name()),
                    span,
                )
                .with_primary_label("this would make replay non-deterministic")
                .with_note(
                    "a fold must be replay-pure: time is data on the envelope (`env.at`), and \
                     entity ids are minted at the edge",
                )
                .with_fix("mint the id in the client's command and read it from the event"),
            );
        }

        let was_in_fold = self.in_fold;
        if p == Prim::Fold {
            self.in_fold = true;
        }
        let checked = self.check_args(args, &param_tys);
        self.in_fold = was_in_fold;
        // Charged *after* the arguments, so that a row variable in the scheme (`map_list`'s `e`)
        // has already absorbed whatever the function argument does.
        self.perform(&latent);
        if p == Prim::HttpFetch {
            self.outbound_host(checked.first(), span);
        }
        if let Some(core) = short_circuit(p, &checked, (*ret).clone(), span) {
            return core;
        }

        Core::new(
            CoreKind::Prim {
                op: p,
                args: checked,
            },
            *ret,
            span,
        )
    }

    /// Charge `net.out(host)` for an `http_fetch`, from the host written at the call site.
    ///
    /// This is the second place the compiler reads an *argument* to decide a row — `raise` is the
    /// first — and the reason is not symmetry. §6.5 derives the egress NetworkPolicy from the
    /// program's `net.out` atoms and nothing else, so a host that arrived in a variable would be a
    /// call the cluster could not be told about. Requiring a literal is what makes the derivation
    /// total: every outbound call in the program names its peer, and the policy is the list.
    ///
    /// What a program does instead of computing a host is compute everything *else* — the path,
    /// the port, the body, the headers — or write one call site per host. A wrapper is still
    /// possible in the direction that matters: a higher-order helper takes a closure, and the
    /// closure names the host, so its row carries the atom out (§3.2's `e`).
    fn outbound_host(&mut self, arg: Option<&Core>, span: Span) {
        let (Some(arg), Some(host)) = (arg, arg.and_then(crate::core::literal_str)) else {
            let at = arg.map(|a| a.span).unwrap_or(span);
            self.diags.push(
                Diagnostic::error(
                    "B0395",
                    "the host of an outbound call has to be written at the call site".to_string(),
                    at,
                )
                .with_primary_label("this is computed, so nothing knows which host it reaches")
                .with_note(
                    "`http_fetch` performs `net.out(host)`, and the cluster's egress policy is \
                     that atom (§6.5). A host that is not written here is a call the deployment \
                     cannot be told about",
                )
                .with_fix(
                    "write the host as a literal and compute the path instead — or take a \
                     closure, so the caller names its own host and the row carries it out",
                ),
            );
            return;
        };
        // `origin` is the client's own server, and the client's channel to it is the socket the
        // runtime already owns. Allowing it here would put an outbound call on the tier that has
        // no way to make one.
        if host.as_ref() == "origin" {
            self.diags.push(
                Diagnostic::error(
                    "B0396",
                    "`origin` is not a host `http_fetch` can call".to_string(),
                    arg.span,
                )
                .with_primary_label("this names the program's own origin")
                .with_note(
                    "`net.out(origin)` is the one outbound atom a client tier discharges, and a \
                     client reaches its server over the command channel rather than by fetching",
                )
                .with_fix("send a command, or name the service's own host"),
            );
            return;
        }
        if !crate::net::is_nameable_host(&host) {
            self.diags.push(
                Diagnostic::error(
                    "B0396",
                    format!("`{host}` is not a host `http_fetch` can call"),
                    arg.span,
                )
                .with_primary_label("this is not a name a `uses net.out(…)` clause could write")
                .with_note(
                    "the host is a DNS name — ASCII labels separated by dots — because it becomes \
                     a NetworkPolicy peer. A scheme, a port or a path is not part of it",
                )
                .with_fix(
                    "give the host alone; the port is a field of the request and the path \
                     is its own argument",
                ),
            );
            return;
        }
        self.perform(&Row::of([Effect::NetOut(host)]));
    }

    /// Build a union variant or a model record from positional or named arguments.
    fn make(&mut self, ty_name: &str, variant: Option<&str>, args: &[Node], span: Span) -> Core {
        let decl = self.types.get(ty_name).cloned();
        let (declared, arity): (Vec<(Arc<str>, Ty)>, usize) = match (&decl, variant) {
            (Some(TyDecl::Union { variants, .. }), Some(v)) => {
                match variants.iter().find(|x| x.name.as_ref() == v) {
                    Some(found) => (found.fields.clone(), found.fields.len()),
                    None => {
                        self.error("B0353", format!("no variant `{v}` on `{ty_name}`"), span);
                        (Vec::new(), 0)
                    }
                }
            }
            (Some(TyDecl::Model { fields, .. }), _) => (fields.clone(), fields.len()),
            (Some(TyDecl::Newtype { inner, .. }), _) => {
                (vec![(Arc::from("value"), inner.clone())], 1)
            }
            _ => {
                self.error("B0354", format!("cannot construct `{ty_name}`"), span);
                (Vec::new(), 0)
            }
        };

        // Fresh type arguments for each declared parameter, so `Some(1)` is `Option[Int]`.
        //
        // The arity comes from the declaration, not from this one variant: `Err` mentions only
        // `Result`'s second parameter, and reading the arity off it would build a
        // `Result[Rejection]` that then fails to unify with `Result[list[Event], Rejection]`.
        let param_count = decl.as_ref().map(|d| d.arity()).unwrap_or(0);
        let ty_args: Vec<Ty> = (0..param_count).map(|_| self.subst.fresh()).collect();

        if args.len() != arity {
            self.error(
                "B0351",
                format!(
                    "`{}` takes {arity} field(s), got {}",
                    variant.unwrap_or(ty_name),
                    args.len()
                ),
                span,
            );
        }

        let mut fields = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let (fname, value_node) = if a.is_form(sym::KW_ARG) && a.args.len() == 2 {
                (a.args[0].as_var().map(|s| s.name.clone()), &a.args[1])
            } else {
                (declared.get(i).map(|(n, _)| n.clone()), a)
            };
            let Some(fname) = fname else {
                self.error("B0344", "cannot tell which field this sets", a.span());
                continue;
            };
            let want = declared
                .iter()
                .find(|(n, _)| *n == fname)
                .map(|(_, t)| ty::instantiate_decl(t, &ty_args));
            let value = self.expr(value_node, want.as_ref());
            match want {
                Some(w) => self.unify(&value.ty, &w, value.span, &format!("field `{fname}`")),
                None => self.error(
                    "B0349",
                    format!("no field `{fname}` on `{}`", variant.unwrap_or(ty_name)),
                    a.span(),
                ),
            }
            fields.push((fname, value));
        }

        Core::new(
            CoreKind::Make {
                ty: Arc::from(ty_name),
                variant: variant.map(Arc::from),
                fields,
            },
            Ty::Con(Arc::from(ty_name), ty_args),
            span,
        )
    }
}

/// A dotted or applied node, back as the text someone wrote — `net.out(api.example.com)`.
fn written_form(n: &Node) -> Option<String> {
    if let Some(s) = n.as_var() {
        return Some(s.as_str().to_string());
    }
    if let Some(s) = n.as_str_lit() {
        return Some(s.to_string());
    }
    if n.is_form(sym::DOT) && n.args.len() >= 2 {
        let base = written_form(&n.args[0])?;
        let field = n.args[1].as_var()?.as_str().to_string();
        let rest = &n.args[2..];
        if rest.is_empty() {
            return Some(format!("{base}.{field}"));
        }
        let args: Vec<String> = rest.iter().filter_map(written_form).collect();
        return Some(format!("{base}.{field}({})", args.join(", ")));
    }
    let head = n.head_name()?;
    if n.args.is_empty() {
        return Some(head.to_string());
    }
    let args: Vec<String> = n.args.iter().filter_map(written_form).collect();
    Some(format!("{head}({})", args.join(", ")))
}

/// Every `Core` a clause holds, for the resolution pass.
fn clause_cores_mut(c: &mut crate::testing::Clause) -> Vec<&mut Core> {
    use crate::testing::{Clause, Count, Expectation};
    match c {
        Clause::Given { events, .. } => vec![events],
        Clause::When { commands, .. } => commands.iter_mut().collect(),
        Clause::Stub { value, .. } => vec![value],
        Clause::Expect { what, .. } => match what {
            Expectation::Holds(e) => vec![e],
            Expectation::PageContains { needle, .. } => vec![needle],
            Expectation::FoldEquals { events, .. } => vec![events],
            Expectation::Performed {
                how: Count::With(e),
                ..
            } => vec![e],
            _ => Vec::new(),
        },
    }
}

/// Parameter names for a builtin type constructor, which has an arity and no declaration.
///
/// `a`, `b`, … — the names the generated reference already renders `list[a]` and `Map[a, b]` with,
/// so a suggestion and the reference agree about what to call the thing in the brackets.
fn letters(n: usize) -> Vec<Arc<str>> {
    (0..n)
        .map(|i| Arc::from(((b'a' + i as u8) as char).to_string().as_str()))
        .collect()
}

/// Walk a `Core` tree applying the final substitution to every recorded type.
/// The name a `raises(...)` atom carries, for the type of a raised value.
///
/// A declared type, and not a builtin: `raise 4` would give a handler nothing to say it catches,
/// and `raises(Int)` would make every integer failure in a program the same failure. A `list[E]`
/// is refused for the same reason — the atom names a constructor, so `list` would be the name and
/// two unrelated lists would collide.
fn error_ty_name(t: &Ty) -> Option<Arc<str>> {
    match t {
        Ty::Con(name, args) if args.is_empty() => match name.as_ref() {
            Ty::INT | Ty::FLOAT | Ty::BOOL | Ty::STR | Ty::UNIT => None,
            _ => Some(name.clone()),
        },
        _ => None,
    }
}

fn resolve_types(c: &mut Core, s: &Subst) {
    c.ty = s.resolve(&c.ty);
    match &mut c.kind {
        CoreKind::Lam { body, .. } => resolve_types(std::sync::Arc::make_mut(body), s),
        CoreKind::App { func, args } => {
            resolve_types(func, s);
            for a in args {
                resolve_types(a, s);
            }
        }
        CoreKind::Prim { args, .. } => {
            for a in args {
                resolve_types(a, s);
            }
        }
        CoreKind::Let { value, body, .. } => {
            resolve_types(value, s);
            resolve_types(body, s);
        }
        CoreKind::If { cond, then, alt } => {
            resolve_types(cond, s);
            resolve_types(then, s);
            resolve_types(alt, s);
        }
        CoreKind::Match { scrutinee, arms } => {
            resolve_types(scrutinee, s);
            for a in arms {
                resolve_types(&mut a.body, s);
            }
        }
        CoreKind::Make { fields, .. } | CoreKind::With { fields, .. } => {
            for (_, f) in fields {
                resolve_types(f, s);
            }
        }
        CoreKind::Field { base, .. } => resolve_types(base, s),
        CoreKind::ListLit(xs) => {
            for x in xs {
                resolve_types(x, s);
            }
        }
        CoreKind::MapLit(kvs) => {
            for (k, v) in kvs {
                resolve_types(k, s);
                resolve_types(v, s);
            }
        }
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
    }
    if let CoreKind::With { base, .. } = &mut c.kind {
        resolve_types(base, s);
    }
}

/// The three shapes a list pattern can have, as keys in the `covered` set the exhaustiveness check
/// reads. Not type names: they never escape this module.
const LIST_EMPTY: &str = "[]";
const LIST_FIXED: &str = "[…]";
const LIST_NONEMPTY: &str = "[…, *rest]";

/// Every row variable written into a type, in no particular order.
fn row_vars_of(t: &Ty, out: &mut Vec<RowVarId>) {
    match t {
        Ty::Var(_) => {}
        Ty::Con(_, args) => {
            for a in args {
                row_vars_of(a, out);
            }
        }
        Ty::Fun(ps, r, row) => {
            for p in ps {
                row_vars_of(p, out);
            }
            row_vars_of(r, out);
            out.extend(row.tails.iter().copied());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::check_str;
    use crate::ty::Effect;

    /// The row inferred for a definition, as printed atom names.
    fn row_of(src: &str, name: &str) -> Vec<String> {
        let (program, d, map) = check_str("t.beck", src);
        assert!(
            !d.iter()
                .any(|x| x.code.starts_with("B03") && x.code != "B0370"),
            "{}",
            d.render(&map)
        );
        program
            .defs
            .get(name)
            .unwrap_or_else(|| panic!("no `{name}` in {:?}", program.defs.keys()))
            .effects
            .iter()
            .map(|e| e.name())
            .collect()
    }

    fn codes(src: &str) -> Vec<&'static str> {
        let (_, d, _) = check_str("t.beck", src);
        d.iter().map(|x| x.code).collect()
    }

    #[test]
    fn an_effect_reached_through_an_undeclared_function_is_still_inferred() {
        // This is the case Phase 1 could not see. Its collection consulted each global's
        // *declared* effects, so an intermediate that declared nothing hid everything behind it —
        // and `mint` declares nothing, because inference is the point of not having to.
        let src = "\
def mint() -> Str:
    return uuid()

def label(prefix: Str) -> Str:
    return prefix + mint()
";
        assert_eq!(row_of(src, "mint"), ["nondet"]);
        assert_eq!(
            row_of(src, "label"),
            ["nondet"],
            "an effect must travel as far as the calls do"
        );
    }

    #[test]
    fn referencing_a_function_performs_nothing_but_applying_it_performs_everything() {
        // The distinction that makes inference different from collection — and the reason
        // `fold(apply_event, …)` is a pure expression even when `apply_event` is not.
        let src = "\
def mint() -> Str:
    return uuid()

def names() -> list[Str]:
    return map_list([\"a\"], lambda x: x)

def held() -> (Str) -> Str:
    return lambda x: x + mint()

def used() -> Str:
    return mint()
";
        assert!(row_of(src, "names").is_empty());
        assert!(
            row_of(src, "held").is_empty(),
            "returning a function that would mint an id mints nothing"
        );
        assert_eq!(row_of(src, "used"), ["nondet"]);
    }

    #[test]
    fn effect_polymorphism_carries_a_lambdas_row_through_map_list() {
        // §3.2's `map : (list[a], (a -> b ! e)) -> list[b] ! e`, from the caller's side: mapping an
        // effectful function is effectful, and mapping a pure one is not — with one `map_list`.
        let src = "\
def pure_labels(xs: list[Str]) -> list[Str]:
    return map_list(xs, lambda x: x + \"!\")

def minted_labels(xs: list[Str]) -> list[Str]:
    return map_list(xs, lambda x: x + uuid())
";
        assert!(row_of(src, "pure_labels").is_empty());
        assert_eq!(row_of(src, "minted_labels"), ["nondet"]);
    }

    /// §3.2's `map : (list[a], (a -> b ! e)) -> list[b] ! e`, for a definition a *user* wrote.
    ///
    /// A parameter's row is quantified in the definition's scheme, so each call site instantiates
    /// its own: a caller that passes a pure function is pure whatever another caller passes.
    /// `docs/33` §33.2 has why a shared variable was both sound and wrong.
    #[test]
    fn a_user_higher_order_function_is_polymorphic_over_its_arguments_row() {
        let src = "\
def apply(f: (Str) -> Str, x: Str) -> Str:
    return f(x)

def pure_use() -> Str:
    return apply(lambda s: s, \"a\")

def impure_use() -> Str:
    return apply(lambda s: s + uuid(), \"b\")
";
        assert!(
            row_of(src, "apply").is_empty(),
            "`apply` performs nothing of its own: {:?}",
            row_of(src, "apply")
        );
        assert!(
            row_of(src, "pure_use").is_empty(),
            "and a pure caller stays pure however another caller uses it: {:?}",
            row_of(src, "pure_use")
        );
        assert_eq!(
            row_of(src, "impure_use"),
            ["nondet"],
            "while the effectful caller is charged for exactly what it passed"
        );
    }

    #[test]
    fn a_generalised_row_is_still_charged_to_whoever_supplies_it() {
        // The other direction, which is the one a mistake here would break silently: the effect has
        // to arrive *somewhere*. `render` is a view, so `nondet` reaching it is a placement error
        // rather than a lost effect — and the caller is where it lands.
        let src = "\
def twice(f: (Int) -> Int, n: Int) -> Int:
    return f(f(n))

def stamped(n: Int) -> Int:
    return twice(lambda m: m + now(), n)

def plain(n: Int) -> Int:
    return twice(lambda m: m + 1, n)
";
        assert!(row_of(src, "twice").is_empty());
        assert_eq!(row_of(src, "stamped"), ["nondet"]);
        assert!(row_of(src, "plain").is_empty());
    }

    #[test]
    fn a_quantified_row_is_charged_even_when_the_body_never_calls_the_argument() {
        // The over-approximation docs/33 §33.3 names, asserted so that it is a decision rather than
        // a surprise. `ignore` never calls `f`, and a caller passing an effectful one is charged
        // anyway — because the row is quantified from the *signature*, before any body is read.
        //
        // It is the safe direction (an effect too many forces a stricter placement; an effect too
        // few would let a fold read a clock), and it is the same rule `uses` already followed: "a
        // declared effect is part of the signature whether or not the body reaches it".
        let src = "\
def ignore(xs: list[Int], f: (Int) -> Int) -> Int:
    return list_len(xs)

def caller(xs: list[Int]) -> Int:
    return ignore(xs, lambda n: now())
";
        assert!(row_of(src, "ignore").is_empty());
        assert_eq!(row_of(src, "caller"), ["nondet"]);
    }

    #[test]
    fn a_definition_that_returns_a_function_keeps_the_older_monomorphic_row() {
        // The limit docs/33 §33.3 names, asserted rather than described. A row variable that also
        // reaches the *return* type is not quantified, because `instantiate` renames syntactic
        // occurrences and the return's row is bound to the parameter's through the substitution —
        // so one side of the call would be renamed and the other would not.
        let src = "\
def hold(f: (Int) -> Int) -> (Int) -> Int:
    return f

def use_pure(n: Int) -> Int:
    return hold(lambda m: m + 1)(n)

def use_impure(n: Int) -> Int:
    return hold(lambda m: m + now())(n)
";
        assert_eq!(
            row_of(src, "use_pure"),
            ["nondet"],
            "still contaminated, and this is the test that will start failing when it is not"
        );
    }

    #[test]
    fn mutual_recursion_needs_no_ordering() {
        // `even` calls `odd` calls `even`. A row bound to a row that mentions it resolves to the
        // least fixed point rather than diverging, which is why no dependency sort is needed.
        let src = "\
def ping(n: Int) -> Str:
    if n < 1:
        return uuid()
    return pong(n - 1)

def pong(n: Int) -> Str:
    return ping(n - 1)
";
        assert_eq!(row_of(src, "ping"), ["nondet"]);
        assert_eq!(row_of(src, "pong"), ["nondet"]);
    }

    #[test]
    fn a_declared_row_is_a_bound_and_exceeding_it_is_an_error() {
        // §3.6: "effect widening is a breaking API change". So the compiler will not widen it.
        let src = "\
def charge(amount: Int) -> Str uses net.out(payments.example.com):
    return uuid()
";
        assert!(codes(src).contains(&"B0370"), "{:?}", codes(src));

        // …and declaring it is enough to make it compile.
        let ok = "\
def charge(amount: Int) -> Str uses net.out(payments.example.com), nondet:
    return uuid()
";
        assert!(!codes(ok).contains(&"B0370"), "{:?}", codes(ok));
        let (program, _, _) = check_str("t.beck", ok);
        let row = &program.defs["charge"].row;
        assert!(row
            .atoms
            .contains(&Effect::NetOut("payments.example.com".into())));
        assert!(row.atoms.contains(&Effect::Nondet));
    }

    #[test]
    fn an_outbound_call_performs_the_host_it_names() {
        // The row is *inferred* from the argument — the `uses` clause below is the bound §3.6
        // makes it, and the atom in it came from the string on the line above.
        let src = "\
def fetch_rate() -> Str uses net.out(rates.example.com), raises(HttpError):
    r = http_fetch(\"rates.example.com\", HttpRequest(method=\"GET\", path=\"/usd\", headers={}, body=\"\", port=80, secrets={}))
    return r.body
";
        assert_eq!(
            row_of(src, "fetch_rate"),
            ["net.out(rates.example.com)", "raises(HttpError)"]
        );
    }

    #[test]
    fn an_outbound_call_to_a_host_it_cannot_name_is_refused() {
        let req =
            "HttpRequest(method=\"GET\", path=\"/\", headers={}, body=\"\", port=80, secrets={})";
        // Computed: nothing downstream could write the NetworkPolicy peer.
        let computed = format!(
            "def go(host: Str) -> Str uses net.out(x.example.com), raises(HttpError):\n    \
             return http_fetch(host, {req}).body\n"
        );
        assert!(
            codes(&computed).contains(&"B0395"),
            "{:?}",
            codes(&computed)
        );

        // A URL is not a host, and neither is a host with a port on it.
        for bad in ["https://x.example.com", "x.example.com:8080", "origin"] {
            let src = format!(
                "def go() -> Str uses net.out(x.example.com), raises(HttpError):\n    \
                 return http_fetch(\"{bad}\", {req}).body\n"
            );
            assert!(codes(&src).contains(&"B0396"), "{bad}: {:?}", codes(&src));
        }
    }

    #[test]
    fn a_declared_effect_survives_an_empty_body() {
        // A stub that will phone home later must say so today: otherwise the edit that fills the
        // body in silently re-places every caller.
        let src = "\
def charge(amount: Int) -> Str uses net.out(payments.example.com):
    return \"receipt\"
";
        assert_eq!(row_of(src, "charge"), ["net.out(payments.example.com)"]);
    }

    #[test]
    fn ambient_effects_are_carried_but_never_printed_in_a_signature() {
        let src = "\
def audit(what: Str) -> Str uses log:
    return what
";
        let (program, _, _) = check_str("t.beck", src);
        let def = &program.defs["audit"];
        assert_eq!(def.effects, vec![Effect::Ambient(crate::ty::Ambient::Log)]);
        assert!(
            def.row.visible().is_empty(),
            "§3.2 elides the ambient set from signatures"
        );
    }

    // --------------------------------------------------------------- parameterised declarations

    const TREE: &str = "\
union Tree[T]:
    Leaf(value: T)
    Node(kids: list[Tree[T]])

def count[T](t: Tree[T]) -> Int:
    match t:
        case Leaf(value):
            return 1
        case Node(kids):
            return list_len(kids)
";

    #[test]
    fn a_declaration_may_take_a_type_parameter_and_mention_itself_under_one() {
        assert_eq!(codes(TREE), Vec::<&str>::new());
    }

    #[test]
    fn a_parameterised_declaration_is_a_different_type_at_each_argument() {
        // The point of the whole feature, and the thing a compiler that ignored the arguments
        // would still compile: `Tree[Int]` and `Tree[Str]` do not unify.
        let src = format!(
            "{TREE}
def ints() -> Tree[Int]:
    return Leaf(value=1)

def strs() -> Tree[Str]:
    return ints()
"
        );
        assert!(codes(&src).contains(&"B0320"), "{:?}", codes(&src));
    }

    #[test]
    fn a_pattern_binds_the_argument_the_scrutinee_carries() {
        // `case Leaf(value)` over a `Tree[Str]` binds a `Str`, not the declaration's parameter.
        let ok = format!(
            "{TREE}
def first(t: Tree[Str]) -> Str:
    match t:
        case Leaf(value):
            return value
        case Node(kids):
            return \"\"
"
        );
        assert_eq!(codes(&ok), Vec::<&str>::new());

        let bad = ok.replace("return value", "return value + 1");
        assert!(!codes(&bad).is_empty(), "a `Str` is not an `Int`");
    }

    #[test]
    fn a_mention_carries_one_argument_per_declared_parameter() {
        for (src, why) in [
            ("union Box[T]:\n    Held(value: T)\n\ndef f(b: Box) -> Int:\n    return 1\n", "none"),
            (
                "union Box[T]:\n    Held(value: T)\n\ndef f(b: Box[Int, Str]) -> Int:\n    return 1\n",
                "two",
            ),
        ] {
            assert!(codes(src).contains(&"B0311"), "{why}: {:?}", codes(src));
        }
    }

    /// And the spelling it suggests is a program.
    ///
    /// The label offered `Set[_]`, and `_` is not a type — so the fix a reader copied out was
    /// itself refused, by `B0310`. The declaration wrote a name down; that is the one to hand
    /// back, and an `impl` head is where it matters most because it binds its own.
    #[test]
    fn the_spelling_a_missing_type_argument_suggests_is_one_that_compiles() {
        let bare = "\
type Set[T] = newtype[Map[T, Bool]]

trait Sized:
    def size(self) -> Int

impl Sized for Set:
    def size(self):
        return map_len(self.value)
";
        let (_, d, map) = check_str("t.beck", bare);
        let text = d.render(&map);
        assert!(text.contains("write `Set[T]`"), "{text}");
        assert!(
            !text.contains("Set[_]"),
            "there is no wildcard type:\n{text}"
        );

        let fixed = bare.replace("impl Sized for Set:", "impl[T] Sized for Set[T]:");
        assert_eq!(
            codes(&fixed),
            Vec::<&str>::new(),
            "the suggestion has to check clean"
        );
    }

    #[test]
    fn a_parameter_a_declaration_never_mentions_is_still_a_parameter() {
        // Arity is declared, not inferred from the fields that happen to use it — so a phantom
        // parameter still distinguishes `Tag[Int]` from `Tag[Str]`, and still has to be written.
        let src = "\
model Tag[T]:
    label: Str

def a() -> Tag[Int]:
    return Tag(label=\"a\")

def b() -> Tag[Str]:
    return a()
";
        assert!(codes(src).contains(&"B0320"), "{:?}", codes(src));
        let bare = src.replace("Tag[Int]", "Tag");
        assert!(codes(&bare).contains(&"B0311"), "{:?}", codes(&bare));
    }

    #[test]
    fn a_type_parameter_may_not_shadow_a_type_or_repeat_itself() {
        let shadow = "model Note:\n    text: Str\n\nmodel Box[Note]:\n    held: Note\n";
        assert!(codes(shadow).contains(&"B0314"), "{:?}", codes(shadow));

        let repeat = "model Pair[T, T]:\n    a: T\n    b: T\n";
        assert!(codes(repeat).contains(&"B0315"), "{:?}", codes(repeat));
    }

    #[test]
    fn a_parameterised_alias_is_expanded_and_applied() {
        // An alias names no type of its own, so `Pairs[Int]` has to *be* `list[Map[Int, Int]]` by
        // the time anything else sees it — including a mismatch report.
        let src = "\
type Pairs[T] = list[Map[T, T]]

def f(xs: Pairs[Int]) -> Int:
    return list_len(xs)

def g(xs: Pairs[Str]) -> Int:
    return f(xs)
";
        let (_, d, map) = check_str("t.beck", src);
        let text = d.render(&map);
        assert!(text.contains("B0320"), "{text}");
        assert!(
            !text.contains("Pairs"),
            "an alias is transparent, so nothing downstream should still be talking about it:\n{text}"
        );
    }

    #[test]
    fn a_definitions_parameter_and_a_declarations_parameter_do_not_meet() {
        // A `def`'s `T` is rigid and a declaration's is positional. The same letter in both is two
        // different things, and the body of the `def` may not assume they are the same.
        let src = "\
union Box[T]:
    Held(value: T)

def unwrap[T](b: Box[T]) -> T:
    match b:
        case Held(value):
            return value
";
        assert_eq!(codes(src), Vec::<&str>::new());

        let bad = src.replace("-> T:", "-> Int:");
        assert!(!codes(&bad).is_empty(), "a `T` is not an `Int`");
    }
}

/// The checker's half of the front end's recursion bound.
///
/// The reader's bound does not cover this one. A macro expands into a tree nobody typed, and the
/// checker is the first pass to walk it — so it counts for itself, against the same ceiling.
#[cfg(test)]
mod nesting_tests {
    use crate::check_str;
    use beck_diag::depth::{MAX_NESTING, STACK_BYTES};

    /// A type nested `n` deep — `list[list[…Int…]]` — which is the checker's *other* recursion.
    fn nested_type(n: usize) -> String {
        let mut ty = String::from("Int");
        for _ in 0..n {
            ty = format!("list[{ty}]");
        }
        format!("def f(x: {ty}) -> Int:\n    return 1\n")
    }

    fn nested_expr(n: usize) -> String {
        format!(
            "def f() -> Int:\n    return {}1{}\n",
            "(".repeat(n),
            ")".repeat(n)
        )
    }

    fn codes(src: &str) -> Vec<String> {
        beck_diag::depth::on_the_front_end_stack(|| {
            let (_, d, _) = check_str("deep.beck", src);
            d.iter().map(|x| x.code.to_string()).collect()
        })
    }

    #[test]
    fn a_type_past_the_ceiling_is_a_diagnostic_rather_than_an_abort() {
        let found = codes(&nested_type(MAX_NESTING as usize + 8));
        assert!(
            found.contains(&"B0390".to_string()),
            "expected the checker's own refusal, got {found:?}"
        );
    }

    #[test]
    fn an_expression_past_the_ceiling_is_refused_by_whichever_pass_reaches_it_first() {
        // The reader gets there first for an expression, which is the point of bounding all three:
        // whichever pass is handed the deep tree is the one that refuses it.
        let found = codes(&nested_expr(MAX_NESTING as usize + 8));
        assert!(
            found.iter().any(|c| c == "B0121" || c == "B0390"),
            "expected a nesting refusal, got {found:?}"
        );
    }

    #[test]
    fn nesting_a_person_would_write_still_checks() {
        assert!(codes(&nested_type(16)).is_empty());
    }

    #[test]
    fn the_ceiling_fits_the_declared_stack() {
        const PROBE_DEPTH: usize = 100;
        let spent = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(|| {
                let src = nested_type(PROBE_DEPTH);
                beck_diag::depth::probe::stack_spent(|| check_str("probe.beck", &src))
            })
            .expect("a thread")
            .join()
            .expect("the probe checks");

        let per_level = spent / PROBE_DEPTH;
        println!("checker: {spent} bytes for {PROBE_DEPTH} levels ({per_level} per level)");
        let needed = MAX_NESTING as usize * per_level * 2;
        assert!(
            needed < STACK_BYTES,
            "a ceiling of {MAX_NESTING} levels at {per_level} bytes each needs {needed} bytes \
             with the margin, against a declared STACK_BYTES of {STACK_BYTES} — raise the \
             declaration or lower the ceiling"
        );
    }
}

/// `a and b` and `a or b`, lowered to the conditional they mean.
///
/// Beck's `and` and `or` were primitives over two `Bool`s, so both operands were evaluated before
/// either operator was applied. That is a difference nobody could see for three phases — every use
/// in the tree is pure, total and cheap — and [`53`](../../../../../docs/53-are-we-fast-yet-report.md)
/// §53.5 is where it became visible: a benchmark written the way its original is written searched
/// from positions its guard had already rejected.
///
/// The lowering happens **here** rather than in the evaluator, and that is the load-bearing choice.
/// Short-circuiting is a property of the language, not of one backend; put it in `interp.rs` and
/// the second backend has to remember it, which is the class of bug the backend seam exists to
/// prevent (`docs/19` §19.9). `CoreKind::If` already means "pick which computation runs", so no IR
/// node, no evaluator case and no runtime change was needed — the third feature running to be
/// added without one.
///
/// The *effect row* is deliberately unchanged. Both operands may run, so both are charged, exactly
/// as for any other `if`. What changes is how often they run, not what they are allowed to do.
///
/// A **bare reference** — `and` passed somewhere as a value — is untouched and still strict, and
/// that is not an oversight: a function value is handed arguments that have already been evaluated,
/// so no function value in any strict language short-circuits. `list_all` is what a fold over
/// conjunction wants.
fn short_circuit(p: Prim, args: &[Core], ty: Ty, span: Span) -> Option<Core> {
    let [lhs, rhs] = args else {
        return None;
    };
    let constant = |b: bool| Core::new(CoreKind::Const(Const::Bool(b)), ty.clone(), span);
    let (then, alt) = match p {
        Prim::And => (rhs.clone(), constant(false)),
        Prim::Or => (constant(true), rhs.clone()),
        _ => return None,
    };
    Some(Core::new(
        CoreKind::If {
            cond: Box::new(lhs.clone()),
            then: Box::new(then),
            alt: Box::new(alt),
        },
        ty,
        span,
    ))
}
