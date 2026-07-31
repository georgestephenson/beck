//! Stage 11 — the infrastructure tier, as a function of the program.
//!
//! [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.4: "No YAML text, no
//! `kubectl` shelling. The compiler builds a typed `InfraGraph` — nodes like `Image`, `Workload`,
//! `Route`, `LogStore`, `SnapshotSchedule`, `Secret`, `Policy`, `Grant` — **derived from program
//! analysis**, exactly as the original sketch demands: a `durable` fold ⇒ a `LogStore` + volume +
//! snapshot schedule; `merge_clients()` ⇒ a websocket ingress route."
//!
//! The claim worth testing is the *derivation*, not the YAML. So the graph is an ordinary value
//! with provenance on every node, and the tests assert that **removing an effect removes a rule** —
//! Phase 0 reported those as the easiest tests in the project to write, and they are still the ones
//! that make "infrastructure is a function of the program" concrete.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use beck_core::graph::{DepGraph, EdgeKind, GraphBuilder, GraphNode, NodeKind};
use beck_core::{Effect, Placed, Tier};

pub mod k8s;

pub use k8s::render;

/// A typed infrastructure node. Diffable, testable, and an ordinary value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Namespace {
        name: String,
    },
    Image {
        name: String,
        entrypoint: String,
    },
    Workload {
        name: String,
        replicas: u32,
        /// Emitted only because a signal is placed on `client`: something has to serve it.
        serves_ui: bool,
    },
    /// `merge_clients()` ⇒ a websocket ingress route.
    Route {
        name: String,
        host: String,
        websocket: bool,
    },
    /// Something has to be addressable before anything can route to it. A `Route` needs a backend,
    /// and a `StatefulSet` needs the headless service its `serviceName` names — both were missing
    /// until a pod tried to resolve them (docs/19 §19.5).
    Service {
        name: String,
        selector: String,
        port: u16,
        /// Headless services give a StatefulSet stable per-pod DNS; a workload's service wants a
        /// cluster IP.
        headless: bool,
    },
    /// A `durable` fold ⇒ a log store, a volume, and a snapshot schedule.
    LogStore {
        name: String,
        volume_gb: u32,
    },
    SnapshotSchedule {
        name: String,
        every_events: u64,
    },
    Secret {
        name: String,
        keys: Vec<String>,
    },
    /// Effect rows ⇒ least-privilege network policy (§6.5).
    Policy {
        name: String,
        allow_ingress_from: Vec<String>,
        allow_egress_to: Vec<String>,
    },
    /// Effect rows ⇒ database grants.
    Grant {
        role: String,
        on: String,
        privileges: Vec<String>,
    },
}

/// A node plus the reason it exists and what it cannot start without.
#[derive(Clone, Debug)]
pub struct Derived {
    pub node: Node,
    /// The program fact that produced it — what `beck explain deploy` prints.
    pub because: String,
    /// The definition or signal whose effect implied it, as a name rather than as prose, so the
    /// dependency graph can draw the edge without parsing [`Derived::because`].
    pub from: Option<String>,
    /// Other objects, by `Kind/name`, that this one references and therefore cannot work without.
    ///
    /// These are the same references the manifests make — a route's `backendRefs`, a stateful set's
    /// `serviceName`, a container's `secretKeyRef`. Recording them here makes them checkable:
    /// docs/19 §19.5's third and fourth defects were both a manifest naming an object that was
    /// never emitted, and neither was visible until a pod tried to resolve it.
    ///
    /// A **label selector is not a reference**. A `Service` selects pods by label and is perfectly
    /// valid with no endpoints, so it does not need its workload; the workload does not need it
    /// either, unless it resolves it by name. Recording selectors here made `LogStore/x` and
    /// `Service/x` each need the other, and the dependency graph — correctly — reported a cycle
    /// that does not exist. What goes here is "cannot start without", not "is related to".
    pub needs: Vec<String>,
}

/// `Kind/name` — the identity of a node in the object graph.
///
/// It has to be both: `Service/todo` and `Workload/todo` are different objects with the same name,
/// and a graph keyed on the name alone would silently merge them.
pub fn id_of(n: &Node) -> String {
    kind_of(n)
}

