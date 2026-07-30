//! The Kubernetes rendering of an [`InfraGraph`].
//!
//! [`docs/06-kubernetes-and-packaging.md`] §6.1 puts orchestrators behind a `Platform` trait and
//! keeps them out of language semantics: "Kubernetes under the hood? Yes — as a **compiler
//! backend** behind a `Platform` trait, never as language semantics." This module is one
//! implementation of that backend; `beck run` is the other, and it needs no cluster, container or
//! registry.
//!
//! Phase 0 built its objects as typed `k8s-openapi` structs, which is the right long-term shape.
//! Phase 1 renders from the typed [`Node`] graph to YAML directly, because the *derivation* — which
//! objects exist and why — is what Phase 1 is proving, and that lives one level up in `graph()`.
//! Swapping the renderer for typed structs changes no test in this crate.

use crate::{InfraGraph, Node};

/// Render the graph as a set of named manifest files, ordered so `kubectl apply -f` works.
pub fn render(graph: &InfraGraph, wire_id: &str) -> Vec<(String, String)> {
    let app = &graph.app;
    let mut out = Vec::new();

    for (i, d) in graph.nodes.iter().enumerate() {
        let body = match &d.node {
            Node::Namespace { name } => format!(
                "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {name}\n  labels:\n    \
                 app.kubernetes.io/name: {name}\n    beck.dev/wire-id: \"{wire_id}\"\n"
            ),
            Node::LogStore { name, volume_gb } => format!(
                "apiVersion: apps/v1\nkind: StatefulSet\nmetadata:\n  name: {name}\n  \
                 namespace: {app}\nspec:\n  serviceName: {name}\n  replicas: 1\n  selector:\n    \
                 matchLabels:\n      app: {name}\n  template:\n    metadata:\n      labels:\n        \
                 app: {name}\n    spec:\n      containers:\n        - name: postgres\n          \
                 image: postgres:16-alpine\n          ports:\n            - containerPort: 5432\n          \
                 volumeMounts:\n            - name: data\n              mountPath: /var/lib/postgresql/data\n  \
                 volumeClaimTemplates:\n    - metadata:\n        name: data\n      spec:\n        \
                 accessModes: [\"ReadWriteOnce\"]\n        resources:\n          requests:\n            \
                 storage: {volume_gb}Gi\n"
            ),
            Node::Secret { name, keys } => {
                let data: String = keys
                    .iter()
                    .map(|k| format!("  {k}: \"\"\n"))
                    .collect();
                format!(
                    "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {name}\n  namespace: {app}\n\
                     type: Opaque\nstringData:\n{data}"
                )
            }
            Node::Workload {
                name,
                replicas,
                serves_ui,
            } => format!(
                "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {name}\n  \
                 namespace: {app}\n  annotations:\n    beck.dev/serves-ui: \"{serves_ui}\"\n\
                 spec:\n  replicas: {replicas}\n  selector:\n    matchLabels:\n      app: {name}\n  \
                 template:\n    metadata:\n      labels:\n        app: {name}\n    spec:\n      \
                 securityContext:\n        runAsNonRoot: true\n        runAsUser: 65532\n      \
                 containers:\n        - name: app\n          image: {app}:dev\n          \
                 args: [\"run\", \"--store\", \"postgres\"]\n          ports:\n            \
                 - containerPort: 8080\n          readinessProbe:\n            httpGet:\n              \
                 path: /readyz\n              port: 8080\n          livenessProbe:\n            \
                 httpGet:\n              path: /healthz\n              port: 8080\n          \
                 lifecycle:\n            preStop:\n              sleep:\n                seconds: 5\n"
            ),
            Node::Route {
                name,
                host,
                websocket,
            } => format!(
                "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  \
                 name: {name}\n  namespace: {app}\n  annotations:\n    \
                 beck.dev/websocket: \"{websocket}\"\nspec:\n  parentRefs:\n    - name: beck-gateway\n      \
                 namespace: gateway-system\n  hostnames:\n    - {host}\n  rules:\n    - matches:\n        \
                 - path:\n            type: PathPrefix\n            value: /\n      backendRefs:\n        \
                 - name: {app}\n          port: 8080\n"
            ),
            Node::Policy {
                name,
                allow_ingress_from,
                allow_egress_to,
            } => {
                let ingress: String = if allow_ingress_from.is_empty() {
                    String::new()
                } else {
                    let peers: String = allow_ingress_from
                        .iter()
                        .map(|p| {
                            format!(
                                "        - namespaceSelector:\n            matchLabels:\n              \
                                 kubernetes.io/metadata.name: {p}\n"
                            )
                        })
                        .collect();
                    format!("  ingress:\n    - from:\n{peers}")
                };
                let egress: String = if allow_egress_to.is_empty() {
                    String::new()
                } else {
                    let peers: String = allow_egress_to
                        .iter()
                        .map(|p| {
                            format!(
                                "        - podSelector:\n            matchLabels:\n              \
                                 app: {p}\n"
                            )
                        })
                        .collect();
                    format!("  egress:\n    - to:\n{peers}")
                };
                format!(
                    "apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:\n  \
                     name: {name}\n  namespace: {app}\nspec:\n  podSelector:\n    matchLabels:\n      \
                     app: {app}\n  policyTypes: [\"Ingress\", \"Egress\"]\n{ingress}{egress}"
                )
            }
            Node::SnapshotSchedule {
                name,
                every_events,
            } => format!(
                "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {name}\n  namespace: {app}\n\
                 data:\n  snapshot_every_events: \"{every_events}\"\n"
            ),
            Node::Grant { role, on, privileges } => format!(
                "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {app}-grants\n  \
                 namespace: {app}\ndata:\n  grants.sql: |\n    -- derived from the program's \
                 effects: it appends and reads, so it may not update or delete\n    \
                 GRANT {} ON {on} TO \"{role}\";\n",
                privileges.join(", ")
            ),
            // The image is not a cluster object; it is emitted as an apko config instead.
            Node::Image { .. } => continue,
        };
        out.push((format!("{:02}-{}.yaml", i * 10, slug(&d.node)), body));
    }
    out
}

