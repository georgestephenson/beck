//! The view, as a dataflow plan rather than as one expression.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.3:
//!
//! > a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one*
//! > shared dataflow whose final per-session operators (filter, project, diff) run per subscriber
//!
//! and [`docs/03-type-and-effect-system.md`](../../../../../docs/03-type-and-effect-system.md) §3.8:
//!
//! > `remaining` updates by ±1 per event, never by recount.
//!
//! [`crate::split`] produces a `Core` *function* of the accumulator — full recompute per event,
//! which Phase 1 called "semantically final, later made incremental". [`crate::incremental`]
//! answers *which* vertices a plan could maintain. This module is the plan itself: the same view,
//! decomposed into operators that a delta can flow through, with [`crate::engine`] as the thing
//! that flows them.
//!
//! # What an operator is
//!
//! Two kinds, and the distinction is the whole design:
//!
//! * A **delta operator** ([`Op::MapValues`], [`Op::MapList`], [`Op::FilterList`], [`Op::SortBy`],
//!   [`Op::Concat`], [`Op::Flatten`], [`Op::FlatMap`], [`Op::Count`], [`Op::IsEmpty`],
//!   [`Op::Join`], [`Op::ArrangeBy`], [`Op::GroupBy`], [`Op::Restrict`]) holds an
//!   ordered *arrangement* — its output as
//!   a keyed collection — and updates it from the changes at its input. Work is proportional to the
//!   change, not to the collection.
//! * A **pointwise operator** ([`Op::Pointwise`]) holds a value and recomputes it when an input
//!   changed. That is what today's runtime does for the whole view, so a plan of nothing but
//!   pointwise operators is exactly as fast as no plan at all — and no slower, which is what makes
//!   this safe to switch on for every program.
//!
//! Everything the decomposition cannot see through becomes one pointwise operator over the plan
//! nodes it reads: a `match`, an `if`, a call through a value, a primitive with no delta rule. The
//! fallback is the reason the engine can be correct for programs it cannot accelerate, and
//! [`Node::because`] records which construct forced it so `beck explain incremental` can say so.
//!
//! # Where the keys come from
//!
//! An arrangement is a `BTreeMap` from an ordering key to a value, and the key is what makes the
//! output's *order* a consequence of the plan rather than of a sort at the end. Iteration order
//! reaches the rendered page and the replay digest ([`crate::pmap`]), so an incremental view that
//! produced the right entries in a different order would be a correctness bug, not a cosmetic one.
//!
//! | operator | key |
//! |---|---|
//! | `map_values(m)` | the map's key — so the arrangement is already in the order `map_values` yields |
//! | `map_list`, `filter_list` | the input's key, unchanged: neither moves an element |
//! | `sort_by(xs, k)` | `k(x)` followed by the input's key — a stable sort, expressed as an order |
//! | `concat_lists([a, b])` | the input's position, followed by that input's key |
//! | `flatten`, `flat_map` | the input's key, followed by the position inside that element's list |
//! | `join` | the left input's key — a lookup answers one left row, so nothing of the right's is needed to separate two |
//! | `arrange_by(xs, k)` | `k(x)` followed by the input's key — `sort_by`'s arrangement, probed by prefix instead of iterated |
//! | `group_by` | the group's key alone — one entry per group, so the collection's order never reaches it |
//! | `semi_join`, `anti_join` | the input's key, unchanged: the index decides *which* rows survive, never where they sit |
//!
//! # What this is not
//!
//! It is not a *query* plan. §4.2 keeps the `Query` sub-language symbolic and nothing compiles one;
//! this compiles the signal graph, which is a different thing that happens to share the word.
//! `beck explain query` prints *this*, and [`crate::fuse`] rewrites it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::check::Def;
use crate::core::{Core, CoreKind, Prim, VarId};
use crate::signal::{signal_elem, Graph, Op as SigOp, SigId};
use crate::split::{Placed, StateRole};
use crate::ty::{Tier, Ty};

pub type OpId = usize;

/// Whether the decomposition may read a loop as a join.
///
/// The off switch [`docs/08`](../../../../../docs/08-roadmap.md) §8.3 item 8 requires of anything
/// the compiler decides for you — "a default nobody has run is a claim, so the switched-off path
/// belongs in a gate beside the fast one". Recognising a join
/// ([`crate::relate`]) changes which operators a program compiles to without the program saying so,
/// which is exactly the kind of choice that item is about, and
/// `scaling.rs::maintaining_a_view_whose_loop_looks_something_up_costs_the_same_at_any_size`
/// measures **both** settings so the gate carries its own evidence that it can fail.
///
/// It is a compile-time switch rather than an `AppConfig` field because a plan is compiled once,
/// before a runtime exists: `beck explain query --no-join` and `beck explain cost --no-join` are
/// where a developer reaches it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Relate {
    /// Read `for x in xs:` whose body looks something up as an equi-join (docs/99 §99.6).
    #[default]
    Recognise,
    /// Leave every loop as the captured `map_list` its source spells, index nothing, and pay the
    /// nested loop per event.
    Refuse,
}

/// A function an operator applies per element, closed over the plan nodes it reads.
///
/// The captures are why this is not simply a `Core` lambda: `lambda t: t.owner == session.actor`
/// reads the session, which is a *node* — so the operator has to be re-run wholesale when a capture
/// changes, and per element when only the collection changed. Both are expressible only if the
/// captured nodes are named.
#[derive(Clone, Debug)]
pub struct Fun {
    /// `Lam` over the captured nodes' variables followed by the element.
    pub code: Core,
    pub captures: Vec<OpId>,
}

/// What one operator does.
#[derive(Clone, Debug)]
pub enum Op {
    /// The durable accumulator, supplied by the caller. The plan's one source.
    State,
    /// The subscriber's `Session`. Everything not downstream of it is shareable between
    /// subscribers (§5.3), and everything downstream of it is that subscriber's — including when
    /// what moved is the *route* rather than the actor, which is the one field of a session that
    /// changes while a subscription is open ([`crate::render::SessionUse`]).
    Session,
    /// Who is connected — `presence()`, supplied by the caller like the other two sources.
    ///
    /// Everything downstream of it is **per subscriber** even though the value is the same for
    /// everybody, and the reason is a clock rather than a privacy rule: the shared dataflow is
    /// versioned by the log's `seq` ([`crate::engine::SharedDataflow`]), and presence moves when
    /// the log does not. Sharing it would need a second version, which is
    /// [`docs/48`](../../../../../docs/48-identity-report.md) §48.13's first unbuilt item.
    Presence,
    /// What everybody is doing — `awareness(f)`, supplied by the caller like the other sources.
    ///
    /// [`Op::Presence`]'s rules, for [`Op::Presence`]'s reason: a roster with a payload is not a
    /// function of the accumulator either, and the shared dataflow is versioned by the log's
    /// `seq`. A separate source rather than a field of the roster because the two move
    /// independently — a client that moves its cursor changes this and not presence.
    Awareness,
    /// A closed expression, evaluated once when the plan is prepared.
    Const,
    /// Recomputed when an input changed. Carries a `Lam` over its inputs.
    Pointwise {
        code: Core,
    },
    /// `map_values(m)` — where every delta in a Beck program is born, because the accumulator is a
    /// value and a plan consumes changes. [`crate::pmap::PMap::diff`] is the conversion.
    MapValues,
    MapList {
        f: Fun,
    },
    FilterList {
        f: Fun,
    },
    SortBy {
        f: Fun,
    },
    /// `concat_lists([a, b, …])` — a union of delta streams, one per named part.
    Concat,
    /// `concat_lists(map_list(xs, f))` as one operator — what [`crate::fuse`] makes of the pair,
    /// and the shape every `for` loop in a `ui:` block has. Applies `f` and takes the resulting
    /// list apart in one step, so the list of lists in between is never arranged.
    FlatMap {
        f: Fun,
    },
    /// `concat_lists(xs)` where `xs` is itself a collection of lists: a flatten.
    ///
    /// A `for` loop in a `ui:` block lowers to `concat_lists(map_list(todos, …))` and
    /// [`crate::fuse`] turns that pair into [`Op::FlatMap`], so this is what remains when the
    /// collection of lists came from somewhere else — a `map_values` whose values are lists, a
    /// `sort_by`, or a `map_list` the fusion refused.
    Flatten,
    /// `list_len` — §3.8's `remaining`. The arrangement's size, so ±1 per delta and never a
    /// recount; and it does not force its input to be materialised.
    Count,
    IsEmpty,
    /// The join a loop already contained: `for x in xs:` whose body asks `map_get(m, k(x))`.
    ///
    /// [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.6 — the
    /// algebra's first **binary** operator, and the reason it is not a syntax: the two programs in
    /// the tree that relate two collections already say what they mean, and what they were missing
    /// was an operator to say it *to*. [`crate::relate`] is the recognition.
    ///
    /// Two inputs, and they are not symmetric. The **left** is the collection being looped over.
    /// The **right** is an *index*: an arrangement whose key's first component is the join key,
    /// which [`Op::MapValues`] over a `Map` already is. One left row matches at most one right row,
    /// because an arrangement's keys are unique by construction (§99.5 decision 2), so this is an
    /// outer equi-join on a unique key and every left row appears exactly once in the output —
    /// with the match, or without one, which is what `map_get`'s `Option` means.
    ///
    /// Maintained from **both** sides (§99.5's bilinear rule): a left row that moved is re-looked
    /// up, and a right row that moved reaches exactly the left rows whose key it answers, through a
    /// reverse index this operator keeps. Neither costs the collection.
    Join {
        /// The join key, as a function of the left element alone. It captures nothing —
        /// [`crate::relate`] refuses the shape otherwise — which is what makes the operator's own
        /// work `O(δ)` rather than `O(n)`.
        key: Fun,
        /// What one probe of the right side returns, which is decided by which index is on it.
        matched: Matching,
    },
    /// A second index over a collection, keyed by something other than what orders it — §99.5
    /// decision 4's `arrange_by`, and [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md)
    /// §99.9 item 3.
    ///
    /// It is the right side of a [`Op::Join`] whose left side asked for a *group*: the collection
    /// the program wrote `filter_list(xs, lambda y: by(y) == …)` over, arranged so that the
    /// equality is a range rather than a scan.
    ///
    /// **Its arrangement is [`Op::SortBy`]'s, and that is worth saying rather than hiding.** Both
    /// key an element by `f(x)` followed by the input's key, so both are one `BTreeMap` in which
    /// equal keys keep the order they arrived in. A sort is that arrangement *iterated*; an index
    /// is that arrangement *probed*. The engine runs one function for the two, and what differs is
    /// the consumer — which is why they are two operators rather than one with a flag: nothing may
    /// fuse a probe the way it fuses a sort, and `beck explain query` should not tell a reader
    /// their program sorts when it does not.
    ArrangeBy {
        /// What to key by, as a function of one element. It captures nothing, for
        /// [`Op::Join`]'s reason.
        key: Fun,
    },
    /// One value per group, maintained — [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md)
    /// §99.9 item 6's `group by`, and the operator that answers a question about a group
    /// **without the group existing**.
    ///
    /// Its output is an arrangement keyed by the group's key alone, holding the aggregate, so a
    /// [`Op::Join`] probes it with [`Matching::Unique`] exactly as it probes a `map_values` — the
    /// answer is `Some(x)` for a group with rows and `None` for one without, which is what
    /// `list_min` of a list and of an empty list already return.
    ///
    /// **It is not an index and it does not arrange the collection.** [`Op::ArrangeBy`] keys every
    /// *row* so that a range answers with the group; this keeps, per group, only as much as its
    /// aggregate needs — a multiset of what the rows projected to for an extreme, a running total
    /// for a sum. A row that arrives moves that and nothing else, and the aggregate moves or it
    /// does not: an event that does not change the answer emits no change and nothing downstream
    /// runs. A `sum` is the aggregate that takes no discount there, because every row that joins
    /// its group changes it.
    ///
    /// **Both ends of that multiset are reachable, and that is the finding.** §99.9 item 6 expected
    /// `min` and `max` to be asymmetric, because a prefix range of *somebody else's* arrangement
    /// can be entered from its start and not from its end: bounding `(g, y)` above needs a
    /// successor of an arbitrary [`crate::Value`] and there is none. A tree this operator builds
    /// itself is keyed by the projection alone and is bounded at both ends by construction, so
    /// `max` costs what `min` costs. The asymmetry belonged to the design rather than to the
    /// problem.
    GroupBy {
        /// The group's key, as a function of one row. It captures nothing, for [`Op::Join`]'s
        /// reason.
        key: Fun,
        /// What each row contributes to its group — the projection under the aggregate, and the
        /// identity when the program asked about the rows themselves.
        of: Fun,
        /// Which end of the group is wanted.
        agg: Agg,
    },
    /// The left rows an index answers, or the ones it does not — the algebra's **difference**, and
    /// the intersection that is its complement
    /// ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7).
    ///
    /// The program wrote `filter_list(xs, lambda x: map_contains(m, k(x)))`, or its negation, and
    /// what that costs today is [`Op::FilterList`]'s rebuild rule: a predicate that reads `m` is a
    /// different predicate whenever `m` moves, so a payment arriving reconsiders every invoice.
    /// [`crate::relate::restriction`] is the recognition, and there is no syntax for the operator
    /// for [`Op::Join`]'s reason.
    ///
    /// **It is the one binary operator whose output is one of its inputs**, and that is the whole
    /// of §99.5 decision 2's "no representational change at all". A [`Op::Join`] emits a *row* —
    /// the left value and what it matched — so the collection below it holds something the program
    /// did not write, which is why a `filter_list` cannot become one: its consumers read the
    /// element. This emits the left element under the left key, so what a consumer reads is what
    /// the `filter_list` gave it, entry for entry.
    ///
    /// Maintained from both sides, as §99.5's bilinear rule requires, and the **right** half is
    /// the one no single-collection test can see: an entry arriving in the index takes rows *out*
    /// of a difference and puts them into an intersection, through the same reverse index
    /// [`Op::Join`] keeps. A left row that moved is one probe.
    ///
    /// **It holds no copy of its left input**, which is what lets a row this operator dropped come
    /// back when the index entry that dropped it leaves. The value is read from the left input
    /// itself — its arrangement, or the shadow this operator already keeps of a plain list — so
    /// the state here is a join key per left row and the reverse index, and never a row.
    Restrict {
        /// The key to probe the index by, as a function of the left element alone. It captures
        /// nothing, for [`Op::Join`]'s reason.
        key: Fun,
        /// Which answer keeps the row.
        keep: Presence,
    },
    /// The values in a collection, each once — the algebra's **δ**, and the last row of §99.4
    /// ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7).
    ///
    /// `list_unique(xs)`, which is `lib/collections.beck`'s `unique`: the first occurrence of each
    /// value, in the order the input held them. **The order is the decision and not a detail.**
    /// The library has a second duplicate-free list — `elements(set_of(xs))`, which is sorted — and
    /// both are maintainable; taking the answer a program already had rather than inventing a third
    /// is the test a second spelling of an old operation has to pass, which is `list_sum`'s rule
    /// applied to an order instead of to a total.
    ///
    /// So the output's key is an **input key**: the smallest one holding each value. That makes the
    /// output a sub-order of the input's, exactly as [`Op::FilterList`]'s is, and it is why nothing
    /// downstream had to learn anything — a consumer reads the values in first-occurrence order
    /// because that is what iterating the arrangement gives.
    ///
    /// **What moves is the interesting half.** A value arriving *before* its own standing first
    /// occurrence moves the published entry — the only operator here whose output entry can change
    /// key without the value changing — and one leaving promotes the next occurrence rather than
    /// dropping the value. Both are `O(log n)`, because the operator keeps the input keys holding
    /// each value in an ordered set and reads one end of it.
    ///
    /// It carries no per-element function: a projection is a [`Op::MapList`] above it, which is how
    /// the program wrote it.
    Distinct,
}