#[derive(Clone, Debug, Default)]
pub struct InfraGraph {
    pub app: String,
    pub nodes: Vec<Derived>,
}

impl InfraGraph {
    pub fn contains(&self, f: impl Fn(&Node) -> bool) -> bool {
        self.nodes.iter().any(|d| f(&d.node))
    }

    /// The provenance table `beck explain deploy` prints (§4.7).
    pub fn explain(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "{} — {} objects\n", self.app, self.nodes.len());
        for d in &self.nodes {
            let _ = writeln!(out, "{:<26} {}", kind_of(&d.node), d.because);
        }
        let _ = writeln!(
            out,
            "\nevery line above is derived from the program; nothing is templated."
        );
        out
    }
}

fn kind_of(n: &Node) -> String {
    match n {
        Node::Namespace { name } => format!("Namespace/{name}"),
        Node::Image { name, .. } => format!("Image/{name}"),
        Node::Workload { name, .. } => format!("Workload/{name}"),
        Node::Route { name, .. } => format!("Route/{name}"),
        Node::LogStore { name, .. } => format!("LogStore/{name}"),
        Node::SnapshotSchedule { name, .. } => format!("Snapshots/{name}"),
        Node::Secret { name, .. } => format!("Secret/{name}"),
        Node::Service { name, .. } => format!("Service/{name}"),
        Node::Policy { name, .. } => format!("Policy/{name}"),
        Node::Grant { role, .. } => format!("Grant/{role}"),
    }
}

/// Derive the infrastructure a placed program implies.
pub fn graph(placed: &Placed) -> InfraGraph {
    // Collect the effects the whole program performs, with the declaration each came from.
    let mut effects: Vec<(Effect, String)> = Vec::new();
    for s in &placed.program.signals {
        for e in &s.effects {
            effects.push((e.clone(), s.name.to_string()));
        }
    }
    for name in &placed.program.def_order {
        for e in &placed.program.defs[name].effects {
            effects.push((e.clone(), name.to_string()));
        }
    }
    let serves_ui = placed
        .program
        .signals
        .iter()
        .any(|s| s.tier == Tier::Client);
    derive(&sanitise(&placed.program.name), &effects, serves_ui)
}

