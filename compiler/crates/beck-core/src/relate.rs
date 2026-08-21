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
//!
//! # The third shape, which is the second one asked a different question
//!
//! `list_len(filter_list(xs, lambda y: g(y) == k(x)))` is the same equi-join again, and what differs
//! is only what a probe returns: a number rather than the rows. Nothing about the index changes, so
//! this is a field of the grouped shape ([`Answers`]) rather than a shape of its own — and it is the
//! first of §99.9 item 6's aggregates, the one the language already had a spelling for. A group that
//! is only ever counted is never built, which is what [`crate::plan::Matching::Count`] is for.
//!
//! # The fourth shape: a number the group's rows decide
//!
//! `list_min(filter_list(…))`, `list_max(…)` and `list_sum(…)`, bare or over a `map_list` of the
//! same filter, are the same question once more — §99.9 item 6's other three aggregates. What
//! differs from the count is that the answer is a function of what the rows *say* rather than only
//! of how many there are, so something has to hold what they contribute:
//! [`crate::plan::Op::GroupBy`], keyed by the group and holding per group whatever its aggregate
//! needs — a multiset of the projection, whose two ends are `min` and `max`, or a running total,
//! which is `sum`.
//!
//! It is the one shape whose *index* is not an index. The other three probe an arrangement of the
//! collection; this probes an arrangement of the **groups**, one entry each. For the extremes the
//! join above it is a [`crate::plan::Matching::Unique`] — the same probe a `map_get` gets, `Some`
//! for a group with rows and `None` for one without, which is what `list_min` returns of a list and
//! of an empty one. For a total it is a [`crate::plan::Matching::Total`], and the difference is
//! what a *missing* entry means: `list_sum` of no rows is `0`, so the probe answers with a value
//! where the extremes answer with an absence.
//!
//! # The fifth shape, which is not a loop at all
//!
//! `filter_list(xs, lambda x: map_contains(m, k(x)))` and its negation are the algebra's
//! **intersection** and **difference** by key — [`crate::plan::Op::Restrict`], §99.9 item 7 — and
//! [`restriction`] is where they are read. The conditions are the same two conditions again: `m`
//! reads only what the function captured, `k` reads only the element.
//!
//! What differs is *where* the shape is looked for, and the reason is what comes out of the
//! operator. A join is recognised at a **site inside a body**, because a loop does other things
//! besides look up and the body has to be rewritten around the row. A restriction has no body to
//! rewrite: it keeps and drops the elements the filter was keeping and dropping, so the predicate
//! is not rewritten, it is *deleted*. That is also why a `filter_list` can have this operator when
//! it cannot have a join — a join's element is a row, and a filter's consumers read the element.
//!
//! The cost it removes is the same one, arrived at from the other side. A predicate that reads a
//! collection is a different predicate whenever that collection moves, so [`crate::engine`]'s
//! rebuild rule reconsiders every element on every event — a nested-loop anti-join with no index.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::Span;

use crate::check::Def;
use crate::core::{free_vars, Arm, Core, CoreKind, Prim, VarId};
use crate::plan::{Agg, Presence};
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
    /// answers — either its rows or a question about them.
    Grouped {
        /// What the collection is arranged by, as a function of one of its own elements.
        by: Core,
        /// The parameter `by` is written over — the filtered element, not the loop's.
        param: VarId,
        /// What the body asked the group for.
        answers: Answers,
    },
}

/// What a body wanted from a group, which decides whether the group has to be built at all.
///
/// [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6: an
/// aggregate that builds the collection in order to measure it has done the work the question was
/// avoiding. All of these are recognised from the same `filter_list` — bare, or wrapped in the
/// question the program asked of it — which is why they are a field of one variant rather than
/// several shapes.
#[derive(Clone, Debug)]
pub enum Answers {
    /// The rows. `filter_list(…)` on its own, whose value is a `list`.
    Rows,
    /// How many rows. `list_len(filter_list(…))`, whose value is an `Int` — and no list is built.
    Count,
    /// A number the group's rows decide, which the group does not have to exist for.
    /// `list_min(filter_list(…))` or `list_sum(…)`, or either over a `map_list` of it — and no
    /// list is built.
    ///
    /// Boxed because it is the only variant carrying an expression, and an enum is as large as its
    /// largest arm wherever it is held.
    Aggregate(Box<Aggregate>),
}

