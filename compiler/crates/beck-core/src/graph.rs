//! The dependency graph — what the program is made of and what depends on what.
//!
//! # Why the compiler owns this
//!
//! .NET Aspire's dashboard shows a resource list and a dependency graph, and it can, because you
//! write an *AppHost*: a second program that declares `AddPostgres("db")`, `WithReference(db)`, and
//! so on. The topology is described twice — once as the application, once as the AppHost — and the
//! two drift.
//!
//! Beck has no AppHost, because the program *is* the AppHost. [`crate::place`] assigns every
//! definition a tier, [`crate::split`] slices the signal graph, and `beck-infra` derives the
//! resource set from the effect rows, each resource carrying the effect that implies it. Nothing
//! about the topology is written down a second time, so nothing about it can disagree. The graph
//! below is not *collected*; it is *read off* what the compiler already knows.
//!
//! # The structure, and why
//!
//! A compressed sparse row adjacency: `offsets[v]..offsets[v + 1]` indexes a contiguous run of
//! `edges`. That is 4 bytes per edge and one cache line per neighbourhood, against the pointer per
//! edge and one allocation per vertex of a `Vec<Vec<_>>`. Both directions are stored, because the
//! two questions a dashboard asks are opposite: *what does this need* (forward) and *what breaks if
//! I change it* (reverse).
//!
//! | operation                                  | time                | space      |
//! |--------------------------------------------|---------------------|------------|
//! | build, including SCCs                       | `O(V + E)`          | `O(V + E)` |
//! | `dependencies`, `dependents`                | `O(1)` to the slice | 0          |
//! | `cycle_of`, `scc_index`, `id`               | `O(1)`, `O(log V)` for `id` | 0 |
//! | `impacted_by` (transitive dependents)       | `O(V' + E')` reached | `O(V')`   |
//! | `topological`                               | `O(1)`, precomputed | 0          |
//!
//! Building is linear and cannot be better: the program has to be read once. Everything the
//! dashboard asks afterwards is a slice index or a bounded traversal, so "almost instant" is not a
//! performance target to chase — it is what the representation makes unavoidable.
//!
//! # Cycles are not errors here
//!
//! [`docs/19-phase-1-report.md`] §19.4 item 4: the signal graph is *legitimately* cyclic —
//! `events` is decided from `todos`, `todos` is folded from `events` — and §3.7 makes the cycle
//! sound. So this does not topologically sort the vertices, which would be impossible. It computes
//! strongly connected components with Tarjan's algorithm and topologically sorts the *condensation*,
//! which always exists. A cycle becomes one box in the dashboard rather than a failure to render.
//!
//! Tarjan rather than Kosaraju because it is one pass rather than two, and it emits components in
//! reverse topological order for free — the layout order the dashboard wants. It is written
//! iteratively: recursion depth would be the longest path in the program, and a compiler should not
//! have a program size at which it overflows the stack.

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_diag::Span;

use crate::check::Program;
use crate::core::{Core, CoreKind};
use crate::ty::{Effect, Tier, Ty, TyDecl};

/// An index into [`DepGraph::nodes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKind {
    /// A model, union, newtype or alias.
    Type,
    /// A top-level function.
    Function,
    /// A top-level signal — a node in the dataflow, not a subroutine.
    Signal,
    /// An infrastructure object the effects imply: a workload, a log store, a route, a policy.
    Resource,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Type => "type",
            NodeKind::Function => "function",
            NodeKind::Signal => "signal",
            NodeKind::Resource => "resource",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    /// A function body mentions another definition.
    Calls,
    /// A signal's expression reads another signal. The dataflow edges, and the ones that may form
    /// a cycle.
    Reads,
    /// A definition constructs, matches on, or is typed by a declared type.
    Uses,
    /// A resource exists *because* of this definition's effects. The edge `beck-infra` already
    /// records as prose in `because`, as structure.
    Implies,
    /// A resource cannot start without another: a route needs a service, a stateful set needs the
    /// headless service its `serviceName` names.
    Needs,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Calls => "calls",
            EdgeKind::Reads => "reads",
            EdgeKind::Uses => "uses",
            EdgeKind::Implies => "implies",
            EdgeKind::Needs => "needs",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub to: NodeId,
    pub kind: EdgeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNode {
    pub name: Arc<str>,
    pub kind: NodeKind,
    /// Where each definition runs. `Tier::Any` for a resource, which is not placed but derived.
    pub tier: Tier,
    pub effects: Vec<Effect>,
    /// For a resource, the sentence `beck-infra` wrote saying which effect implied it. Empty for
    /// program definitions, whose reason for existing is that someone wrote them.
    pub because: String,
    /// The declaration site, so the dashboard can link a resource to the line that caused it.
    pub span: Span,
}

