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
//! Not checked, and named in the Phase 1 report rather than implied: row polymorphism on records,
//! inferred effect rows (§3.2 — effects are declared and collected, not inferred), trait
//! constraints on type variables, and `var` mutability (a `var` binding is checked as an ordinary
//! immutable one).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics, Span};
use beck_syntax::{sym, Lit, Node, ScopeSet, Symbol};

use crate::core::{Arm, Const, Core, CoreKind, Pattern, Prim, VarId};
use crate::prelude;
use crate::ty::{Effect, Mismatch, Scheme, Subst, Tier, Ty, TyDecl, Variant};

/// A checked module: everything the placement checker, the splitter and the runtime need.
#[derive(Clone, Debug)]
pub struct Program {
    pub name: String,
    pub types: BTreeMap<Arc<str>, TyDecl>,
    pub defs: BTreeMap<Arc<str>, Def>,
    /// Source order, so diagnostics and `beck explain` are stable.
    pub def_order: Vec<Arc<str>>,
    pub signals: Vec<SignalDecl>,
    pub tests: Vec<TestDef>,
}

#[derive(Clone, Debug)]
pub struct Def {
    pub name: Arc<str>,
    pub params: Vec<(VarId, Arc<str>, Ty)>,
    pub ret: Ty,
    /// The whole definition as a lambda, so evaluating the name yields a callable value.
    pub body: Core,
    pub tier: Tier,
    /// Effects the signature declares with `uses`, unioned with those collected from the body.
    pub effects: Vec<Effect>,
    pub declared_effects: Vec<Effect>,
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
    pub span: Span,
    pub tier_span: Span,
}

#[derive(Clone, Debug)]
pub struct TestDef {
    pub name: Arc<str>,
    pub body: Core,
    pub span: Span,
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
    def_effects: BTreeMap<Arc<str>, Vec<Effect>>,
    next_var: VarId,
    /// Set while checking a fold's function, so §3.7's determinism rule can be enforced.
    in_fold: bool,
}