/// What a group was asked for, and what its rows contribute to the question.
#[derive(Clone, Debug)]
pub struct Aggregate {
    /// Which question.
    pub agg: Agg,
    /// What each row contributes, as a function of one row of the *filtered* collection: the
    /// `map_list`'s function, or the identity when the program asked about the rows themselves.
    pub of: Core,
    /// The parameter [`Aggregate::of`] is written over — the filtered element, not the loop's.
    pub param: VarId,
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

/// One membership test, as the restriction that answers it.
///
/// [`Lookup`]'s sibling, and the difference is what comes out: a lookup produces a *row* and this
/// produces the element the filter was given, kept or dropped. So there is no rewritten body here
/// — the operator has no per-element function beyond the key, because a predicate the index
/// answers is not a predicate any more.
pub struct Membership {
    /// The collection whose keys decide, in the caller's variables: the first argument of the
    /// `map_contains`.
    pub over: Core,
    /// The key to probe it by, as a function of the element. This is the `Fun` body of
    /// [`crate::plan::Op::Restrict`].
    pub key: Core,
    /// The parameter `key` is written over — the filtered element.
    pub param: VarId,
    /// Which answer keeps the row: the predicate as written, or its negation.
    pub keep: Presence,
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
    /// The aggregate's projection reads something other than the row it is applied to, so what
    /// each row contributes to its group is not a function of that row.
    ProjectionReadsMoreThanTheRow,
    /// Recognising it would not remove the capture that costs the rebuild, so it buys nothing.
    NothingSaved,
    /// Nothing in the predicate asks another collection whether it holds a key, so there is no
    /// difference and no intersection here.
    NoMembership,
    /// The predicate asks a collection whether it holds a key **and something else besides**, so
    /// the filter is not the membership test — it contains one.
    MembershipAndMore,
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
            Refusal::ProjectionReadsMoreThanTheRow => {
                "what it takes the smallest or largest of reads more than the row it is applied \
                 to, so a group's answer is not a function of the group"
                    .into()
            }
            Refusal::NothingSaved => {
                "rewriting it against an index would not remove what its function captured, so it \
                 would cost an index and save nothing"
                    .into()
            }
            Refusal::NoMembership => {
                "its predicate asks no other collection whether it holds a key".into()
            }
            Refusal::MembershipAndMore => {
                "its predicate asks another collection whether it holds a key and asks something \
                 else as well, so the filter is not that question — splitting it into two \
                 operators is a rewrite rather than a reading"
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
    //
    // Outermost first — [`lookups`] collects in pre-order — and a site under one that **qualified**
    // is skipped: two chosen sites on one spine would collide under the rewrite, and an aggregate's
    // own filter is exactly that case. A site under one that *failed* is still considered, which is
    // the difference between this and skipping every nested site outright: `list_min` over a filter
    // whose projection reads the loop's element is not an aggregate this can maintain, and the
    // filter under it is still the group the program would otherwise re-scan.
    let mut chosen: Vec<(Vec<usize>, Core, Core, Index)> = Vec::new();
    let mut claimed: Vec<&[usize]> = Vec::new();
    let mut why = Refusal::NoLookup;
    for site in &sites {
        if claimed.iter().any(|outer| site.starts_with(outer)) {
            continue;
        }
        match qualify(&body, site, elem, defs, captured) {
            Ok((over, key, index)) => {
                claimed.push(site);
                chosen.push((site.clone(), over, key, index));
            }
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

/// Try to read a filter's predicate as a **membership test** against another collection.
///
/// `filter_list(xs, lambda x: map_contains(m, k(x)))` is the intersection of `xs` with `m`'s keys
/// and its negation is the difference — [`crate::plan::Op::Restrict`], and
/// [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7. The two
/// conditions are [`recognise`]'s, for [`recognise`]'s reasons: `m` may read only what the function
/// **captured**, so the collection is a node the plan can index once, and `k` may read only the
/// **element**, so the probe is a function of the row alone.
///
/// What it does *not* share with [`recognise`] is the search. A join is recognised at a site
/// *inside* a body, because a loop does other things as well as look up; a filter's predicate is
/// the whole of what the operator computes, so a predicate that is a membership test **and
/// something else** is not this shape at all. Splitting `p(x) and map_contains(m, k(x))` into two
/// operators is a rewrite the fuser owns rather than a recognition, and it is named as absent in
/// §99.10 rather than attempted here.
pub fn restriction(
    f: &Core,
    defs: &BTreeMap<Arc<str>, Def>,
    captured: &BTreeSet<VarId>,
) -> Result<Membership, Refusal> {
    let Some((elem, body)) = lambda(f, defs) else {
        return Err(Refusal::NoMembership);
    };
    let mut fresh = 1 + max_var(&body).max(captured.iter().copied().max().unwrap_or(0));
    let body = inline(&body, defs, &mut Vec::new(), &mut fresh, DEPTH);

    // A predicate that *contains* a membership test without being one is told apart from a
    // predicate that has nothing to do with one, because the two want opposite things said about
    // them: the first is a program left at `O(n)` per event by a rewrite this does not do, which
    // §99.9 item 5 is explicit is not a conservative choice, and the second is every ordinary
    // filter in the tree.
    let refuse = |c: &Core| match contains_membership(c, captured) {
        true => Refusal::MembershipAndMore,
        false => Refusal::NoMembership,
    };

    let mut lets = BTreeMap::new();
    let mut at = peel(&body, &mut lets);
    // One `not` and no more. Two would cancel, and a predicate written `not not c` is not a shape
    // worth carrying a loop for — it is refused with the same sentence anything else gets.
    let mut keep = Presence::In;
    if let CoreKind::Prim {
        op: Prim::Not,
        args,
    } = &at.kind
    {
        if args.len() != 1 {
            return Err(refuse(&body));
        }
        keep = Presence::NotIn;
        at = peel(&args[0].clone(), &mut lets);
    }
    let CoreKind::Prim {
        op: Prim::MapContains,
        args,
    } = &at.kind
    else {
        return Err(refuse(&body));
    };
    if args.len() != 2 {
        return Err(refuse(&body));
    }

    let over = resolve(&args[0], &lets, DEPTH);
    let reads_over = reads(&over);
    if reads_over.contains(&elem) || !reads_over.is_subset(captured) {
        return Err(Refusal::CollectionReadsTheElement);
    }
    let key = resolve(&args[1], &lets, DEPTH);
    if !reads(&key).is_subset(&BTreeSet::from([elem])) {
        return Err(Refusal::KeyReadsMoreThanTheElement);
    }
    Ok(Membership {
        over,
        key,
        param: elem,
        keep,
    })
}

/// Whether an expression asks a collection the plan could index whether it holds a key.
///
/// Not "whether there is a `map_contains`" — the collection has to be one the plan already has, or
/// the answer would be about a map built per element and there would be nothing to index. That is
/// the same first condition [`qualify`] applies, checked here only to decide which sentence a
/// refusal carries.
fn contains_membership(c: &Core, captured: &BTreeSet<VarId>) -> bool {
    if let CoreKind::Prim {
        op: Prim::MapContains,
        args,
    } = &c.kind
    {
        if args.len() == 2 && reads(&args[0]).is_subset(captured) {
            return true;
        }
    }
    children(c)
        .into_iter()
        .any(|child| contains_membership(child, captured))
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
    // An aggregate's site is the `filter_list` under it, asked a different question. Everything
    // below reads the filter, and only the answer differs.
    let (at, asked) = match asked(at) {
        Some(pair) => pair,
        None => (at, Asked::Rows),
    };
    let CoreKind::Prim { op, args } = &at.kind else {
        unreachable!("a site is where a `map_get` or a `filter_list` is")
    };
    // Resolved against the `let`s the inliner left, because an argument that was not cheap enough
    // to substitute is bound rather than copied — so the collection may be a variable standing for
    // one.
    let mut lets = BTreeMap::new();
    collect_lets(body, site, &mut lets);
    let outer = lets.clone();

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
    let answers = match asked {
        Asked::Rows => Answers::Rows,
        Asked::Count => Answers::Count,
        // The identity, written as the *variable node* the predicate already reads `y` through
        // rather than as one this function builds: a `Core` carries its type, and the type of the
        // row is not something the shape of a `filter_list` says.
        Asked::Aggregated { agg, of: None } => Answers::Aggregate(Box::new(Aggregate {
            agg,
            of: find_var(&by, y).ok_or(Refusal::PredicateIsNotAnEquality)?,
            param: y,
        })),
        Asked::Aggregated { agg, of: Some(f) } => {
            let (z, of) = lambda(f, defs).ok_or(Refusal::ProjectionReadsMoreThanTheRow)?;
            let of = resolve(&of, &outer, DEPTH);
            if !reads(&of).is_subset(&BTreeSet::from([z])) {
                return Err(Refusal::ProjectionReadsMoreThanTheRow);
            }
            Answers::Aggregate(Box::new(Aggregate { agg, of, param: z }))
        }
    };
    Ok((
        over,
        key,
        Index::Grouped {
            by,
            param: y,
            answers,
        },
    ))
}

/// What a site asks of a group, before the group's own key is known.
///
/// [`Answers`] is the same thing with the projection resolved, which cannot happen until the
/// filter's parameter has been read out of its predicate — so this carries the *unresolved*
/// function and [`qualify`] finishes it.
enum Asked<'a> {
    Rows,
    Count,
    Aggregated {
        agg: Agg,
        /// The `map_list`'s function, or `None` for an aggregate of the rows themselves.
        of: Option<&'a Core>,
    },
}

/// The `filter_list` under an aggregate, and the question the aggregate asked of it.
///
/// `None` for anything that is not one, which is what keeps [`lookups`] and [`qualify`] agreeing
/// about what a site is: a site is a `map_get`, a `filter_list`, or a node this function reads.
///
/// The wrappers are alternatives rather than a chain, so they are listed here rather than peeled in
/// a loop, and a reader looking for "which shapes count as an aggregate" finds them in one place.
fn asked(c: &Core) -> Option<(&Core, Asked<'_>)> {
    let CoreKind::Prim { op, args } = &c.kind else {
        return None;
    };
    match op {
        Prim::ListLen if args.len() == 1 && is_filter(&args[0]) => Some((&args[0], Asked::Count)),
        Prim::ListMin | Prim::ListMax | Prim::ListSum if args.len() == 1 => {
            let agg = match op {
                Prim::ListMin => Agg::Min,
                Prim::ListMax => Agg::Max,
                _ => Agg::Sum,
            };
            if is_filter(&args[0]) {
                return Some((&args[0], Asked::Aggregated { agg, of: None }));
            }
            // `list_min(map_list(filter_list(…), f))` — the aggregate of what each row projects
            // to, which is how anybody writes "the earliest of their deadlines" or "what they are
            // owed" rather than "the smallest of their rows".
            let CoreKind::Prim {
                op: Prim::MapList,
                args: mapped,
            } = &args[0].kind
            else {
                return None;
            };
            match mapped.len() == 2 && is_filter(&mapped[0]) {
                true => Some((
                    &mapped[0],
                    Asked::Aggregated {
                        agg,
                        of: Some(&mapped[1]),
                    },
                )),
                false => None,
            }
        }
        _ => None,
    }
}

/// Whether this node is `filter_list(xs, p)`.
fn is_filter(c: &Core) -> bool {
    matches!(
        &c.kind,
        CoreKind::Prim { op: Prim::FilterList, args } if args.len() == 2
    )
}

/// The first node in an expression that reads `v`, so a variable can be recovered with the type it
/// was written with.
fn find_var(c: &Core, v: VarId) -> Option<Core> {
    if matches!(&c.kind, CoreKind::Var(id) if *id == v) {
        return Some(c.clone());
    }
    children(c).into_iter().find_map(|child| find_var(child, v))
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
///
/// An aggregate counts as a site only when the `filter_list` it measures is *syntactically* under
/// it ([`asked`]). That is not a shortcut, it is what keeps the aggregate from swallowing the
/// group: a site inside another one is skipped, so admitting every `list_len` would hide the
/// `filter_list` under it behind an outer site that could not qualify. Written this way, the outer
/// site exists exactly when the inner one would have qualified too. What it costs is an aggregate
/// written through a `let` — `g = filter_list(…)` then `list_len(g)` — which is recognised as the
/// group rather than as the question, and is slower rather than wrong.
fn lookups(c: &Core, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    if matches!(
        &c.kind,
        CoreKind::Prim {
            op: Prim::MapGet | Prim::FilterList,
            args
        } if args.len() == 2
    ) || asked(c).is_some()
    {
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
    write_fingerprint(c, None, &mut out);
    out
}

/// The same, for an expression written over one **bound** parameter, whose number is written
/// canonically.
///
/// This is the form every index's key takes, and it is a separate function because the difference
/// is not cosmetic: `lambda b: b.lot` and `lambda c: c.lot` are the same key, and `Core` numbers
/// variables per definition, so two loops that index the same collection by the same key arrive
/// here with different numbers. Fingerprinting them apart built **two identical arrangements** — an
/// arrangement is memory per subscriber as well as work per event
/// ([`docs/23`](../../../../../docs/23-incremental-views-report.md) §23.14), so the safe direction
/// was not free.
///
/// Only the parameter is normalised, and that is enough because an index key **reads nothing else**
/// — the recogniser refuses the shape otherwise. A `let` bound inside one would still fingerprint
/// by its number, which is the same conservatism one level down and has no program.
pub fn fingerprint_fun(param: VarId, body: &Core) -> String {
    let mut out = String::new();
    write_fingerprint(body, Some(param), &mut out);
    out
}

fn write_fingerprint(c: &Core, param: Option<VarId>, out: &mut String) {
    use std::fmt::Write;
    match &c.kind {
        CoreKind::Const(v) => {
            let _ = write!(out, "c{v:?}");
        }
        CoreKind::Var(v) => match param == Some(*v) {
            true => out.push_str("v_"),
            false => {
                let _ = write!(out, "v{v}");
            }
        },
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
            write_fingerprint(body, param, out);
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
        write_fingerprint(child, param, out);
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
