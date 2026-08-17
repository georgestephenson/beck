//! Recognising the join a loop already contains.
//!
//! [`docs/99-the-data-tier-means-of-combination.md`](../../../../../docs/99-the-data-tier-means-of-combination.md)
//! §99.6:
//!
//! > `for x in xs:` whose body contains `map_get(ys, k(x))` **is** an equi-join […] Recognising the
//! > shape and emitting a `Join` instead of a captured `FlatMap` would make `27-review.beck` and
//! > `examples/board.beck` faster **with no edit to either program**.
//!
//! The cost this removes is not a constant. A per-element function that captured the accumulator is
//! a *different function* on every event, so [`crate::engine`]'s rebuild rule reapplies it to
//! every element — a nested-loop join with no index, re-run from scratch per event.
//! `27-review.beck` is the corpus program that has one, and it did not know it did.
//!
//! # What is recognised, stated as the condition rather than as the shape
//!
//! One `map_get(m, k)` inside the loop's body, where
//!
//! * `m` reads only what the function **captured** — so the collection being looked up in is a node
//!   the plan already has, or can build, rather than something derived per element; and
//! * `k` reads only the **element** — so the join key is a function of the left row alone, which is
//!   what makes it an *equi*-join rather than a predicate.
//!
//! Both conditions are about which variables an expression reads, so both survive the lookup being
//! written behind a call: `27-review`'s is three definitions deep (`verdict_for` → `map_get`), and
//! §99.6 forecast that as the case inference would fail on. It does not, because the body is
//! inlined before it is searched — but the *limit* is real and moved rather than removed, and
//! [`Refusal`] is where it is named.
//!
//! # Why the index is `map_values` and not an `arrange_by`
//!
//! §99.9 item 3 schedules `arrange_by` — a second index over a collection, keyed by something other
//! than its ordering key — before the join. The shape that actually occurs in the tree does not
//! need one: the right side is a `Map` field of the accumulator, and
//! [`crate::plan::Op::MapValues`]'s arrangement is *already* keyed by the map's key, which is the
//! join key. So the index here is an operator that existed, and `arrange_by` lands with the first
//! program whose right side is a list. Building it now would put an operator with a delta rule and
//! no program into the engine, which is the hole [`crate::plan::OPERATORS`] exists to refuse.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::Span;

use crate::check::Def;
use crate::core::{free_vars, Arm, Core, CoreKind, Prim, VarId};
use crate::ty::{Tier, Ty};

/// The two halves of a joined row, as the field names the rewritten body reads them by.
///
/// A record rather than a two-element list because a `Field` is what `Core` already has: no
/// primitive indexes a list, and inventing one for this would put a form in the language whose only
/// caller is a rewrite.
pub const LEFT: &str = "left";
pub const RIGHT: &str = "right";

/// The type name the joined row carries. Nothing checks it — the plan runs after the checker — but
/// a value that prints as `Join(left=…, right=…)` in a panic is worth the four bytes.
pub const ROW: &str = "Join";

/// A loop whose body looked something up, taken apart.
pub struct Recognised {
    /// The collection to index, in the caller's variables: the first argument of the `map_get`.
    pub map: Core,
    /// The join key as a function of the element alone — the `Fun` body of [`crate::plan::Op::Join`].
    pub key: Core,
    /// The element parameter `key` is written over.
    pub elem: VarId,
    /// The loop body, with the lookup replaced by a read of the joined row's right half, over a
    /// fresh element parameter that is the row rather than the left value.
    pub body: Core,
    /// The parameter `body` now takes: one joined row.
    pub row: VarId,
}

/// Why a body that contained a `map_get` was not recognised as a join.
///
/// §99.6's rule for the case inference cannot see: "compile it the slow way and *say so*". These
/// reach [`crate::plan::Node::because`], so `beck explain cost` prints the reason beside the
/// operator that pays for it rather than leaving a reader to guess which of the conditions failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No `map_get` anywhere in the body: this loop relates nothing.
    NoLookup,
    /// More than one, and which to index on is a plan choice nothing here is equipped to make.
    Several(usize),
    /// The collection looked up in is derived per element, so there is nothing to index once.
    MapReadsTheElement,
    /// The key reads something other than the element, so it is not an equi-join on the left row.
    KeyReadsMoreThanTheElement,
    /// Recognising it would not remove the capture that costs the rebuild, so it buys nothing.
    NothingSaved,
}

