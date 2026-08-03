//! The signal graph, as a graph.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../../docs/03-type-and-effect-system.md) §3.7:
//! "**The signal graph is a graph, not a pipeline.** This section reads top-to-bottom, and the
//! programs it describes do not: `events` is decided from the state, and the state is folded from
//! `events`. The cycle is real and it is sound."
//!
//! Phase 1 and Phase 2 read the graph by *recognising one shape*: find the `merge_clients()`, find
//! the `durable`, find the `decide`, find the first client-placed signal, and inline everything
//! between them ([`docs/19-phase-1-report.md`](../../../../../docs/19-phase-1-report.md) §19.9). That
//! was legitimate narrowness because it announced itself — nine diagnostics refused every other
//! shape — and it was named as debt by two phases running. It also had a hole neither report knew
//! about: a program with *two* durable folds matched the shape, was accepted, and was sliced with
//! both folds reading the same accumulator. See
//! [`docs/23-general-slicer-report.md`](../../../../../docs/23-general-slicer-report.md) §23.2.
//!
//! This module is the replacement. It does not recognise a shape. It builds the graph the program
//! wrote — one vertex per signal operation, including the ones nested inside a declaration —
//! computes its strongly connected components, and hands [`crate::split`] a structure to slice.
//! What used to be "the durable one" is now "the vertices whose op is [`Op::Durable`]", and there
//! may be any number of them.
//!
//! # What a vertex is
//!
//! A *declared* signal contributes one vertex per prim application in its expression, not one per
//! declaration. `todos: Signal[State] = durable(fold(apply_event, empty, events))` is two vertices
//! — a [`Op::Durable`] over a [`Op::Fold`] — because the fold is a node in the dataflow whether or
//! not the program gave it a name. Only the outermost carries the declared name; the inner one is
//! labelled `todos·fold` so a diagnostic and `beck explain flow` can still point at it.
//!
//! That is the difference between a graph and a pattern: `map2(f, durable(fold(…)), summary)`
//! needs no new case here, because there was never a case to begin with.
//!
//! # Cycles
//!
//! The condensation is computed by [`crate::graph::DepGraph`], which already does Tarjan
//! iteratively over a CSR adjacency and numbers components in topological order. Reusing it rather
//! than writing a second SCC pass is the point of it being a separate module.
//!
//! One rule is imposed on the result: **every cycle must contain a fold**. The `decide → durable →
//! fold → decide` cycle is sound because the fold is where the recursion bottoms out — the
//! accumulator is a value the slicer can take as a parameter. A cycle of pure `signal_map`s has no
//! such point and is a program with no meaning; [`Graph::build`] refuses it by name rather than
//! looping.

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics, Span};

use crate::check::Program;
use crate::core::{Core, CoreKind, Prim};
use crate::graph::{DepGraph, EdgeKind, GraphBuilder, GraphNode, NodeId, NodeKind};
use crate::ty::{Tier, Ty};

/// An index into [`Graph::nodes`].
pub type SigId = usize;

/// The accumulator a program with several durable folds is compiled to.
///
/// §3.7 fixes "one totally-ordered log per application", and the runtime holds one accumulator over
/// it. A program that writes two `durable` folds has not asked for two logs; it has asked for two
/// projections of one. [`crate::split`] fuses them into a record of this type, which is why the
/// name is unwritable in the surface syntax: it is a compiler product, and no module publishes it.
pub const FUSED_STATE: &str = "$State";