/// The derivation itself: effects in, objects out.
///
/// Separated from [`graph`] so the claim can be tested directly. "Removing an effect removes a
/// policy rule" is a statement about *this* function, and asserting it by deleting `durable` from
/// a program would only prove that a program without durable state does not compile — which is a
/// different, and less interesting, fact.
pub fn derive(app: &str, effects: &[(Effect, String)], serves_ui: bool) -> InfraGraph {
    let app = app.to_string();
    let mut out = Emit::default();
    // Named up front so the `needs` edges below read as references rather than as string building.
    let (svc, log_svc) = (format!("Service/{app}"), format!("Service/{app}-log"));
    let (image, workload) = (format!("Image/{app}:dev"), format!("Workload/{app}"));
    let (log, secret) = (
        format!("LogStore/{app}-log"),
        format!("Secret/{app}-log-credentials"),
    );

    out.push(
        Node::Namespace { name: app.clone() },
        "every program gets one namespace",
    );
    out.push(
        Node::Image {
            name: format!("{app}:dev"),
            entrypoint: "/usr/bin/beck".into(),
        },
        "the service partition is one binary",
    );

    let has = |e: Effect| {
        effects
            .iter()
            .find(|(x, _)| *x == e)
            .map(|(_, w)| w.clone())
    };

    // `merge_clients()` ⇒ a websocket ingress route, and something for it to route *to*.
    if let Some(from) = has(Effect::Ingress) {
        out.push(
            Node::Service {
                name: app.clone(),
                selector: app.clone(),
                port: 8080,
                headless: false,
            },
            &format!("`{from}` accepts connections, so the workload needs an address"),
        )
        .caused_by(&from);
        out.push(
            Node::Route {
                name: format!("{app}-route"),
                host: format!("{app}.beck.localhost"),
                websocket: true,
            },
            &format!("`{from}` carries `ingress`, so clients need a websocket route"),
        )
        .caused_by(&from)
        .needing(&[&svc]);
    }

    // A `durable` fold ⇒ a log store, a volume, and a snapshot schedule.
    if let Some(from) = has(Effect::Durable) {
        out.push(
            Node::LogStore {
                name: format!("{app}-log"),
                volume_gb: 10,
            },
            &format!("`{from}` is `durable`, so the log needs a volume"),
        )
        .caused_by(&from)
        .needing(&[&log_svc, &secret]);
        out.push(
            Node::SnapshotSchedule {
                name: format!("{app}-snapshots"),
                every_events: 1000,
            },
            &format!("`{from}` is a fold, so its accumulator is snapshotted"),
        )
        .caused_by(&from)
        .needing(&[&log]);
        out.push(
            Node::Service {
                name: format!("{app}-log"),
                selector: format!("{app}-log"),
                port: 5432,
                headless: true,
            },
            &format!("`{from}` needs a log store, and the fold has to be able to resolve it"),
        )
        .caused_by(&from);
        out.push(
            Node::Secret {
                name: format!("{app}-log-credentials"),
                keys: vec!["url".into(), "password".into()],
            },
            "the log store is reached with credentials, never a literal",
        )
        .caused_by(&from);
        out.push(
            Node::Grant {
                role: format!("{app}-app"),
                on: "beck_log".into(),
                // Append-only by construction: nothing in the program can delete an event, so
                // nothing in the grant permits it.
                privileges: vec!["SELECT".into(), "INSERT".into()],
            },
            "the program appends and reads events, and never updates or deletes one",
        )
        .caused_by(&from)
        .needing(&[&log]);
    }

    let mut workload_needs = vec![image.clone()];
    if has(Effect::Durable).is_some() {
        // The Deployment reads the log's URL from the secret and resolves the log by service name.
        workload_needs.push(secret.clone());
        workload_needs.push(log_svc.clone());
    }
    out.push(
        Node::Workload {
            name: app.clone(),
            replicas: 1,
            serves_ui,
        },
        if serves_ui {
            "a signal is placed on `client`, so the server renders and streams patches"
        } else {
            "the service partition needs somewhere to run"
        },
    )
    .needing(
        &workload_needs
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    );

    // Effect rows ⇒ least-privilege network policy. §3.5's "least-privilege infra, computed": the
    // egress list *is* the program's `net.out` atoms, so a host nobody calls is a host the cluster
    // will not let this workload reach — and adding a call adds the rule, in the same commit.
    let mut egress = Vec::new();
    let mut policy_needs = vec![workload.clone()];
    if has(Effect::Durable).is_some() {
        egress.push(format!("{app}-log"));
        policy_needs.push(log_svc.clone());
    }
    let mut hosts: Vec<String> = effects
        .iter()
        .filter_map(|(e, _)| match e {
            // The own origin is this workload's own Service, which the ingress rule already covers.
            Effect::NetOut(h) if h.as_ref() != "origin" => Some(h.to_string()),
            _ => None,
        })
        .collect();
    hosts.sort();
    hosts.dedup();
    let derived_from: Vec<String> = effects
        .iter()
        .filter(|(e, _)| matches!(e, Effect::NetOut(_)))
        .map(|(e, w)| format!("`{w}` performs `{}`", e.name()))
        .collect();
    egress.extend(hosts.iter().cloned());
    out.push(
        Node::Policy {
            name: format!("{app}-policy"),
            allow_ingress_from: if has(Effect::Ingress).is_some() {
                vec!["gateway".into()]
            } else {
                Vec::new()
            },
            allow_egress_to: egress,
        },
        &if derived_from.is_empty() {
            "the policy is the effect row: no `net.out` in the program, no egress rule in the \
             cluster"
                .to_string()
        } else {
            format!("the policy is the effect row: {}", derived_from.join("; "))
        },
    )
    .needing(&policy_needs.iter().map(String::as_str).collect::<Vec<_>>());

    InfraGraph {
        app,
        nodes: out.nodes,
    }
}

