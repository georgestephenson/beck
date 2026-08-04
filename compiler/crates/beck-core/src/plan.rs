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
//!   [`Op::Concat`], [`Op::Count`], [`Op::IsEmpty`]) holds an ordered *arrangement* — its output as
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
//!
//! # What this is not
//!
//! It is not a *query* plan. §4.2 keeps the `Query` sub-language symbolic and `docs/20` §20.5 says
//! `beck explain query` waits for an engine to compile one; this compiles the signal graph, which
//! is a different thing that happens to share the word.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::check::Program;
use crate::core::{Core, CoreKind, Prim, VarId};
use crate::signal::{signal_elem, Graph, Op as SigOp, SigId};
use crate::split::{Placed, StateRole};
use crate::ty::{Tier, Ty};

pub type OpId = usize;

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
    /// The subscriber's `Session`. Constant for the life of one subscription, which is what makes
    /// everything not downstream of it shareable (§5.3).
    Session,
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
    /// `concat_lists(xs)` where `xs` is itself a collection of lists: a flatten.
    ///
    /// This is the shape every `for` loop in a `ui:` block takes — the macro lowers
    /// `for t in todos:` to `concat_lists(map_list(todos, …))` — so it is the operator that decides
    /// whether rendering a list is incremental at all.
    Flatten,
    /// `list_len` — §3.8's `remaining`. The arrangement's size, so ±1 per delta and never a
    /// recount; and it does not force its input to be materialised.
    Count,
    IsEmpty,
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::State => "state",
            Op::Session => "session",
            Op::Const => "const",
            Op::Pointwise { .. } => "recompute",
            Op::MapValues => "map_values",
            Op::MapList { .. } => "map_list",
            Op::FilterList { .. } => "filter_list",
            Op::SortBy { .. } => "sort_by",
            Op::Concat => "concat_lists",
            Op::Flatten => "flatten",
            Op::Count => "list_len",
            Op::IsEmpty => "list_is_empty",
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
                | Op::Count
                | Op::IsEmpty
        )
    }

    /// Whether this is an input to the dataflow rather than a step in it.
    pub fn is_source(&self) -> bool {
        matches!(self, Op::State | Op::Session | Op::Const)
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
        )
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub op: Op,
    pub inputs: Vec<OpId>,
    /// Set when this operator is a fallback: which construct had no delta rule.
    pub because: Option<String>,
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

    /// Compile the view of a sliced program.
    ///
    /// Works from the *graph* rather than from [`crate::split::Roles::view`], for the reason
    /// [`23`](../../../../../docs/23-general-slicer-report.md) built the graph in the first place: the
    /// sliced expression has already lost which signal each part came from, and a plan whose nodes
    /// cannot be named is a plan no report can explain.
    pub fn compile(placed: &Placed) -> Plan {
        let graph = &placed.graph;
        let mut b = Builder {
            program: &placed.program,
            nodes: Vec::new(),
            constants: BTreeMap::new(),
            cse: BTreeMap::new(),
            inlining: Vec::new(),
            states: &placed.roles.states,
            state: 0,
            session: 0,
            vertices: BTreeMap::new(),
        };
        b.state = b.push(Op::State, Vec::new(), None);
        b.session = b.push(Op::Session, Vec::new(), None);

        let root = match graph.by_name.get(&placed.roles.page_name).copied() {
            Some(page) if page < graph.nodes.len() => b.vertex(graph, page),
            // A **library** has no page, and an empty graph to look it up in. Its plan is its two
            // sources and a unit — nothing renders it, and the `unwrap_or(0)` this replaced indexed
            // vertex zero of a graph with no vertices (docs/27 §27.4).
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
            signals,
        };
        plan.finish();
        plan
    }

    /// Propagate `per_session` forward and count consumers.
    fn finish(&mut self) {
        for i in 0..self.nodes.len() {
            let per = matches!(self.nodes[i].op, Op::Session)
                || self.nodes[i]
                    .inputs
                    .iter()
                    .any(|&j| self.nodes[j].per_session);
            self.nodes[i].per_session = per;
            if let Op::MapList { f } | Op::FilterList { f } | Op::SortBy { f } = &self.nodes[i].op {
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

    /// Every node an operator reads, including the ones its per-element function captured.
    pub fn dependencies(&self, i: OpId) -> Vec<OpId> {
        let mut out = self.nodes[i].inputs.clone();
        if let Op::MapList { f } | Op::FilterList { f } | Op::SortBy { f } = &self.nodes[i].op {
            out.extend(f.captures.iter().copied());
        }
        out
    }
}

struct Builder<'a> {
    program: &'a Program,
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
        free_vars(f, &mut BTreeSet::new(), &mut free);
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
            CoreKind::Lam { params, body } => Some((params.clone(), (**body).clone())),
            CoreKind::Global(name) if !self.inlining.contains(name) => {
                let def = self.program.defs.get(name)?;
                match &def.body.kind {
                    CoreKind::Lam { params, body } => Some((params.clone(), (**body).clone())),
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
            },
        );
        let key = format!("prim/{}/{ids:?}", op.name());
        self.shared(key, Op::Pointwise { code }, ids, because)
    }

    /// The per-element function of a collection operator, closed over the operators it reads.
    fn fun(&mut self, f: &Core, scope: &Scope, elem_ty: &Ty) -> Fun {
        let mut free = BTreeSet::new();
        free_vars(f, &mut BTreeSet::new(), &mut free);
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
        free_vars(c, &mut BTreeSet::new(), &mut free);
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
            params,
            body: Box::new(body),
        },
        last_use: false,
    }
}

