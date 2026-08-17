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
//!   [`Op::Concat`], [`Op::Flatten`], [`Op::FlatMap`], [`Op::Count`], [`Op::IsEmpty`]) holds an
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
//!
//! # What this is not
//!
//! It is not a *query* plan. §4.2 keeps the `Query` sub-language symbolic and nothing compiles one;
//! this compiles the signal graph, which is a different thing that happens to share the word.
//! `beck explain query` prints *this*, and [`crate::fuse`] rewrites it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::check::Program;
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
    },
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
        )
    }

    /// The per-element function this operator carries, whatever it is applied to.
    ///
    /// One accessor rather than the five-way `if let` that was written out at each of the four
    /// places that remap captures: a new operator with a [`Fun`] missed at one of them would be a
    /// capture the plan never renumbered, which is a wrong `OpId` rather than a compile error.
    pub fn fun_mut(&mut self) -> Option<&mut Fun> {
        match self {
            Op::MapList { f }
            | Op::FilterList { f }
            | Op::SortBy { f }
            | Op::FlatMap { f }
            | Op::Join { key: f } => Some(f),
            _ => None,
        }
    }

    /// The same function, borrowed.
    pub fn fun(&self) -> Option<&Fun> {
        match self {
            Op::MapList { f }
            | Op::FilterList { f }
            | Op::SortBy { f }
            | Op::FlatMap { f }
            | Op::Join { key: f } => Some(f),
            _ => None,
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
            program: &placed.program,
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
            if let Some(f) = self.nodes[i].op.fun() {
                let captured: Vec<OpId> = f.captures.clone();
                if captured.iter().any(|&j| self.nodes[j].per_session) {
                    self.nodes[i].per_session = true;
                }
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
            if let Some(f) = node.op.fun_mut() {
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
        if let Some(f) = self.nodes[i].op.fun() {
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
        "\n  #{} is the root — the page.\n\
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
        Op::Join { .. } => "δ keys applied on the left, and on the right the rows each moved \
                            index entry answers  —  neither is n"
            .to_string(),
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
    let moves = cadences(plan);
    let charges: Vec<Charge> = (0..plan.nodes.len())
        .map(|i| {
            let cost = op_cost(plan, i);
            // The join's key function is deliberately not here: it captures nothing by
            // construction, and printing an empty capture line for it would read as a cost.
            let captures = match &plan.nodes[i].op {
                Op::MapList { f } | Op::FilterList { f } | Op::SortBy { f } | Op::FlatMap { f } => {
                    f.captures.clone()
                }
                _ => Vec::new(),
            };
            let captured = captures
                .iter()
                .map(|&j| moves[j])
                .max()
                .map(|worst| (captures, worst));
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
                        "       {:<14} not read as a join (docs/99 §99.6): {why}",
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
                 \x20        reconsidered on every event — docs/99 §99.3, and the algebra has no\n\
                 \x20        operator for what this program is doing.",
                captured.join(" ")
            );
        }
    }
    let _ = writeln!(
        out,
        "\n  These are the plan's arithmetic rather than a measurement: `Work` is what\n\
         \x20 `Engine::render` counts, so `measure_incremental` checks this arithmetic against the\n\
         \x20 count rather than against a clock."
    );
    out
}

struct Builder<'a> {
    program: &'a Program,
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
                let def = self.program.defs.get(name)?;
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
            CoreKind::Global(name) => match self.program.defs.get(name) {
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
                // Only `map_list`, and the restriction is about what an arrangement holds rather
                // than about what can be recognised. A join's element is a *row* — the left value
                // and what it matched — and `map_list` is the one of the three that does not keep
                // its element: it stores `f(x)`, which is the same value whether `x` arrived alone
                // or in a row. `filter_list` and `sort_by` store the element itself, so rewriting
                // either would put rows into the collection its consumers read.
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
    /// so two joins on one collection share one index by the hash-consing that already exists
    /// (§99.5 decision 4) — and the join, taking the previous join's rows on its left. Then one loop
    /// over the last one's rows.
    ///
    /// `Err` carries the reason nothing was rewritten, which the caller hangs on the operator that
    /// pays for it; `Err(None)` is the ordinary case of a loop that relates nothing.
    fn joined(&mut self, xs: OpId, f: &Core, scope: &Scope) -> Result<OpId, Option<String>> {
        let known: BTreeSet<VarId> = scope.keys().copied().collect();
        let found = match crate::relate::recognise(f, &self.program.defs, &known) {
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
            let m = self.expr(&lookup.map, scope);
            let index = self.shared(format!("map_values/{m}"), Op::MapValues, vec![m], None);
            let key = Fun {
                code: lam(vec![lookup.param], lookup.key),
                captures: Vec::new(),
            };
            left = self.push(Op::Join { key }, vec![left, index], None);
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