/// Accumulates derived nodes, so provenance and references can be attached to the node just pushed
/// without repeating it.
#[derive(Default)]
struct Emit {
    nodes: Vec<Derived>,
}

impl Emit {
    fn push(&mut self, node: Node, because: &str) -> &mut Emit {
        self.nodes.push(Derived {
            node,
            because: because.to_string(),
            from: None,
            needs: Vec::new(),
        });
        self
    }

    /// The definition whose effect implied the node just pushed.
    fn caused_by(&mut self, who: &str) -> &mut Emit {
        if let Some(last) = self.nodes.last_mut() {
            last.from = Some(who.to_string());
        }
        self
    }

    /// The objects the node just pushed references.
    fn needing(&mut self, what: &[&str]) -> &mut Emit {
        if let Some(last) = self.nodes.last_mut() {
            last.needs = what.iter().map(|s| s.to_string()).collect();
        }
        self
    }
}

/// The whole system as one graph: every definition, every signal, every type, and every object the
/// effects imply, with the edges between them.
///
/// This is what makes the program its own AppHost. Aspire needs a second program to say that the
/// web front end references the database; here the `Implies` edges come from the effect that
/// produced each object and the `Needs` edges from the references the manifests make, so the graph
/// cannot describe a topology the deployment does not have.
///
/// `O(program size + V + E)`, one pass each. See [`beck_core::graph`] for the representation.
pub fn dependency_graph(placed: &Placed) -> DepGraph {
    let infra = graph(placed);
    let mut b = beck_core::graph::from_program(&placed.program);
    add_resources(&mut b, &infra);
    b.finish()
}

/// Add the infrastructure objects to a graph builder that already holds the program.
pub fn add_resources(b: &mut GraphBuilder, infra: &InfraGraph) {
    for d in &infra.nodes {
        b.node(GraphNode {
            name: id_of(&d.node).into(),
            kind: NodeKind::Resource,
            tier: Tier::Any,
            effects: Vec::new(),
            because: d.because.clone(),
            span: Default::default(),
        });
    }
    for d in &infra.nodes {
        let id = b.id(&id_of(&d.node)).expect("just added");
        // The object exists *because* of a definition's effect: an edge from the object to the
        // program, so `impacted_by` on a signal reaches the infrastructure it causes.
        if let Some(from) = &d.from {
            b.edge_to_name(id, from, EdgeKind::Implies);
        }
        for need in &d.needs {
            b.edge_to_name(id, need, EdgeKind::Needs);
        }
    }
}

/// Write the object graph, the image configs, the program, and the provenance table.
///
/// `source` is the program itself. It has to be here: the image ships the *toolchain*, and a
/// container told to `beck run` with no source file has nothing to serve. That was invisible until
/// a container was actually asked to serve a page (docs/19 §19.5).
pub fn emit(placed: &Placed, source: &str, out: &Path) -> Result<Vec<PathBuf>> {
    let graph = graph(placed);
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let mut written = Vec::new();

    for (name, body) in k8s::render(&graph, &placed.wire_id) {
        let path = out.join(&name);
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        written.push(path);
    }

    // Two files, in build order: melange turns the binary into a package, apko turns packages into
    // an image. apko copies nothing from the host — see `k8s::apko` for why that is the point and
    // not a limitation.
    // The program, at the name the melange pipeline installs from.
    let app_source = out.join("app.beck");
    std::fs::write(&app_source, source)
        .with_context(|| format!("writing {}", app_source.display()))?;
    written.push(app_source);

    let melange = out.join("image.melange.yaml");
    std::fs::write(&melange, k8s::melange(&graph))?;
    written.push(melange);

    let apko = out.join("image.apko.yaml");
    std::fs::write(&apko, k8s::apko(&graph))?;
    written.push(apko);

    let explain = out.join("explain.txt");
    std::fs::write(&explain, graph.explain())?;
    written.push(explain);

    Ok(written)
}

/// Apply the emitted graph to a local cluster — rung 3 of the parity ladder (§6.6).
pub fn up(out: &Path) -> Result<()> {
    let status = std::process::Command::new("kubectl")
        .arg("apply")
        .arg("-f")
        .arg(out)
        .status()
        .context(
            "running kubectl — `beck up` needs a cluster; `beck run` deliberately needs nothing",
        )?;
    if !status.success() {
        anyhow::bail!("kubectl apply failed");
    }
    Ok(())
}

fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_lowercase();
    if trimmed.is_empty() {
        "beck-app".into()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(src: &str) -> Placed {
        let (placed, d, map) = beck_core::compile_str("app.beck", src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        placed.expect("compiles")
    }

    const PROGRAM: &str = r#"
union Command:
    Ping

union Event:
    Pinged

union Rejection:
    No

model State:
    n: Int

def apply_event(s: State, env: Envelope[Event]) -> State:
    return s.with(n=(s.n + 1))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    return Ok(value=[Pinged])

def view(s: State, session: Session) -> Html:
    return ui:
        main: str(s.n)

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, st, validate)

@on(data)
st: Signal[State] = durable(fold(apply_event, State(n=0), events))

@on(client)
page: Signal[Html] = per_session(st, view)
"#;

    #[test]
    fn a_durable_fold_produces_a_volume_and_a_snapshot_schedule() {
        let g = graph(&compile(PROGRAM));
        assert!(g.contains(|n| matches!(n, Node::LogStore { .. })));
        assert!(g.contains(|n| matches!(n, Node::SnapshotSchedule { .. })));
    }

    #[test]
    fn merge_clients_produces_a_websocket_route() {
        let g = graph(&compile(PROGRAM));
        assert!(g.contains(|n| matches!(
            n,
            Node::Route {
                websocket: true,
                ..
            }
        )));
    }

    #[test]
    fn removing_an_effect_removes_the_rule_it_implied() {
        // The test Phase 0 found easiest to write, and still the one that makes "infrastructure is
        // a function of the program" concrete: take `durable` away and the volume, the snapshot
        // schedule, the credentials and the grant all go with it — while the route survives,
        // because `ingress` is untouched.
        let both = vec![
            (Effect::Ingress, "proposals".to_string()),
            (Effect::Durable, "st".to_string()),
        ];
        let with_durable = derive("app", &both, true);
        assert!(with_durable.contains(|n| matches!(n, Node::LogStore { .. })));
        assert!(with_durable.contains(|n| matches!(n, Node::Route { .. })));

        let ingress_only = vec![(Effect::Ingress, "proposals".to_string())];
        let g = derive("app", &ingress_only, true);
        assert!(!g.contains(|n| matches!(n, Node::LogStore { .. })));
        assert!(!g.contains(|n| matches!(n, Node::SnapshotSchedule { .. })));
        assert!(!g.contains(|n| matches!(n, Node::Grant { .. })));
        assert!(!g.contains(|n| matches!(n, Node::Secret { .. })));
        assert!(g.contains(|n| matches!(n, Node::Route { .. })));

        // And with no ingress there is no route, and no ingress rule in the policy.
        let durable_only = vec![(Effect::Durable, "st".to_string())];
        let h = derive("app", &durable_only, false);
        assert!(!h.contains(|n| matches!(n, Node::Route { .. })));
        assert!(h.contains(|n| matches!(
            n,
            Node::Policy { allow_ingress_from, .. } if allow_ingress_from.is_empty()
        )));
    }

    #[test]
    fn the_grant_is_append_only_because_the_program_is() {
        let g = graph(&compile(PROGRAM));
        let grant = g
            .nodes
            .iter()
            .find_map(|d| match &d.node {
                Node::Grant { privileges, .. } => Some(privileges.clone()),
                _ => None,
            })
            .expect("a grant exists");
        assert_eq!(grant, ["SELECT", "INSERT"]);
    }

    #[test]
    fn no_network_effect_means_no_egress_beyond_the_log() {
        let g = graph(&compile(PROGRAM));
        let policy = g
            .nodes
            .iter()
            .find_map(|d| match &d.node {
                Node::Policy {
                    allow_egress_to, ..
                } => Some(allow_egress_to.clone()),
                _ => None,
            })
            .expect("a policy exists");
        assert_eq!(policy, ["app-log"]);
    }

    #[test]
    fn every_node_carries_why_it_exists() {
        let g = graph(&compile(PROGRAM));
        assert!(g.nodes.iter().all(|d| !d.because.is_empty()));
        let text = g.explain();
        assert!(text.contains("carries `ingress`"), "{text}");
        assert!(text.contains("no `net.out` in the program"), "{text}");
    }

    #[test]
    fn everything_the_manifests_reference_is_also_emitted() {
        // The Route named a backend and the StatefulSet named a serviceName; neither existed until
        // a pod tried to resolve them. A graph that points at objects it does not emit is not a
        // graph (docs/19 §19.5).
        let g = graph(&compile(PROGRAM));
        let files = crate::k8s::render(&g, "id");
        let all: String = files.iter().map(|(_, b)| b.as_str()).collect();

        let emitted: Vec<String> = g
            .nodes
            .iter()
            .filter_map(|d| match &d.node {
                Node::Service { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert!(
            emitted.contains(&"app".to_string()),
            "workload service: {emitted:?}"
        );
        assert!(
            emitted.contains(&"app-log".to_string()),
            "log service: {emitted:?}"
        );

        // The log store is a StatefulSet, so its service must be headless.
        assert!(all.contains("clusterIP: None"), "{all}");
        // …and the credentials must actually point somewhere.
        assert!(
            all.contains("postgres://postgres:beck@app-log.app.svc:5432"),
            "{all}"
        );
    }

    /// Every cross-reference the rendered manifests make, as `(kind, name)`: a stateful set's
    /// `serviceName`, a container's `secretKeyRef`, a route's `backendRefs`.
    fn manifest_references(yaml: &str) -> Vec<(&'static str, String)> {
        let mut refs = Vec::new();
        let mut expecting: Option<&'static str> = None;
        for line in yaml.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("serviceName:") {
                refs.push(("Service", rest.trim().to_string()));
                continue;
            }
            if let Some(kind) = expecting.take() {
                if let Some(n) = t
                    .strip_prefix("- name:")
                    .or_else(|| t.strip_prefix("name:"))
                {
                    refs.push((kind, n.trim().to_string()));
                    continue;
                }
            }
            expecting = match t {
                "secretKeyRef:" => Some("Secret"),
                "backendRefs:" => Some("Service"),
                _ => None,
            };
        }
        refs
    }

    #[test]
    fn no_reference_between_objects_dangles() {
        // Half of docs/19 §19.5: `needs` may not name an object that was never emitted.
        let g = graph(&compile(PROGRAM));
        let emitted: Vec<String> = g.nodes.iter().map(|d| id_of(&d.node)).collect();
        for d in &g.nodes {
            for need in &d.needs {
                assert!(
                    emitted.contains(need),
                    "{} needs {need}, which is not emitted. Emitted: {emitted:?}",
                    id_of(&d.node)
                );
            }
        }
        assert!(
            g.nodes.iter().any(|d| !d.needs.is_empty()),
            "no object references any other, so this test proves nothing"
        );
    }

    #[test]
    fn the_manifests_reference_only_what_the_graph_declares() {
        // The other half, and the direction that actually bit: the YAML named a backend Service and
        // a `serviceName` that the graph did not contain, and nothing said so until a pod tried to
        // resolve them. Manifest references ⊆ `needs` ⊆ emitted objects, checked without naming a
        // single object, so a new reference added to a template cannot slip past.
        let g = graph(&compile(PROGRAM));
        let files = crate::k8s::render(&g, "id");
        let all: String = files.iter().map(|(_, b)| b.as_str()).collect();

        let refs = manifest_references(&all);
        assert!(
            refs.len() >= 3,
            "expected the stateful set, the secrets and the route to reference things: {refs:?}"
        );
        let declared: Vec<String> = g.nodes.iter().flat_map(|d| d.needs.clone()).collect();
        for (kind, name) in &refs {
            let id = format!("{kind}/{name}");
            assert!(
                declared.contains(&id),
                "the manifests reference {id}, which no object declares needing. \
                 Declared: {declared:?}"
            );
        }
    }

    #[test]
    fn the_only_cycle_is_the_one_the_architecture_intends() {
        // The signal graph cycles on purpose: `events` is decided from `todos`, `todos` is folded
        // from `events` (docs/19 §19.4 item 4). Infrastructure must *not* cycle — an object graph
        // with a cycle has no start order — and the first version of `needs` produced one by
        // recording a Service's label selector as if it were a reference to the workload.
        let g = dependency_graph(&compile(PROGRAM));
        let cycles: Vec<Vec<&str>> = g
            .cycles()
            .map(|c| c.iter().map(|n| &*g.node(*n).name).collect())
            .collect();
        for cycle in &cycles {
            assert!(
                cycle.iter().all(|n| !n.contains('/')),
                "infrastructure objects must not depend on each other in a cycle: {cycle:?}"
            );
        }
        assert_eq!(
            cycles.len(),
            1,
            "expected exactly the signal cycle: {cycles:?}"
        );
    }

    #[test]
    fn changing_a_signal_reaches_the_infrastructure_it_caused() {
        // The dashboard question — "what does this affect?" — answered across the whole stack,
        // because the resources are vertices in the same graph as the code that implies them.
        let placed = compile(PROGRAM);
        let g = dependency_graph(&placed);
        let state = g.id("st").expect("the durable signal is a vertex");
        let impacted: Vec<&str> = g
            .impacted_by(state)
            .iter()
            .map(|n| &*g.node(*n).name)
            .collect();
        assert!(
            impacted.contains(&"LogStore/app-log"),
            "the durable fold implies the log store: {impacted:?}"
        );
        assert!(
            impacted.contains(&"Snapshots/app-snapshots"),
            "…and the snapshot schedule: {impacted:?}"
        );
        // The namespace exists whatever the program says, so it must *not* be reachable from one
        // signal — otherwise "impact" degenerates into "everything".
        assert!(
            !impacted.contains(&"Namespace/app"),
            "impact should not include what no effect implied: {impacted:?}"
        );
    }

    #[test]
    fn the_image_ships_the_program_and_the_workload_runs_it() {
        // An image with the toolchain and no program is an image that cannot serve anything, and
        // nothing short of starting a container says so.
        let g = graph(&compile(PROGRAM));
        let melange = crate::k8s::melange(&g);
        assert!(
            melange.contains(crate::k8s::APP_SOURCE),
            "the package must install the program:\n{melange}"
        );
        let workload = crate::k8s::render(&g, "id")
            .into_iter()
            .find(|(n, _)| n.contains("workload"))
            .map(|(_, b)| b)
            .expect("a workload exists");
        assert!(
            workload.contains(crate::k8s::APP_SOURCE),
            "the container must be told which program to run:\n{workload}"
        );
        // …and it must be able to reach the log store the `durable` effect asked for.
        assert!(workload.contains("BECK_POSTGRES_URL"), "{workload}");
        assert!(workload.contains("secretKeyRef"), "{workload}");
    }

    #[test]
    fn the_image_configs_name_the_binary_as_a_package_not_a_host_file() {
        // apko copies nothing from the host, so a config that hardlinks a path the packages never
        // created cannot work — the mistake Phase 0's hand-written config made, invisible until
        // the build was first run (docs/19 §19.5).
        let g = graph(&compile(PROGRAM));
        let apko = crate::k8s::apko(&g);
        assert!(apko.contains("app@local"), "{apko}");
        assert!(
            !apko.contains("type: hardlink"),
            "the binary must arrive as a package, not as a hardlink to a host file:\n{apko}"
        );
        let melange = crate::k8s::melange(&g);
        assert!(melange.contains("install -m755 beck"), "{melange}");
        assert!(melange.contains("targets.destdir"), "{melange}");
    }

    #[test]
    fn the_rendered_manifests_parse_as_kubernetes_objects() {
        let g = graph(&compile(PROGRAM));
        let files = render(&g, "abc123");
        assert!(!files.is_empty());
        for (name, body) in &files {
            assert!(name.ends_with(".yaml"), "{name}");
            assert!(body.contains("apiVersion:"), "{name}:\n{body}");
            assert!(body.contains("kind:"), "{name}:\n{body}");
        }
    }
}