/// What a vertex does. One variant per construct in §3.7's signal vocabulary.
#[derive(Clone, Debug)]
pub enum Op {
    /// `merge_clients()` — the one place time and nondeterminism enter.
    Ingress,
    /// `decide(proposals, state, validate)` — §3.5's authority chokepoint, as a node.
    Decide { validate: Core },
    /// `fold(step, init, stream)`. The accumulator, and the point at which a cycle bottoms out.
    Fold { step: Core, init: Core },
    /// `durable(signal)` — the accumulator that survives a restart, and therefore the one the log
    /// is *of*.
    Durable,
    /// `signal_map(s, f)`.
    Map { f: Core },
    /// `map2(f, a, b)`.
    Map2 { f: Core },
    /// `per_session(s, f)` — §3.8's fanout point, first-class so that Phase 3 can share the
    /// arrangement above it.
    PerSession { f: Core },
    /// `filter_map(s, f)` on a stream.
    FilterMap { f: Core },
    /// A signal declared as another signal: `mirror: Signal[T] = todos`.
    Alias,
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::Ingress => "merge_clients",
            Op::Decide { .. } => "decide",
            Op::Fold { .. } => "fold",
            Op::Durable => "durable",
            Op::Map { .. } => "signal_map",
            Op::Map2 { .. } => "map2",
            Op::PerSession { .. } => "per_session",
            Op::FilterMap { .. } => "filter_map",
            Op::Alias => "alias",
        }
    }

    /// Whether this vertex carries a `Stream` rather than a `Signal` — occurrences rather than a
    /// value defined at all times (§3.7). A view is a function of signals, so a stream vertex on a
    /// view's path is an error rather than a missing feature.
    pub fn is_stream(&self) -> bool {
        matches!(self, Op::Ingress | Op::Decide { .. } | Op::FilterMap { .. })
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    /// The declared name, when this vertex *is* a signal declaration rather than a sub-expression
    /// of one.
    pub name: Option<Arc<str>>,
    /// What to call it in a diagnostic: the declared name, or `<parent>·<op>` for an inner vertex.
    pub label: Arc<str>,
    pub op: Op,
    pub ty: Ty,
    pub tier: Tier,
    /// The vertices this one reads, in the order the construct takes them.
    pub inputs: Vec<SigId>,
    pub span: Span,
}

/// A dataflow edge whose two ends are on different tiers.
///
/// §4.3: "Every signal edge that crosses tiers becomes a subscription: the server side gets a diff
/// operator, the client side a resumable `(subscription, seq)` consumer." Phase 1 and Phase 2 knew
/// about exactly one crossing and printed a sentence about it; this enumerates them, and gives each
/// the content-derived id a resumable subscription is keyed by.
#[derive(Clone, Debug)]
pub struct Cut {
    /// The consumer — the downstream end, which subscribes.
    pub to: SigId,
    /// The producer — the upstream end, which diffs and streams.
    pub from: SigId,
    pub carries: Ty,
    /// `blake3(module, producer, consumer, structural(carried))[..16]`, by the same rule as the
    /// command channel's operation id: content, not names a human maintains.
    pub id: String,
}

/// The signal graph of one program.
#[derive(Clone, Debug)]
pub struct Graph {
    pub nodes: Vec<Node>,
    /// Declared signal names to their vertices. Inner vertices are not in here — they have no name
    /// a program can write.
    pub by_name: BTreeMap<Arc<str>, SigId>,
    /// The condensation, for cycles and for order.
    pub dep: DepGraph,
    pub cuts: Vec<Cut>,
    /// Vertices nothing reads. A view is one; so is a materialised read model.
    pub sinks: Vec<SigId>,
}

impl Graph {
    pub fn node(&self, id: SigId) -> &Node {
        &self.nodes[id]
    }

    /// Every vertex, dependencies before dependents, cycle members adjacent.
    pub fn order(&self) -> Vec<SigId> {
        self.dep
            .topological()
            .iter()
            .map(|n| n.0 as usize)
            .collect()
    }

    /// What reads this vertex.
    pub fn consumers(&self, id: SigId) -> Vec<SigId> {
        self.dep
            .dependents(NodeId(id as u32))
            .iter()
            .map(|e| e.to.0 as usize)
            .collect()
    }