/// A dependency graph over one program and the infrastructure its effects imply.
#[derive(Clone, Debug)]
pub struct DepGraph {
    nodes: Vec<GraphNode>,
    by_name: BTreeMap<Arc<str>, NodeId>,
    out_offsets: Vec<u32>,
    out_edges: Vec<Edge>,
    in_offsets: Vec<u32>,
    in_edges: Vec<Edge>,
    /// Which strongly connected component each node belongs to. Components are numbered in
    /// topological order of the condensation.
    scc_of: Vec<u32>,
    /// Members of each component, grouped: `scc_members[scc_offsets[c]..scc_offsets[c + 1]]`.
    scc_members: Vec<NodeId>,
    scc_offsets: Vec<u32>,
    /// Every node, in an order where a node comes after everything it depends on — except within a
    /// cycle, where the members are adjacent and in no meaningful order.
    order: Vec<NodeId>,
}

impl DepGraph {
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn edge_count(&self) -> usize {
        self.out_edges.len()
    }

    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &GraphNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (NodeId(i as u32), n))
    }

    pub fn node(&self, id: NodeId) -> &GraphNode {
        &self.nodes[id.0 as usize]
    }

    pub fn id(&self, name: &str) -> Option<NodeId> {
        self.by_name.get(name).copied()
    }

    /// What this node depends on.
    pub fn dependencies(&self, id: NodeId) -> &[Edge] {
        let i = id.0 as usize;
        &self.out_edges[self.out_offsets[i] as usize..self.out_offsets[i + 1] as usize]
    }

    /// What depends on this node — the direction that answers "what breaks if I change this".
    pub fn dependents(&self, id: NodeId) -> &[Edge] {
        let i = id.0 as usize;
        &self.in_edges[self.in_offsets[i] as usize..self.in_offsets[i + 1] as usize]
    }

    /// The strongly connected component this node is in. A single-element slice when the node is
    /// not in a cycle, which is the common case.
    pub fn cycle_of(&self, id: NodeId) -> &[NodeId] {
        self.members_of(self.scc_of[id.0 as usize])
    }

    fn members_of(&self, scc: u32) -> &[NodeId] {
        let c = scc as usize;
        &self.scc_members[self.scc_offsets[c] as usize..self.scc_offsets[c + 1] as usize]
    }

    /// Component index, in topological order of the condensation.
    pub fn scc_index(&self, id: NodeId) -> u32 {
        self.scc_of[id.0 as usize]
    }

    /// The cycles: components with more than one member. Empty for an acyclic program; the todo
    /// example has exactly one, and it is the point of the architecture rather than a mistake.
    pub fn cycles(&self) -> impl Iterator<Item = &[NodeId]> {
        (0..self.scc_offsets.len() as u32 - 1)
            .map(|c| self.members_of(c))
            .filter(|m| m.len() > 1)
    }

    /// Dependencies before dependents, with cycle members adjacent.
    pub fn topological(&self) -> &[NodeId] {
        &self.order
    }

    /// A layer per node: 0 for something that depends on nothing, otherwise one more than the
    /// deepest thing it depends on. Cycle members share a layer, because within a cycle there is no
    /// "deeper".
    ///
    /// This is the x-coordinate of a layered drawing — the shape Aspire's graph view has, and the
    /// reason it is computed here rather than in the browser: the condensation is already in
    /// topological order, so one pass over it in that order gives every layer. `O(V + E)`, against
    /// the iterative force-directed relaxation a client-side layout would need.
    pub fn layers(&self) -> Vec<u32> {
        let mut scc_layer = vec![0u32; self.scc_offsets.len() - 1];
        // Components are numbered so that a dependency's component comes first; visiting them in
        // order means every dependency's layer is final before it is read.
        for c in 0..scc_layer.len() as u32 {
            let mut deepest = 0;
            for &m in self.members_of(c) {
                for e in self.dependencies(m) {
                    let d = self.scc_of[e.to.0 as usize];
                    if d != c {
                        deepest = deepest.max(scc_layer[d as usize] + 1);
                    }
                }
            }
            scc_layer[c as usize] = deepest;
        }
        self.scc_of.iter().map(|c| scc_layer[*c as usize]).collect()
    }

    /// Everything that transitively depends on `id`, including `id`. Breadth-first over the reverse
    /// edges, so it costs the size of the affected region rather than the size of the program.
    pub fn impacted_by(&self, id: NodeId) -> Vec<NodeId> {
        self.impact(id).into_iter().map(|(n, _)| n).collect()
    }

    /// The same, with each node's distance in hops from `id`, and nearest first.
    ///
    /// The distance is what makes the answer usable rather than merely correct: "37 things depend
    /// on this" is a number, and "4 things depend on it directly, and the rest through them" is an
    /// answer. Breadth-first, so the first time a node is reached is by a shortest path.
    pub fn impact(&self, id: NodeId) -> Vec<(NodeId, u32)> {
        let mut seen = vec![false; self.nodes.len()];
        let mut queue = std::collections::VecDeque::from([(id, 0)]);
        let mut out = Vec::new();
        seen[id.0 as usize] = true;
        while let Some((v, d)) = queue.pop_front() {
            out.push((v, d));
            for e in self.dependents(v) {
                if !seen[e.to.0 as usize] {
                    seen[e.to.0 as usize] = true;
                    queue.push_back((e.to, d + 1));
                }
            }
        }
        out
    }
}

