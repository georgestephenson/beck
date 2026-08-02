//! The dashboard: resources, the dependency graph, metrics and logs, from the one program.
//!
//! # What it is, and what it is not
//!
//! Aspire's dashboard reads two things: an AppHost that *declares* the topology, and an OTLP feed
//! that reports on it. Beck has no AppHost — [`beck_core::graph`] explains why — so this reads the
//! compiled program for structure and [`mod@crate::telemetry`] for behaviour. The consequence is worth
//! naming: the resource list here cannot disagree with what `beck build` emits or with what is
//! running, because all three are the same derivation of the same source.
//!
//! It is not an observability product. It is one screen with four panes over data the process
//! already has, and every number on it is either read off the graph (free) or off an atomic
//! counter (one relaxed load). There is no collector, no database, no retention policy, and no
//! sampling.
//!
//! # Everything is served from memory
//!
//! The graph is built once at start and never rebuilt — the program does not change while the
//! process runs. Metrics are atomics. Logs are a bounded ring. So every endpoint is `O(size of the
//! answer)` and none of them touch the log store, which matters: a dashboard that queries the
//! durable log on every refresh is a dashboard that makes the thing it monitors slower.
//!
//! The page has no external anything — no CDN, no framework, no fonts — for the same reason the
//! thin client does not: an operator's dashboard should work on a cluster with no egress, and the
//! network policy this very program derives forbids that egress.
//!
//! # The page is hand-written HTML, and should not stay that way
//!
//! [`dash.html`](./dash.html) is a hand-written page served as a string. That is the same thing
//! `phase0/` was: the output the compiler ought to generate, written by hand because the compiler
//! could not yet generate it. A dashboard is a view over state — a resource table, a graph, a
//! metrics pane, a log tail — which is precisely the shape `page: Signal[Html] = per_session(...)`
//! describes. **It should be a Beck program**, and then the compiler's own diffing client would
//! stream it, `ui:` would build it, and it would be the first proof that Beck is good enough to
//! write Beck's tools in.
//!
//! What stands in the way is not the view: `ui:` could express this page today. It is that the
//! dashboard's state is not a `durable` fold over an event stream — it is a live read of atomic
//! counters and a compile-time graph — and Beck has no way yet to say "a signal whose value comes
//! from the host". That is the missing construct, and it is a language question, not a dashboard
//! one. Recorded in `docs/19-phase-1-report.md` §19.7.

use std::sync::Arc;

use beck_core::graph::{DepGraph, NodeKind};
use beck_core::Placed;
use serde_json::{json, Value as J};

use crate::app::App;
use crate::telemetry::telemetry;

/// The compiled facts the dashboard shows, computed once.
pub struct Dashboard {
    app_name: String,
    /// Node and edge lists, pre-rendered: the program cannot change under a running process, so
    /// this is built at start and served from memory forever after.
    graph: J,
    resources: J,
}

impl Dashboard {
    /// Build from the placed program and the graph its effects imply.
    ///
    /// `graph` is passed in rather than derived here because `beck-rt` does not depend on
    /// `beck-infra` — the runtime does not know what Kubernetes is, which is the point.
    pub fn new(placed: &Placed, graph: &DepGraph, resources: Vec<ResourceRow>) -> Dashboard {
        Dashboard {
            app_name: placed.program.name.clone(),
            graph: graph_json(graph),
            resources: json!(resources
                .into_iter()
                .map(|r| json!({
                    "id": r.id,
                    "kind": r.kind,
                    "name": r.name,
                    "because": r.because,
                    "needs": r.needs,
                    "detail": r.detail,
                }))
                .collect::<Vec<_>>()),
        }
    }

    /// Route a dashboard request. `None` when the path is not ours.
    pub fn route(&self, path: &str, app: &Arc<App>) -> Option<(&'static str, String)> {
        match path {
            "/_beck" | "/_beck/" => Some(("text/html; charset=utf-8", PAGE.to_string())),
            "/_beck/graph" => Some(("application/json", self.graph.to_string())),
            "/_beck/resources" => Some(("application/json", self.resources.to_string())),
            "/_beck/metrics" => Some((
                "application/json",
                json!({
                    "app": self.app_name,
                    "store": app.store_kind(),
                    "head": app.head(),
                    "metrics": telemetry().snapshot(),
                })
                .to_string(),
            )),
            "/_beck/logs" => Some((
                "application/json",
                json!({ "records": telemetry().records(200) }).to_string(),
            )),
            // The same numbers in the wire format a collector speaks, for anyone who would rather
            // point Grafana at this than read the page.
            "/_beck/otlp/metrics" => Some((
                "application/json",
                telemetry().otlp_metrics(&self.app_name).to_string(),
            )),
            "/_beck/otlp/logs" => Some((
                "application/json",
                telemetry().otlp_logs(&self.app_name, 200).to_string(),
            )),
            _ => None,
        }
    }
}

