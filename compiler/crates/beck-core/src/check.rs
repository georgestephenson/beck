//! Resolution and typechecking, elaborating straight into `Core`.
//!
//! Stages 4 and 5 of [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md)
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

use beck_diag::{Diagnostic, Diagnostics, Span};
use beck_syntax::{sym, Lit, Node, ScopeSet, Symbol};

use crate::core::{Arm, Const, Core, CoreKind, Pattern, Prim, VarId};
use crate::iface::Interface;
use crate::prelude;
use crate::ty::{Effect, Mismatch, Row, RowVarId, Scheme, Subst, Tier, Ty, TyDecl, Variant};

/// A checked module: everything the placement checker, the splitter and the runtime need.
#[derive(Clone, Debug)]
pub struct Program {
    pub name: String,
    pub types: BTreeMap<Arc<str>, TyDecl>,
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

/// The four types a test's clauses are checked against — see [`Checker::test_subjects`].
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
    /// Types declared in this module, in source order.
    own_types: Vec<Arc<str>>,
    /// The row *variable* standing for each definition's inferred row, minted before any body is
    /// checked so that callers can name it and mutual recursion needs no ordering.
    def_row: BTreeMap<Arc<str>, RowVarId>,
    /// What the body currently being checked has been seen to perform.
    row: Row,
    next_var: VarId,
    /// Set while checking a fold's function, so §3.7's determinism rule can be enforced.
    in_fold: bool,
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
        own_types: Vec::new(),
        def_row: BTreeMap::new(),
        row: Row::empty(),
        next_var: 0,
        in_fold: false,
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