// -------------------------------------------------------------------------------------------
// Building
// -------------------------------------------------------------------------------------------

/// Accumulates vertices and edges before they are frozen into CSR form.
///
/// Split from [`DepGraph`] so `beck-infra` can add the resource vertices — it depends on
/// `beck-core`, not the other way round — without either crate knowing the other's node kinds.
#[derive(Default)]
pub struct GraphBuilder {
    nodes: Vec<GraphNode>,
    by_name: BTreeMap<Arc<str>, NodeId>,
    edges: Vec<(NodeId, Edge)>,
}

impl GraphBuilder {
    pub fn new() -> GraphBuilder {
        GraphBuilder::default()
    }

    /// Add a vertex, or return the existing one if the name is already known.
    pub fn node(&mut self, node: GraphNode) -> NodeId {
        if let Some(id) = self.by_name.get(&node.name) {
            return *id;
        }
        let id = NodeId(self.nodes.len() as u32);
        self.by_name.insert(node.name.clone(), id);
        self.nodes.push(node);
        id
    }

    pub fn id(&self, name: &str) -> Option<NodeId> {
        self.by_name.get(name).copied()
    }

    pub fn edge(&mut self, from: NodeId, to: NodeId, kind: EdgeKind) {
        self.edges.push((from, Edge { to, kind }));
    }

    /// Add an edge to a name, if that name is a vertex. Silently ignores unknown names: a body
    /// mentions prims and locals as well as globals, and those are not vertices.
    pub fn edge_to_name(&mut self, from: NodeId, to: &str, kind: EdgeKind) {
        if let Some(to) = self.id(to) {
            if to != from {
                self.edge(from, to, kind);
            }
        }
    }