    pub fn find(&self, f: impl Fn(&Op) -> bool) -> Vec<SigId> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| f(&n.op))
            .map(|(i, _)| i)
            .collect()
    }

    /// The durable accumulators, in declaration order — what the log is of.
    pub fn states(&self) -> Vec<SigId> {
        self.find(|o| matches!(o, Op::Durable))
    }

    pub fn ingress(&self) -> Vec<SigId> {
        self.find(|o| matches!(o, Op::Ingress))
    }

    pub fn decides(&self) -> Vec<SigId> {
        self.find(|o| matches!(o, Op::Decide { .. }))
    }

    /// The name a report should use for a vertex.
    pub fn label(&self, id: SigId) -> &str {
        &self.nodes[id].label
    }

    // -------------------------------------------------------------------------------------
    // Building
    // -------------------------------------------------------------------------------------

    /// Build the graph a checked program declares, or refuse it by name.
    pub fn build(program: &Program, diags: &mut Diagnostics) -> Option<Graph> {
        let mut by_name = BTreeMap::new();
        for (i, s) in program.signals.iter().enumerate() {
            by_name.insert(s.name.clone(), i);
        }

        let mut b = Builder {
            by_name: &by_name,
            nodes: (0..program.signals.len()).map(|_| None).collect(),
            labels: program.signals.iter().map(|s| s.name.clone()).collect(),
            diags,
            ok: true,
        };
        for (i, s) in program.signals.iter().enumerate() {
            let Some((op, inputs)) = b.classify(&s.expr, &s.name, s.tier) else {
                continue;
            };
            b.nodes[i] = Some(Node {
                name: Some(s.name.clone()),
                label: s.name.clone(),
                op,
                ty: s.ty.clone(),
                tier: s.tier,
                inputs,
                span: s.span,
            });
        }
        if !b.ok {
            return None;
        }
        let nodes: Vec<Node> = b.nodes.into_iter().collect::<Option<Vec<_>>>()?;

        // The condensation, from the module that already knows how to compute one.
        let mut gb = GraphBuilder::new();
        for n in &nodes {
            gb.node(GraphNode {
                name: n.label.clone(),
                kind: NodeKind::Signal,
                tier: n.tier,
                effects: Vec::new(),
                because: String::new(),
                span: n.span,
            });
        }
        for (i, n) in nodes.iter().enumerate() {
            for &input in &n.inputs {
                gb.edge(NodeId(i as u32), NodeId(input as u32), EdgeKind::Reads);
            }
        }
        let dep = gb.finish();

        // §3.7's cycle is sound because a fold is in it: the accumulator is a value, so slicing
        // stops there. A cycle without one is a signal defined in terms of itself, and there is
        // nothing to compute.
        let mut ok = true;
        for cycle in dep.cycles() {
            if cycle
                .iter()
                .any(|c| matches!(nodes[c.0 as usize].op, Op::Fold { .. }))
            {
                continue;
            }
            ok = false;
            let members: Vec<&str> = cycle
                .iter()
                .map(|c| nodes[c.0 as usize].label.as_ref())
                .collect();
            diags.push(
                Diagnostic::error(
                    "B0509",
                    format!("`{}` is defined in terms of itself", members[0]),
                    nodes[cycle[0].0 as usize].span,
                )
                .with_primary_label(format!("the cycle is {}", members.join(" → ")))
                .with_note(
                    "§3.7's `events → todos → events` cycle is sound because a `fold` is in it: an \
                     accumulator is a value, so the recursion has a bottom. This one has no fold, \
                     so there is no first value to compute",
                ),
            );
        }
        if !ok {
            return None;
        }

        let sinks: Vec<SigId> = (0..nodes.len())
            .filter(|i| dep.dependents(NodeId(*i as u32)).is_empty())
            .collect();

        let mut cuts = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            for &input in &n.inputs {
                let up = &nodes[input];
                if n.tier == Tier::Any || up.tier == Tier::Any || n.tier == up.tier {
                    continue;
                }
                let carries = signal_elem(&up.ty);
                let mut h = blake3::Hasher::new();
                h.update(program.name.as_bytes());
                h.update(up.label.as_bytes());
                h.update(b"\x00");
                h.update(n.label.as_bytes());
                h.update(b"\x00");
                h.update(crate::iface::structural(&carries, &program.types).as_bytes());
                cuts.push(Cut {
                    to: i,
                    from: input,
                    carries,
                    id: h.finalize().to_hex()[..16].to_string(),
                });
            }
        }

        Some(Graph {
            nodes,
            by_name,
            dep,
            cuts,
            sinks,
        })
    }
}

