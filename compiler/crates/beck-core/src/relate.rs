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
//! # The second shape: a filter that is a lookup into an index nobody built
//!
//! One `filter_list(xs, lambda y: g(y) == k(x))` inside the loop's body, where
//!
//! * `xs` reads only what the function **captured**, as above;
//! * `g` reads only the **filtered** element, so it is a key the collection can be arranged by; and
//! * `k` reads only the **loop's** element, so the probe is a function of the left row alone.
//!
//! That is the same equi-join with a different right side. `map_get`'s collection is a `Map` whose
//! own key *is* the join key, so [`crate::plan::Op::MapValues`]'s arrangement already answers it and
//! at most one row comes back. A filter's collection is keyed by something else entirely, so the
//! index has to be built — [`crate::plan::Op::ArrangeBy`], §99.9 item 3 — and several rows share a
//! key, so what comes back is the **group**.
//!
//! The group is the rows the predicate would have kept, in the order the collection held them,
//! because the index's key is `g(y)` followed by the collection's own key and the probe takes the
//! range under `g(y)`. That the two agree at all is a fact about `Prim::Eq` rather than a
//! convention: `==` is [`crate::Value`]'s own total order compared for equality, which is the order
//! the arrangement is a `BTreeMap` in.
//!
//! **What this does not do, stated here because the operator's name promises more.** The group is a
//! `list`, because the expression it replaced was one and its consumer loops over it. So a row
//! added to a group rebuilds *that group's* list and no other — the scan over the whole collection
//! is gone and the capture with it, but the group's own size is still paid. Removing that is
//! `group by` (§99.9 item 6), which is why item 6 follows this one rather than standing beside it.

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

/// One lookup, as the join that answers it.
pub struct Lookup {
    /// The collection to index, in the caller's variables: the first argument of the `map_get` or
    /// of the `filter_list`.
    pub over: Core,
    /// The join key, over the row the *previous* join in the chain produced — over the element
    /// itself for the first. This is the `Fun` body of [`crate::plan::Op::Join`].
    pub key: Core,
    /// The parameter `key` is written over.
    pub param: VarId,
    /// Which index answers it, and therefore what one probe returns.
    pub index: Index,
}

/// The index a lookup is answered from — the one difference between the two shapes recognised.
#[derive(Clone, Debug)]
pub enum Index {
    /// `map_get(m, k(x))`: the collection is a `Map` whose own key is the join key, so the index is
    /// the [`crate::plan::Op::MapValues`] arrangement that already exists and hash-consing shares
    /// it with every other reader of the same collection. At most one row answers.
    Unique,
    /// `filter_list(xs, lambda y: by(y) == k(x))`: nothing keys `xs` by `by`, so the index is an
    /// [`crate::plan::Op::ArrangeBy`] built for the purpose. Several rows share a key and the group
    /// answers.
    Grouped {
        /// What the collection is arranged by, as a function of one of its own elements.
        by: Core,
        /// The parameter `by` is written over — the filtered element, not the loop's.
        param: VarId,
    },
}

/// A loop whose body looked things up, taken apart.
///
/// # Why this is a list rather than one lookup
///
/// A row that shows two related things is an ordinary shape — `corpus/33-awareness.beck` renders a
/// person's whereabouts *and* their note, so its loop body looks up in two collections — and a rule
/// that refused it would leave the capture in place and the whole collection reconsidered per event,
/// which is the cost the operator exists to remove. So every qualifying lookup gets a join, chained:
/// each takes the previous one's rows on its left, and the row a body finally reads is nested,
/// `{left: {left: x, right: a₁}, right: a₂}`.
///
/// The chain is not free and the cost is memory rather than time: each join holds one row per left
/// row (§99.5 decision 4), so a body with four lookups arranges the collection four times over. What
/// it is *not* is the plan choice §99.8 is about — nothing here decides an order, because a lookup
/// is against an index and there is no side to swap.
pub struct Recognised {
    /// One per lookup, in the order the joins are chained.
    pub lookups: Vec<Lookup>,
    /// The element parameter the original body was written over.
    pub elem: VarId,
    /// The loop body, with each lookup replaced by a read of the row that answers it, over a fresh
    /// parameter that is the last join's row rather than the left value.
    pub body: Core,
    /// The parameter `body` now takes.
    pub row: VarId,
}