    /// Freeze into CSR form and compute the components. `O(V + E)`.
    pub fn finish(mut self) -> DepGraph {
        let v = self.nodes.len();

        // Deduplicate: a body that calls `mine` three times is one edge, not three. Sorting by
        // (from, to, kind) also groups the edges by source, which is what CSR construction wants.
        self.edges
            .sort_unstable_by_key(|(from, e)| (from.0, e.to.0, e.kind));
        self.edges
            .dedup_by_key(|(from, e)| (from.0, e.to.0, e.kind));

        let (out_offsets, out_edges) = csr(v, self.edges.iter().map(|(f, e)| (*f, *e)));
        let (in_offsets, in_edges) = csr(
            v,
            self.edges.iter().map(|(f, e)| {
                (
                    e.to,
                    Edge {
                        to: *f,
                        kind: e.kind,
                    },
                )
            }),
        );

        let (scc_of, scc_members, scc_offsets) = tarjan(v, &out_offsets, &out_edges);
        let order = scc_members.clone();

        DepGraph {
            nodes: self.nodes,
            by_name: self.by_name,
            out_offsets,
            out_edges,
            in_offsets,
            in_edges,
            scc_of,
            scc_members,
            scc_offsets,
            order,
        }
    }
}

/// Counting sort into compressed sparse rows: one pass to count degrees, a prefix sum, one pass to
/// place. `O(V + E)`, no per-vertex allocation.
fn csr(v: usize, edges: impl Iterator<Item = (NodeId, Edge)> + Clone) -> (Vec<u32>, Vec<Edge>) {
    let mut offsets = vec![0u32; v + 1];
    for (from, _) in edges.clone() {
        offsets[from.0 as usize + 1] += 1;
    }
    for i in 0..v {
        offsets[i + 1] += offsets[i];
    }
    let mut out = vec![
        Edge {
            to: NodeId(0),
            kind: EdgeKind::Calls
        };
        offsets[v] as usize
    ];
    let mut cursor = offsets.clone();
    for (from, e) in edges {
        let slot = &mut cursor[from.0 as usize];
        out[*slot as usize] = e;
        *slot += 1;
    }
    (offsets, out)
}

/// Tarjan's strongly-connected-components, iteratively.
///
/// Returns each node's component, the members grouped by component, and the group offsets.
///
/// A component is closed only once every component reachable from it has been closed. An edge here
/// means "depends on", so a dependency's component is always closed first, and Tarjan's natural
/// output order *is* the order the dashboard wants: everything a node depends on comes before it,
/// except inside a cycle, where there is no such order and the members are adjacent instead.
fn tarjan(v: usize, offsets: &[u32], edges: &[Edge]) -> (Vec<u32>, Vec<NodeId>, Vec<u32>) {
    const UNVISITED: u32 = u32::MAX;

    let mut index = vec![UNVISITED; v]; // discovery time
    let mut low = vec![0u32; v];
    let mut on_stack = vec![false; v];
    let mut stack: Vec<NodeId> = Vec::new();
    let mut next_index = 0u32;
    // Components in discovery order, which is reverse topological order.
    let mut comps: Vec<Vec<NodeId>> = Vec::new();

    // The explicit call stack: (vertex, how far through its edges we are).
    let mut work: Vec<(u32, u32)> = Vec::new();

    for root in 0..v as u32 {
        if index[root as usize] != UNVISITED {
            continue;
        }
        work.push((root, offsets[root as usize]));
        index[root as usize] = next_index;
        low[root as usize] = next_index;
        next_index += 1;
        stack.push(NodeId(root));
        on_stack[root as usize] = true;

        while let Some((node, edge_cursor)) = work.last_mut() {
            let n = *node as usize;
            if *edge_cursor < offsets[n + 1] {
                let e = edges[*edge_cursor as usize];
                *edge_cursor += 1;
                let w = e.to.0 as usize;
                if index[w] == UNVISITED {
                    index[w] = next_index;
                    low[w] = next_index;
                    next_index += 1;
                    stack.push(e.to);
                    on_stack[w] = true;
                    work.push((e.to.0, offsets[w]));
                } else if on_stack[w] {
                    low[n] = low[n].min(index[w]);
                }
            } else {
                // Done with this vertex: close a component if it is a root, then fold into parent.
                if low[n] == index[n] {
                    let mut comp = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w.0 as usize] = false;
                        comp.push(w);
                        if w.0 as usize == n {
                            break;
                        }
                    }
                    comps.push(comp);
                }
                work.pop();
                if let Some((parent, _)) = work.last() {
                    let p = *parent as usize;
                    low[p] = low[p].min(low[n]);
                }
            }
        }
    }

    let mut scc_of = vec![0u32; v];
    let mut members = Vec::with_capacity(v);
    let mut scc_offsets = Vec::with_capacity(comps.len() + 1);
    scc_offsets.push(0);
    for (c, comp) in comps.iter().enumerate() {
        for &m in comp {
            scc_of[m.0 as usize] = c as u32;
            members.push(m);
        }
        scc_offsets.push(members.len() as u32);
    }
    (scc_of, members, scc_offsets)
}

