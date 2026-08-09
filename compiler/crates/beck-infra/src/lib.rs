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

pub mod compose;
pub mod k8s;
pub mod platform;
pub mod provider;
pub mod sbom;

/// What `beck build` calls the bill of materials it writes. The `.cdx.json` suffix is CycloneDX's
/// own convention, which is what tools look for.
pub const SBOM_FILE: &str = "sbom.cdx.json";
pub mod substrate;
pub mod yaml;

pub use platform::Platform;

pub use k8s::render;

/// The namespace the ingress gateway runs in.
///
/// Named once because two objects have to agree about it: the route's `parentRefs` points the
/// gateway at this workload, and the NetworkPolicy's ingress rule is what lets the gateway's
/// packets in. They disagreed — the route said `gateway-system`, the policy said `gateway` — so
/// the policy admitted a namespace that does not exist and denied the one that does. Nothing said
/// so, because each object was correct on its own.
pub const GATEWAY_NAMESPACE: &str = "gateway-system";

/// The port the service partition listens on.
///
/// One constant, used by the container, its Service, the route that sends to it and the ingress
/// rule that admits it. Written out five times it reads as correct on each line and is a 503 in a
/// cluster.
pub const APP_PORT: u16 = 8080;

/// The port the log store listens on, used by its Service, its container, the URL in the
/// credentials and the egress rule that permits the connection.
pub const LOG_PORT: u16 = 5432;

/// Where the program lives inside a container, whichever platform put it there.
///
/// The image ships the *toolchain*; without the program beside it there is nothing to run, and
/// `beck run` with no source file fails at startup. Obvious in hindsight, invisible until a
/// container was actually asked to serve a page (docs/19 §19.5). It is here rather than in `k8s`
/// because the melange pipeline installs it, the Kubernetes container is told to run it and the
/// Compose service mounts it — three places, one path.
pub const APP_SOURCE: &str = "/app/app.beck";

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
        /// Whether this workload connects to a [`Node::LogStore`], and therefore needs the
        /// credentials for one.
        ///
        /// Not cosmetic. The container read `BECK_POSTGRES_URL` from a Secret unconditionally, so
        /// a program with no `durable` effect emitted a `secretKeyRef` to a Secret nothing derived
        /// — a pod that sits in `CreateContainerConfigError` forever. Found by the generated-graph
        /// suite; no program in the corpus lacks a durable fold, so no example test could see it.
        reads_log: bool,
        /// Whether the program performs `fs.write(path)`, and therefore whether its container's
        /// root filesystem may be read-only.
        ///
        /// [`docs/06`](../../../../docs/06-kubernetes-and-packaging.md) §6.5 lists a read-only root
        /// filesystem among the defaults that "should be *unavoidable*", and it could not be
        /// derived until [`docs/81`](../../../../docs/81-fs-is-two-atoms-report.md) split `fs` into
        /// a read and a write: one atom naming a path could not say whether the program writes.
        /// Secure is the default and the row is what relaxes it, which is the direction that fails
        /// safe — a program that writes and forgets to say so gets a container that refuses the
        /// write, not a container anybody can write to.
        writes_files: bool,
        /// Whether the program declared `identity = managed()`, and therefore whether its container
        /// is told where its issuer is.
        ///
        /// The same field as [`Node::Workload::reads_log`] and the same failure it exists to
        /// prevent: a `secretKeyRef` to a Secret nothing derived is a pod that sits in
        /// `CreateContainerConfigError` forever.
        reads_identity: bool,
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
        /// Namespaces allowed to open a connection to this workload.
        allow_ingress_from: Vec<String>,
        /// Workloads *inside* the cluster this one may reach, by pod label and port.
        allow_egress_to: Vec<Peer>,
        /// Hosts *outside* the cluster, from the program's `net.out` atoms.
        ///
        /// Separate from [`Node::Policy::allow_egress_to`] because Kubernetes cannot express them
        /// the same way, and conflating the two produced a policy that was quietly wrong: a core
        /// `NetworkPolicy` egress peer is an `ipBlock`, a namespace selector or a pod selector —
        /// never a DNS name. Phase 1 emitted `podSelector: {app: payments.example.com}`, which
        /// matches no pod, so the rule the §3.5 claim rests on allowed nothing at all. See
        /// [`crate::k8s`] for what is emitted instead and what it does not enforce.
        allow_egress_hosts: Vec<String>,
    },
    /// `identity = managed()` ⇒ an identity provider, a volume, and a realm derived from the
    /// application (D6).
    ///
    /// Vendor-free like every other node: "an identity provider with a volume", and
    /// [`crate::provider`] is what turns it into an image. The `realm` is JSON rather than a
    /// structure because it is *somebody else's* schema — Keycloak's — and a typed mirror of it
    /// here would be a second copy to keep in step with a product this project does not own.
    IdentityProvider {
        name: String,
        volume_gb: u32,
        /// The realm to import at startup, derived from the application's own name and route.
        realm: String,
    },
    /// Effect rows ⇒ database grants.
    Grant {
        role: String,
        on: String,
        privileges: Vec<String>,
    },
}