/// The element a `Signal[T]` or `Stream[T]` carries.
pub fn signal_elem(t: &Ty) -> Ty {
    match t {
        Ty::Con(n, args)
            if (n.as_ref() == Ty::STREAM || n.as_ref() == Ty::SIGNAL) && args.len() == 1 =>
        {
            args[0].clone()
        }
        other => other.clone(),
    }
}

/// The synthetic accumulator a program with several durable folds is compiled to.
///
/// One field per fold, named for the signal that declared it — so `beck explain flow` and a
/// diagnostic can say `$State.counts` and mean something the programmer wrote.
pub fn fused_state_decl(folds: &[(Arc<str>, Ty)]) -> crate::ty::TyDecl {
    crate::ty::TyDecl::Model {
        name: Arc::from(FUSED_STATE),
        params: Vec::new(),
        fields: folds.to_vec(),
    }
}

/// Every `durable` a set of signal declarations holds, labelled exactly as [`Graph::build`] labels
/// it, in declaration order.
///
/// [`crate::split`] reads this off the graph. The **checker** has to answer the same question
/// before a graph exists, because a `test` block's `state` is typed against the accumulator and a
/// fused one is a type the program did not write. Both go through here so the two cannot disagree
/// about how many folds there are or what their fields are called.
///
/// `resolve` is the caller's substitution: mid-check a declaration's type is still a variable, and
/// after checking it is not.
pub fn durables(
    signals: &[crate::check::SignalDecl],
    resolve: &mut dyn FnMut(&Ty) -> Ty,
) -> Vec<(Arc<str>, Ty)> {
    fn walk(expr: &Core, owner: &Arc<str>, out: &mut Vec<(Arc<str>, Ty)>, top: bool) {
        let CoreKind::Prim { op, args } = &expr.kind else {
            return;
        };
        if *op == Prim::Durable {
            // The same rule [`Builder::input`] uses: the outermost vertex of a declaration carries
            // the declared name, an inner one is `<owner>·<op>`. Only durables can collide with
            // durables, so counting them alone gives the same suffixes the full walk does.
            let label: Arc<str> = if top {
                owner.clone()
            } else {
                let base = format!("{owner}·durable");
                let mut candidate: Arc<str> = Arc::from(base.as_str());
                let mut n = 2;
                while out.iter().any(|(l, _)| *l == candidate) {
                    candidate = Arc::from(format!("{base}{n}"));
                    n += 1;
                }
                candidate
            };
            out.push((label, expr.ty.clone()));
        }
        for a in args {
            walk(a, owner, out, false);
        }
    }
    let mut out = Vec::new();
    for s in signals {
        walk(&s.expr, &s.name, &mut out, true);
    }
    // Mid-check a `durable`'s type is still a variable, so resolve before unwrapping the `Signal`.
    for (_, ty) in out.iter_mut() {
        *ty = signal_elem(&resolve(ty));
    }
    out
}

struct Builder<'a, 'd> {
    by_name: &'a BTreeMap<Arc<str>, SigId>,
    nodes: Vec<Option<Node>>,
    /// Every label handed out so far. An inner vertex is named for its owner and its op, and one
    /// declaration can hold two of the same op — `map2(f, signal_map(a, g), signal_map(b, h))` —
    /// so the second gets a number. Labels are the graph's vertex keys, and two vertices sharing
    /// one would silently become a single vertex.
    labels: std::collections::BTreeSet<Arc<str>>,
    diags: &'d mut Diagnostics,
    ok: bool,
}