impl Refusal {
    /// The sentence `beck explain` prints, in the voice the rest of the plan's reasons use.
    pub fn because(&self) -> String {
        match self {
            Refusal::NoLookup => "its body relates nothing to the collection it loops over".into(),
            Refusal::Several(n) => format!(
                "its body looks up in {n} collections, and which one to index is a plan choice \
                 (docs/99 §99.8)"
            ),
            Refusal::MapReadsTheElement => {
                "the collection it looks up in is derived from the element, so there is nothing to \
                 index once"
                    .into()
            }
            Refusal::KeyReadsMoreThanTheElement => {
                "the key it looks up by reads more than the element, so it is not an equi-join on \
                 the left row"
                    .into()
            }
            Refusal::NothingSaved => {
                "rewriting it as a join would not remove what its function captured, so it would \
                 cost an index and save nothing"
                    .into()
            }
        }
    }
}

/// How far a body is inlined before it is searched.
///
/// `27-review`'s lookup is two calls deep (`verdict_for`, then the `map_get` in its body) and the
/// key one more (`payload`). Four is that with room, and it is a *bound* rather than a budget
/// because the thing it stops is a body that grows exponentially in a chain of calls, not a slow
/// compile.
const DEPTH: usize = 4;

/// Try to read a loop's per-element function as a join.
///
/// `f` is the function as written — a `Lam` of one parameter, or a global that resolves to one.
/// `captured` is the set of variables the enclosing plan has operators for, which is what decides
/// whether the collection being looked up in is something the plan can index.
pub fn recognise(
    f: &Core,
    defs: &BTreeMap<Arc<str>, Def>,
    captured: &BTreeSet<VarId>,
) -> Result<Recognised, Refusal> {
    let (elem, body) = match lambda(f, defs) {
        Some(pair) => pair,
        None => return Err(Refusal::NoLookup),
    };
    let mut fresh = 1 + max_var(&body).max(captured.iter().copied().max().unwrap_or(0));
    let body = inline(&body, defs, &mut Vec::new(), &mut fresh, DEPTH);

    // The lookups, deepest-first is not wanted and neither is any order: there must be exactly one,
    // because choosing between two is the plan question §99.8 opens and this is not it.
    let mut sites: Vec<Vec<usize>> = Vec::new();
    lookups(&body, &mut Vec::new(), &mut sites);
    let site = match sites.len() {
        0 => return Err(Refusal::NoLookup),
        1 => sites.remove(0),
        n => return Err(Refusal::Several(n)),
    };

    let (map, key) = {
        let at = follow(&body, &site);
        let CoreKind::Prim { args, .. } = &at.kind else {
            unreachable!("a site is where a `map_get` is")
        };
        (args[0].clone(), args[1].clone())
    };
    // Resolved against the `let`s the inliner left, because an argument that was not cheap enough
    // to substitute is bound rather than copied — so the map may be a variable standing for one.
    let mut lets = BTreeMap::new();
    collect_lets(&body, &site, &mut lets);
    let map = resolve(&map, &lets, DEPTH);
    let key = resolve(&key, &lets, DEPTH);

    if reads(&map).contains(&elem) {
        return Err(Refusal::MapReadsTheElement);
    }
    if !reads(&map).is_subset(captured) {
        return Err(Refusal::MapReadsTheElement);
    }
    if !reads(&key).is_subset(&BTreeSet::from([elem])) {
        return Err(Refusal::KeyReadsMoreThanTheElement);
    }

    // The rewrite: the lookup becomes a read of the row's right half, and the element becomes a
    // read of its left half. Both are `let`s rather than substitutions so the body is written once
    // however many times it mentions the element.
    let row = fresh;
    let answer = fresh + 1;
    let mut rewritten = body;
    let ty = follow(&rewritten, &site).ty.clone();
    *follow_mut(&mut rewritten, &site) = var(answer, ty, Span::NONE);
    let body = bind(
        elem,
        field(row, LEFT, Ty::unit()),
        bind(answer, field(row, RIGHT, Ty::unit()), rewritten),
    );

    Ok(Recognised {
        map,
        key,
        elem,
        body,
        row,
    })
}

// -------------------------------------------------------------------------------------------
// Reading a function
// -------------------------------------------------------------------------------------------