/// Why a body that contained a `map_get` was not recognised as a join.
///
/// §99.6's rule for the case inference cannot see: "compile it the slow way and *say so*". These
/// reach [`crate::plan::Node::because`], so `beck explain cost` prints the reason beside the
/// operator that pays for it rather than leaving a reader to guess which of the conditions failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing in the body that could relate: no `map_get` and no `filter_list`.
    NoLookup,
    /// The collection looked up in is derived per element, so there is nothing to index once.
    CollectionReadsTheElement,
    /// The key reads something other than the element, so it is not an equi-join on the left row.
    KeyReadsMoreThanTheElement,
    /// The filter's predicate is not an equality with one side over each element, so there is no
    /// key to arrange the collection by.
    PredicateIsNotAnEquality,
    /// Recognising it would not remove the capture that costs the rebuild, so it buys nothing.
    NothingSaved,
}

impl Refusal {
    /// The sentence `beck explain` prints, in the voice the rest of the plan's reasons use.
    pub fn because(&self) -> String {
        match self {
            Refusal::NoLookup => "its body relates nothing to the collection it loops over".into(),
            Refusal::CollectionReadsTheElement => {
                "the collection it looks up in is derived from the element, so there is nothing to \
                 index once"
                    .into()
            }
            Refusal::KeyReadsMoreThanTheElement => {
                "the key it looks up by reads more than the element, so it is not an equi-join on \
                 the left row"
                    .into()
            }
            Refusal::PredicateIsNotAnEquality => {
                "the predicate it filters by is not an equality between a function of the row and \
                 a function of the element, so there is no key to arrange the collection by"
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

    let mut sites: Vec<Vec<usize>> = Vec::new();
    lookups(&body, &mut Vec::new(), &mut sites);
    if sites.is_empty() {
        return Err(Refusal::NoLookup);
    }

    // Each site tested on its own, and the first failure kept only in case *none* qualifies: a body
    // with one lookup this can index and one it cannot is still worth indexing once.
    let mut chosen: Vec<(Vec<usize>, Core, Core, Index)> = Vec::new();
    let mut why = Refusal::NoLookup;
    for site in &sites {
        // A lookup inside another one's arguments cannot qualify — its enclosing key would then read
        // a captured collection — but the paths would also collide under the rewrite, so the check
        // is here rather than left to the conditions.
        if sites
            .iter()
            .any(|other| other != site && site.starts_with(other))
        {
            continue;
        }
        match qualify(&body, site, elem, defs, captured) {
            Ok((over, key, index)) => chosen.push((site.clone(), over, key, index)),
            Err(refused) => why = refused,
        }
    }
    if chosen.is_empty() {
        return Err(why);
    }

    // The rewrite. Each lookup becomes a read of the row that answers it, and the element becomes a
    // read through the chain's left spine — `let`s rather than substitutions, so the body is written
    // once however many times it mentions either.
    let row = fresh;
    let n = chosen.len();
    let answers: Vec<VarId> = (0..n as VarId).map(|k| fresh + 1 + k).collect();
    let mut rewritten = body;
    for ((site, _, _, _), &answer) in chosen.iter().zip(&answers) {
        // Replacing a node with a variable changes no ancestor's arity and no sibling's path, and
        // the descendants that would have been invalidated were skipped above — so the order these
        // are applied in does not matter.
        let ty = follow(&rewritten, site).ty.clone();
        *follow_mut(&mut rewritten, site) = var(answer, ty, Span::NONE);
    }
    for (i, &answer) in answers.iter().enumerate().rev() {
        rewritten = bind(answer, field_of(spine(row, n - 1 - i), RIGHT), rewritten);
    }
    let body = bind(elem, spine(row, n), rewritten);

    // One join per lookup, each keyed over the row the one before it produced. `param` is fresh per
    // stage because the key function is a `Fun` of its own and its parameter is not the element any
    // more once there is a stage below it.
    let lookups: Vec<Lookup> = chosen
        .into_iter()
        .enumerate()
        .map(|(i, (_, over, key, index))| {
            let param = fresh + 1 + n as VarId + i as VarId;
            let mut key = key;
            if i > 0 {
                substitute(&mut key, elem, &spine(param, i));
            }
            Lookup {
                over,
                key,
                param: if i == 0 { elem } else { param },
                index,
            }
        })
        .collect();

    Ok(Recognised {
        lookups,
        elem,
        body,
        row,
    })
}

/// One site, as the index that answers it — or the condition that failed.
///
/// The two shapes differ only in where the join key on the right comes from, which is why they are
/// one function: `map_get` is told the key by the collection it reads, and `filter_list` has to be
/// read out of an equality. Everything else — that the collection is something the plan already
/// holds, that the probe is a function of the loop's element alone — is the same condition twice.
fn qualify(
    body: &Core,
    site: &[usize],
    elem: VarId,
    defs: &BTreeMap<Arc<str>, Def>,
    captured: &BTreeSet<VarId>,
) -> Result<(Core, Core, Index), Refusal> {
    let at = follow(body, site);
    let CoreKind::Prim { op, args } = &at.kind else {
        unreachable!("a site is where a `map_get` or a `filter_list` is")
    };
    // Resolved against the `let`s the inliner left, because an argument that was not cheap enough
    // to substitute is bound rather than copied — so the collection may be a variable standing for
    // one.
    let mut lets = BTreeMap::new();
    collect_lets(body, site, &mut lets);

    let over = resolve(&args[0], &lets, DEPTH);
    let reads_over = reads(&over);
    if reads_over.contains(&elem) || !reads_over.is_subset(captured) {
        return Err(Refusal::CollectionReadsTheElement);
    }
    let only = |c: &Core, v: VarId| {
        let r = reads(c);
        r.contains(&v) && r.is_subset(&BTreeSet::from([v]))
    };

    if *op == Prim::MapGet {
        let key = resolve(&args[1], &lets, DEPTH);
        if !reads(&key).is_subset(&BTreeSet::from([elem])) {
            return Err(Refusal::KeyReadsMoreThanTheElement);
        }
        return Ok((over, key, Index::Unique));
    }

    let Some((y, pred)) = lambda(&args[1], defs) else {
        return Err(Refusal::PredicateIsNotAnEquality);
    };
    // A parameter that is also the loop's would make the two sides of the equality
    // indistinguishable. `Core`'s variables are numbered per definition and the inliner renames
    // above everything in sight, so this cannot happen — and a rewrite that is wrong about which
    // element it read would be wrong silently, which is what makes it worth a line.
    if y == elem {
        return Err(Refusal::PredicateIsNotAnEquality);
    }
    // The predicate's own bindings join the ones in scope at the site: an equality written through
    // a name is still an equality.
    let mut inner = lets;
    let pred = peel(&pred, &mut inner);
    let CoreKind::Prim {
        op: Prim::Eq,
        args: sides,
    } = &pred.kind
    else {
        return Err(Refusal::PredicateIsNotAnEquality);
    };
    let left = resolve(&sides[0], &inner, DEPTH);
    let right = resolve(&sides[1], &inner, DEPTH);
    // `==` is symmetric and a program may write it either way round, so which side is the index key
    // is read from what each side *reads* rather than from its position.
    let (by, key) = if only(&left, y) && only(&right, elem) {
        (left, right)
    } else if only(&right, y) && only(&left, elem) {
        (right, left)
    } else {
        return Err(Refusal::PredicateIsNotAnEquality);
    };
    Ok((over, key, Index::Grouped { by, param: y }))
}

/// An expression with its leading `let`s taken off and remembered.
fn peel(c: &Core, lets: &mut BTreeMap<VarId, Core>) -> Core {
    match &c.kind {
        CoreKind::Let { var, value, body } => {
            lets.insert(*var, (**value).clone());
            peel(body, lets)
        }
        _ => c.clone(),
    }
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

/// Every `map_get` and every `filter_list` in an expression, as the path of child indices that
/// reaches it.
///
/// A path rather than a pointer because the rewrite happens afterwards and has to reach the same
/// node in a `&mut` walk; `Core` is a tree of boxes, so there is no id to hold on to.
///
/// A site inside a nested `lambda` is not found, because [`children`] does not enter one: a lookup
/// there is a lookup per *call* of that function rather than per element.
fn lookups(c: &Core, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if matches!(
        &c.kind,
        CoreKind::Prim {
            op: Prim::MapGet | Prim::FilterList,
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

/// An expression's shape as a string, so two that are the same expression share one index.
///
/// The plan's hash-consing keys on a string, and the collection alone is not enough for
/// `arrange_by`: two joins over one collection by *different* keys are two indexes, and two by the
/// same key are one. `Core` is not `Eq`, and the parts of it that are not the expression — spans,
/// and the annotations [`crate::liveness`], [`crate::fields`] and [`crate::frames`] leave — would
/// make two identical expressions look different, so this writes down what a reader would call the
/// expression and nothing else.
///
/// Being wrong in the safe direction costs an index rather than an answer: two fingerprints that
/// differ where the expressions agree build two indexes that hold the same thing.
pub fn fingerprint(c: &Core) -> String {
    let mut out = String::new();
    write_fingerprint(c, &mut out);
    out
}

fn write_fingerprint(c: &Core, out: &mut String) {
    use std::fmt::Write;
    match &c.kind {
        CoreKind::Const(v) => {
            let _ = write!(out, "c{v:?}");
        }
        CoreKind::Var(v) => {
            let _ = write!(out, "v{v}");
        }
        CoreKind::Global(n) => {
            let _ = write!(out, "g{n}");
        }
        CoreKind::Prim { op, .. } => {
            let _ = write!(out, "p{}", op.name());
        }
        CoreKind::Field { name, .. } => {
            let _ = write!(out, "f{name}");
        }
        CoreKind::Make { ty, variant, .. } => {
            let _ = write!(out, "m{ty}.{}", variant.as_deref().unwrap_or(""));
        }
        CoreKind::Lam { params, body } => {
            let _ = write!(out, "l{params:?}");
            write_fingerprint(body, out);
        }
        CoreKind::Let { var, .. } => {
            let _ = write!(out, "b{var}");
        }
        CoreKind::App { .. } => out.push('a'),
        CoreKind::If { .. } => out.push('i'),
        CoreKind::Match { .. } => out.push('s'),
        CoreKind::With { fields, .. } => {
            let _ = write!(
                out,
                "w{:?}",
                fields.iter().map(|(n, _)| n).collect::<Vec<_>>()
            );
        }
        CoreKind::ListLit(_) => out.push('['),
        CoreKind::MapLit(_) => out.push('{'),
    }
    out.push('(');
    for child in children(c) {
        write_fingerprint(child, out);
        out.push(',');
    }
    out.push(')');
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

fn field_of(base: Core, name: &str) -> Core {
    Core {
        kind: CoreKind::Field {
            base: Box::new(base),
            name: Arc::from(name),
        },
        ty: Ty::unit(),
        tier: Tier::Any,
        span: Span::NONE,
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}

/// A row's **left spine**: `row.left.left…`, `depth` steps up the chain of joins.
///
/// Stage `i`'s row holds stage `i - 1`'s row on its left and stage `i`'s answer on its right, so
/// walking `depth` steps left from the last row is how the body reaches an earlier stage's answer —
/// and walking all the way is how it reaches the element the loop was written over.
fn spine(v: VarId, depth: usize) -> Core {
    let mut out = var(v, Ty::unit(), Span::NONE);
    for _ in 0..depth {
        out = field_of(out, LEFT);
    }
    out
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
