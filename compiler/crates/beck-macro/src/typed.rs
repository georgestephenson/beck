//! Typed macros: the half of §2.4 that wants the checker's answers.
//!
//! An ordinary `macro` runs before anything has been inferred, which is why `derive_json` is
//! handed a **declaration** rather than an expression — a model's fields are in its syntax, so no
//! type information is needed and none can be had. A `typed macro` is the other case: it is called
//! with expressions, and what it wants to know is what those expressions *are*.
//!
//! So a typed macro is expanded by the **checker**, at the call site, once the arguments have been
//! inferred. The body is the same language an untyped macro body is ([`crate::interp`]), with one
//! name added: `node_ty(e)` answers with the type the checker gave `e`, as a value the body can
//! ask questions of.
//!
//! # What a body sees
//!
//! [`TyRepr`] is the value, and it is reached through the ordinary record notation rather than
//! through a family of builtins:
//!
//! | Written | Answers |
//! |---|---|
//! | `t.name` | `"Int"`, `"list"`, `"Todo"` — the head, with no arguments |
//! | `t.kind` | `"builtin"`, `"model"`, `"union"`, `"newtype"`, `"fn"`, `"param"`, `"unknown"` |
//! | `t.args` | `list[Int]` answers `[Int]`; a function answers its parameter types |
//! | `t.result` | a function's result type |
//! | `t.fields` | a model's fields, as `{name, ty}` records, with the type's own arguments substituted in |
//! | `t.variants` | a union's variants, as `{name, fields}` records |
//! | `t.inner` | what a `newtype` wraps |
//!
//! `fields`, `variants` and `inner` are read **on access** rather than carried in the value, and
//! that is not an optimisation: `model Tree: left: Tree` is a type whose fields mention itself, so
//! a value holding its own fields eagerly would not be a finite value. A *type expression* is
//! always finite; only looking into a declaration recurses, and a body that recurses without a base
//! case is stopped by the same nesting bound every other compile-time call is.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use beck_diag::depth::Nesting;
use beck_diag::{Diagnostics, Span};
use beck_syntax::Node;

use crate::{declares_a_macro, declares_a_typed_macro, Expander};

/// What a declaration is, for a macro asking what it may look into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TyKind {
    /// The language's own: `Int`, `Str`, `list`, `Map`, `Option`, and everything else in the
    /// prelude. A macro may read its arguments and not its insides.
    Builtin,
    Model,
    Union,
    Newtype,
}

impl TyKind {
    pub fn name(self) -> &'static str {
        match self {
            TyKind::Builtin => "builtin",
            TyKind::Model => "model",
            TyKind::Union => "union",
            TyKind::Newtype => "newtype",
        }
    }
}

/// The type of an expression, as a macro body sees it.
///
/// A projection of the checker's `Ty` rather than the thing itself: this crate must not depend on
/// the type checker, which depends on it. What is dropped is what a macro has no use for — a
/// function's effect row, and the identity of a unification variable.
#[derive(Clone, Debug, PartialEq)]
pub enum TyRepr {
    /// A named type applied to arguments: `Int`, `list[Str]`, `Todo`.
    Con {
        name: Arc<str>,
        kind: TyKind,
        args: Vec<TyRepr>,
    },
    /// A declaration's own type parameter, seen while looking into that declaration — the `T` of
    /// `model Box[T]`. Substituted away by [`TypeEnv::fields`] when the type being looked into
    /// carries arguments, so a body meets one only where the type it asked about was generic.
    Param { name: Arc<str>, index: usize },
    Fun {
        params: Vec<TyRepr>,
        result: Box<TyRepr>,
    },
    /// Inference had no answer here — an argument whose type is still open, or one whose own
    /// checking failed.
    Unknown,
}

impl TyRepr {
    pub fn kind_name(&self) -> &'static str {
        match self {
            TyRepr::Con { kind, .. } => kind.name(),
            TyRepr::Param { .. } => "param",
            TyRepr::Fun { .. } => "fn",
            TyRepr::Unknown => "unknown",
        }
    }

    /// The head, with no arguments — what a body matches on.
    pub fn head(&self) -> Arc<str> {
        match self {
            TyRepr::Con { name, .. } | TyRepr::Param { name, .. } => name.clone(),
            TyRepr::Fun { .. } => Arc::from("->"),
            TyRepr::Unknown => Arc::from("?"),
        }
    }

    pub fn args(&self) -> Vec<TyRepr> {
        match self {
            TyRepr::Con { args, .. } => args.clone(),
            TyRepr::Fun { params, .. } => params.clone(),
            _ => Vec::new(),
        }
    }

    /// This type with a declaration's parameters replaced by the arguments a mention of it carried.
    fn substitute(&self, args: &[TyRepr]) -> TyRepr {
        match self {
            TyRepr::Param { index, .. } => args.get(*index).cloned().unwrap_or(TyRepr::Unknown),
            TyRepr::Con {
                name,
                kind,
                args: inner,
            } => TyRepr::Con {
                name: name.clone(),
                kind: *kind,
                args: inner.iter().map(|a| a.substitute(args)).collect(),
            },
            TyRepr::Fun { params, result } => TyRepr::Fun {
                params: params.iter().map(|p| p.substitute(args)).collect(),
                result: Box::new(result.substitute(args)),
            },
            TyRepr::Unknown => TyRepr::Unknown,
        }
    }
}