/// A workload this one is allowed to reach, and the port it is reached on.
///
/// The port is here rather than assumed because "may talk to the log store" and "may open any port
/// on the log store's pods" are different grants, and only the first is what the program asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub app: String,
    pub port: u16,
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

    /// The same table, plus what the chosen platform cannot express.
    ///
    /// The second half is the reason [`Platform::unsupported`] exists. A platform that renders
    /// eight of nine objects produces a deployment that looks like the one the effects asked for,
    /// and the missing one is a `Policy` — so the gap has to be *in the output a person reads*,
    /// not inferable from its absence.
    pub fn explain_for(&self, platform: &dyn Platform) -> String {
        use std::fmt::Write as _;
        let mut out = self.explain();
        let gaps = platform.unsupported(self);
        let _ = writeln!(out, "\nplatform: {}", platform.name());
        if gaps.is_empty() {
            let _ = writeln!(out, "every object above is expressible on this platform.");
            return out;
        }
        let _ = writeln!(
            out,
            "\n{} object(s) this platform cannot express, and what that costs:\n",
            gaps.len()
        );
        for (what, why) in gaps {
            let _ = writeln!(out, "{what:<26} {why}");
        }
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
        Node::IdentityProvider { name, .. } => format!("IdentityProvider/{name}"),
        Node::Grant { role, .. } => format!("Grant/{role}"),
    }
}

/// The name a browser reaches this application at.
///
/// One function because three objects have to agree about it: the `Route` that admits the request,
/// the realm's redirect URI, and its allowed web origin.
pub fn route_host(app: &str) -> String {
    format!("{app}.beck.localhost")
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
    // `identity = external(issuer="…")` is a **peer**, so it enters the derivation as the atom a
    // peer enters it as. Nothing below this line knows the difference between a host the program
    // calls with `http_fetch` and the one the runtime fetches a key set from, which is the point:
    // §6.5's egress rule is "the hosts this program named", and an issuer is one of them
    // ([`docs/94`](../../../../docs/94-oidc-relying-party-report.md) §94.7).
    //
    // `managed()` deliberately does **not**: its issuer is a `Service` this same derivation is
    // about to emit, so it is a peer *inside* the cluster and a `net.out` host is the wrong
    // vocabulary for it — `derive` gets the declaration instead, and the difference between the two
    // shows up as the difference between a rule Kubernetes enforces and one it cannot (§94.10).
    if let Some(beck_core::check::IdentityDecl::External { host, .. }) = &placed.program.identity {
        effects.push((Effect::NetOut(host.clone()), "identity".to_string()));
    }
    let managed = matches!(
        placed.program.identity,
        Some(beck_core::check::IdentityDecl::Managed { .. })
    );
    let serves_ui = placed
        .program
        .signals
        .iter()
        .any(|s| s.tier == Tier::Client);
    derive_with(
        &sanitise(&placed.program.name),
        &effects,
        serves_ui,
        managed,
    )
}