impl Builder<'_, '_> {
    /// Turn one signal expression into an op and the vertices it reads, creating vertices for any
    /// nested prim application on the way.
    fn classify(&mut self, expr: &Core, owner: &Arc<str>, tier: Tier) -> Option<(Op, Vec<SigId>)> {
        match &expr.kind {
            CoreKind::Global(name) => {
                let id = self.reference(name, expr)?;
                Some((Op::Alias, vec![id]))
            }
            CoreKind::Prim { op, args } => match (op, args.len()) {
                (Prim::MergeClients, 0) => Some((Op::Ingress, Vec::new())),
                (Prim::Decide, 3) => {
                    let proposals = self.input(&args[0], owner, tier)?;
                    let state = self.input(&args[1], owner, tier)?;
                    Some((
                        Op::Decide {
                            validate: args[2].clone(),
                        },
                        vec![proposals, state],
                    ))
                }
                (Prim::Fold, 3) => {
                    let stream = self.input(&args[2], owner, tier)?;
                    Some((
                        Op::Fold {
                            step: args[0].clone(),
                            init: args[1].clone(),
                        },
                        vec![stream],
                    ))
                }
                (Prim::Durable, 1) => {
                    let inner = self.input(&args[0], owner, tier)?;
                    Some((Op::Durable, vec![inner]))
                }
                (Prim::SignalMap, 2) => {
                    let input = self.input(&args[0], owner, tier)?;
                    Some((Op::Map { f: args[1].clone() }, vec![input]))
                }
                (Prim::SignalMap2, 3) => {
                    let a = self.input(&args[1], owner, tier)?;
                    let b = self.input(&args[2], owner, tier)?;
                    Some((Op::Map2 { f: args[0].clone() }, vec![a, b]))
                }
                (Prim::PerSession, 2) => {
                    let input = self.input(&args[0], owner, tier)?;
                    Some((Op::PerSession { f: args[1].clone() }, vec![input]))
                }
                (Prim::StreamFilterMap, 2) => {
                    let input = self.input(&args[0], owner, tier)?;
                    Some((Op::FilterMap { f: args[1].clone() }, vec![input]))
                }
                (other, n) => {
                    self.fail(
                        Diagnostic::error(
                            "B0507",
                            format!("`{}` is not a signal construct", other.name()),
                            expr.span,
                        )
                        .with_primary_label(format!("applied to {n} arguments here"))
                        .with_note(
                            "§3.7's signal vocabulary is `merge_clients`, `filter_map`, `fold`, \
                             `durable`, `signal_map`, `map2`, `per_session` and `decide`; a \
                             signal's expression is built from those and nothing else",
                        ),
                    );
                    None
                }
            },
            _ => {
                self.fail(
                    Diagnostic::error("B0508", "unsupported signal expression", expr.span)
                        .with_primary_label("a signal is a node in the dataflow, not a computation")
                        .with_note(
                            "the computation goes in a `def`, and the signal names it: \
                             `summary: Signal[Summary] = signal_map(counts, summarise)`",
                        ),
                );
                None
            }
        }
    }

    /// The vertex an argument denotes: a named signal, or a fresh vertex for a nested application.
    fn input(&mut self, expr: &Core, owner: &Arc<str>, tier: Tier) -> Option<SigId> {
        if let CoreKind::Global(name) = &expr.kind {
            return self.reference(name, expr);
        }
        let (op, inputs) = self.classify(expr, owner, tier)?;
        let label = self.label(format!("{owner}·{}", op.name()));
        self.nodes.push(Some(Node {
            name: None,
            label,
            op,
            ty: expr.ty.clone(),
            tier,
            inputs,
            span: expr.span,
        }));
        Some(self.nodes.len() - 1)
    }

    fn reference(&mut self, name: &Arc<str>, at: &Core) -> Option<SigId> {
        match self.by_name.get(name) {
            Some(id) => Some(*id),
            None => {
                self.fail(
                    Diagnostic::error(
                        "B0506",
                        format!("`{name}` is not a signal"),
                        at.span,
                    )
                    .with_primary_label("a signal's inputs are other signals")
                    .with_note(
                        "a function is applied *through* a construct — `signal_map(s, f)` — rather \
                         than named as an input",
                    ),
                );
                None
            }
        }
    }

    fn label(&mut self, base: String) -> Arc<str> {
        let mut candidate: Arc<str> = Arc::from(base.as_str());
        let mut n = 2;
        while self.labels.contains(&candidate) {
            candidate = Arc::from(format!("{base}{n}"));
            n += 1;
        }
        self.labels.insert(candidate.clone());
        candidate
    }

    fn fail(&mut self, d: Diagnostic) {
        self.ok = false;
        self.diags.push(d);
    }
}