impl fmt::Display for TyRepr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TyRepr::Con { name, args, .. } if args.is_empty() => write!(f, "{name}"),
            TyRepr::Con { name, args, .. } => {
                write!(f, "{name}[")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, "]")
            }
            TyRepr::Param { name, .. } => write!(f, "{name}"),
            TyRepr::Fun { params, result } => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {result}")
            }
            TyRepr::Unknown => write!(f, "?"),
        }
    }
}

/// A declaration's fields, or one variant's: a name and the type written beside it.
pub type Fields = Vec<(Arc<str>, TyRepr)>;

/// A union's variants: a name and what that variant holds.
pub type Variants = Vec<(Arc<str>, Fields)>;

/// What one `model`, `union` or `newtype` declaration holds.
///
/// Field types are written with the declaration's parameters in place ([`TyRepr::Param`]);
/// [`TypeEnv::fields`] is what puts a mention's arguments back in.
#[derive(Clone, Debug, Default)]
pub struct DeclInfo {
    pub fields: Fields,
    pub variants: Variants,
    pub inner: Option<TyRepr>,
}

/// What the checker inferred, for the one call being expanded.
///
/// Keyed by [`Span`], which is what a `Node` carries and therefore the only identity an argument
/// has once it has been substituted into a template. Where several nodes share one position —
/// code an earlier macro expansion generated borrows the call site's — the **outermost** wins,
/// because the checker records a parent after its children.
#[derive(Clone, Debug, Default)]
pub struct TypeEnv {
    nodes: HashMap<Span, TyRepr>,
    decls: HashMap<Arc<str>, DeclInfo>,
}

impl TypeEnv {
    pub fn new() -> TypeEnv {
        TypeEnv::default()
    }

    /// Forget the last call's expressions, keeping the module's declarations.
    ///
    /// The declarations are the whole module's and are collected once; what a body may ask about
    /// belongs to one call site and must not outlive it.
    pub fn clear_nodes(&mut self) {
        self.nodes.clear();
    }

    pub fn record(&mut self, span: Span, ty: TyRepr) {
        if !span.is_none() {
            self.nodes.insert(span, ty);
        }
    }

    pub fn declare(&mut self, name: Arc<str>, decl: DeclInfo) {
        self.decls.insert(name, decl);
    }

    pub fn of(&self, span: Span) -> Option<&TyRepr> {
        self.nodes.get(&span)
    }

    /// A model's fields, or a newtype's one field, with the mention's arguments substituted in.
    pub fn fields(&self, t: &TyRepr) -> Fields {
        let TyRepr::Con { name, args, .. } = t else {
            return Vec::new();
        };
        let Some(decl) = self.decls.get(name) else {
            return Vec::new();
        };
        decl.fields
            .iter()
            .map(|(n, ft)| (n.clone(), ft.substitute(args)))
            .collect()
    }

    /// A union's variants, each with its own fields, substituted the same way.
    pub fn variants(&self, t: &TyRepr) -> Variants {
        let TyRepr::Con { name, args, .. } = t else {
            return Vec::new();
        };
        let Some(decl) = self.decls.get(name) else {
            return Vec::new();
        };
        decl.variants
            .iter()
            .map(|(n, fs)| {
                let fields = fs
                    .iter()
                    .map(|(fname, ft)| (fname.clone(), ft.substitute(args)))
                    .collect();
                (n.clone(), fields)
            })
            .collect()
    }

    /// What a `newtype` wraps, or [`TyRepr::Unknown`] for anything else.
    pub fn inner(&self, t: &TyRepr) -> TyRepr {
        let TyRepr::Con { name, args, .. } = t else {
            return TyRepr::Unknown;
        };
        match self.decls.get(name).and_then(|d| d.inner.as_ref()) {
            Some(i) => i.substitute(args),
            None => TyRepr::Unknown,
        }
    }
}

/// Where a typed expansion starts minting hygiene scopes: the **even** numbers.
///
/// The two expanders run over one module and must not mint the same scope, or a binding one
/// introduced would be visible to a reference the other introduced — hygiene failing silently,
/// which is the one way it can fail. Parity keeps them apart *by construction*, which is what this
/// wants rather than a bound: [`Expander::fresh_scope`] counts the odd numbers, this counts the
/// even ones, and neither has to know how many the other spent.
const SCOPE_BASE: u32 = 2;

/// The typed macros a module has, and everything their bodies may reach.
///
/// Built where the untyped expansion ends and handed to the checker, which is the only thing that
/// can expand one: a typed macro's body asks what its arguments *are*, and until the checker has
/// run there is no answer.
#[derive(Clone, Debug)]
pub struct TypedExpander {
    /// Whether anything in scope is a typed macro. Held rather than derived, because the checker
    /// asks at every call it walks.
    has_typed: bool,
    macros: HashMap<Arc<str>, crate::MacroDef>,
    defs: HashMap<Arc<str>, crate::interp::FnDef>,
    next_scope: u32,
    steps: u64,
    steps_spent: bool,
    fuel: u64,
    spent: bool,
}