/// The derivation itself: effects in, objects out.
///
/// Separated from [`graph`] so the claim can be tested directly. "Removing an effect removes a
/// policy rule" is a statement about *this* function, and asserting it by deleting `durable` from
/// a program would only prove that a program without durable state does not compile — which is a
/// different, and less interesting, fact.
pub fn derive(app: &str, effects: &[(Effect, String)], serves_ui: bool) -> InfraGraph {
    derive_with(app, effects, serves_ui, false)
}

/// The same, for a program that asked this deployment to provision its identity provider.
///
/// A separate entry point rather than a fourth argument on [`derive`], because every existing
/// caller is asserting something about the effect row and `managed()` is not in one: it is a
/// declaration, and the two arrive by different routes on purpose (§94.10).
pub fn derive_with(
    app: &str,
    effects: &[(Effect, String)],
    serves_ui: bool,
    managed_identity: bool,
) -> InfraGraph {
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
                port: APP_PORT,
                headless: false,
            },
            &format!("`{from}` accepts connections, so the workload needs an address"),
        )
        .caused_by(&from);
        out.push(
            Node::Route {
                name: format!("{app}-route"),
                host: route_host(&app),
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
                port: LOG_PORT,
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
            reads_log: has(Effect::Durable).is_some(),
            // Any path, not a named one: the flag is about the container's root filesystem, and a
            // program that writes anywhere needs it writable. Deriving a *mount* for the path is a
            // separate question and is not answered here (`docs/82` §82.5).
            writes_files: effects.iter().any(|(e, _)| matches!(e, Effect::FsWrite(_))),
            reads_identity: managed_identity,
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

    // `identity = managed()` ⇒ an identity provider, wired to this application. D6: "the InfraGraph
    // provisions Keycloak … wired via OIDC automatically" — and *wired* is the word that costs
    // something, because a provider a browser is sent to and that does not know this application's
    // redirect URI is a provider that refuses every login. The realm is derived from the two things
    // the graph already knows: the application's name and the route's own origin.
    let identity_svc = format!("Service/{app}-identity");
    if managed_identity {
        // The same host the Route above carries, because the realm's redirect URI has to be a
        // URL a browser will actually arrive from — two places that build it are two places for a
        // login to fail with `invalid_redirect_uri`.
        let origin = format!("https://{}", route_host(&app));
        let realm = provider::DEFAULT.realm(&app, &origin).to_string();
        out.push(
            Node::IdentityProvider {
                name: format!("{app}-identity"),
                volume_gb: 1,
                realm,
            },
            "`identity = managed()`, so the deployment provisions the provider rather than naming \
             somebody else's",
        )
        .caused_by("identity");
        out.push(
            Node::Service {
                name: format!("{app}-identity"),
                selector: format!("{app}-identity"),
                port: provider::DEFAULT.port,
                headless: false,
            },
            "the application resolves its issuer by name, so the provider needs an address",
        )
        .needing(&[&format!("IdentityProvider/{app}-identity")]);
        out.push(
            Node::Secret {
                name: format!("{app}-identity-credentials"),
                keys: vec![
                    "admin-password".into(),
                    "issuer".into(),
                    "realm.json".into(),
                ],
            },
            "the provider is administered with a credential, and the application is told where its \
             issuer is rather than computing it",
        );
    }

    // Effect rows ⇒ least-privilege network policy. §3.5's "least-privilege infra, computed": the
    // egress list *is* the program's `net.out` atoms, so a host nobody calls is a host the cluster
    // will not let this workload reach — and adding a call adds the rule, in the same commit.
    let mut egress: Vec<Peer> = Vec::new();
    let mut policy_needs = vec![workload.clone()];
    if has(Effect::Durable).is_some() {
        egress.push(Peer {
            app: format!("{app}-log"),
            port: LOG_PORT,
        });
        policy_needs.push(log_svc.clone());
    }
    // A **peer**, not a host: a managed issuer is a pod in this namespace, so the egress rule is
    // one Kubernetes can actually enforce — which the `allow_egress_hosts` list for an external
    // issuer is not (see [`Node::Policy`]). §94.10 is why that asymmetry is worth stating.
    if managed_identity {
        egress.push(Peer {
            app: format!("{app}-identity"),
            port: provider::DEFAULT.port,
        });
        policy_needs.push(identity_svc.clone());
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
    // What the rule is derived *from*, in the program's own words. `net.out` atoms and — since the
    // provider a `managed()` declaration provisions is a peer rather than a host — the declaration
    // itself, which reaches the policy by a different route and would otherwise be an egress rule
    // `beck explain deploy` said the program had no reason for.
    let mut derived_from: Vec<String> = effects
        .iter()
        .filter(|(e, _)| matches!(e, Effect::NetOut(_)))
        .map(|(e, w)| format!("`{w}` performs `{}`", e.name()))
        .collect();
    if managed_identity {
        derived_from
            .push("`identity = managed()`, so the provider is a peer in this namespace".into());
    }
    out.push(
        Node::Policy {
            name: format!("{app}-policy"),
            allow_ingress_from: if has(Effect::Ingress).is_some() {
                vec![GATEWAY_NAMESPACE.into()]
            } else {
                Vec::new()
            },
            allow_egress_to: egress,
            allow_egress_hosts: hosts.clone(),
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

/// The subdirectory the Kubernetes manifests are written to.
///
/// Kept as a constant because the conformance suite and the CI job name it, and because it is the
/// default platform's answer. The general question — "which directory holds only the files an apply
/// consumes" — is [`Platform::manifest_dir`], and it is the platform's to answer:
/// `kubectl apply -f <dir>` and `docker compose -f <file>` want different things.
pub const MANIFEST_DIR: &str = "k8s";

/// Write everything a deployment needs, for the default platform.
pub fn emit(placed: &Placed, source: &str, out: &Path) -> Result<Vec<PathBuf>> {
    emit_with(placed, source, out, platform::default_platform().as_ref())
}

/// Write everything a deployment needs, for a chosen platform.
///
/// The layout, for Kubernetes:
///
/// ```text
/// <out>/k8s/000-namespace.yaml …   the manifests, and nothing else
/// <out>/image.apko.yaml            the image, declaratively
/// <out>/image.melange.yaml         the package the binary ships in
/// <out>/app.beck                   the program, at the path the package installs from
/// <out>/explain.txt                why each object exists — and what this platform cannot express
/// ```
///
/// …and for Compose, `<out>/compose/compose.yaml` with no image configs. The *shape* is the
/// platform's; what is constant is that the manifest directory holds manifests and nothing else
/// (docs/20 §20.4 item 14), and that the program travels with them.
///
/// `source` is the program itself. It has to be here: the image ships the *toolchain*, and a
/// container told to `beck run` with no source file has nothing to serve. That was invisible until
/// a container was actually asked to serve a page (docs/19 §19.5).
pub fn emit_with(
    placed: &Placed,
    source: &str,
    out: &Path,
    platform: &dyn Platform,
) -> Result<Vec<PathBuf>> {
    let graph = graph(placed);
    let manifests = out.join(platform.manifest_dir());
    std::fs::create_dir_all(&manifests)
        .with_context(|| format!("creating {}", manifests.display()))?;
    let mut written = Vec::new();

    let write = |path: PathBuf, body: String, written: &mut Vec<PathBuf>| -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        written.push(path);
        Ok(())
    };

    for (name, body) in platform.manifests(&graph, &placed.wire_id) {
        write(manifests.join(&name), body, &mut written)?;
    }
    for (name, body) in platform.build_inputs(&graph) {
        write(out.join(&name), body, &mut written)?;
    }
    write(out.join("app.beck"), source.to_string(), &mut written)?;
    // Beside the manifests rather than instead of them: a bill of materials is an artefact of the
    // build, and one that has to be asked for separately is one that goes stale.
    write(
        out.join(SBOM_FILE),
        sbom::render(&graph, source, &placed.wire_id),
        &mut written,
    )?;
    write(
        out.join("explain.txt"),
        graph.explain_for(platform),
        &mut written,
    )?;

    Ok(written)
}

/// Apply the emitted graph — rung 3 of the parity ladder (§6.6).
pub fn up(out: &Path) -> Result<()> {
    up_with(out, platform::default_platform().as_ref())
}

/// Apply the emitted graph to a chosen platform's target.
pub fn up_with(out: &Path, platform: &dyn Platform) -> Result<()> {
    // The manifest directory, not the output root: the image configs are YAML too, and no target
    // has any idea what an apko file is.
    platform.apply(&out.join(platform.manifest_dir()))
}

/// The longest suffix any derived object name carries: `<app>-log-credentials`.
///
/// Every other one is shorter — `-snapshots`, `-policy`, `-grants`, `-route`, `-log`. The budget
/// below is what keeps the *longest* of them inside the API's limit, which is the only version of
/// this arithmetic that stays true when a new object is added with a new suffix.
const LONGEST_SUFFIX: usize = "-log-credentials".len();

/// A Kubernetes object name is an RFC 1123 label: at most 63 characters, lowercase alphanumerics
/// and dashes, starting and ending with an alphanumeric.
pub const MAX_NAME: usize = 63;

/// Turn a module name into something Kubernetes will accept as the name of an object *and* as the
/// stem of every object derived from it.
///
/// The length cap is not decoration. A module called `customer-facing-order-management-service`
/// produces `…-log-credentials`, and the API server rejects a name over 63 characters — so an
/// application would compile, derive, render, and fail at `kubectl apply` with a message about a
/// field nobody wrote. Found by the generated-graph suite (`tests/manifest_properties.rs`), which
/// is exactly the input nobody types by hand.
pub fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut trimmed = cleaned.trim_matches('-').to_lowercase();
    trimmed.truncate(MAX_NAME - LONGEST_SUFFIX);
    // Truncation can leave the dash that was in the middle of a word at the end of the name.
    let trimmed = trimmed.trim_end_matches('-');
    if trimmed.is_empty() {
        "beck-app".into()
    } else {
        trimmed.to_string()
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
        let (peers, hosts) = g
            .nodes
            .iter()
            .find_map(|d| match &d.node {
                Node::Policy {
                    allow_egress_to,
                    allow_egress_hosts,
                    ..
                } => Some((allow_egress_to.clone(), allow_egress_hosts.clone())),
                _ => None,
            })
            .expect("a policy exists");
        assert_eq!(
            peers,
            [Peer {
                app: "app-log".into(),
                port: LOG_PORT
            }]
        );
        assert!(
            hosts.is_empty(),
            "the program calls nothing outside: {hosts:?}"
        );
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
    fn everything_the_graph_references_is_also_derived() {
        // The Route named a backend and the StatefulSet named a serviceName; neither existed until
        // a pod tried to resolve them. A graph that points at objects it does not emit is not a
        // graph (docs/19 §19.5).
        //
        // This is the *graph* half. The manifest half — that the emitted YAML references only
        // objects the graph declares, and that a `secretKeyRef`, a `serviceName` and a
        // `backendRefs.port` all resolve — is `tests/manifests.rs`, where the objects can be walked
        // instead of the text being scraped.
        let g = graph(&compile(PROGRAM));
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
}