fn var(v: VarId, ty: Ty, span: beck_diag::Span) -> Core {
    Core {
        kind: CoreKind::Var(v),
        ty,
        tier: Tier::Any,
        span,
        last_use: false,
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

/// Every variable an expression reads and does not itself bind.
fn free_vars(c: &Core, bound: &mut BTreeSet<VarId>, out: &mut BTreeSet<VarId>) {
    match &c.kind {
        CoreKind::Var(v) => {
            if !bound.contains(v) {
                out.insert(*v);
            }
        }
        CoreKind::Const(_) | CoreKind::Global(_) => {}
        CoreKind::Lam { params, body } => {
            let added: Vec<VarId> = params
                .iter()
                .copied()
                .filter(|p| bound.insert(*p))
                .collect();
            free_vars(body, bound, out);
            for p in added {
                bound.remove(&p);
            }
        }
        CoreKind::App { func, args } => {
            free_vars(func, bound, out);
            for a in args {
                free_vars(a, bound, out);
            }
        }
        CoreKind::Prim { args, .. } => {
            for a in args {
                free_vars(a, bound, out);
            }
        }
        CoreKind::Let { var, value, body } => {
            free_vars(value, bound, out);
            let added = bound.insert(*var);
            free_vars(body, bound, out);
            if added {
                bound.remove(var);
            }
        }
        CoreKind::If { cond, then, alt } => {
            free_vars(cond, bound, out);
            free_vars(then, bound, out);
            free_vars(alt, bound, out);
        }
        CoreKind::Match { scrutinee, arms } => {
            free_vars(scrutinee, bound, out);
            for a in arms {
                let added: Vec<VarId> = a
                    .pattern
                    .binders()
                    .into_iter()
                    .filter(|p| bound.insert(*p))
                    .collect();
                free_vars(&a.body, bound, out);
                for p in added {
                    bound.remove(&p);
                }
            }
        }
        CoreKind::Make { fields, .. } => {
            for (_, f) in fields {
                free_vars(f, bound, out);
            }
        }
        CoreKind::Field { base, .. } => free_vars(base, bound, out),
        CoreKind::With { base, fields } => {
            free_vars(base, bound, out);
            for (_, f) in fields {
                free_vars(f, bound, out);
            }
        }
        CoreKind::ListLit(items) => {
            for i in items {
                free_vars(i, bound, out);
            }
        }
        CoreKind::MapLit(pairs) => {
            for (k, v) in pairs {
                free_vars(k, bound, out);
                free_vars(v, bound, out);
            }
        }
    }
}