fn slug(n: &Node) -> String {
    match n {
        Node::Namespace { .. } => "namespace".into(),
        Node::Image { .. } => "image".into(),
        Node::Workload { .. } => "workload".into(),
        Node::Route { .. } => "route".into(),
        Node::LogStore { .. } => "log-store".into(),
        Node::SnapshotSchedule { .. } => "snapshots".into(),
        Node::Secret { .. } => "secret".into(),
        Node::Policy { .. } => "policy".into(),
        Node::Grant { .. } => "grants".into(),
    }
}

/// The image, declaratively — no Dockerfile, no daemon, no build-time package manager.
///
/// §6.2: because an apko build performs no arbitrary execution, the same config and package
/// versions yield the same digest on any machine. The contents below are the whole "operating
/// system" of a Beck service: CA certificates, time zones, and a statically linked binary that is
/// compiler output. There is no shell, no package manager, no init.
pub fn apko(graph: &InfraGraph) -> String {
    format!(
        "# Generated by `beck build` from {app}. Do not edit.\n\
         contents:\n  repositories:\n    - https://packages.wolfi.dev/os\n  keyring:\n    \
         - https://packages.wolfi.dev/os/wolfi-signing.rsa.pub\n  packages:\n    \
         - ca-certificates-bundle\n    - tzdata\n\n\
         entrypoint:\n  command: /usr/bin/beck\n\n\
         cmd: run --store postgres --addr 0.0.0.0:8080\n\n\
         accounts:\n  groups:\n    - groupname: nonroot\n      gid: 65532\n  users:\n    \
         - username: nonroot\n      uid: 65532\n      gid: 65532\n  run-as: 65532\n\n\
         paths:\n  - path: /usr/bin/beck\n    type: hardlink\n    source: /beck\n    \
         uid: 65532\n    gid: 65532\n    permissions: 0o755\n\n\
         archs:\n  - x86_64\n  - aarch64\n\n\
         annotations:\n  org.opencontainers.image.description: >-\n    \
         The {app} application, compiled by Beck.\n",
        app = graph.app
    )
}