/// Which side of an index's answer a [`Op::Restrict`] keeps.
///
/// Two operators in one, because they are one delta rule read in two directions: the same probe,
/// the same reverse index, the same cost, and a program that shows both halves of a partition
/// shares one index between them (§99.5 decision 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// `map_contains(m, k(x))` — kept when the index holds the key. The **intersection** by key,
    /// which is a semi-join.
    In,
    /// `not map_contains(m, k(x))` — kept when it does not. The **difference** by key, which is an
    /// anti-join, and §99.4's one missing row that a program in the tree was already paying for.
    NotIn,
}

impl Presence {
    /// Whether an index holding the key (or not holding it) keeps the row.
    pub fn keeps(self, present: bool) -> bool {
        match self {
            Presence::In => present,
            Presence::NotIn => !present,
        }
    }

    /// The name that reaches a sharing key and `beck explain query`.
    pub fn name(self) -> &'static str {
        match self {
            Presence::In => "semi_join",
            Presence::NotIn => "anti_join",
        }
    }
}

/// Which aggregate a [`Op::GroupBy`] maintains.
///
/// Three rather than four: `count` is the join's own tally ([`Matching::Count`]) because a count
/// needs nothing of the row at all.
///
/// Every one of them is a function of **which numbers** its group's rows project to and of nothing
/// else — not of the order they arrived in, not of the order the collection holds them in. That is
/// what makes a maintained answer and a recomputed one the same value rather than nearly the same
/// one, and it is why `sum` waited for a spelling rather than for an implementation
/// ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6). It is a
/// statement about the answer rather than about the state: what an operator has to *keep* in order
/// to give it differs per aggregate, and for a sum it is not a multiset at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agg {
    /// The smallest projection in the group, by [`crate::Value`]'s own total order — which is what
    /// `list_min` compares, so the maintained answer and the recomputed one are the same function
    /// of the same set.
    Min,
    /// The largest, and it costs what [`Agg::Min`] costs.
    Max,
    /// The total of the group's projections — `list_sum`, whose answer is `Int` and whose empty
    /// group is `0` rather than `None`, which is what [`Matching::Total`] exists to say.
    ///
    /// **This one keeps no multiset.** A running total moves by `+n` and `-n` as rows arrive and
    /// leave, so the group's *distinct values* are not a thing it has to know, and the probe is
    /// `O(1)` rather than the `O(distinct)` a sum derived from [`Agg::Min`]'s tree would cost. The
    /// accumulator is wider than the answer for `list_sum`'s reason: the sum is exact and the
    /// failure is a property of the total, not of the way there.
    Sum,
}

impl Agg {
    /// The name that reaches a sharing key and `beck explain query`.
    pub fn name(self) -> &'static str {
        match self {
            Agg::Min => "min",
            Agg::Max => "max",
            Agg::Sum => "sum",
        }
    }
}

/// What one probe of a join's right side returns.
///
/// The two are not a detail of the index: they are what the expression the join replaced evaluated
/// to, so a join that returned the wrong one would render a different page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matching {
    /// An [`Op::MapValues`] index, whose keys are unique. `Some(row)` or `None` — which is what
    /// `map_get` returned and what its callers `match` on.
    Unique,
    /// An [`Op::ArrangeBy`] index, whose keys are not. The whole group, in the indexed
    /// collection's own order, as a `list` — which is what the `filter_list` returned.
    Group,
    /// The same index, asked **how many** rather than which — `list_len` over the same
    /// `filter_list`, whose answer is an `Int`.
    ///
    /// [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's
    /// first aggregate, and the one the language already had a spelling for. The join keeps a count
    /// per key beside its reverse index and moves it by ±1 as the index moves, so the answer costs
    /// nothing and **no group is built**. That is the whole difference from [`Matching::Group`],
    /// which pays the group's size on every event that touches it.
    Count,
    /// The same one-entry-per-group index [`Matching::Unique`] probes, read as a **value rather
    /// than an option**: [`Op::GroupBy`] with [`Agg::Sum`], whose answer is an `Int`.
    ///
    /// A key with no entry is a group with no rows, and the sum of no numbers is `0` — where
    /// `list_min` of the same empty group is `None`. So the two probes differ in exactly one place
    /// and it is what a missing entry *means*, which is a property of the aggregate rather than of
    /// the index.
    ///
    /// The third answer is the one worth naming: an entry that is not an `Int` — `None`, as
    /// [`Op::GroupBy`] publishes it — is a group whose total does not fit one, and probing it
    /// raises what `list_sum` raises. It is published rather
    /// than raised at maintenance time because **a group nobody asks about must not fail a render**
    /// — the recompute only ever sums the groups the loop reaches, and a maintained plan that
    /// failed on the others would disagree with it about whether the program failed at all.
    Total,
}