/// Check a module that macro expansion has already run over.
pub fn check_module(module: &Node, diags: &mut Diagnostics) -> Program {
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
        def_effects: BTreeMap::new(),
        next_var: 0,
        in_fold: false,
    };
    for (name, prim, scheme) in prelude::prims() {
        ck.prims.insert(Arc::from(name), (prim, scheme));
        ck.globals.push(Binding {
            name: Arc::from(name),
            scopes: ScopeSet::empty(),
            kind: BindKind::Prim(prim),
        });
    }

    let items: Vec<&Node> = module.args.iter().skip(1).collect();
    ck.collect_types(&items);
    ck.register_type_constructors();
    ck.collect_signatures(&items);
    ck.collect_signal_names(&items);
    ck.check_items(&items, name)
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

    // ------------------------------------------------------------------ declarations

    /// Strip `@on(...)` decorators, returning the inner item and the tier it names.
    fn undecorate<'n>(&mut self, item: &'n Node) -> (&'n Node, Option<(Tier, Span)>) {
        let mut inner = item;
        let mut tier = None;
        while inner.is_form(sym::DECORATE) && inner.args.len() == 2 {
            let deco = &inner.args[0];
            if deco.has_head(sym::ON) && deco.args.len() == 1 {
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
        (inner, tier)
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
            } else if item.is_form(sym::TYPE) {
                TyDecl::Alias {
                    name: name.clone(),
                    ty: self.ty_from_node(&item.args[1]),
                }
            } else {
                continue;
            };
            if self.types.insert(name.clone(), decl).is_some() {
                self.error(
                    "B0302",
                    format!("type `{name}` is declared twice"),
                    item.span(),
                );
            }
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
            let declared: Vec<Effect> = item
                .args
                .get(3)
                .map(|u| {
                    u.args
                        .iter()
                        .filter_map(|e| {
                            let text = e.head_name()?;
                            match Effect::parse(text) {
                                Some(eff) => Some(eff),
                                None => {
                                    self.error(
                                        "B0305",
                                        format!("`{text}` is not an effect"),
                                        e.span(),
                                    );
                                    None
                                }
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            self.schemes
                .insert(name.clone(), Scheme::mono(Ty::Fun(params, Box::new(ret))));
            self.def_effects.insert(name.clone(), declared);
            self.globals.push(Binding {
                name: name.clone(),
                scopes: ScopeSet::empty(),
                kind: BindKind::Global(name.clone()),
            });
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
        let mut defs = BTreeMap::new();
        let mut def_order = Vec::new();
        let mut signals = Vec::new();
        let mut tests = Vec::new();

        for item in items {
            let (inner, tier) = self.undecorate(item);
            let (tier, tier_span) = tier.unwrap_or((Tier::Any, inner.span()));

            if inner.is_form(sym::DEF) {
                if let Some(def) = self.check_def(inner, tier, tier_span) {
                    def_order.push(def.name.clone());
                    defs.insert(def.name.clone(), def);
                }
            } else if inner.is_form(sym::LET) || inner.is_form(sym::VAR) {
                if let Some(s) = self.check_signal(inner, tier, tier_span) {
                    signals.push(s);
                }
            } else if inner.is_form(sym::TEST) && inner.args.len() == 2 {
                let tname: Arc<str> = inner.args[0]
                    .as_str_lit()
                    .map(Arc::from)
                    .unwrap_or_else(|| Arc::from("test"));
                let before = self.locals.len();
                let body = self.block(&inner.args[1].args, None);
                self.locals.truncate(before);
                tests.push(TestDef {
                    name: tname,
                    body,
                    span: inner.span(),
                });
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
                        .with_note("trait resolution arrives with Phase 2's effect work"),
                    );
                }
            } else {
                self.error("B0307", "unsupported top-level item", inner.span());
            }
        }

        // Resolve every recorded type through the substitution so that what leaves the checker is
        // ground wherever inference succeeded.
        for def in defs.values_mut() {
            def.ret = self.subst.resolve(&def.ret);
            for p in &mut def.params {
                p.2 = self.subst.resolve(&p.2);
            }
            resolve_types(&mut def.body, &self.subst);
        }
        for s in &mut signals {
            s.ty = self.subst.resolve(&s.ty);
            resolve_types(&mut s.expr, &self.subst);
        }

        Program {
            name,
            types: self.types,
            defs,
            def_order,
            signals,
            tests,
        }
    }

    fn check_def(&mut self, item: &Node, tier: Tier, tier_span: Span) -> Option<Def> {
        let name = item.args[0].as_var()?.name.clone();
        let scheme = self.schemes.get(&name)?.clone();
        let Ty::Fun(param_tys, ret) = scheme.ty.clone() else {
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
        let body = match body_node {
            Some(b) => self.block(&b.args, Some(&ret)),
            None => Core::new(CoreKind::Const(Const::Unit), Ty::unit(), item.span()),
        };
        self.unify(&body.ty, &ret, body.span, "return type");
        self.locals.truncate(before);

        let declared = self.def_effects.get(&name).cloned().unwrap_or_default();
        let mut collected = declared.clone();
        let def_effects = self.def_effects.clone();
        body.effects(
            &|n| def_effects.get(n).cloned().unwrap_or_default(),
            &mut collected,
        );
        collected.sort_unstable();
        collected.dedup();

        let span = item.span();
        let lam = Core {
            kind: CoreKind::Lam {
                params: params.iter().map(|(id, _, _)| *id).collect(),
                body: Box::new(body),
            },
            ty: Ty::Fun(param_tys, ret.clone()),
            tier,
            span,
        };

        Some(Def {
            name,
            params,
            ret: *ret,
            body: lam,
            tier,
            effects: collected,
            declared_effects: declared,
            span,
            tier_span,
        })
    }

    fn check_signal(&mut self, item: &Node, tier: Tier, tier_span: Span) -> Option<SignalDecl> {
        let target = &item.args[0];
        let (name_node, annot) = if target.is_form(sym::ANNOT) && target.args.len() == 2 {
            (&target.args[0], Some(&target.args[1]))
        } else {
            (target, None)
        };
        let name = name_node.as_var()?.name.clone();
        let expected = annot.map(|t| self.ty_from_node(t));

        let expr = self.expr(&item.args[1], expected.as_ref());
        if let Some(e) = &expected {
            self.unify(&expr.ty, e, expr.span, "declared type");
        }

        let def_effects = self.def_effects.clone();
        let mut effects = Vec::new();
        expr.effects(
            &|n| def_effects.get(n).cloned().unwrap_or_default(),
            &mut effects,
        );
        effects.sort_unstable();
        effects.dedup();

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
            effects,
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
            return Ty::Fun(params, Box::new(ret));
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

    fn unify(&mut self, actual: &Ty, expected: &Ty, span: Span, what: &str) {
        if let Err(e) = self.subst.unify(actual, expected) {
            let msg = match e {
                Mismatch::Different(a, b) => {
                    format!("{what} mismatch: expected `{b}`, found `{a}`")
                }
                Mismatch::Arity(a, b) => {
                    format!("{what} takes {b} argument(s), got {a}")
                }
                Mismatch::Infinite => format!("{what} would be an infinite type"),
            };
            self.error("B0320", msg, span);
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
            self.unify(&alt.ty, &then.ty, then.span, "the two branches");
            let ty = then.ty.clone();
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
                self.unify(&alt.ty, &then.ty, alt.span, "the two branches");
                let ty = then.ty.clone();
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
                let Ty::Fun(params, ret) = ty.clone() else {
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
                    Ty::Fun(params, ret),
                    span,
                )
            }
            BindKind::Ctor(union, variant) => self.make(&union, Some(&variant), &[], span),
            BindKind::Model(model) => self.make(&model, None, &[], span),
        }
    }

    fn lambda(&mut self, n: &Node, expected: Option<&Ty>, span: Span) -> Core {
        let want: Option<(Vec<Ty>, Ty)> = expected.and_then(|t| match self.subst.resolve(t) {
            Ty::Fun(ps, r) => Some((ps, *r)),
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
        let body = self.body_expr(&n.args[1], ret_want.as_ref());
        self.locals.truncate(before);
        let ret = body.ty.clone();
        Core::new(
            CoreKind::Lam {
                params: ids,
                body: Box::new(body),
            },
            Ty::Fun(tys, Box::new(ret)),
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
        let (param_tys, ret) = match &ftype {
            Ty::Fun(ps, r) => (ps.clone(), (**r).clone()),
            _ => {
                let ps: Vec<Ty> = args.iter().map(|_| self.subst.fresh()).collect();
                let r = self.subst.fresh();
                self.unify(
                    &func.ty,
                    &Ty::Fun(ps.clone(), Box::new(r.clone())),
                    span,
                    "callee",
                );
                (ps, r)
            }
        };
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
        let Ty::Fun(param_tys, ret) = ty else {
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
        if p == Prim::NewUuid && self.in_fold {
            self.diags.push(
                Diagnostic::error("B0360", "`uuid()` cannot be called inside a fold", span)
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

fn substitute(t: &Ty, m: &BTreeMap<u32, Ty>) -> Ty {
    match t {
        Ty::Var(v) => m.get(v).cloned().unwrap_or(Ty::Var(*v)),
        Ty::Con(n, args) => Ty::Con(n.clone(), args.iter().map(|a| substitute(a, m)).collect()),
        Ty::Fun(ps, r) => Ty::Fun(
            ps.iter().map(|p| substitute(p, m)).collect(),
            Box::new(substitute(r, m)),
        ),
    }
}

fn max_scheme_var(t: &Ty) -> Option<u32> {
    match t {
        Ty::Var(v) if *v >= 1_000_000 => Some(*v),
        Ty::Var(_) => None,
        Ty::Con(_, args) => args.iter().filter_map(max_scheme_var).max(),
        Ty::Fun(ps, r) => ps
            .iter()
            .filter_map(max_scheme_var)
            .chain(max_scheme_var(r))
            .max(),
    }
}

/// Walk a `Core` tree applying the final substitution to every recorded type.
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