/// Read the vertices and edges of a checked program.
///
/// One walk of every definition body, so `O(program size)`.
pub fn from_program(program: &Program) -> GraphBuilder {
    let mut b = GraphBuilder::new();

    // Vertices first, all of them, so a body walked in any order can find what it references. The
    // signal graph is cyclic, so there is no order in which this would not be needed.
    for name in program.types.keys() {
        b.node(GraphNode {
            name: name.clone(),
            kind: NodeKind::Type,
            tier: Tier::Any,
            effects: Vec::new(),
            because: String::new(),
            // `TyDecl` carries no span — nothing has needed one yet. A type is the one vertex the
            // dashboard cannot link back to its line.
            span: Span::NONE,
        });
    }
    for name in &program.def_order {
        let Some(def) = program.defs.get(name) else {
            continue;
        };
        b.node(GraphNode {
            name: name.clone(),
            kind: NodeKind::Function,
            tier: def.tier,
            effects: def.effects.clone(),
            because: String::new(),
            span: def.span,
        });
    }
    for sig in &program.signals {
        b.node(GraphNode {
            name: sig.name.clone(),
            kind: NodeKind::Signal,
            tier: sig.tier,
            effects: sig.effects.clone(),
            because: String::new(),
            span: sig.span,
        });
    }

    // A model's field types and a union's variant payloads are dependencies too: changing `Id`
    // changes `Todo`, and the dashboard should say so.
    for (name, decl) in &program.types {
        let from = b.id(name).expect("just added");
        match decl {
            TyDecl::Model { fields, .. } => {
                for (_, ty) in fields {
                    add_type_edges(&mut b, from, ty);
                }
            }
            TyDecl::Union { variants, .. } => {
                for v in variants {
                    for (_, ty) in &v.fields {
                        add_type_edges(&mut b, from, ty);
                    }
                }
            }
            TyDecl::Newtype { inner: ty, .. } | TyDecl::Alias { ty, .. } => {
                add_type_edges(&mut b, from, ty)
            }
        }
    }

    for name in &program.def_order {
        let Some(def) = program.defs.get(name) else {
            continue;
        };
        let from = b.id(name).expect("just added");
        for (_, _, ty) in &def.params {
            add_type_edges(&mut b, from, ty);
        }
        add_type_edges(&mut b, from, &def.ret);
        add_body_edges(&mut b, from, &def.body, EdgeKind::Calls);
    }
    for sig in &program.signals {
        let from = b.id(&sig.name).expect("just added");
        add_type_edges(&mut b, from, &sig.ty);
        add_body_edges(&mut b, from, &sig.expr, EdgeKind::Reads);
    }
    b
}

/// Edges from a definition to every declared type its signature or body mentions.
fn add_type_edges(b: &mut GraphBuilder, from: NodeId, ty: &Ty) {
    match ty {
        Ty::Con(name, args) => {
            if b.id(name)
                .is_some_and(|t| b.nodes[t.0 as usize].kind == NodeKind::Type)
            {
                b.edge_to_name(from, name, EdgeKind::Uses);
            }
            for a in args {
                add_type_edges(b, from, a);
            }
        }
        Ty::Fun(args, ret, _) => {
            for a in args {
                add_type_edges(b, from, a);
            }
            add_type_edges(b, from, ret);
        }
        Ty::Var(_) => {}
    }
}