/// Every operator the engine implements, by name.
///
/// Published for the same reason [`crate::fuse::RULES`] is: `fusion.rs` holds this set to the
/// operators the programs in the tree actually compile to, so an operator with a delta rule and no
/// program is a hole in the differential rather than a line in a match.
pub const OPERATORS: &[&str] = &[
    "state",
    "session",
    "presence",
    "awareness",
    "const",
    "recompute",
    "map_values",
    "map_list",
    "filter_list",
    "sort_by",
    "concat_lists",
    "flatten",
    "flat_map",
    "list_len",
    "list_is_empty",
    "join",
    "arrange_by",
    "group_by",
    "semi_join",
    "anti_join",
    "distinct",
];

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::State => "state",
            Op::Session => "session",
            Op::Presence => "presence",
            Op::Awareness => "awareness",
            Op::Const => "const",
            Op::Pointwise { .. } => "recompute",
            Op::MapValues => "map_values",
            Op::MapList { .. } => "map_list",
            Op::FilterList { .. } => "filter_list",
            Op::SortBy { .. } => "sort_by",
            Op::Concat => "concat_lists",
            Op::Flatten => "flatten",
            Op::FlatMap { .. } => "flat_map",
            Op::Count => "list_len",
            Op::IsEmpty => "list_is_empty",
            Op::Join { .. } => "join",
            Op::ArrangeBy { .. } => "arrange_by",
            Op::GroupBy { .. } => "group_by",
            // Two names for one variant, because which of them a plan holds is the difference
            // between a page showing what is outstanding and one showing what is settled, and a
            // reader of `beck explain query` should not have to open the key line to find out.
            Op::Restrict {
                keep: Presence::In, ..
            } => "semi_join",
            Op::Restrict {
                keep: Presence::NotIn,
                ..
            } => "anti_join",
            Op::Distinct => "distinct",
        }
    }

    /// Whether this operator is maintained by delta rather than recomputed.
    pub fn maintained(&self) -> bool {
        matches!(
            self,
            Op::MapValues
                | Op::MapList { .. }
                | Op::FilterList { .. }
                | Op::SortBy { .. }
                | Op::Concat
                | Op::Flatten
                | Op::FlatMap { .. }
                | Op::Count
                | Op::IsEmpty
                | Op::Join { .. }
                | Op::ArrangeBy { .. }
                | Op::GroupBy { .. }
                | Op::Restrict { .. }
                | Op::Distinct
        )
    }

    /// Whether this is an input to the dataflow rather than a step in it.
    pub fn is_source(&self) -> bool {
        matches!(
            self,
            Op::State | Op::Session | Op::Presence | Op::Awareness | Op::Const
        )
    }

    /// What orders this operator's arrangement — the table in this module's own documentation, as
    /// a sentence, so `beck explain query` states the thing that makes the output *order* a
    /// consequence of the plan rather than of a sort at the end.
    pub fn key(&self) -> &'static str {
        match self {
            Op::State | Op::Session | Op::Presence | Op::Awareness | Op::Const => "a source",
            Op::Pointwise { .. } | Op::Count | Op::IsEmpty => "a value, not an arrangement",
            Op::MapValues => "the map's key",
            Op::MapList { .. } | Op::FilterList { .. } => "the input's key, unchanged",
            Op::SortBy { .. } => "the sort key, then the input's key — a stable sort as an order",
            Op::Concat => "which input, then that input's key",
            Op::Flatten | Op::FlatMap { .. } => {
                "the input's key, then the position inside its list"
            }
            // §99.5 decision 1 asks for the left key followed by the right key. A lookup matches at
            // most one right row, so the right component is determined by the left one and adding
            // it would only make an unmatched row's key shorter than a matched one's. The rule and
            // this are the same rule: iteration is left-order-major, which is what a `for` over the
            // left side already means.
            Op::Join { .. } => "the left input's key — left-order-major, as the loop was",
            // The same two components `sort_by` has, and the second is load-bearing for a
            // different reason: it is what makes a group come back in the order the collection
            // held it, which is the order the `filter_list` this replaced returned.
            Op::ArrangeBy { .. } => {
                "the key it indexes by, then the input's key — one range per key"
            }
            // One component and no second, which is the difference from `arrange_by` above: this
            // holds one entry per *group* rather than one per row, so a probe is a point lookup
            // and the collection's own order never reaches the output.
            Op::GroupBy { .. } => "the group's key — one entry per group, and no row",
            // The input's key, exactly as `filter_list` above — which is the point of the operator
            // rather than a coincidence: it keeps and drops the left collection's own elements, so
            // nothing of the index's order reaches the output.
            Op::Restrict { .. } => {
                "the input's key, unchanged — the index decides which, not where"
            }
            // A key of the input, as `filter_list` above — but *which* one is the operator's own
            // answer rather than the program's, and it is what makes the output's order the order
            // the values were first seen in.
            Op::Distinct => "the input's key of each value's first occurrence",
        }
    }

    /// Whether this operator's output is an arrangement rather than a value.
    pub fn is_arrangement(&self) -> bool {
        matches!(
            self,
            Op::MapValues
                | Op::MapList { .. }
                | Op::FilterList { .. }
                | Op::SortBy { .. }
                | Op::Concat
                | Op::Flatten
                | Op::FlatMap { .. }
                | Op::Join { .. }
                | Op::ArrangeBy { .. }
                | Op::GroupBy { .. }
                | Op::Restrict { .. }
                | Op::Distinct
        )
    }

    /// Every per-element function this operator carries, whatever each is applied to.
    ///
    /// One accessor rather than the five-way `if let` that was written out at each of the four
    /// places that remap captures: a new operator with a [`Fun`] missed at one of them would be a
    /// capture the plan never renumbered, which is a wrong `OpId` rather than a compile error.
    ///
    /// It returns a *list* because [`Op::GroupBy`] carries two, and an accessor that returned the
    /// first would reintroduce exactly the defect the paragraph above describes — silently, for
    /// the second one only.
    pub fn funs_mut(&mut self) -> Vec<&mut Fun> {
        match self {
            Op::MapList { f }
            | Op::FilterList { f }
            | Op::SortBy { f }
            | Op::FlatMap { f }
            | Op::Join { key: f, .. }
            | Op::ArrangeBy { key: f }
            | Op::Restrict { key: f, .. } => vec![f],
            Op::GroupBy { key, of, .. } => vec![key, of],
            _ => Vec::new(),
        }
    }

    /// The same functions, borrowed. Order is the order the engine prepares them in.
    pub fn funs(&self) -> Vec<&Fun> {
        match self {
            Op::MapList { f }
            | Op::FilterList { f }
            | Op::SortBy { f }
            | Op::FlatMap { f }
            | Op::Join { key: f, .. }
            | Op::ArrangeBy { key: f }
            | Op::Restrict { key: f, .. } => vec![f],
            Op::GroupBy { key, of, .. } => vec![key, of],
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub op: Op,
    pub inputs: Vec<OpId>,
    /// Set when this operator is a fallback: which construct had no delta rule.
    pub because: Option<String>,
    /// Set on a loop whose body looks a collection up and which [`crate::relate`] would not read as
    /// a join: which of its conditions failed.
    ///
    /// [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.6's rule for
    /// the shape inference cannot see — "compile it the slow way and *say so*". It is separate from
    /// [`Node::because`] because this operator is not a fallback: it has a delta rule and applies
    /// it, and what it is missing is the *index* that would stop it applying it to everything.
    pub relate: Option<String>,
    /// True when this node reads the session, directly or through an input. §5.3's boundary: the
    /// nodes for which this is false are the shared dataflow, the rest run per subscriber.
    pub per_session: bool,
    /// How many operators read this one. Two or more is §5.3's shared prefix, at the granularity
    /// the engine actually shares at.
    pub consumers: usize,
}

/// The view as a dataflow.
///
/// Nodes are in dependency order — every input's index is less than its consumer's — so the engine
/// is one forward pass with no scheduling.
#[derive(Clone, Debug)]
pub struct Plan {
    pub nodes: Vec<Node>,
    /// Constants, in the same index space as `nodes`, for the ones whose op is [`Op::Const`].
    pub constants: BTreeMap<OpId, Core>,
    pub root: OpId,
    pub state: OpId,
    pub session: OpId,
    pub presence: OpId,
    pub awareness: OpId,
    /// The declared signals that survived as nodes, so a report can use the program's own names.
    pub signals: Vec<(Arc<str>, OpId)>,
}

impl Plan {
    /// How many *operators* are maintained by delta, and how many are recomputed.
    ///
    /// Sources and constants are neither: the accumulator, the session and a string literal are
    /// inputs to the dataflow rather than steps in it, and counting them as "recomputed" would
    /// make every program look worse than it is by a fixed amount.
    pub fn counts(&self) -> (usize, usize) {
        let operators = self.nodes.iter().filter(|n| !n.op.is_source());
        let maintained = operators.clone().filter(|n| n.op.maintained()).count();
        (maintained, operators.count() - maintained)
    }

    /// The operators whose per-element function captured something that moves **on every event**,
    /// so the whole collection is reapplied whenever anything happens.
    ///
    /// This is the defect [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md)
    /// §99.3 found by sweeping the tree by hand: a loop body that reads the accumulator is a
    /// *different function* after every event, so §23.13's rebuild rule reapplies it to every
    /// element — a nested-loop join with no index, invisible until the collection is large. The
    /// operators of §99.9 remove it where the shape can be recognised, and §99.6's rule for the
    /// shape that cannot is "compile it the slow way and say so", which is
    /// [`Node::because`].
    ///
    /// Published so the sweep can be a **standing property rather than a thing somebody re-runs**.
    /// It was re-run by hand three times, and the third time found a site that had arrived one
    /// change after the second and been missed — the figure in the document was stale because the
    /// tree had grown under it ([`docs/08`](../../../../../docs/08-roadmap.md) §8.5.6's third decay
    /// direction). `incremental.rs::no_program_in_the_tree_reapplies_a_collection_per_event` is
    /// what re-runs it now.
    ///
    /// A **per-subscription** capture is not this and is not returned: a function that captured the
    /// session is reapplied when a subscriber navigates, which is a route change rather than an
    /// event, and calling the two the same thing is what made the hand sweep hard to read.
    pub fn reapplied_per_event(&self) -> Vec<OpId> {
        captured_per_node(self)
            .into_iter()
            .enumerate()
            .filter(|(_, c)| {
                c.as_ref()
                    .is_some_and(|(_, cadence)| *cadence == Cadence::PerEvent)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The nodes that do not read the session: §5.3's shared dataflow.
    pub fn shared(&self) -> Vec<OpId> {
        (0..self.nodes.len())
            .filter(|&i| !self.nodes[i].per_session)
            .collect()
    }

    /// Compile the view of a sliced program, and fuse it.
    ///
    /// Everything downstream — the engine, the read models, both reports — reads the *fused* plan,
    /// so there is one plan a program has rather than two that could disagree.
    /// [`Plan::unfused`] is what the differential gate compares against.
    pub fn compile(placed: &Placed) -> Plan {
        Plan::compile_with(placed, Relate::default())
    }

    /// The same, with [`Relate`] said out loud.
    pub fn compile_with(placed: &Placed, relate: Relate) -> Plan {
        crate::fuse::fuse(Plan::unfused_with(placed, relate)).0
    }

    /// A plan for one expression over collections the caller supplies — the read model's SQL,
    /// compiled into the operators a program's view compiles to
    /// ([`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 9).
    ///
    /// `tables` names the fields of the record the engine is handed as its **state**: the `i`th of
    /// them holds the `i`th table's rows, and the expression reads it as `Var(i)`. That is the whole
    /// of the arrangement — a query has no session, no presence and no accumulator of its own, so
    /// the one source a view already has is the one a query uses too, and no operator here is new.
    ///
    /// The expression is *built* rather than written, so it carries no [`CoreKind::Global`] and
    /// there is nothing for a definition table to answer; the decomposition is given an empty table. The
    /// consequence worth stating is that a query is held to exactly the recognitions a program is:
    /// [`crate::relate`] reads the loop and emits the [`Op::Join`], the [`Op::ArrangeBy`] and the
    /// [`Op::GroupBy`], so a `join` in SQL and a `for` loop that looks something up are the same
    /// operators with the same delta rules and not two implementations that agree.
    pub fn of_query(tables: &[Arc<str>], body: &Core) -> Plan {
        Plan::of_query_with(tables, body, Relate::default())
    }

    /// The same, with [`Relate`] said out loud — which is what lets a gate measure both settings.
    pub fn of_query_with(tables: &[Arc<str>], body: &Core, relate: Relate) -> Plan {
        let defs = BTreeMap::new();
        let mut b = Builder {
            defs: &defs,
            relate,
            nodes: Vec::new(),
            constants: BTreeMap::new(),
            cse: BTreeMap::new(),
            inlining: Vec::new(),
            states: &[],
            state: 0,
            session: 0,
            presence: 0,
            awareness: 0,
            vertices: BTreeMap::new(),
        };
        b.state = b.push(Op::State, Vec::new(), None);
        b.session = b.push(Op::Session, Vec::new(), None);
        b.presence = b.push(Op::Presence, Vec::new(), None);
        b.awareness = b.push(Op::Awareness, Vec::new(), None);

        let state = b.state;
        let mut scope = Scope::new();
        for (i, name) in tables.iter().enumerate() {
            let code = lam(
                vec![0],
                Core {
                    kind: CoreKind::Field {
                        base: Box::new(var(0, Ty::unit(), beck_diag::Span::NONE)),
                        name: name.clone(),
                    },
                    ty: Ty::unit(),
                    tier: Tier::Any,
                    span: beck_diag::Span::NONE,
                    last_use: false,
                    order: crate::fields::UNORDERED,
                    locals: 0,
                },
            );
            let id = b.shared(
                format!("field/{name}/{state}"),
                Op::Pointwise { code },
                vec![state],
                None,
            );
            scope.insert(i as VarId, id);
        }
        let root = b.expr(body, &scope);

        let mut plan = Plan {
            nodes: b.nodes,
            constants: b.constants,
            root,
            state: b.state,
            session: b.session,
            presence: b.presence,
            awareness: b.awareness,
            signals: Vec::new(),
        };
        plan.finish();
        plan.prune();
        crate::fuse::fuse(plan).0
    }

    /// The plan as the decomposition produced it, before [`crate::fuse`] rewrites it.
    ///
    /// Works from the *graph* rather than from [`crate::split::Roles::view`], for the reason
    /// [`docs/23`](../../../../../docs/23-incremental-views-report.md) built the graph in the first place: the
    /// sliced expression has already lost which signal each part came from, and a plan whose nodes
    /// cannot be named is a plan no report can explain.
    pub fn unfused(placed: &Placed) -> Plan {
        Plan::unfused_with(placed, Relate::default())
    }

    /// The same, with [`Relate`] said out loud.
    pub fn unfused_with(placed: &Placed, relate: Relate) -> Plan {
        let graph = &placed.graph;
        let mut b = Builder {
            defs: &placed.program.defs,
            relate,
            nodes: Vec::new(),
            constants: BTreeMap::new(),
            cse: BTreeMap::new(),
            inlining: Vec::new(),
            states: &placed.roles.states,
            state: 0,
            session: 0,
            presence: 0,
            awareness: 0,
            vertices: BTreeMap::new(),
        };
        b.state = b.push(Op::State, Vec::new(), None);
        b.session = b.push(Op::Session, Vec::new(), None);
        b.presence = b.push(Op::Presence, Vec::new(), None);
        b.awareness = b.push(Op::Awareness, Vec::new(), None);

        let root = match graph.by_name.get(&placed.roles.page_name).copied() {
            Some(page) if page < graph.nodes.len() => b.vertex(graph, page),
            // A **library** has no page, and an empty graph to look it up in. Its plan is its two
            // sources and a unit — nothing renders it, and the `unwrap_or(0)` this replaced indexed
            // vertex zero of a graph with no vertices (docs/27 §27.2).
            _ => {
                let id = b.push(Op::Const, Vec::new(), None);
                b.constants.insert(
                    id,
                    Core {
                        kind: CoreKind::Const(crate::core::Const::Unit),
                        ty: Ty::unit(),
                        tier: Tier::Any,
                        span: beck_diag::Span::NONE,
                        last_use: false,
                        order: crate::fields::UNORDERED,
                        locals: 0,
                    },
                );
                id
            }
        };

        let mut signals: Vec<(Arc<str>, OpId)> = Vec::new();
        for (&sig, &id) in &b.vertices {
            if let Some(name) = &graph.node(sig).name {
                signals.push((name.clone(), id));
            }
        }
        signals.sort();

        let mut plan = Plan {
            nodes: b.nodes,
            constants: b.constants,
            root,
            state: b.state,
            session: b.session,
            presence: b.presence,
            awareness: b.awareness,
            signals,
        };
        plan.finish();
        plan.prune();
        plan
    }

    /// Propagate `per_session` forward and count consumers.
    pub(crate) fn finish(&mut self) {
        for node in &mut self.nodes {
            node.per_session = false;
            node.consumers = 0;
        }
        for i in 0..self.nodes.len() {
            let per = matches!(self.nodes[i].op, Op::Session | Op::Presence | Op::Awareness)
                || self.nodes[i]
                    .inputs
                    .iter()
                    .any(|&j| self.nodes[j].per_session);
            self.nodes[i].per_session = per;
            let captured: Vec<OpId> = self.nodes[i]
                .op
                .funs()
                .iter()
                .flat_map(|f| f.captures.iter().copied())
                .collect();
            if captured.iter().any(|&j| self.nodes[j].per_session) {
                self.nodes[i].per_session = true;
            }
        }
        for i in 0..self.nodes.len() {
            for j in self.dependencies(i) {
                self.nodes[j].consumers += 1;
            }
        }
    }

    /// Drop every operator the plan's roots cannot reach, renumber, and recompute what
    /// [`Plan::finish`] computes. Returns the old-to-new map.
    ///
    /// The decomposition builds an operator for every argument of a call it inlines and for every
    /// `let`'s value, before it knows whether the body reads them — and a bounded call's arguments
    /// include one dictionary per method of each bound ([`27`](../../../../../docs/27-the-walls-come-down-report.md)),
    /// of which the body may use none. Deciding that lazily would mean a scope of thunks rather
    /// than of operators, which changes the order operators are created in and therefore what
    /// hash-consing shares; pruning afterwards costs one pass and changes nothing else.
    ///
    /// The roots are the page, the two sources, and every **named** signal — a name is projected as
    /// a read-model table ([`docs/23`](../../../../../docs/23-incremental-views-report.md)), so it
    /// keeps its operator alive whether or not the page reads it.
    pub(crate) fn prune(&mut self) -> BTreeMap<OpId, OpId> {
        let mut live = vec![false; self.nodes.len()];
        let mut stack = vec![
            self.root,
            self.state,
            self.session,
            self.presence,
            self.awareness,
        ];
        stack.extend(self.signals.iter().map(|(_, id)| *id));
        while let Some(id) = stack.pop() {
            if std::mem::replace(&mut live[id], true) {
                continue;
            }
            stack.extend(self.dependencies(id));
        }

        let mut map = BTreeMap::new();
        let mut next = 0;
        for (i, &keep) in live.iter().enumerate() {
            if keep {
                map.insert(i, next);
                next += 1;
            }
        }
        // Dependency order survives renumbering because it is monotone: an input's index was below
        // its consumer's, and a monotone map keeps it there.
        let mut nodes = Vec::with_capacity(next);
        for (i, node) in std::mem::take(&mut self.nodes).into_iter().enumerate() {
            if !live[i] {
                continue;
            }
            let mut node = node;
            node.inputs.iter_mut().for_each(|id| *id = map[id]);
            for f in node.op.funs_mut() {
                f.captures.iter_mut().for_each(|id| *id = map[id]);
            }
            nodes.push(node);
        }
        self.nodes = nodes;
        self.constants = std::mem::take(&mut self.constants)
            .into_iter()
            .filter_map(|(id, c)| map.get(&id).map(|&n| (n, c)))
            .collect();
        self.signals.iter_mut().for_each(|(_, id)| *id = map[&*id]);
        self.root = map[&self.root];
        self.state = map[&self.state];
        self.session = map[&self.session];
        self.presence = map[&self.presence];
        self.awareness = map[&self.awareness];
        self.finish();
        map
    }

    /// Every node an operator reads, including the ones its per-element function captured.
    pub fn dependencies(&self, i: OpId) -> Vec<OpId> {
        let mut out = self.nodes[i].inputs.clone();
        for f in self.nodes[i].op.funs() {
            out.extend(f.captures.iter().copied());
        }
        out
    }

    /// The names this plan gives one operator, if any — a declared signal is a name a developer
    /// wrote, and a report that can use it should.
    pub fn names_of(&self, i: OpId) -> Vec<&str> {
        self.signals
            .iter()
            .filter(|(_, id)| *id == i)
            .map(|(n, _)| n.as_ref())
            .collect()
    }
}

/// `beck explain query` — the view as a dataflow plan, operator by operator.
///
/// [`04`](../../../../../docs/04-compiler-architecture.md) §4.7 asks for this command and
/// [`20`](../../../../../docs/20-phase-2-report.md) §20.5 says why it could not exist: "the `Query`
/// sub-language is deliberately symbolic and there is no plan to explain". There is one now, and
/// this prints it — including the two things a developer cannot see any other way: which operator
/// **orders** the output, and which side of §5.3's session cut each one is on.
pub fn query_report(plan: &Plan) -> String {
    query_report_of(plan, "the page")
}

/// The same report over a plan whose root is not a page.
///
/// A `select` compiles to a plan too ([`crate::query`]), and its root is a table of rows rather
/// than a rendering — which is the one sentence of this report that would otherwise be false.
pub fn query_report_of(plan: &Plan, root: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "the view as a dataflow plan (§5.3). Operators are in dependency order, so every input\n\
         is above its consumer and the engine is one forward pass.\n"
    );
    for (i, node) in plan.nodes.iter().enumerate() {
        let deps = plan.dependencies(i);
        let reads = if deps.is_empty() {
            String::new()
        } else {
            format!(
                "← {}",
                deps.iter()
                    .map(|d| format!("#{d}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        let names = plan.names_of(i);
        let _ = writeln!(
            out,
            "  #{:<3} {:<14} {:<12} {:<12} {}",
            i,
            node.op.name(),
            reads,
            if node.per_session {
                "per session"
            } else {
                "shared"
            },
            match (node.consumers, names.is_empty()) {
                (_, false) => format!("`{}`", names.join("`, `")),
                (0, true) => String::new(),
                (1, true) => String::new(),
                (n, true) => format!("read by {n}"),
            }
        );
        if node.op.is_arrangement() {
            let _ = writeln!(out, "       {:<14} ordered by {}", "", node.op.key());
        }
        if let Some(why) = &node.because {
            let _ = writeln!(out, "       {:<14} recomputed: {why}", "");
        }
    }
    let (maintained, recomputed) = plan.counts();
    let arrangements = plan.nodes.iter().filter(|n| n.op.is_arrangement()).count();
    let _ = writeln!(
        out,
        "\n  #{} is the root — {root}.\n\
         \x20 {maintained} maintained, {recomputed} recomputed, {arrangements} of them holding an \
         arrangement.",
        plan.root
    );
    out
}

/// What one operator costs per event, in the units [`crate::engine::Work`] counts.
///
/// `δ` is how many entries moved at its input and `n` how many its input holds. The distinction is
/// the whole point of the engine, so a cost that mentions `n` is a cost worth reading.
fn op_cost(plan: &Plan, i: OpId) -> String {
    let node = &plan.nodes[i];
    // A pointwise operator forces every arrangement it reads into a `Value::List`, which copies
    // that arrangement's entries: docs/23 §23.8's "the page's children are still assembled in
    // full", located at the operator that does it.
    let forced: Vec<OpId> = node
        .inputs
        .iter()
        .copied()
        .filter(|&j| plan.nodes[j].op.is_arrangement())
        .collect();
    match &node.op {
        Op::State | Op::Session | Op::Presence | Op::Awareness => {
            "—  a source, read by reference".to_string()
        }
        Op::Const => "—  evaluated once, when the plan is prepared".to_string(),
        Op::Pointwise { .. } if forced.is_empty() => {
            "1 recompute, and only when an input moved".to_string()
        }
        Op::Pointwise { .. } => format!(
            "1 recompute + n entries copied, forcing {}",
            forced
                .iter()
                .map(|j| format!("#{j}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Op::MapValues => "δ touched  —  O(δ log n), the persistent map's own diff".to_string(),
        Op::MapList { .. } => "δ applications, δ touched".to_string(),
        Op::FilterList { .. } => "δ applications, at most δ touched".to_string(),
        Op::SortBy { .. } => {
            "δ applications, at most 2δ touched — a move is a remove and an insert".to_string()
        }
        Op::Concat => "δ touched".to_string(),
        Op::Flatten => "the entries of each changed element's list".to_string(),
        Op::FlatMap { .. } => {
            "δ applications, then the entries of each changed element's list".to_string()
        }
        Op::Count | Op::IsEmpty => "O(1)  —  the arrangement's size, never a recount".to_string(),
        // Both halves of §99.5's bilinear rule, in one line, because a reader who only sees the
        // first would think an index answers a question and never receives one.
        Op::Join {
            matched: Matching::Unique,
            ..
        } => "δ keys applied on the left, and on the right the rows each moved \
              index entry answers  —  neither is n"
            .to_string(),
        // The honest half of §99.9 item 3, at the operator that pays it: the scan is gone and the
        // group is not. A left row whose group moved is rebuilt whole, because the expression this
        // replaced evaluated to a `list`.
        Op::Join {
            matched: Matching::Group,
            ..
        } => "δ keys applied on the left, and on the right one group rebuilt per key that \
              moved  —  the group, never the collection"
            .to_string(),
        Op::Join {
            matched: Matching::Count,
            ..
        } => "δ keys applied on the left, and on the right ±1 per moved index entry  —  the \
              group is counted, never built"
            .to_string(),
        Op::Join {
            matched: Matching::Total,
            ..
        } => "δ keys applied on the left, and on the right one entry per group whose total \
              moved  —  the group is totalled, never built"
            .to_string(),
        Op::ArrangeBy { .. } => "δ applications, at most 2δ touched — a move is a remove and an \
                                 insert. The probe is the join's cost, not this one's"
            .to_string(),
        // Two applications rather than one — the group's key and the projection — and then one
        // entry per group whose answer *moved*, which is the half worth stating: an event that
        // adds a row behind the extreme changes nothing and nothing downstream of it runs. A
        // `sum` is the one aggregate whose answer moves whenever its group does, so it is the one
        // that never takes that discount.
        Op::GroupBy { agg, .. } => format!(
            "2δ applications, at most δ touched  —  one {} per group, and the group is never built",
            agg.name()
        ),
        // The bilinear rule again, and the right half is the one worth printing: an index entry
        // that moved reaches exactly the rows waiting on its key, and each of those either enters
        // the output or leaves it. Neither side is n.
        Op::Restrict { keep, .. } => format!(
            "δ keys applied on the left, and on the right the rows each moved index entry {}  \
             —  neither is n",
            match keep {
                Presence::In => "admits or withdraws",
                Presence::NotIn => "withdraws or admits",
            }
        ),
        // No applications at all — the operator has no per-element function — and the touched
        // count is what a value's *first occurrence* moving costs: one remove and one insert, on
        // the values that moved rather than on the distinct ones.
        Op::Distinct => {
            "at most 2δ touched, no applications  —  a value whose first occurrence moved is a \
             remove and an insert"
                .to_string()
        }
    }
}

/// How often an operator's value moves, which is what decides whether capturing it costs anything.
///
/// A per-element function that captured another operator is a different function when that
/// operator moves, so the whole collection is reconsidered. Whether that is a defect or a
/// non-event depends entirely on **what it captured**: a constant never moves, a session moves
/// when a subscriber navigates, and anything downstream of the state moves on every event the fold
/// admits. Printing the three the same way is what
/// [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.3 found, and it
/// left a reader tracing inputs back to `#0` by hand — one of the real cases in the corpus is two
/// hops away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cadence {
    /// Constants all the way down: computed once when the plan is prepared.
    Never,
    /// The session or the roster. It moves while a subscription is open, but not with the log.
    PerSubscription,
    /// The state. It moves on every event.
    PerEvent,
}

impl Cadence {
    /// What a capture of something moving this often costs, as the whole clause.
    ///
    /// One sentence per cadence rather than a rate and a reason assembled separately, because the
    /// three differ in what a reader should *do* about them and not only in a frequency.
    fn line(self, captured: &str) -> String {
        match self {
            Cadence::PerEvent => format!(
                "n applications on every event — its function captured {captured}, which is \
                 downstream of the state"
            ),
            Cadence::PerSubscription => format!(
                "n applications when the session moves, which is not per event — its function \
                 captured {captured}"
            ),
            Cadence::Never => {
                format!("no cost per event — its function captured {captured}, which never moves")
            }
        }
    }
}

/// Every operator's [`Cadence`], in one pass.
///
/// One pass is enough because the plan's nodes are in dependency order — every input's index is
/// less than its consumer's, which [`Plan`] states as its invariant — so an input's answer is
/// always already in hand.
fn cadences(plan: &Plan) -> Vec<Cadence> {
    let mut out: Vec<Cadence> = Vec::with_capacity(plan.nodes.len());
    for (i, node) in plan.nodes.iter().enumerate() {
        let own = match node.op {
            Op::State => Cadence::PerEvent,
            Op::Session | Op::Presence | Op::Awareness => Cadence::PerSubscription,
            _ => Cadence::Never,
        };
        let inherited = node
            .inputs
            .iter()
            .map(|&j| {
                debug_assert!(j < i, "the plan's nodes are in dependency order");
                out[j]
            })
            .max()
            .unwrap_or(Cadence::Never);
        out.push(own.max(inherited));
    }
    out
}

/// What each operator's per-element function captured, and how often that moves.
///
/// `None` for an operator that has no per-element function, or whose function captured nothing.
/// The join's key function is deliberately excluded: it captures nothing by construction, and an
/// empty capture line for it would read as a cost.
///
/// **One computation, several readers**, which is [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md)
/// §99.9 item 2's lesson applied a second time: [`cost_report`] prints these, its summary counts
/// them, and [`Plan::reapplied_per_event`] answers a question about them. Three readers deriving
/// the same fact separately is how the tally and the body came to disagree the first time.
fn captured_per_node(plan: &Plan) -> Vec<Option<(Vec<OpId>, Cadence)>> {
    let moves = cadences(plan);
    plan.nodes
        .iter()
        .map(|node| {
            let captures = match &node.op {
                Op::MapList { f } | Op::FilterList { f } | Op::SortBy { f } | Op::FlatMap { f } => {
                    f.captures.clone()
                }
                _ => Vec::new(),
            };
            captures
                .iter()
                .map(|&j| moves[j])
                .max()
                .map(|worst| (captures, worst))
        })
        .collect()
}

/// One operator's line in the report, and the facts the summary is counted from.
///
/// The summary is derived from these rather than recomputed beside them, which is the shape of the
/// defect this replaced: the tally counted one thing and the body printed another, so a program
/// whose loop captured the accumulator was told "1 of 29" when two operators cost `O(n)`.
struct Charge {
    cost: String,
    /// What the operator's per-element function captured, and how often that moves.
    captured: Option<(Vec<OpId>, Cadence)>,
    /// Why this operator costs `O(n)` per event, or `None` when it does not.
    linear: Option<Linear>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Linear {
    /// It forces an arrangement into a list.
    Forced,
    /// Its per-element function captured something that moves with the state.
    Captured,
}

/// `beck explain cost` — what one event costs this view.
///
/// [`20`](../../../../../docs/20-phase-2-report.md) §20.5 left this command unbuilt with a reason
/// rather than a shrug: `beck explain place` already prints every candidate's cost, and "whether a
/// separate `cost` view earns its place is a question for when there is a second cost dimension to
/// show". The plan is that second dimension. Placement costs are about *where* a definition runs,
/// once, at compile time; these are about what the program does *per event*, for as long as it is
/// running, and no placement decision can see them.
pub fn cost_report(plan: &Plan) -> String {
    use std::fmt::Write;
    let mut captures = captured_per_node(plan);
    let charges: Vec<Charge> = (0..plan.nodes.len())
        .map(|i| {
            let cost = op_cost(plan, i);
            let captured = std::mem::take(&mut captures[i]);
            let linear = if cost.contains("n entries copied") {
                Some(Linear::Forced)
            } else if captured
                .as_ref()
                .is_some_and(|(_, c)| *c == Cadence::PerEvent)
            {
                Some(Linear::Captured)
            } else {
                None
            };
            Charge {
                cost,
                captured,
                linear,
            }
        })
        .collect();

    let mut out = String::new();
    let _ = writeln!(
        out,
        "what one event costs this view, in the units the engine counts (§3.8).\n\
         \x20 δ is how many entries moved; n is how many the collection holds.\n"
    );
    for (i, charge) in charges.iter().enumerate() {
        let _ = writeln!(
            out,
            "  #{:<3} {:<14} {}",
            i,
            plan.nodes[i].op.name(),
            charge.cost
        );
        // An operator whose per-element function reads another operator is a different function
        // when that one moves, so the whole collection is reconsidered. Saying *what it captured*
        // is not enough — a reader needs to know how often that thing moves, which is the
        // difference between a non-event and the most expensive line in the report.
        if let Some((captured, cadence)) = &charge.captured {
            let names = captured
                .iter()
                .map(|j| format!("#{j}"))
                .collect::<Vec<_>>()
                .join(" or ");
            let _ = writeln!(out, "       {:<14} {}", "", cadence.line(&names));
            // Only under a per-event capture, which is the one cadence a join would have removed.
            // Under a captured `const` the sentence would be true and pointless.
            if *cadence == Cadence::PerEvent {
                if let Some(why) = &plan.nodes[i].relate {
                    let _ = writeln!(
                        out,
                        "       {:<14} not read as a relational operator (docs/99 §99.6): {why}",
                        ""
                    );
                }
            }
        }
    }
    let _ = writeln!(out);

    let of = |which: Linear| -> Vec<String> {
        charges
            .iter()
            .enumerate()
            .filter(|(_, c)| c.linear == Some(which))
            .map(|(i, _)| format!("#{i}"))
            .collect()
    };
    let (forced, captured) = (of(Linear::Forced), of(Linear::Captured));
    let total = forced.len() + captured.len();
    if total == 0 {
        let _ = writeln!(
            out,
            "  Nothing here is proportional to the collection: no operator forces an arrangement\n\
             \x20 into a list, and no per-element function captured anything that moves with the\n\
             \x20 state, so one event costs what the event changed."
        );
    } else {
        // Two reasons, counted together and named apart. They are not the same defect and they do
        // not have the same fix: one is a constant factor of the arrangement's representation, and
        // the other is a program that escaped the view algebra.
        let _ = writeln!(
            out,
            "  {total} of {} operators cost O(n) per event, for {} reason{}:",
            plan.nodes.len(),
            if forced.is_empty() || captured.is_empty() {
                "one"
            } else {
                "two"
            },
            if forced.is_empty() || captured.is_empty() {
                ""
            } else {
                "s"
            }
        );
        if !forced.is_empty() {
            let _ = writeln!(
                out,
                "    {}  a recompute needs a `list` and an arrangement is a keyed collection —\n\
                 \x20        docs/23 §23.8's remaining constant factor.",
                forced.join(" ")
            );
        }
        if !captured.is_empty() {
            let _ = writeln!(
                out,
                "    {}  a per-element function captured the state, so the whole collection is\n\
                 \x20        reconsidered on every event — docs/99 §99.3. `beck explain query`\n\
                 \x20        says whether the algebra has an operator this shape was not read as.",
                captured.join(" ")
            );
        }
    }
    let _ = writeln!(
        out,
        "\n  These are the plan's arithmetic rather than a measurement: `Work` is what\n\
         \x20 `Engine::render` counts, so `measure_incremental` checks this arithmetic against the\n\
         \x20 count rather than against a clock. What an operator does *inside* a per-element\n\
         \x20 function is not in these lines and is in `Work::steps`, which is what the backend\n\
         \x20 executed — the number that tells an opaque operator's cost from its arity."
    );
    out
}

struct Builder<'a> {
    /// The definitions a call may be inlined from. The whole `Program` was never read for
    /// anything else, and a query compiled by [`Plan::of_query`] has none — its expression was
    /// built rather than written, so it carries no [`CoreKind::Global`] to resolve.
    defs: &'a BTreeMap<Arc<str>, Def>,
    relate: Relate,
    nodes: Vec<Node>,
    constants: BTreeMap<OpId, Core>,
    /// Structural hash-consing, so `state.todos` read from two places is one operator with two
    /// consumers rather than two operators. That is the fact §5.3's arrangement sharing is about,
    /// and it has to be a property of the plan before it can be a property of the engine.
    cse: BTreeMap<String, OpId>,
    /// Definitions currently being inlined, so a recursive one falls back rather than looping.
    inlining: Vec<Arc<str>>,
    states: &'a [StateRole],
    state: OpId,
    session: OpId,
    presence: OpId,
    awareness: OpId,
    vertices: BTreeMap<SigId, OpId>,
}

/// The symbolic environment: a program variable, and the operator that produces its value.
type Scope = BTreeMap<VarId, OpId>;

impl Builder<'_> {
    fn push(&mut self, op: Op, inputs: Vec<OpId>, because: Option<String>) -> OpId {
        self.nodes.push(Node {
            op,
            inputs,
            because,
            relate: None,
            per_session: false,
            consumers: 0,
        });
        self.nodes.len() - 1
    }

    /// Push, or reuse an identical operator already in the plan.
    fn shared(&mut self, key: String, op: Op, inputs: Vec<OpId>, because: Option<String>) -> OpId {
        if let Some(&id) = self.cse.get(&key) {
            return id;
        }
        let id = self.push(op, inputs, because);
        self.cse.insert(key, id);
        id
    }

    // ---------------------------------------------------------------------------------------
    // The graph
    // ---------------------------------------------------------------------------------------

    /// The operator one signal vertex's value comes from.
    fn vertex(&mut self, graph: &Graph, id: SigId) -> OpId {
        let id = follow_alias(graph, id);
        if let Some(&done) = self.vertices.get(&id) {
            return done;
        }
        let node = graph.node(id);
        // A durable accumulator is where the plan's sources are: the state parameter, or the field
        // of it that this fold occupies when several were fused.
        if let Some(role) = self.states.iter().find(|s| s.node == id) {
            let out = match &role.field {
                None => self.state,
                Some(f) => {
                    let code = lam(
                        vec![0],
                        Core {
                            kind: CoreKind::Field {
                                base: Box::new(var(0, Ty::unit(), node.span)),
                                name: f.clone(),
                            },
                            ty: role.ty.clone(),
                            tier: Tier::Any,
                            span: node.span,
                            last_use: false,
                            order: crate::fields::UNORDERED,
                            locals: 0,
                        },
                    );
                    let state = self.state;
                    self.shared(
                        format!("field/{f}/{state}"),
                        Op::Pointwise { code },
                        vec![state],
                        None,
                    )
                }
            };
            self.vertices.insert(id, out);
            return out;
        }

        let out = match &node.op {
            SigOp::Map { f } => {
                let input = self.vertex(graph, node.inputs[0]);
                self.apply(
                    f,
                    vec![input],
                    &Scope::new(),
                    signal_elem(&node.ty),
                    node.span,
                )
            }
            SigOp::Map2 { f } => {
                let a = self.vertex(graph, node.inputs[0]);
                let b = self.vertex(graph, node.inputs[1]);
                self.apply(
                    f,
                    vec![a, b],
                    &Scope::new(),
                    signal_elem(&node.ty),
                    node.span,
                )
            }
            SigOp::Presence => self.presence,
            // Like presence, and for the same reason: what `f` is applied to is every *other*
            // subscriber's session, which this dataflow does not hold. The runtime does
            // (`beck_rt::awareness`), and hands the answer in as a source.
            SigOp::Awareness { .. } => self.awareness,
            // Not a source: a **constant**, and that is the whole statement this plan makes about
            // freshness. A plan is what the *server* renders through, and a server renders the
            // state it has recorded — so `freshness()` here is `Confirmed` and never moves. The
            // engine therefore treats it as it treats a string literal: evaluated once when the
            // plan is prepared, and never a reason to recompute anything below it.
            //
            // A page that branched on it would be refused Mode A before reaching here
            // (`crate::render`, `B0518`); what does reach here is the SSR of a Mode B page, whose
            // first paint is by construction the confirmed one.
            SigOp::Freshness => {
                let id = self.push(Op::Const, Vec::new(), None);
                self.constants.insert(
                    id,
                    Core {
                        kind: CoreKind::Make {
                            ty: Arc::from("Freshness"),
                            variant: Some(Arc::from("Confirmed")),
                            fields: Vec::new(),
                        },
                        ty: Ty::con("Freshness"),
                        tier: Tier::Any,
                        span: node.span,
                        last_use: false,
                        order: crate::fields::UNORDERED,
                        locals: 0,
                    },
                );
                id
            }
            // A constant, for `SigOp::Freshness`'s reason stated about the other client-held fact.
            // A plan is what the *server* renders through and a server has received no gestures, so
            // the accumulator here is `init` and never moves — which is not an approximation but
            // the right answer: before any gesture, the interface state *is* its initial value.
            //
            // A page that reads one is refused Mode A (`crate::render`, `B0522`), so what reaches
            // here is the SSR of a Mode B page, whose first paint is by construction the one with
            // no gesture applied. The client's kernel holds the accumulator from then on.
            SigOp::Gestures { init, .. } => {
                let id = self.push(Op::Const, Vec::new(), None);
                self.constants.insert(id, init.clone());
                id
            }
            SigOp::PerSession { f } => {
                let input = self.vertex(graph, node.inputs[0]);
                let session = self.session;
                self.apply(
                    f,
                    vec![input, session],
                    &Scope::new(),
                    signal_elem(&node.ty),
                    node.span,
                )
            }
            // The slicer has already refused every other op before a plan is asked for — a stream
            // under a view, a fold that is not durable, a cycle with no fold. Reaching one here
            // would be a plan compiled for a program that did not slice, so it becomes an opaque
            // node rather than a panic: the engine recomputes and the report says why.
            other => self.push(
                Op::Pointwise {
                    code: lam(vec![0], var(0, Ty::unit(), node.span)),
                },
                vec![self.state],
                Some(format!("`{}` is not a view operator", other.name())),
            ),
        };
        self.vertices.insert(id, out);
        out
    }

    /// `f(args…)`, symbolically: inline the function and decompose its body.
    ///
    /// `scope` is the caller's, needed only for the fallback: a function expression this analysis
    /// cannot see into may still *read* variables the plan has operators for, and an opaque node
    /// has to take them as inputs rather than leave them unbound.
    fn apply(
        &mut self,
        f: &Core,
        args: Vec<OpId>,
        scope: &Scope,
        ty: Ty,
        span: beck_diag::Span,
    ) -> OpId {
        let Some((params, body)) = self.as_lambda(f) else {
            return self.opaque_call(
                f,
                args,
                scope,
                ty,
                span,
                "a view applies a function this analysis cannot see into",
            );
        };
        if params.len() != args.len() {
            return self.opaque_call(
                f,
                args,
                scope,
                ty,
                span,
                "a view applies a function to a different number of arguments",
            );
        }
        // The guard goes on here rather than at the call site, because `as_lambda` consults it: a
        // definition may not be inlined into its own body, and pushing the name before resolving it
        // would refuse the outermost call as well as the recursive one.
        let named = match &f.kind {
            CoreKind::Global(n) => {
                self.inlining.push(n.clone());
                true
            }
            _ => false,
        };
        let inner: Scope = params.into_iter().zip(args).collect();
        let out = self.expr(&body, &inner);
        if named {
            self.inlining.pop();
        }
        out
    }

    /// One operator for a call this analysis will not enter, over the arguments *and* whatever the
    /// function expression itself reads.
    fn opaque_call(
        &mut self,
        f: &Core,
        args: Vec<OpId>,
        scope: &Scope,
        ty: Ty,
        span: beck_diag::Span,
        why: &str,
    ) -> OpId {
        let mut free = BTreeSet::new();
        crate::core::free_vars(f, &mut BTreeSet::new(), &mut free);
        let captured: Vec<VarId> = free.into_iter().filter(|v| scope.contains_key(v)).collect();
        let base = captured.iter().copied().max().unwrap_or(0) + 1;
        let ps: Vec<VarId> = (0..args.len() as VarId).map(|i| base + i).collect();
        let call = Core {
            kind: CoreKind::App {
                func: Box::new(f.clone()),
                args: ps.iter().map(|&p| var(p, Ty::unit(), span)).collect(),
            },
            ty,
            tier: Tier::Any,
            span,
            last_use: false,
            order: crate::fields::UNORDERED,
            locals: 0,
        };
        let mut params = captured.clone();
        params.extend(ps);
        let mut inputs: Vec<OpId> = captured.iter().map(|v| scope[v]).collect();
        inputs.extend(args);
        self.push(
            Op::Pointwise {
                code: lam(params, call),
            },
            inputs,
            Some(why.to_string()),
        )
    }

    /// A function expression as parameters and a body, following one level of naming.
    fn as_lambda(&self, f: &Core) -> Option<(Vec<VarId>, Core)> {
        match &f.kind {
            CoreKind::Lam { params, body } => Some((params.to_vec(), (**body).clone())),
            CoreKind::Global(name) if !self.inlining.contains(name) => {
                let def = self.defs.get(name)?;
                match &def.body.kind {
                    CoreKind::Lam { params, body } => Some((params.to_vec(), (**body).clone())),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    // ---------------------------------------------------------------------------------------
    // The expression
    // ---------------------------------------------------------------------------------------

    /// Decompose one expression into operators, in the scope of the variables already bound to
    /// operators.
    fn expr(&mut self, c: &Core, scope: &Scope) -> OpId {
        match &c.kind {
            CoreKind::Var(v) => match scope.get(v) {
                Some(&id) => id,
                // A variable bound by something the decomposition did not enter — a `match` arm's
                // binder reached through a path that should not exist. Opaque rather than wrong.
                None => self.opaque(c, scope, "a variable bound outside the plan"),
            },
            CoreKind::Const(_) => {
                let key = format!("const/{:?}", c.kind);
                let id = self.shared(key, Op::Const, Vec::new(), None);
                self.constants.entry(id).or_insert_with(|| c.clone());
                id
            }
            CoreKind::Let { var, value, body } => {
                let v = self.expr(value, scope);
                let mut inner = scope.clone();
                inner.insert(*var, v);
                self.expr(body, &inner)
            }
            CoreKind::App { func, args } => {
                let ids: Vec<OpId> = args.iter().map(|a| self.expr(a, scope)).collect();
                self.apply(func, ids, scope, c.ty.clone(), c.span)
            }
            CoreKind::Prim { op, args } => self.prim(c, *op, args, scope),
            // The two constructs a delta cannot be pushed through: both pick which computation
            // runs, and a change to the scrutinee can move the answer between arms.
            CoreKind::If { .. } => self.opaque(
                c,
                scope,
                "an `if` picks which computation runs, and a delta can move it between branches",
            ),
            CoreKind::Match { .. } => self.opaque(
                c,
                scope,
                "a `match` on the input picks which computation runs, and a delta can move it \
                 between arms",
            ),
            CoreKind::Lam { .. } => self.opaque(c, scope, "a function used as a value"),
            CoreKind::Global(name) => match self.defs.get(name) {
                Some(def) if !matches!(def.body.kind, CoreKind::Lam { .. }) => {
                    let body = def.body.clone();
                    self.expr(&body, &Scope::new())
                }
                _ => self.opaque(c, scope, "a definition used as a value"),
            },
            // Structural constructors are pointwise: a change at an input is a change at the
            // output, and there is nothing collection-shaped to maintain.
            CoreKind::Make {
                ty,
                variant,
                fields,
            } => {
                let ids: Vec<OpId> = fields.iter().map(|(_, v)| self.expr(v, scope)).collect();
                let ps: Vec<VarId> = (0..fields.len() as VarId).collect();
                let code = lam(
                    ps.clone(),
                    Core {
                        kind: CoreKind::Make {
                            ty: ty.clone(),
                            variant: variant.clone(),
                            fields: fields
                                .iter()
                                .zip(&ps)
                                .map(|((n, f), &p)| (n.clone(), var(p, f.ty.clone(), f.span)))
                                .collect(),
                        },
                        ty: c.ty.clone(),
                        tier: c.tier,
                        span: c.span,
                        last_use: false,
                        // The same field names in the same written order, so the layout the pass
                        // computed for the literal is the layout of the operator that replaces it.
                        order: c.order,
                        locals: 0,
                    },
                );
                let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_ref()).collect();
                let key = format!("make/{ty}/{variant:?}/{names:?}/{ids:?}");
                self.shared(key, Op::Pointwise { code }, ids, None)
            }
            CoreKind::Field { base, name } => {
                let b = self.expr(base, scope);
                let code = lam(
                    vec![0],
                    Core {
                        kind: CoreKind::Field {
                            base: Box::new(var(0, base.ty.clone(), c.span)),
                            name: name.clone(),
                        },
                        ty: c.ty.clone(),
                        tier: c.tier,
                        span: c.span,
                        last_use: false,
                        order: crate::fields::UNORDERED,
                        locals: 0,
                    },
                );
                self.shared(
                    format!("field/{name}/{b}"),
                    Op::Pointwise { code },
                    vec![b],
                    None,
                )
            }
            CoreKind::With { base, fields } => {
                let mut ids = vec![self.expr(base, scope)];
                ids.extend(fields.iter().map(|(_, v)| self.expr(v, scope)));
                let ps: Vec<VarId> = (0..ids.len() as VarId).collect();
                let code = lam(
                    ps.clone(),
                    Core {
                        kind: CoreKind::With {
                            base: Box::new(var(0, base.ty.clone(), c.span)),
                            fields: fields
                                .iter()
                                .zip(&ps[1..])
                                .map(|((n, f), &p)| (n.clone(), var(p, f.ty.clone(), f.span)))
                                .collect(),
                        },
                        ty: c.ty.clone(),
                        tier: c.tier,
                        span: c.span,
                        last_use: false,
                        order: crate::fields::UNORDERED,
                        locals: 0,
                    },
                );
                self.push(Op::Pointwise { code }, ids, None)
            }
            CoreKind::ListLit(items) => {
                let ids: Vec<OpId> = items.iter().map(|i| self.expr(i, scope)).collect();
                let ps: Vec<VarId> = (0..items.len() as VarId).collect();
                let code = lam(
                    ps.clone(),
                    Core {
                        kind: CoreKind::ListLit(
                            items
                                .iter()
                                .zip(&ps)
                                .map(|(i, &p)| var(p, i.ty.clone(), i.span))
                                .collect(),
                        ),
                        ty: c.ty.clone(),
                        tier: c.tier,
                        span: c.span,
                        last_use: false,
                        order: crate::fields::UNORDERED,
                        locals: 0,
                    },
                );
                let key = format!("list/{ids:?}");
                self.shared(key, Op::Pointwise { code }, ids, None)
            }
            CoreKind::MapLit(pairs) => {
                let mut ids = Vec::new();
                for (k, v) in pairs {
                    ids.push(self.expr(k, scope));
                    ids.push(self.expr(v, scope));
                }
                let ps: Vec<VarId> = (0..ids.len() as VarId).collect();
                let code = lam(
                    ps.clone(),
                    Core {
                        kind: CoreKind::MapLit(
                            pairs
                                .iter()
                                .enumerate()
                                .map(|(i, (k, v))| {
                                    (
                                        var(ps[i * 2], k.ty.clone(), k.span),
                                        var(ps[i * 2 + 1], v.ty.clone(), v.span),
                                    )
                                })
                                .collect(),
                        ),
                        ty: c.ty.clone(),
                        tier: c.tier,
                        span: c.span,
                        last_use: false,
                        order: crate::fields::UNORDERED,
                        locals: 0,
                    },
                );
                self.push(Op::Pointwise { code }, ids, None)
            }
        }
    }

    /// A primitive application: a delta operator when there is a rule for it, pointwise otherwise.
    fn prim(&mut self, c: &Core, op: Prim, args: &[Core], scope: &Scope) -> OpId {
        match (op, args.len()) {
            (Prim::MapValues, 1) => {
                let m = self.expr(&args[0], scope);
                self.shared(format!("map_values/{m}"), Op::MapValues, vec![m], None)
            }
            (Prim::MapList, 2) | (Prim::FilterList, 2) | (Prim::SortBy, 2) => {
                let xs = self.expr(&args[0], scope);
                // Only `map_list` becomes a join, and the restriction is about what an arrangement
                // holds rather than about what can be recognised. A join's element is a *row* — the
                // left value and what it matched — and `map_list` is the one of the three that does
                // not keep its element: it stores `f(x)`, which is the same value whether `x`
                // arrived alone or in a row. `filter_list` and `sort_by` store the element itself,
                // so rewriting either would put rows into the collection its consumers read.
                if op == Prim::MapList && self.relate == Relate::Recognise {
                    match self.joined(xs, &args[1], scope) {
                        Ok(id) => return id,
                        Err(why) => {
                            let f = self.fun(&args[1], scope, &args[0].ty);
                            let id = self.push(Op::MapList { f }, vec![xs], None);
                            self.nodes[id].relate = why;
                            return id;
                        }
                    }
                }
                // A `filter_list` keeps its element, so the sentence above is exactly why it gets
                // the *other* binary operator: `Op::Restrict` emits the left element under the left
                // key, which is what this node already published (§99.9 item 7).
                if op == Prim::FilterList && self.relate == Relate::Recognise {
                    match self.restricted(xs, &args[1], scope) {
                        Ok(id) => return id,
                        Err(why) => {
                            let f = self.fun(&args[1], scope, &args[0].ty);
                            let id = self.push(Op::FilterList { f }, vec![xs], None);
                            self.nodes[id].relate = why;
                            return id;
                        }
                    }
                }
                let f = self.fun(&args[1], scope, &args[0].ty);
                let node = match op {
                    Prim::MapList => Op::MapList { f },
                    Prim::FilterList => Op::FilterList { f },
                    _ => Op::SortBy { f },
                };
                self.push(node, vec![xs], None)
            }
            // `concat_lists` takes one argument: a list *of* lists. The `ui:` loop lowering builds
            // it as a literal, which is the shape a union of delta streams needs — and the only
            // shape a plan can enumerate the inputs of.
            (Prim::ConcatLists, 1) => match &args[0].kind {
                CoreKind::ListLit(parts) => {
                    let ids: Vec<OpId> = parts.iter().map(|p| self.expr(p, scope)).collect();
                    self.push(Op::Concat, ids, None)
                }
                // Not a literal: a computed collection whose elements are lists, which is what
                // `for t in todos:` lowers to. That is a flatten, and a flatten has a delta rule —
                // one element's list is replaced, and the rest keep their place because the key
                // says where they are.
                _ => {
                    let xs = self.expr(&args[0], scope);
                    self.shared(format!("flatten/{xs}"), Op::Flatten, vec![xs], None)
                }
            },
            (Prim::ListLen, 1) => {
                let xs = self.expr(&args[0], scope);
                self.shared(format!("count/{xs}"), Op::Count, vec![xs], None)
            }
            // A **lowering** rather than a recognition, which is why it takes no [`Relate`] switch
            // where the join and the difference do: `list_unique` names the operator, so there is
            // no shape being read and no choice being made on the program's behalf. What the
            // primitive bought is exactly that — a fold spelling the same thing is opaque, and
            // `docs/99` §99.9 item 7 is the argument for giving it a name.
            (Prim::ListUnique, 1) => {
                let xs = self.expr(&args[0], scope);
                self.shared(format!("distinct/{xs}"), Op::Distinct, vec![xs], None)
            }
            (Prim::ListIsEmpty, 1) => {
                let xs = self.expr(&args[0], scope);
                self.shared(format!("empty/{xs}"), Op::IsEmpty, vec![xs], None)
            }
            _ => self.pointwise_prim(c, op, args, scope, None),
        }
    }

    fn pointwise_prim(
        &mut self,
        c: &Core,
        op: Prim,
        args: &[Core],
        scope: &Scope,
        because: Option<String>,
    ) -> OpId {
        let ids: Vec<OpId> = args.iter().map(|a| self.expr(a, scope)).collect();
        let ps: Vec<VarId> = (0..args.len() as VarId).collect();
        let code = lam(
            ps.clone(),
            Core {
                kind: CoreKind::Prim {
                    op,
                    args: args
                        .iter()
                        .zip(&ps)
                        .map(|(a, &p)| var(p, a.ty.clone(), a.span))
                        .collect(),
                },
                ty: c.ty.clone(),
                tier: c.tier,
                span: c.span,
                last_use: false,
                order: crate::fields::UNORDERED,
                locals: 0,
            },
        );
        let key = format!("prim/{}/{ids:?}", op.name());
        self.shared(key, Op::Pointwise { code }, ids, because)
    }

    /// `map_list(xs, f)` where `f` looks something up, as a join and a loop over its rows.
    ///
    /// [`docs/99`](../../../../../docs/99-the-data-tier-means-of-combination.md) §99.6: the loop is
    /// not edited and no syntax is added — the operators are emitted for the program that was
    /// already there. Per lookup, two nodes: the index — a `map_values` the plan may already have,
    /// or an `arrange_by` built for the purpose — and the join, taking the previous join's rows on
    /// its left. Then one loop over the last one's rows.
    ///
    /// Every index here is [`Builder::shared`], so two joins that want the same one get the same
    /// node (§99.5 decision 4). For the built ones that needs the key function to be part of the
    /// sharing key, and [`crate::relate::fingerprint_fun`] is what makes one into a string — with
    /// the key's own parameter written canonically, because `Core` numbers variables per definition
    /// and two loops that index the same collection by the same key would otherwise build two
    /// identical arrangements.
    ///
    /// `Err` carries the reason nothing was rewritten, which the caller hangs on the operator that
    /// pays for it; `Err(None)` is the ordinary case of a loop that relates nothing.
    fn joined(&mut self, xs: OpId, f: &Core, scope: &Scope) -> Result<OpId, Option<String>> {
        let known: BTreeSet<VarId> = scope.keys().copied().collect();
        let found = match crate::relate::recognise(f, self.defs, &known) {
            Ok(found) => found,
            Err(crate::relate::Refusal::NoLookup) => return Err(None),
            Err(why) => return Err(Some(why.because())),
        };

        // What the rewritten body still reads, which is the whole point: the capture that made
        // every event reconsider every element has to be *gone*, or this buys an index and a second
        // arrangement for nothing.
        let mut free = BTreeSet::new();
        crate::core::free_vars(&found.body, &mut BTreeSet::new(), &mut free);
        let kept: Vec<VarId> = free.into_iter().filter(|v| scope.contains_key(v)).collect();
        let mut before = BTreeSet::new();
        crate::core::free_vars(f, &mut BTreeSet::new(), &mut before);
        let was = before.iter().filter(|v| scope.contains_key(v)).count();
        if kept.len() >= was {
            return Err(Some(crate::relate::Refusal::NothingSaved.because()));
        }

        let mut left = xs;
        for lookup in found.lookups {
            let over = self.expr(&lookup.over, scope);
            let (index, matched) = match lookup.index {
                crate::relate::Index::Unique => (
                    self.shared(
                        format!("map_values/{over}"),
                        Op::MapValues,
                        vec![over],
                        None,
                    ),
                    Matching::Unique,
                ),
                crate::relate::Index::Grouped { by, param, answers } => {
                    let fp = crate::relate::fingerprint_fun(param, &by);
                    let key = Fun {
                        code: lam(vec![param], by),
                        captures: Vec::new(),
                    };
                    match answers {
                        crate::relate::Answers::Rows => (
                            self.shared(
                                format!("arrange_by/{over}/{fp}"),
                                Op::ArrangeBy { key },
                                vec![over],
                                None,
                            ),
                            Matching::Group,
                        ),
                        crate::relate::Answers::Count => (
                            self.shared(
                                format!("arrange_by/{over}/{fp}"),
                                Op::ArrangeBy { key },
                                vec![over],
                                None,
                            ),
                            Matching::Count,
                        ),
                        // An aggregate's right side is not the collection at all: it is one entry
                        // per group, so the join probes it the way it probes a `map_get`'s map.
                        // Two aggregates over the same collection, key and projection share one
                        // node; two that differ in any of the three do not, which is what the
                        // projection's own fingerprint is in the sharing key for.
                        crate::relate::Answers::Aggregate(aggregate) => {
                            let crate::relate::Aggregate {
                                agg,
                                of,
                                param: row,
                            } = *aggregate;
                            let name = format!(
                                "group_by/{}/{over}/{fp}/{}",
                                agg.name(),
                                crate::relate::fingerprint_fun(row, &of)
                            );
                            let of = Fun {
                                code: lam(vec![row], of),
                                captures: Vec::new(),
                            };
                            // The one place the aggregates differ downstream, and it is what an
                            // absent entry *means*: no rows is `None` for an extreme and `0` for
                            // a total.
                            let matched = match agg {
                                Agg::Sum => Matching::Total,
                                Agg::Min | Agg::Max => Matching::Unique,
                            };
                            (
                                self.shared(name, Op::GroupBy { key, of, agg }, vec![over], None),
                                matched,
                            )
                        }
                    }
                }
            };
            let key = Fun {
                code: lam(vec![lookup.param], lookup.key),
                captures: Vec::new(),
            };
            left = self.push(Op::Join { key, matched }, vec![left, index], None);
        }
        let join = left;

        let mut params = kept.clone();
        params.push(found.row);
        Ok(self.push(
            Op::MapList {
                f: Fun {
                    code: lam(params, found.body),
                    captures: kept.iter().map(|v| scope[v]).collect(),
                },
            },
            vec![join],
            None,
        ))
    }

    /// `filter_list(xs, p)` where `p` asks another collection whether it holds a key, as the
    /// difference or the intersection that answers it.
    ///
    /// [`Op::Restrict`], and §99.9 item 7. Two nodes rather than [`Builder::joined`]'s three per
    /// lookup, because there is no loop left over: the operator keeps and drops the elements the
    /// filter was keeping and dropping, so nothing downstream has to be re-projected.
    ///
    /// The index is the same [`Builder::shared`] `map_values` a `map_get` join would build, so a
    /// program that both looks a key up and asks whether it exists indexes the collection once —
    /// and so do the two halves of a partition, which is the shape `corpus/38-outstanding.beck`
    /// carries.
    fn restricted(&mut self, xs: OpId, f: &Core, scope: &Scope) -> Result<OpId, Option<String>> {
        let known: BTreeSet<VarId> = scope.keys().copied().collect();
        let found = match crate::relate::restriction(f, self.defs, &known) {
            Ok(found) => found,
            // The ordinary case: a filter that relates nothing, which is most of them.
            Err(crate::relate::Refusal::NoMembership) => return Err(None),
            Err(why) => return Err(Some(why.because())),
        };
        // [`Builder::joined`]'s rule, and it reads shorter here because there is nothing left to
        // capture: the operator's only function is the key and the key reads the element alone. So
        // what has to be true is that the predicate captured *something*, and a `map_contains`
        // against a constant table is a filter that was already `O(δ)`.
        let mut before = BTreeSet::new();
        crate::core::free_vars(f, &mut BTreeSet::new(), &mut before);
        if !before.iter().any(|v| scope.contains_key(v)) {
            return Err(Some(crate::relate::Refusal::NothingSaved.because()));
        }
        let over = self.expr(&found.over, scope);
        let index = self.shared(
            format!("map_values/{over}"),
            Op::MapValues,
            vec![over],
            None,
        );
        let key = Fun {
            code: lam(vec![found.param], found.key),
            captures: Vec::new(),
        };
        Ok(self.push(
            Op::Restrict {
                key,
                keep: found.keep,
            },
            vec![xs, index],
            None,
        ))
    }

    /// The per-element function of a collection operator, closed over the operators it reads.
    fn fun(&mut self, f: &Core, scope: &Scope, elem_ty: &Ty) -> Fun {
        let mut free = BTreeSet::new();
        crate::core::free_vars(f, &mut BTreeSet::new(), &mut free);
        let captured: Vec<VarId> = free.into_iter().filter(|v| scope.contains_key(v)).collect();
        // The element parameter cannot collide with a captured variable, because a captured one is
        // free in `f` and this one is bound by the lambda this builds.
        let x = captured.iter().copied().max().unwrap_or(0) + 1;
        let mut params = captured.clone();
        params.push(x);
        let call = Core {
            kind: CoreKind::App {
                func: Box::new(f.clone()),
                args: vec![var(x, signal_elem(elem_ty), f.span)],
            },
            ty: Ty::unit(),
            tier: Tier::Any,
            span: f.span,
            last_use: false,
            order: crate::fields::UNORDERED,
            locals: 0,
        };
        Fun {
            code: lam(params, call),
            captures: captured.iter().map(|v| scope[v]).collect(),
        }
    }

    /// One operator for an expression the decomposition will not enter, over the plan nodes it
    /// reads.
    fn opaque(&mut self, c: &Core, scope: &Scope, because: &str) -> OpId {
        let mut free = BTreeSet::new();
        crate::core::free_vars(c, &mut BTreeSet::new(), &mut free);
        let params: Vec<VarId> = free.into_iter().filter(|v| scope.contains_key(v)).collect();
        let inputs: Vec<OpId> = params.iter().map(|v| scope[v]).collect();
        let code = lam(params, c.clone());
        self.push(Op::Pointwise { code }, inputs, Some(because.to_string()))
    }
}

// -------------------------------------------------------------------------------------------
// Small `Core` constructors
// -------------------------------------------------------------------------------------------

fn lam(params: Vec<VarId>, body: Core) -> Core {
    Core {
        ty: Ty::fun(params.iter().map(|_| Ty::unit()).collect(), body.ty.clone()),
        tier: body.tier,
        span: body.span,
        kind: CoreKind::Lam {
            params: params.into(),
            body: Arc::new(body),
        },
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}

fn var(v: VarId, ty: Ty, span: beck_diag::Span) -> Core {
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

fn follow_alias(graph: &Graph, mut id: SigId) -> SigId {
    let mut guard = 0;
    while matches!(graph.node(id).op, SigOp::Alias) && guard < graph.nodes.len() {
        id = graph.node(id).inputs[0];
        guard += 1;
    }
    id
}