    // Imported names arrive before anything local is collected, so a local definition may shadow
    // one and the diagnostic points at the local.
    for (module_name, iface) in imports {
        let (types, names) = iface.exports();
        for (n, d) in types {
            ck.types.insert(n, d);
        }
        for (n, e) in names {
            ck.schemes.insert(n.clone(), e.scheme);
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
    ck.collect_signatures(&items);
    ck.collect_signal_names(&items);
    let mut program = ck.check_items(&items, name);
    program.imports = imports.iter().map(|(n, _)| n.clone()).collect();
    program
}

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
            // A placeholder, so that `ty_from_node`'s "cannot find type" check passes while the
            // real declaration is still being built. It is never observed: `collect_types`
            // overwrites every one of them, and a name that reaches a later pass unfilled would be
            // a name no declaration produced.
            let placeholder = if item.is_form(sym::MODEL) {
                TyDecl::Model {
                    name: name.clone(),
                    fields: Vec::new(),
                }
            } else if item.is_form(sym::UNION) {
                TyDecl::Union {
                    name: name.clone(),
                    variants: Vec::new(),
                }
            } else if item.is_form(sym::NEWTYPE) {
                TyDecl::Newtype {
                    name: name.clone(),
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
            if !item.is_form(sym::TYPE) || item.args.len() < 2 {
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
            pending.insert(name.clone(), (item.args[1].clone(), item.span()));
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
        let Some((node, span)) = pending.get(name) else {
            return;
        };
        if resolving.contains(name) {
            self.error(
                "B0312",
                format!(
                    "type alias `{name}` is defined in terms of itself — an alias is transparent, \
                     so this describes no type; a `union` may be recursive, an alias may not"
                ),
                *span,
            );
            // Registered as an alias for a fresh variable, so that every *other* mention of it
            // reports nothing further: one cycle is one diagnostic.
            let ty = self.subst.fresh();
            self.types.insert(
                name.clone(),
                TyDecl::Alias {
                    name: name.clone(),
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
        let ty = self.ty_from_node(node);
        self.types.insert(
            name.clone(),
            TyDecl::Alias {
                name: name.clone(),
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
            let decl = if item.is_form(sym::MODEL) {
                let fields = item.args[1..]
                    .iter()
                    .filter_map(|f| self.field_decl(f))
                    .collect();
                TyDecl::Model {
                    name: name.clone(),
                    fields,
                }
            } else if item.is_form(sym::UNION) {
                let variants = item.args[1..]
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
                    variants,
                }
            } else if item.is_form(sym::NEWTYPE) {
                TyDecl::Newtype {
                    name: name.clone(),
                    inner: self.ty_from_node(&item.args[1]),
                }
            } else {
                // `type` aliases were resolved by `collect_aliases`, and everything else is not a
                // type declaration.
                continue;
            };
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
                TyDecl::Union { name, variants } => {
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
            if !item.is_form(sym::DEF) || item.args.len() < 4 {
                continue;
            }
            let Some(name) = item.args[0].as_var().map(|s| s.name.clone()) else {
                continue;
            };
            let params: Vec<Ty> = item.args[1]
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
            let ret = match item.args[2].args.first() {
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
            let declared = self.declared_row(item.args.get(3));

            // The definition's latent row is a *variable*, bound once its body has been checked.
            // Minting it here is what lets any definition call any other in any order.
            let rv = self.subst.fresh_row_var();
            self.schemes.insert(
                name.clone(),
                Scheme::mono(Ty::fun_eff(params, ret, Row::var(rv))),
            );
            self.def_row.insert(name.clone(), rv);
            self.declared.insert(name.clone(), declared);
            self.globals.push(Binding {
                name: name.clone(),
                scopes: ScopeSet::empty(),
                kind: BindKind::Global(name.clone()),
            });
        }
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
            match Effect::parse(&text) {
                Some(atom) => row.add(atom),
                None => self.error(
                    "B0305",
                    format!(
                        "`{}` is not an effect",
                        if text.is_empty() { "?" } else { &text }
                    ),
                    e.span(),
                ),
            }
        }
        row
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
            {
                // Declarations; already collected, or (traits/impls) parsed and carried but not
                // yet given semantics — see the Phase 1 report.
                if inner.is_form(sym::TRAIT) || inner.is_form(sym::IMPL) {
                    self.diags.push(
                        Diagnostic::warning(
                            "B0306",
                            "traits are parsed but not yet checked",
                            inner.span(),
                        )
                        .with_note(
                            "Phase 2 built the effect system this was once expected to arrive \
                             with, and trait resolution did not come with it: it is still \
                             unimplemented, and this warning is the only thing standing between a \
                             `trait` and silence",
                        ),
                    );
                }
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
            if def.declared_effects.is_empty() {
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

        Program {
            name,
            types: self.types,
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
    fn test_subjects(
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
        // `docs/23-general-slicer-report.md` §23.4. The checker has to know which before a signal
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

    fn check_test(
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
                        Clause::When {
                            actor: stmt.args[0].as_str_lit().map(Arc::from),
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
                        Clause::Expect {
                            what: Expectation::PageContains {
                                needle,
                                actor: stmt.args.get(1).and_then(|a| a.as_str_lit()).map(Arc::from),
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
                        let tier = ck.test_tier(&stmt.args[1])?;
                        Clause::Expect {
                            what: Expectation::Place { what, tier },
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
            .filter(|e| !e.is_ambient())
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

        let before = self.locals.len();
        let mut params = Vec::new();
        for (p, ty) in item.args[1].args.iter().zip(&param_tys) {
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

        let body_node = item.args.get(4);
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

        let declared = self.declared.get(&name).cloned().unwrap_or_default();
        // A declared effect is part of the signature whether or not the body reaches it: a stub
        // that will phone home later must say so today, or its callers would be re-placed by the
        // edit that fills the body in.
        let inferred = performed.union(&declared);
        if let Some(rv) = self.def_row.get(&name).copied() {
            self.subst.bind_row(rv, inferred.clone());
        }

        let lam = Core {
            kind: CoreKind::Lam {
                params: params.iter().map(|(id, _, _)| *id).collect(),
                body: Box::new(body),
            },
            ty: Ty::Fun(param_tys, ret.clone(), latent),
            tier,
            span,
        };

        let mut declared_effects: Vec<Effect> = declared.atoms.iter().cloned().collect();
        declared_effects.sort();
        Some(Def {
            name,
            params,
            ret: *ret,
            body: lam,
            tier,
            effects: Vec::new(),
            row: inferred,
            declared_effects,
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
        let span = n.span();
        if n.has_head("fn-type") && n.args.len() >= 2 {
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
        let args: Vec<Ty> = n.args.iter().map(|a| self.ty_from_node(a)).collect();

        // Aliases are transparent; newtypes are not — that is what "ids of different entities must
        // not be interchangeable" (§3.1) means.
        if let Some(TyDecl::Alias { ty, .. }) = self.types.get(name) {
            let ty = ty.clone();
            if !args.is_empty() {
                self.error("B0309", format!("`{name}` takes no type arguments"), span);
            }
            return ty;
        }

        let known = prelude::builtin_arity(name);
        if known.is_none() && !self.types.contains_key(name) {
            self.error("B0310", format!("cannot find type `{name}`"), span);
            return self.subst.fresh();
        }
        if let Some(arity) = known {
            if args.len() != arity {
                self.error(
                    "B0311",
                    format!(
                        "`{name}` takes {arity} type argument(s), got {}",
                        args.len()
                    ),
                    span,
                );
            }
        }
        Ty::Con(Arc::from(name), args)
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
            "+" if n.args.len() == 2 => {
                // Ad-hoc, bidirectional: `+` is Int addition unless one side is already known to
                // be a Str, in which case it concatenates. Phase 1 has no numeric type class, and
                // a `(a, a) -> a` scheme would let `Bool + Bool` typecheck.
                let lhs = self.expr(&n.args[0], None);
                let rhs = self.expr(&n.args[1], None);
                let is_str = self.subst.resolve(&lhs.ty).con_name() == Some(Ty::STR)
                    || self.subst.resolve(&rhs.ty).con_name() == Some(Ty::STR)
                    || expected
                        .map(|t| t.con_name() == Some(Ty::STR))
                        .unwrap_or(false);
                let want = if is_str { Ty::str_() } else { Ty::int() };
                self.unify(&lhs.ty, &want, lhs.span, "operand of `+`");
                self.unify(&rhs.ty, &want, rhs.span, "operand of `+`");
                Core::new(
                    CoreKind::Prim {
                        op: Prim::Add,
                        args: vec![lhs, rhs],
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

    fn var_ref(&mut self, s: &Symbol, span: Span) -> Core {
        let Some(b) = self.resolve(s).cloned() else {
            self.error("B0340", format!("cannot find `{s}` in this scope"), span);
            let t = self.subst.fresh();
            return Core::new(CoreKind::Const(Const::Unit), t, span);
        };
        match b.kind {
            BindKind::Local(id, ty) => Core::new(CoreKind::Var(id), ty, span),
            BindKind::Global(name) => {
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
                        body: Box::new(Core::new(
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
                body: Box::new(body),
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
        let mut mapping: BTreeMap<u32, Ty> = BTreeMap::new();
        if let Ty::Con(name, args) = self.subst.resolve(scrut) {
            if name.as_ref() == union {
                // Declared type parameters appear as the prelude's `A`, `B` variables in order.
                for (i, a) in args.iter().enumerate() {
                    mapping.insert(1_000_000 + i as u32, a.clone());
                }
            }
        }
        fields
            .iter()
            .map(|(n, t)| (n.clone(), substitute(t, &mapping)))
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

    /// How many type parameters a declaration has, counted over every field of every variant.
    fn decl_arity(&self, decl: &Option<TyDecl>) -> usize {
        let fields: Vec<&Ty> = match decl {
            Some(TyDecl::Union { variants, .. }) => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|(_, t)| t))
                .collect(),
            Some(TyDecl::Model { fields, .. }) => fields.iter().map(|(_, t)| t).collect(),
            Some(TyDecl::Newtype { inner, .. }) => vec![inner],
            _ => Vec::new(),
        };
        fields
            .into_iter()
            .filter_map(max_scheme_var)
            .max()
            .map(|m| (m - 1_000_000 + 1) as usize)
            .unwrap_or(0)
    }

    fn model_fields(&self, ty: &Ty) -> BTreeMap<Arc<str>, Ty> {
        let Some(name) = ty.con_name() else {
            return BTreeMap::new();
        };
        match self.types.get(name) {
            Some(TyDecl::Model { fields, .. }) => {
                let mut mapping = BTreeMap::new();
                if let Ty::Con(_, args) = ty {
                    for (i, a) in args.iter().enumerate() {
                        mapping.insert(1_000_000 + i as u32, a.clone());
                    }
                }
                fields
                    .iter()
                    .map(|(n, t)| (n.clone(), substitute(t, &mapping)))
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
            Some(BindKind::Ctor(union, variant)) => {
                self.make(&union, Some(&variant), &n.args, span)
            }
            Some(BindKind::Model(model)) => self.make(&model, None, &n.args, span),
            Some(BindKind::Global(name)) => {
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

        Core::new(
            CoreKind::Prim {
                op: p,
                args: checked,
            },
            *ret,
            span,
        )
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
        // The arity comes from the whole declaration, not from this one variant: `Err` mentions
        // only `Result`'s second parameter, and reading the arity off it would build a
        // `Result[Rejection]` that then fails to unify with `Result[list[Event], Rejection]`.
        let param_count = self.decl_arity(&decl);
        let ty_args: Vec<Ty> = (0..param_count).map(|_| self.subst.fresh()).collect();
        let mapping: BTreeMap<u32, Ty> = ty_args
            .iter()
            .enumerate()
            .map(|(i, t)| (1_000_000 + i as u32, t.clone()))
            .collect();

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
                .map(|(_, t)| substitute(t, &mapping));
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

fn substitute(t: &Ty, m: &BTreeMap<u32, Ty>) -> Ty {
    match t {
        Ty::Var(v) => m.get(v).cloned().unwrap_or(Ty::Var(*v)),
        Ty::Con(n, args) => Ty::Con(n.clone(), args.iter().map(|a| substitute(a, m)).collect()),
        Ty::Fun(ps, r, row) => Ty::Fun(
            ps.iter().map(|p| substitute(p, m)).collect(),
            Box::new(substitute(r, m)),
            row.clone(),
        ),
    }
}

fn max_scheme_var(t: &Ty) -> Option<u32> {
    match t {
        Ty::Var(v) if *v >= 1_000_000 => Some(*v),
        Ty::Var(_) => None,
        Ty::Con(_, args) => args.iter().filter_map(max_scheme_var).max(),
        Ty::Fun(ps, r, _) => ps
            .iter()
            .filter_map(max_scheme_var)
            .chain(max_scheme_var(r))
            .max(),
    }
}

/// Walk a `Core` tree applying the final substitution to every recorded type.
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

fn resolve_types(c: &mut Core, s: &Subst) {
    c.ty = s.resolve(&c.ty);
    match &mut c.kind {
        CoreKind::Lam { body, .. } => resolve_types(body, s),
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

    #[test]
    fn a_user_higher_order_function_is_polymorphic_enough_for_two_call_sites() {
        // The parameter's row is monomorphic within the module and *subsumes* rather than equates,
        // so a pure argument at one call and an effectful one at another both check — and `apply`
        // ends up with the union, which is the sound direction.
        let src = "\
def apply(f: (Str) -> Str, x: Str) -> Str:
    return f(x)

def pure_use() -> Str:
    return apply(lambda s: s, \"a\")

def impure_use() -> Str:
    return apply(lambda s: s + uuid(), \"b\")
";
        assert_eq!(row_of(src, "apply"), ["nondet"]);
        assert_eq!(row_of(src, "pure_use"), ["nondet"]);
        assert_eq!(row_of(src, "impure_use"), ["nondet"]);
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
}