/// One row of the resource table — an infrastructure object, flattened for display.
pub struct ResourceRow {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub because: String,
    pub needs: Vec<String>,
    pub detail: String,
}

/// The graph as the page wants it: nodes with a layer already assigned, and edges as indices.
///
/// Layering happens here rather than in the browser because [`DepGraph::layers`] is one linear pass
/// over a condensation that is already topologically ordered, and a client-side force-directed
/// layout would be an iterative approximation of the answer the server can compute exactly.
fn graph_json(g: &DepGraph) -> J {
    let layers = g.layers();
    let nodes: Vec<J> = g
        .nodes()
        .map(|(id, n)| {
            json!({
                "name": n.name.as_ref(),
                "kind": n.kind.as_str(),
                "tier": format!("{:?}", n.tier).to_lowercase(),
                "effects": n.effects.iter().map(|e| format!("{e:?}").to_lowercase()).collect::<Vec<_>>(),
                "because": n.because,
                "layer": layers[id.0 as usize],
                "cycle": g.cycle_of(id).len() > 1,
            })
        })
        .collect();
    let mut edges = Vec::new();
    for (id, _) in g.nodes() {
        for e in g.dependencies(id) {
            edges.push(json!({ "from": id.0, "to": e.to.0, "kind": e.kind.as_str() }));
        }
    }
    json!({
        "nodes": nodes,
        "edges": edges,
        "cycles": g.cycles().map(|c| {
            c.iter().map(|n| g.node(*n).name.to_string()).collect::<Vec<_>>()
        }).collect::<Vec<_>>(),
        "counts": {
            "type": g.nodes().filter(|(_, n)| n.kind == NodeKind::Type).count(),
            "function": g.nodes().filter(|(_, n)| n.kind == NodeKind::Function).count(),
            "signal": g.nodes().filter(|(_, n)| n.kind == NodeKind::Signal).count(),
            "resource": g.nodes().filter(|(_, n)| n.kind == NodeKind::Resource).count(),
            "edges": g.edge_count(),
        }
    })
}

/// The page. One file, no dependencies, ~10 KB.
const PAGE: &str = include_str!("dash.html");

#[cfg(test)]
mod tests {
    use super::*;
    use beck_core::graph::{EdgeKind, GraphBuilder, GraphNode, NodeId};
    use beck_core::Tier;

    fn tiny_graph() -> DepGraph {
        let mut b = GraphBuilder::new();
        for (name, kind) in [
            ("events", NodeKind::Signal),
            ("todos", NodeKind::Signal),
            ("Workload/app", NodeKind::Resource),
        ] {
            b.node(GraphNode {
                name: name.into(),
                kind,
                tier: Tier::Any,
                effects: Vec::new(),
                because: String::new(),
                span: Default::default(),
            });
        }
        b.edge(NodeId(0), NodeId(1), EdgeKind::Reads);
        b.edge(NodeId(1), NodeId(0), EdgeKind::Reads);
        b.edge(NodeId(2), NodeId(1), EdgeKind::Implies);
        b.finish()
    }

    #[test]
    fn the_graph_json_carries_layout_and_cycles() {
        let j = graph_json(&tiny_graph());
        assert_eq!(j["counts"]["signal"], 2);
        assert_eq!(j["counts"]["resource"], 1);
        assert_eq!(j["edges"].as_array().unwrap().len(), 3);

        // The cycle is reported as a cycle rather than as a layout failure.
        assert_eq!(j["cycles"].as_array().unwrap().len(), 1);
        let nodes = j["nodes"].as_array().unwrap();
        assert!(nodes[0]["cycle"].as_bool().unwrap());
        assert!(nodes[1]["cycle"].as_bool().unwrap());
        assert!(!nodes[2]["cycle"].as_bool().unwrap());
        // …and the cycle members share a layer, with what depends on them to the right.
        assert_eq!(nodes[0]["layer"], nodes[1]["layer"]);
        assert!(nodes[2]["layer"].as_u64() > nodes[1]["layer"].as_u64());
    }

    #[test]
    fn the_page_needs_nothing_from_the_network() {
        // The network policy this compiler derives has no egress beyond the log. A dashboard that
        // pulls a chart library from a CDN is a dashboard that is blank in the cluster it monitors.
        for offender in ["http://", "https://", "//cdn", "<script src=", "@import"] {
            assert!(
                !PAGE.contains(offender),
                "the dashboard page references {offender}, which the cluster's own egress policy \
                 forbids"
            );
        }
        assert!(
            PAGE.contains("/_beck/graph"),
            "the page must fetch the graph"
        );
    }
}
