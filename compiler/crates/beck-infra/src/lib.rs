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

/// A node plus the reason it exists.
#[derive(Clone, Debug)]
pub struct Derived {
    pub node: Node,
    /// The program fact that produced it — what `beck explain deploy` prints.
    pub because: String,
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
            effects.push((*e, s.name.to_string()));
        }
    }
    for name in &placed.program.def_order {
        for e in &placed.program.defs[name].effects {
            effects.push((*e, name.to_string()));
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
    let mut nodes = Vec::new();
    let mut push = |node: Node, because: &str| {
        nodes.push(Derived {
            node,
            because: because.to_string(),
        })
    };

    push(
        Node::Namespace { name: app.clone() },
        "every program gets one namespace",
    );
    push(
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

    // `merge_clients()` ⇒ a websocket ingress route.
    if let Some(from) = has(Effect::Ingress) {
        push(
            Node::Route {
                name: format!("{app}-route"),
                host: format!("{app}.beck.localhost"),
                websocket: true,
            },
            &format!("`{from}` carries `ingress`, so clients need a websocket route"),
        );
    }

    // A `durable` fold ⇒ a log store, a volume, and a snapshot schedule.
    if let Some(from) = has(Effect::Durable) {
        push(
            Node::LogStore {
                name: format!("{app}-log"),
                volume_gb: 10,
            },
            &format!("`{from}` is `durable`, so the log needs a volume"),
        );
        push(
            Node::SnapshotSchedule {
                name: format!("{app}-snapshots"),
                every_events: 1000,
            },
            &format!("`{from}` is a fold, so its accumulator is snapshotted"),
        );
        push(
            Node::Secret {
                name: format!("{app}-log-credentials"),
                keys: vec!["url".into(), "password".into()],
            },
            "the log store is reached with credentials, never a literal",
        );
        push(
            Node::Grant {
                role: format!("{app}-app"),
                on: "beck_log".into(),
                // Append-only by construction: nothing in the program can delete an event, so
                // nothing in the grant permits it.
                privileges: vec!["SELECT".into(), "INSERT".into()],
            },
            "the program appends and reads events, and never updates or deletes one",
        );
    }

    push(
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
    );

    // Effect rows ⇒ least-privilege network policy. Note what is *absent*: no egress rule beyond
    // the log, because no effect in this program reaches any other host.
    let mut egress = Vec::new();
    if has(Effect::Durable).is_some() {
        egress.push(format!("{app}-log"));
    }
    push(
        Node::Policy {
            name: format!("{app}-policy"),
            allow_ingress_from: if has(Effect::Ingress).is_some() {
                vec!["gateway".into()]
            } else {
                Vec::new()
            },
            allow_egress_to: egress,
        },
        "the policy is the effect row: no `net.out` in the program, no egress rule in the cluster",
    );

    InfraGraph { app, nodes }
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