/// A one-parameter function as its parameter and its body, following one level of naming.
fn lambda(f: &Core, defs: &BTreeMap<Arc<str>, Def>) -> Option<(VarId, Core)> {
    match &f.kind {
        CoreKind::Lam { params, body } if params.len() == 1 => Some((params[0], (**body).clone())),
        CoreKind::Global(name) => match &defs.get(name)?.body.kind {
            CoreKind::Lam { params, body } if params.len() == 1 => {
                Some((params[0], (**body).clone()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Every `map_get` in an expression, as the path of child indices that reaches it.
///
/// A path rather than a pointer because the rewrite happens afterwards and has to reach the same
/// node in a `&mut` walk; `Core` is a tree of boxes, so there is no id to hold on to.
fn lookups(c: &Core, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if matches!(
        &c.kind,
        CoreKind::Prim {
            op: Prim::MapGet,
            args
        } if args.len() == 2
    ) {
        out.push(path.clone());
    }
    for (i, child) in children(c).into_iter().enumerate() {
        path.push(i);
        lookups(child, path, out);
        path.pop();
    }
}

/// The `let` bindings that are in scope at a path, so an expression under it can be resolved.
fn collect_lets(c: &Core, path: &[usize], out: &mut BTreeMap<VarId, Core>) {
    let Some((&step, rest)) = path.split_first() else {
        return;
    };
    if let CoreKind::Let { var, value, .. } = &c.kind {
        // Child 1 is the body: a binding is in scope there and not in its own value.
        if step == 1 {
            out.insert(*var, (**value).clone());
        }
    }
    if let Some(child) = children(c).into_iter().nth(step) {
        collect_lets(child, rest, out);
    }
}

/// An expression with its `let`-bound variables expanded, so that what it *reads* is what it
/// really reads rather than what the inliner named.
fn resolve(c: &Core, lets: &BTreeMap<VarId, Core>, depth: usize) -> Core {
    if depth == 0 {
        return c.clone();
    }
    if let CoreKind::Var(v) = &c.kind {
        if let Some(bound) = lets.get(v) {
            return resolve(bound, lets, depth - 1);
        }
        return c.clone();
    }
    let mut out = c.clone();
    for child in children_mut(&mut out) {
        *child = resolve(child, lets, depth);
    }
    out
}

fn reads(c: &Core) -> BTreeSet<VarId> {
    let mut out = BTreeSet::new();
    free_vars(c, &mut BTreeSet::new(), &mut out);
    out
}

// -------------------------------------------------------------------------------------------
// Inlining, so that a lookup written behind a call is still a lookup
// -------------------------------------------------------------------------------------------

/// Inline the calls a search would otherwise have to see through.
///
/// Two rules, and the second is what keeps this from changing what the program means:
///
/// * a callee's body is **α-renamed** above every variable in sight before it is used, so nothing
///   it binds can capture what the caller passed;
/// * an argument is **substituted** only when it is cheap and cannot fail — a variable, a constant,
///   a field path over those — and is otherwise bound with a `let`. Substituting a call would
///   evaluate it once per mention and *not at all* when the parameter is unused, and a view may
///   raise, so "pure" is not on its own enough to make copying an argument free.
fn inline(
    c: &Core,
    defs: &BTreeMap<Arc<str>, Def>,
    stack: &mut Vec<Arc<str>>,
    fresh: &mut VarId,
    depth: usize,
) -> Core {
    if depth == 0 {
        return c.clone();
    }
    let mut out = c.clone();
    for child in children_mut(&mut out) {
        *child = inline(child, defs, stack, fresh, depth);
    }
    let CoreKind::App { func, args } = &out.kind else {
        return out;
    };
    let (params, body, named) = match &func.kind {
        CoreKind::Lam { params, body } => (params.to_vec(), (**body).clone(), None),
        CoreKind::Global(name) if !stack.contains(name) => match defs.get(name) {
            Some(def) => match &def.body.kind {
                CoreKind::Lam { params, body } => {
                    (params.to_vec(), (**body).clone(), Some(name.clone()))
                }
                _ => return out,
            },
            None => return out,
        },
        _ => return out,
    };
    if params.len() != args.len() {
        return out;
    }

    let offset = *fresh;
    let mut body = body;
    let top = max_var(&body);
    rename(&mut body, offset);
    *fresh = offset + top + 1;

    let mut bound = body;
    for (p, arg) in params.iter().zip(args).rev() {
        let p = p + offset;
        if simple(arg) {
            substitute(&mut bound, p, arg);
        } else {
            bound = bind(p, arg.clone(), bound);
        }
    }
    if let Some(name) = named {
        stack.push(name);
        let deeper = inline(&bound, defs, stack, fresh, depth - 1);
        stack.pop();
        return deeper;
    }
    inline(&bound, defs, stack, fresh, depth - 1)
}

/// Whether copying an expression is free: it cannot fail, cannot allocate a call frame, and reading
/// it twice costs what reading it once did.
fn simple(c: &Core) -> bool {
    match &c.kind {
        CoreKind::Var(_) | CoreKind::Const(_) | CoreKind::Global(_) => true,
        CoreKind::Field { base, .. } => simple(base),
        _ => false,
    }
}

/// Shift every variable an expression binds or reads, so a callee's body cannot capture a caller's.
fn rename(c: &mut Core, by: VarId) {
    match &mut c.kind {
        CoreKind::Var(v) => *v += by,
        CoreKind::Lam { params, body } => {
            *params = params.iter().map(|p| p + by).collect();
            let mut inner = (**body).clone();
            rename(&mut inner, by);
            *body = Arc::new(inner);
            return;
        }
        CoreKind::Let { var, .. } => *var += by,
        CoreKind::Match { arms, .. } => {
            for arm in arms.iter_mut() {
                rename_pattern(&mut arm.pattern, by);
            }
        }
        _ => {}
    }
    for child in children_mut(c) {
        rename(child, by);
    }
}

fn rename_pattern(p: &mut crate::core::Pattern, by: VarId) {
    use crate::core::Pattern;
    match p {
        Pattern::Wildcard | Pattern::Const(_) => {}
        Pattern::Bind(v) => *v += by,
        Pattern::At { var, inner } => {
            *var += by;
            rename_pattern(inner, by);
        }
        Pattern::Ctor { binds, .. } => binds.iter_mut().for_each(|(_, p)| rename_pattern(p, by)),
        Pattern::Or(alts) => alts.iter_mut().for_each(|p| rename_pattern(p, by)),
        Pattern::List { items, rest } => {
            items.iter_mut().for_each(|p| rename_pattern(p, by));
            if let Some(Some(v)) = rest {
                *v += by;
            }
        }
    }
}

/// Replace a variable with an expression, stopping wherever the variable is rebound.
///
/// The rebinding check is not defensive: [`rename`] has already made a collision impossible for the
/// callers here, and it is written anyway because a substitution that is wrong about scope is wrong
/// silently and only on a program that shadows.
fn substitute(c: &mut Core, v: VarId, to: &Core) {
    match &mut c.kind {
        CoreKind::Var(x) if *x == v => {
            let ty = c.ty.clone();
            let span = c.span;
            *c = to.clone();
            c.ty = ty;
            c.span = span;
            return;
        }
        CoreKind::Lam { params, body } => {
            if params.contains(&v) {
                return;
            }
            let mut inner = (**body).clone();
            substitute(&mut inner, v, to);
            *body = Arc::new(inner);
            return;
        }
        CoreKind::Let { var, value, body } => {
            substitute(value, v, to);
            if *var != v {
                substitute(body, v, to);
            }
            return;
        }
        CoreKind::Match { scrutinee, arms } => {
            substitute(scrutinee, v, to);
            for arm in arms.iter_mut() {
                if arm.pattern.binders().contains(&v) {
                    continue;
                }
                for e in arm.exprs_mut() {
                    substitute(e, v, to);
                }
            }
            return;
        }
        _ => {}
    }
    for child in children_mut(c) {
        substitute(child, v, to);
    }
}

// -------------------------------------------------------------------------------------------
// Walking a `Core` by position
// -------------------------------------------------------------------------------------------

/// Every subexpression, in the order a path indexes them.
///
/// One function paired with [`children_mut`], and they must agree: a path found by the first is
/// followed by the second, so a kind listed in one and not the other would rewrite the wrong node.
fn children(c: &Core) -> Vec<&Core> {
    match &c.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => Vec::new(),
        // A `Lam`'s body is behind an `Arc`, so it is not reachable as a `&mut` child. Nothing
        // below one is searched for a lookup: a lookup inside a nested function is a lookup per
        // *call* of that function, which is not the shape this recognises.
        CoreKind::Lam { .. } => Vec::new(),
        CoreKind::App { func, args } => {
            let mut out = vec![&**func];
            out.extend(args.iter());
            out
        }
        CoreKind::Prim { args, .. } => args.iter().collect(),
        CoreKind::Let { value, body, .. } => vec![&**value, &**body],
        CoreKind::If { cond, then, alt } => vec![&**cond, &**then, &**alt],
        CoreKind::Match { scrutinee, arms } => {
            let mut out = vec![&**scrutinee];
            out.extend(arms.iter().flat_map(Arm::exprs));
            out
        }
        CoreKind::Make { fields, .. } => fields.iter().map(|(_, v)| v).collect(),
        CoreKind::Field { base, .. } => vec![&**base],
        CoreKind::With { base, fields } => {
            let mut out = vec![&**base];
            out.extend(fields.iter().map(|(_, v)| v));
            out
        }
        CoreKind::ListLit(items) => items.iter().collect(),
        CoreKind::MapLit(pairs) => pairs.iter().flat_map(|(k, v)| [k, v]).collect(),
    }
}

fn children_mut(c: &mut Core) -> Vec<&mut Core> {
    match &mut c.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => Vec::new(),
        CoreKind::Lam { .. } => Vec::new(),
        CoreKind::App { func, args } => {
            let mut out = vec![&mut **func];
            out.extend(args.iter_mut());
            out
        }
        CoreKind::Prim { args, .. } => args.iter_mut().collect(),
        CoreKind::Let { value, body, .. } => vec![&mut **value, &mut **body],
        CoreKind::If { cond, then, alt } => vec![&mut **cond, &mut **then, &mut **alt],
        CoreKind::Match { scrutinee, arms } => {
            let mut out = vec![&mut **scrutinee];
            out.extend(arms.iter_mut().flat_map(Arm::exprs_mut));
            out
        }
        CoreKind::Make { fields, .. } => fields.iter_mut().map(|(_, v)| v).collect(),
        CoreKind::Field { base, .. } => vec![&mut **base],
        CoreKind::With { base, fields } => {
            let mut out = vec![&mut **base];
            out.extend(fields.iter_mut().map(|(_, v)| v));
            out
        }
        CoreKind::ListLit(items) => items.iter_mut().collect(),
        CoreKind::MapLit(pairs) => pairs.iter_mut().flat_map(|(k, v)| [k, v]).collect(),
    }
}

fn follow<'a>(c: &'a Core, path: &[usize]) -> &'a Core {
    match path.split_first() {
        None => c,
        Some((&i, rest)) => follow(children(c).swap_remove(i), rest),
    }
}

fn follow_mut<'a>(c: &'a mut Core, path: &[usize]) -> &'a mut Core {
    match path.split_first() {
        None => c,
        Some((&i, rest)) => follow_mut(children_mut(c).swap_remove(i), rest),
    }
}