/// Edges from a definition to everything its body names.
///
/// `default_kind` distinguishes a function calling a function from a signal reading a signal; a
/// reference to a *signal* is always `Reads`, whoever makes it, because that is a dataflow edge.
fn add_body_edges(b: &mut GraphBuilder, from: NodeId, core: &Core, default_kind: EdgeKind) {
    walk(core, &mut |c| match &c.kind {
        CoreKind::Global(name) => {
            let kind = match b.id(name).map(|id| b.nodes[id.0 as usize].kind) {
                Some(NodeKind::Signal) => EdgeKind::Reads,
                Some(NodeKind::Type) => EdgeKind::Uses,
                _ => default_kind,
            };
            b.edge_to_name(from, name, kind);
        }
        CoreKind::Make { ty, .. } => b.edge_to_name(from, ty, EdgeKind::Uses),
        _ => {}
    });
}

/// Pre-order walk of a `Core` tree.
fn walk(core: &Core, f: &mut impl FnMut(&Core)) {
    f(core);
    match &core.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
        CoreKind::Lam { body, .. } => walk(body, f),
        CoreKind::App { func, args } => {
            walk(func, f);
            args.iter().for_each(|a| walk(a, f));
        }
        CoreKind::Prim { args, .. } => args.iter().for_each(|a| walk(a, f)),
        CoreKind::Let { value, body, .. } => {
            walk(value, f);
            walk(body, f);
        }
        CoreKind::If { cond, then, alt } => {
            walk(cond, f);
            walk(then, f);
            walk(alt, f);
        }
        CoreKind::Match { scrutinee, arms } => {
            walk(scrutinee, f);
            arms.iter().for_each(|a| walk(&a.body, f));
        }
        CoreKind::Make { fields, .. } | CoreKind::With { fields, .. } => {
            fields.iter().for_each(|(_, v)| walk(v, f));
        }
        CoreKind::Field { base, .. } => walk(base, f),
        CoreKind::ListLit(xs) => xs.iter().for_each(|x| walk(x, f)),
        CoreKind::MapLit(kvs) => kvs.iter().for_each(|(k, v)| {
            walk(k, f);
            walk(v, f);
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a graph from adjacency written as `(from, to)` name pairs, so the algorithms can be
    /// tested without a program in front of them.
    fn graph_of(names: &[&str], edges: &[(&str, &str)]) -> DepGraph {
        let mut b = GraphBuilder::new();
        for n in names {
            b.node(GraphNode {
                name: Arc::from(*n),
                kind: NodeKind::Function,
                tier: Tier::Any,
                effects: Vec::new(),
                because: String::new(),
                span: Span::NONE,
            });
        }
        for (f, t) in edges {
            let (f, t) = (b.id(f).unwrap(), b.id(t).unwrap());
            b.edge(f, t, EdgeKind::Calls);
        }
        b.finish()
    }

    #[test]
    fn edges_go_both_ways_and_duplicates_collapse() {
        let g = graph_of(&["a", "b", "c"], &[("a", "b"), ("a", "b"), ("c", "b")]);
        let b = g.id("b").unwrap();
        assert_eq!(
            g.dependencies(g.id("a").unwrap()).len(),
            1,
            "duplicate not collapsed"
        );
        assert_eq!(g.dependencies(b).len(), 0);
        assert_eq!(g.dependents(b).len(), 2);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn a_cycle_becomes_one_component_rather_than_a_failure() {
        // The shape of the todo program: proposals → events → todos → proposals is not a mistake.
        let g = graph_of(
            &["merge", "events", "todos", "page", "unrelated"],
            &[
                ("events", "merge"),
                ("events", "todos"),
                ("todos", "events"),
                ("page", "todos"),
            ],
        );
        let cycles: Vec<Vec<&str>> = g
            .cycles()
            .map(|c| {
                let mut names: Vec<&str> = c.iter().map(|n| &*g.node(*n).name).collect();
                names.sort_unstable();
                names
            })
            .collect();
        assert_eq!(cycles, vec![vec!["events", "todos"]]);
        assert_eq!(g.cycle_of(g.id("page").unwrap()).len(), 1, "not in a cycle");
        assert_eq!(g.cycle_of(g.id("unrelated").unwrap()).len(), 1);
    }

    #[test]
    fn the_condensation_is_topologically_ordered() {
        let g = graph_of(
            &["merge", "events", "todos", "page"],
            &[
                ("events", "merge"),
                ("events", "todos"),
                ("todos", "events"),
                ("page", "todos"),
            ],
        );
        let scc = |n: &str| g.scc_index(g.id(n).unwrap());
        // A dependency's component comes first; the cycle members share one.
        assert!(
            scc("merge") < scc("events"),
            "dependency must precede dependent"
        );
        assert_eq!(
            scc("events"),
            scc("todos"),
            "cycle members share a component"
        );
        assert!(scc("todos") < scc("page"));

        // …and `topological` lists nodes in that order, with cycle members adjacent.
        let order: Vec<&str> = g.topological().iter().map(|n| &*g.node(*n).name).collect();
        let at = |n: &str| order.iter().position(|x| *x == n).unwrap();
        assert!(at("merge") < at("events"));
        assert!(at("todos") < at("page"));
        assert_eq!(order.len(), 4);
    }

    #[test]
    fn impact_is_the_transitive_dependents_and_stops_there() {
        let g = graph_of(
            &["util", "a", "b", "unrelated", "other"],
            &[("a", "util"), ("b", "a"), ("other", "unrelated")],
        );
        let mut impacted: Vec<&str> = g
            .impacted_by(g.id("util").unwrap())
            .iter()
            .map(|n| &*g.node(*n).name)
            .collect();
        impacted.sort_unstable();
        assert_eq!(impacted, vec!["a", "b", "util"]);

        // The other half of the claim: it costs the affected region, not the program.
        assert_eq!(g.impacted_by(g.id("other").unwrap()).len(), 1);

        // Distances separate "depends on this directly" from "depends on it through something".
        let hops: Vec<(&str, u32)> = g
            .impact(g.id("util").unwrap())
            .iter()
            .map(|(n, d)| (&*g.node(*n).name, *d))
            .collect();
        assert_eq!(hops, vec![("util", 0), ("a", 1), ("b", 2)]);
    }

    #[test]
    fn a_cycle_reached_from_outside_pulls_in_the_whole_component() {
        let g = graph_of(&["x", "y", "z"], &[("x", "y"), ("y", "x"), ("z", "x")]);
        assert_eq!(g.impacted_by(g.id("y").unwrap()).len(), 3);
    }

    #[test]
    fn layers_put_dependencies_to_the_left_and_cycles_in_one_column() {
        let g = graph_of(
            &["merge", "events", "todos", "page", "loner"],
            &[
                ("events", "merge"),
                ("events", "todos"),
                ("todos", "events"),
                ("page", "todos"),
            ],
        );
        let layers = g.layers();
        let l = |n: &str| layers[g.id(n).unwrap().0 as usize];
        assert_eq!(l("merge"), 0, "depends on nothing");
        assert_eq!(l("loner"), 0);
        assert_eq!(l("events"), 1);
        assert_eq!(l("todos"), 1, "a cycle is one column, not two");
        assert_eq!(l("page"), 2);
    }

    #[test]
    fn deep_chains_do_not_overflow_the_stack() {
        // Tarjan is iterative precisely so this holds. Recursive Tarjan overflows here in debug.
        let names: Vec<String> = (0..100_000).map(|i| format!("n{i}")).collect();
        let mut b = GraphBuilder::new();
        for n in &names {
            b.node(GraphNode {
                name: Arc::from(n.as_str()),
                kind: NodeKind::Function,
                tier: Tier::Any,
                effects: Vec::new(),
                because: String::new(),
                span: Span::NONE,
            });
        }
        for i in 0..names.len() - 1 {
            b.edge(NodeId(i as u32), NodeId(i as u32 + 1), EdgeKind::Calls);
        }
        let g = b.finish();
        assert_eq!(g.len(), 100_000);
        assert_eq!(g.cycles().count(), 0);
        // A 100,000-long chain is one long topological order, and every node is impacted by the last.
        assert_eq!(g.impacted_by(NodeId(99_999)).len(), 100_000);
    }
}