impl Default for TypedExpander {
    fn default() -> TypedExpander {
        TypedExpander {
            has_typed: false,
            macros: HashMap::new(),
            defs: HashMap::new(),
            next_scope: SCOPE_BASE,
            steps: crate::interp::MAX_STEPS,
            steps_spent: false,
            fuel: crate::MAX_EXPANSION,
            spent: false,
        }
    }
}

impl TypedExpander {
    /// The typed macros of a module and of the modules it imports.
    ///
    /// `imported` are **parsed** modules, for the reason [`crate::expand_module_with`] gives: a
    /// macro is published by a module's source, because it has no signature for an interface to
    /// carry.
    ///
    /// Collection reports nothing. Duplicate names are refused where the untyped expander collects
    /// the same declarations one phase earlier, and reporting them again here would be the same
    /// `B0200` twice.
    pub fn collect(module: &Node, imported: &[&Node]) -> TypedExpander {
        // Nothing is copied for a module with no typed macro in scope, which is nearly every
        // module: collecting the compile-time-callable `def`s copies a body each, and this pass
        // would otherwise pay that a second time for every module that has an ordinary macro.
        if !declares_a_typed_macro(module) && !imported.iter().any(|m| declares_a_typed_macro(m)) {
            return TypedExpander::default();
        }
        let mut quiet = Diagnostics::new();
        let mut ex = Expander::collecting(&mut quiet);
        let brings_macros = imported.iter().any(|m| declares_a_macro(m));
        for m in imported {
            ex.collect_macros_from(m, brings_macros);
        }
        ex.collect_macros_from(module, brings_macros);
        TypedExpander {
            has_typed: true,
            macros: std::mem::take(&mut ex.macros),
            defs: std::mem::take(&mut ex.defs),
            ..TypedExpander::default()
        }
    }

    /// Whether this module has any typed macro at all — the question worth asking before the
    /// checker carries a probe around, and the one asked at every call site in the module.
    pub fn is_empty(&self) -> bool {
        !self.has_typed
    }

    /// Whether a module-wide budget has run out, so nothing more will ever expand.
    ///
    /// A budget is spent **once** and reported once, and the checker infers a call's arguments
    /// inside a rollback — so a caller that discards what a probe reported has to ask this, or the
    /// only report there will ever be is the one it just deleted, and every expansion afterwards
    /// quietly produces nothing.
    pub fn exhausted(&self) -> bool {
        self.spent || self.steps_spent
    }

    /// Say again that a budget ran out, for a caller that threw the first report away.
    pub fn report_exhaustion(&self, span: Span, diags: &mut Diagnostics) {
        if self.spent {
            diags.push(crate::too_much(span));
        } else if self.steps_spent {
            diags.push(crate::interp::ran_too_long(span));
        }
    }

    /// Whether `name` is a typed macro, and therefore this expander's to expand.
    pub fn declares(&self, name: &str) -> bool {
        self.has_typed && self.macros.get(name).is_some_and(|d| d.typed)
    }

    /// Every typed macro's name, for a diagnostic that wants to say what one is.
    pub fn names(&self) -> Vec<Arc<str>> {
        let mut out: Vec<Arc<str>> = self
            .macros
            .values()
            .filter(|d| d.typed)
            .map(|d| d.name.clone())
            .collect();
        out.sort();
        out
    }

    /// Expand one call, with what the checker inferred about its arguments.
    ///
    /// The four-part hygiene dance is the untyped expander's, unchanged — a typed macro differs in
    /// *when* it runs and in what its body may ask, not in how a name it introduces is scoped.
    pub fn expand(
        &mut self,
        call: &Node,
        types: &TypeEnv,
        diags: &mut Diagnostics,
    ) -> Option<Node> {
        let name = call.head_name()?.to_string();
        let def = self.macros.get(name.as_str()).filter(|d| d.typed)?.clone();
        let mut ex = Expander {
            macros: std::mem::take(&mut self.macros),
            defs: std::mem::take(&mut self.defs),
            next_scope: self.next_scope,
            steps: self.steps,
            steps_spent: self.steps_spent,
            fuel: self.fuel,
            spent: self.spent,
            nesting: Nesting::new(),
            types: Some(types),
            diags,
        };
        // An untyped macro inside what a typed one produced still expands. The two phases are
        // ordered, not exclusive: a template that writes `unless(…)` means what it says wherever it
        // was written, and by this point nothing else will ever look at that call.
        let out = match ex.apply_macro(&def, call) {
            Some(out) if ex.charge(&out, call.span()) => Some(ex.expand(&out, 1)),
            _ => None,
        };
        self.next_scope = ex.next_scope;
        self.steps = ex.steps;
        self.steps_spent = ex.steps_spent;
        self.fuel = ex.fuel;
        self.spent = ex.spent;
        self.macros = std::mem::take(&mut ex.macros);
        self.defs = std::mem::take(&mut ex.defs);
        out
    }
}