/// The highest variable an expression names, so a fresh one can be chosen above it.
///
/// It descends into a `Lam`'s body, which [`children`] deliberately does not: a variable that only
/// a nested function binds is still a variable a rename would collide with.
fn max_var(c: &Core) -> VarId {
    let mut top = match &c.kind {
        CoreKind::Var(v) => *v,
        CoreKind::Let { var, .. } => *var,
        CoreKind::Lam { params, body } => {
            max_var(body).max(params.iter().copied().max().unwrap_or(0))
        }
        CoreKind::Match { arms, .. } => arms
            .iter()
            .filter_map(|a| a.pattern.binders().into_iter().max())
            .max()
            .unwrap_or(0),
        _ => 0,
    };
    for child in children(c) {
        top = top.max(max_var(child));
    }
    top
}

// -------------------------------------------------------------------------------------------
// Small `Core` constructors
// -------------------------------------------------------------------------------------------

fn var(v: VarId, ty: Ty, span: Span) -> Core {
    Core {
        kind: CoreKind::Var(v),
        ty,
        tier: Tier::Any,
        span,
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}

fn field(base: VarId, name: &str, ty: Ty) -> Core {
    Core {
        kind: CoreKind::Field {
            base: Box::new(var(base, Ty::unit(), Span::NONE)),
            name: Arc::from(name),
        },
        ty,
        tier: Tier::Any,
        span: Span::NONE,
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}

fn bind(v: VarId, value: Core, body: Core) -> Core {
    Core {
        ty: body.ty.clone(),
        tier: body.tier,
        span: body.span,
        kind: CoreKind::Let {
            var: v,
            value: Box::new(value),
            body: Box::new(body),
        },
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}
