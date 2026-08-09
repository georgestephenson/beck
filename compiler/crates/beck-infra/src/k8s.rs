//! The Kubernetes rendering of an [`InfraGraph`].
//!
//! [`docs/06-kubernetes-and-packaging.md`](../../../../../docs/06-kubernetes-and-packaging.md) §6.1
//! puts orchestrators behind a `Platform` trait and keeps them out of language semantics:
//! "Kubernetes under the hood? Yes — as a **compiler backend** behind a `Platform` trait, never as
//! language semantics." This module is one implementation of that backend; `beck run` is the other,
//! and it needs no cluster, container or registry.
//!
//! # Objects, not strings
//!
//! Phase 1 rendered these manifests with `format!`, and Phase 2 shipped that way. The reasoning
//! recorded at the time was that "the *derivation* — which objects exist and why — is what is being
//! proved, and that lives one level up in `graph()`". That is true and it is not sufficient. The
//! derivation being right does not make the emission right, and the emission had exactly one test:
//! that each file contained the substrings `apiVersion:` and `kind:`. A manifest with `replias: 1`,
//! or a `targetPort` under `metadata`, or a `matchLabels` that selects nothing, passes that test
//! and fails in a cluster.
//!
//! So the objects are now [`k8s_openapi`] structs — the Kubernetes API's own types, generated from
//! its OpenAPI schema — serialised through [`crate::yaml`]. What that buys, precisely:
//!
//! * a **misspelled field does not compile**, and a required field left out does not compile;
//! * a field of the wrong *type* does not compile — `IntOrString` for a `targetPort`, `Quantity`
//!   for storage, `i32` for `replicas`;
//! * the apiVersion and kind come from the type rather than from a string, so they cannot drift
//!   apart from the body.
//!
//! # What it does not buy, and what covers the gap
//!
//! Two things, named here rather than left to be discovered:
//!
//! 1. **Gateway API is a CRD**, so its types are not in `k8s-openapi` and [`gateway`] below defines
//!    the subset this emitter uses by hand. Those field names are checked by nothing but review.
//!    The alternative — the `gateway-api` crate — pulls the whole `kube` client (146 crates,
//!    hyper and tokio) into a compiler that makes no API calls, and pins an older `k8s-openapi`.
//!    Not worth it for one object; worth revisiting when there are five.
//! 2. **A schema cannot see a cluster.** Every field being well-typed says nothing about whether a
//!    Service's selector matches any pod, whether a container's `secretKeyRef` names a Secret that
//!    is emitted, or whether the port a route sends to is the port the container listens on. Those
//!    are the failures that actually happen, and they are `tests/manifests.rs` — which parses the
//!    emitted YAML back with a third-party parser and checks the objects against *each other*.

use std::collections::{BTreeMap, BTreeSet};

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, StatefulSet, StatefulSetSpec};
use k8s_openapi::api::core::v1::{
    Capabilities, ConfigMap, Container, ContainerPort, EnvVar, EnvVarSource, HTTPGetAction,
    KeyToPath, Lifecycle, LifecycleHandler, Namespace, PersistentVolumeClaim,
    PersistentVolumeClaimSpec, PodSecurityContext, PodSpec, PodTemplateSpec, Probe, SeccompProfile,
    Secret, SecretKeySelector, SecretVolumeSource, SecurityContext, Service, ServicePort,
    ServiceSpec, SleepAction, Volume, VolumeMount, VolumeResourceRequirements,
};
use k8s_openapi::api::networking::v1::{
    IPBlock, NetworkPolicy, NetworkPolicyEgressRule, NetworkPolicyIngressRule, NetworkPolicyPeer,
    NetworkPolicyPort, NetworkPolicySpec,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use serde_json::Value;

use crate::provider::DEFAULT as PROVIDER;
use crate::substrate::DEFAULT as SUBSTRATE;
use crate::yaml;
use crate::{InfraGraph, Node};

/// Where the program lives inside the image — [`crate::APP_SOURCE`], because the Compose platform
/// mounts it at the same path and two answers would be one bug.
pub const APP_SOURCE: &str = crate::APP_SOURCE;

/// [`crate::APP_PORT`] as the API's own integer type.
pub const APP_PORT: i32 = crate::APP_PORT as i32;

/// [`crate::LOG_PORT`] as the API's own integer type.
pub const LOG_PORT: i32 = crate::LOG_PORT as i32;

/// The Kubernetes target: the one §6.1 chose, and one implementation of [`crate::platform`].
pub struct Kubernetes;

impl crate::platform::Platform for Kubernetes {
    fn name(&self) -> &'static str {
        "kubernetes"
    }

    fn manifest_dir(&self) -> &'static str {
        // `kubectl apply -f <dir>` reads every `.yaml` it finds, so the manifests get a directory
        // to themselves and the image configs stay outside it (docs/20 §20.4 item 14).
        "k8s"
    }

    fn manifests(&self, graph: &InfraGraph, wire_id: &str) -> Vec<crate::platform::Artefact> {
        render(graph, wire_id)
    }

    fn build_inputs(&self, graph: &InfraGraph) -> Vec<crate::platform::Artefact> {
        // Two files, in build order: melange turns the binary into a package, apko turns packages
        // into an image. apko copies nothing from the host — see [`apko`] for why that is the point
        // and not a limitation.
        vec![
            ("image.melange.yaml".to_string(), melange(graph)),
            ("image.apko.yaml".to_string(), apko(graph)),
        ]
    }

    fn apply(&self, manifests: &std::path::Path) -> anyhow::Result<()> {
        use anyhow::Context as _;
        let status = std::process::Command::new("kubectl")
            .arg("apply")
            .arg("-f")
            .arg(manifests)
            .status()
            .context(
                "running kubectl — `beck up --platform kubernetes` needs a cluster; `beck run` \
                 deliberately needs nothing",
            )?;
        if !status.success() {
            anyhow::bail!("kubectl apply failed");
        }
        Ok(())
    }
}

/// Render the graph as a set of named manifest files, ordered so `kubectl apply -f` works.
pub fn render(graph: &InfraGraph, wire_id: &str) -> Vec<(String, String)> {
    objects(graph, wire_id)
        .into_iter()
        .map(|(name, value)| (name, yaml::to_yaml(&value)))
        .collect()
}

/// The same manifests as objects, before they become text.
///
/// Exposed because a test that wants to ask "does this Service select this Deployment's pods"
/// should not have to scrape YAML to find out, and because `beck explain deploy` may one day want
/// to diff objects rather than files.
pub fn objects(graph: &InfraGraph, wire_id: &str) -> Vec<(String, Value)> {
    let app = &graph.app;
    let mut out = Vec::new();

    for (i, d) in apply_order(graph).into_iter().enumerate() {
        let value = match &d.node {
            Node::Namespace { name } => to_value(&Namespace {
                metadata: ObjectMeta {
                    name: Some(name.clone()),
                    labels: Some(labels_with(
                        name,
                        [("beck.dev/wire-id".to_string(), wire_id.to_string())],
                    )),
                    ..Default::default()
                },
                ..Default::default()
            }),

            Node::LogStore { name, volume_gb } => to_value(&log_store(app, name, *volume_gb)),

            Node::Service {
                name,
                selector,
                port,
                headless,
            } => to_value(&Service {
                metadata: meta(app, name),
                spec: Some(ServiceSpec {
                    // A headless Service is what gives a StatefulSet stable per-pod DNS. The
                    // literal string is the API's own sentinel, not a placeholder.
                    cluster_ip: headless.then(|| "None".to_string()),
                    selector: Some(BTreeMap::from([("app".to_string(), selector.clone())])),
                    ports: Some(vec![ServicePort {
                        port: i32::from(*port),
                        target_port: Some(IntOrString::Int(i32::from(*port))),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            }),

            Node::Secret { name, keys } => to_value(&Secret {
                metadata: meta(app, name),
                type_: Some("Opaque".to_string()),
                // A working default, not an empty placeholder: the emitter knows the log store's
                // service name, so it can write a URL that resolves. §6.6's parity ladder wants
                // rung 3 to work from `git clone` the way rung 0 does; a production deploy
                // overwrites this Secret with real credentials.
                string_data: Some(
                    keys.iter()
                        .map(|k| {
                            let v = match k.as_str() {
                                "url" => log_url(app),
                                "password" => SUBSTRATE.dev_password().to_string(),
                                "admin-password" => PROVIDER.dev_password().to_string(),
                                // The issuer the application will be told about — the Service this
                                // graph emitted, and the realm it derived. Written here rather than
                                // computed by the runtime, for `log_url`'s reason: two places that
                                // build one URL are two places for it to be wrong.
                                "issuer" => PROVIDER.issuer(&format!("{app}-identity"), app),
                                // Read off the node the graph already derived rather than derived
                                // again here: `docs/92` §92.2's rule — one function, and the gate
                                // reads the rendered output back rather than calling it twice.
                                "realm.json" => graph
                                    .nodes
                                    .iter()
                                    .find_map(|d| match &d.node {
                                        Node::IdentityProvider { realm, .. } => Some(realm.clone()),
                                        _ => None,
                                    })
                                    .unwrap_or_default(),
                                _ => String::new(),
                            };
                            (k.clone(), v)
                        })
                        .collect(),
                ),
                ..Default::default()
            }),

            Node::Workload {
                name,
                replicas,
                serves_ui,
                reads_log,
                writes_files,
                reads_identity,
            } => to_value(&workload(
                app,
                name,
                *replicas,
                *serves_ui,
                *reads_log,
                *writes_files,
                *reads_identity,
            )),

            Node::IdentityProvider {
                name,
                volume_gb,
                realm,
            } => to_value(&identity_provider(app, name, *volume_gb, realm)),

            Node::Route {
                name,
                host,
                websocket,
            } => to_value(&gateway::http_route(app, name, host, *websocket)),

            Node::Policy {
                name,
                allow_ingress_from,
                allow_egress_to,
                allow_egress_hosts,
            } => to_value(&policy(
                app,
                name,
                allow_ingress_from,
                allow_egress_to,
                allow_egress_hosts,
            )),

            Node::SnapshotSchedule { name, every_events } => to_value(&ConfigMap {
                metadata: meta(app, name),
                data: Some(BTreeMap::from([(
                    "snapshot_every_events".to_string(),
                    every_events.to_string(),
                )])),
                ..Default::default()
            }),

            Node::Grant {
                role,
                on,
                privileges,
            } => to_value(&ConfigMap {
                metadata: meta(app, &format!("{app}-grants")),
                data: Some(BTreeMap::from([(
                    "grants.sql".to_string(),
                    format!(
                        "-- derived from the program's effects: it appends and reads, so it may \
                         not update or delete\nGRANT {} ON {on} TO \"{role}\";\n",
                        privileges.join(", ")
                    ),
                )])),
                ..Default::default()
            }),

            // The image is not a cluster object; it is emitted as an apko config instead.
            Node::Image { .. } => continue,
        };
        // Three digits, not two. `kubectl apply -f <dir>` reads files in *lexical* order, and with
        // two digits the eleventh object was named `100-…`, which sorts before `20-…`. The prefix
        // exists to carry the order; a width that overflows silently reverses it.
        out.push((format!("{:03}-{}.yaml", i * 10, slug(&d.node)), value));
    }
    out
}

/// The graph's objects, ordered so that nothing is applied before what it references.
///
/// `kubectl apply -f <dir>` applies files in lexical order and does not sort by dependency, so the
/// emitter has to. Kubernetes is eventually consistent and would converge anyway — a StatefulSet
/// created before its headless Service retries until the Service appears — but "it converges after
/// a few backoffs" and "it comes up" are different experiences, and the ordering is free.
///
/// A stable topological sort over [`crate::Derived::needs`]: ties keep derivation order, so the
/// file names stay put when an unrelated object is added. `needs` is already known to be acyclic —
/// `the_only_cycle_is_the_one_the_architecture_intends` in `lib.rs` is the test — and a cycle here
/// would emit the remaining nodes in derivation order rather than looping.
pub fn apply_order(graph: &InfraGraph) -> Vec<&crate::Derived> {
    let mut placed: Vec<bool> = vec![false; graph.nodes.len()];
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<&crate::Derived> = Vec::with_capacity(graph.nodes.len());

    while out.len() < graph.nodes.len() {
        let ready: Vec<usize> = graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, d)| !placed[*i] && d.needs.iter().all(|n| done.contains(n)))
            .map(|(i, _)| i)
            .collect();
        if ready.is_empty() {
            // Unreachable for an acyclic graph; emitting the rest in derivation order beats
            // dropping them, and `lib.rs` is where the acyclicity is asserted.
            out.extend(
                graph
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !placed[*i])
                    .map(|(_, d)| d),
            );
            break;
        }
        for i in ready {
            placed[i] = true;
            done.insert(crate::id_of(&graph.nodes[i].node));
            out.push(&graph.nodes[i]);
        }
    }
    out
}

/// The log store: a StatefulSet, because its volume is its identity.
/// The identity provider: somebody else's image, a volume, and the realm this graph derived.
///
/// A StatefulSet for [`log_store`]'s reason — its volume is its identity — and with the three
/// hardening constants but **not** a read-only root filesystem, which is `docs/82` §82.3's
/// asymmetry: a derived manifest may make claims about the program's own container and not about a
/// dependency's.
fn identity_provider(app: &str, name: &str, volume_gb: u32, realm: &str) -> StatefulSet {
    StatefulSet {
        metadata: meta(app, name),
        spec: Some(StatefulSetSpec {
            service_name: Some(name.to_string()),
            replicas: Some(1),
            selector: selector(name),
            template: {
                let mut template = pod(
                    name,
                    PodSpec {
                        containers: vec![Container {
                            name: "identity".to_string(),
                            image: Some(PROVIDER.image.to_string()),
                            security_context: Some(hardened(None)),
                            args: Some(PROVIDER.start_args.iter().map(|a| a.to_string()).collect()),
                            env: Some(vec![
                                EnvVar {
                                    name: "KC_BOOTSTRAP_ADMIN_USERNAME".to_string(),
                                    value: Some("admin".to_string()),
                                    ..Default::default()
                                },
                                from_secret(
                                    "KC_BOOTSTRAP_ADMIN_PASSWORD",
                                    &format!("{app}-identity-credentials"),
                                    "admin-password",
                                ),
                                // The provider is reached at its Service name from inside the
                                // cluster and at the gateway from outside, and a token's `iss` has
                                // to be the one the application compares against — so it is told
                                // the same string the application is told.
                                EnvVar {
                                    name: "KC_HOSTNAME_STRICT".to_string(),
                                    value: Some("false".to_string()),
                                    ..Default::default()
                                },
                            ]),
                            ports: Some(vec![ContainerPort {
                                container_port: i32::from(PROVIDER.port),
                                ..Default::default()
                            }]),
                            volume_mounts: Some(vec![
                                VolumeMount {
                                    name: "data".to_string(),
                                    mount_path: PROVIDER.data_dir.to_string(),
                                    ..Default::default()
                                },
                                VolumeMount {
                                    name: "realm".to_string(),
                                    mount_path: PROVIDER.import_dir.to_string(),
                                    ..Default::default()
                                },
                            ]),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                );
                // The realm travels as a projected file rather than as an argument: it is JSON with
                // a redirect URI in it, and a command line is visible in the process table.
                if let Some(spec) = template.spec.as_mut() {
                    spec.volumes = Some(vec![Volume {
                        name: "realm".to_string(),
                        secret: Some(SecretVolumeSource {
                            secret_name: Some(format!("{app}-identity-credentials")),
                            // Only the realm, not the admin password: a Secret mounted whole would
                            // put the administrator's credential on the provider's import path.
                            items: Some(vec![KeyToPath {
                                key: "realm.json".to_string(),
                                path: "realm.json".to_string(),
                                ..Default::default()
                            }]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }]);
                }
                let _ = realm;
                template
            },
            volume_claim_templates: Some(vec![PersistentVolumeClaim {
                metadata: ObjectMeta {
                    name: Some("data".to_string()),
                    ..Default::default()
                },
                spec: Some(PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                    resources: Some(VolumeResourceRequirements {
                        requests: Some(BTreeMap::from([(
                            "storage".to_string(),
                            Quantity(format!("{volume_gb}Gi")),
                        )])),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn log_store(app: &str, name: &str, volume_gb: u32) -> StatefulSet {
    StatefulSet {
        metadata: meta(app, name),
        spec: Some(StatefulSetSpec {
            service_name: Some(name.to_string()),
            replicas: Some(1),
            selector: selector(name),
            template: pod(
                name,
                PodSpec {
                    containers: vec![Container {
                        name: SUBSTRATE.store.to_string(),
                        image: Some(SUBSTRATE.image.to_string()),
                        // The three constants, and deliberately **not** a read-only root
                        // filesystem: this is somebody else's image, Postgres writes its socket
                        // and its temporary files outside the volume, and whether it does is not a
                        // fact any Beck effect row knows. `docs/82` §82.3 says so rather than
                        // leaving the asymmetry to be noticed.
                        security_context: Some(hardened(None)),
                        env: Some(vec![
                            from_secret("POSTGRES_PASSWORD", &credentials(app), "password"),
                            EnvVar {
                                name: "PGDATA".to_string(),
                                value: Some(SUBSTRATE.pgdata()),
                                ..Default::default()
                            },
                        ]),
                        ports: Some(vec![ContainerPort {
                            container_port: i32::from(SUBSTRATE.port),
                            ..Default::default()
                        }]),
                        volume_mounts: Some(vec![VolumeMount {
                            name: "data".to_string(),
                            mount_path: SUBSTRATE.data_dir.to_string(),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ),
            volume_claim_templates: Some(vec![PersistentVolumeClaim {
                metadata: ObjectMeta {
                    name: Some("data".to_string()),
                    ..Default::default()
                },
                spec: Some(PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                    resources: Some(VolumeResourceRequirements {
                        requests: Some(BTreeMap::from([(
                            "storage".to_string(),
                            Quantity(format!("{volume_gb}Gi")),
                        )])),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// §6.5's "unavoidable" container defaults, minus the one that is a function of the program.
///
/// > Non-obvious defaults that should be *unavoidable*, because they are what separates "generated
/// > YAML" from "production-grade generated YAML": non-root + read-only root filesystem + dropped
/// > capabilities + `seccomp: RuntimeDefault` …
///
/// These three are constants: nothing a Beck program can do needs a Linux capability, needs to gain
/// privileges partway through, or needs a syscall outside the runtime's default profile. They are
/// applied to **every** container this emitter writes, including the substrate's, because they cost
/// a third-party image nothing either.
///
/// The fourth — `readOnlyRootFilesystem` — is not a constant and is passed in, because for the app
/// container it is a function of the program's effect row and for the substrate's it is a fact
/// about somebody else's image (`docs/82` §82.3).
fn hardened(read_only_root: Option<bool>) -> SecurityContext {
    SecurityContext {
        allow_privilege_escalation: Some(false),
        capabilities: Some(Capabilities {
            drop: Some(vec!["ALL".to_string()]),
            ..Default::default()
        }),
        seccomp_profile: Some(SeccompProfile {
            type_: "RuntimeDefault".to_string(),
            ..Default::default()
        }),
        read_only_root_filesystem: read_only_root,
        ..Default::default()
    }
}

/// The service partition: one binary, told which program to run.
#[allow(clippy::too_many_arguments)]
fn workload(
    app: &str,
    name: &str,
    replicas: u32,
    serves_ui: bool,
    reads_log: bool,
    writes_files: bool,
    reads_identity: bool,
) -> Deployment {
    let mut metadata = meta(app, name);
    metadata.annotations = Some(BTreeMap::from([(
        "beck.dev/serves-ui".to_string(),
        serves_ui.to_string(),
    )]));
    Deployment {
        metadata,
        spec: Some(DeploymentSpec {
            replicas: Some(replicas as i32),
            selector: selector(name),
            // §6.5 names this among the unavoidable defaults. Two is enough to roll back to and
            // enough to see what the last rollout changed; unbounded is a cluster keeping every
            // ReplicaSet a deploy has ever made.
            revision_history_limit: Some(2),
            template: pod(
                name,
                PodSpec {
                    security_context: Some(PodSecurityContext {
                        run_as_non_root: Some(true),
                        run_as_user: Some(65532),
                        ..Default::default()
                    }),
                    containers: vec![Container {
                        name: "app".to_string(),
                        image: Some(format!("{app}:dev")),
                        // The one derived bit of the security context: the root filesystem is
                        // read-only unless the program's own row says it writes a file.
                        security_context: Some(hardened(Some(!writes_files))),
                        args: Some(vec![
                            "run".to_string(),
                            APP_SOURCE.to_string(),
                            "--store".to_string(),
                            // The store follows the log: a program with no `durable` effect has no
                            // log store to point at, and telling it to use one is how a pod ends up
                            // waiting on a Secret nobody emitted.
                            //
                            // Both answers are stores the runtime does not keep in a *file*, which
                            // is what makes `readOnlyRootFilesystem` above sound: the deployed path
                            // writes nothing outside the substrate's own volume. A file-backed
                            // store here would need a volume, and would need that flag to know.
                            if reads_log { SUBSTRATE.store } else { "memory" }.to_string(),
                            "--addr".to_string(),
                            format!("0.0.0.0:{APP_PORT}"),
                        ]),
                        // Where the log is, and where the issuer is: two facts the program does
                        // not write down, supplied by the deployment that provisioned each.
                        env: {
                            let mut env = Vec::new();
                            if reads_log {
                                env.push(from_secret(
                                    "BECK_POSTGRES_URL",
                                    &credentials(app),
                                    "url",
                                ));
                            }
                            if reads_identity {
                                env.push(from_secret(
                                    PROVIDER.issuer_var,
                                    &format!("{app}-identity-credentials"),
                                    "issuer",
                                ));
                            }
                            (!env.is_empty()).then_some(env)
                        },
                        ports: Some(vec![ContainerPort {
                            container_port: APP_PORT,
                            ..Default::default()
                        }]),
                        readiness_probe: Some(http_probe("/readyz")),
                        liveness_probe: Some(http_probe("/healthz")),
                        // Stop accepting before the socket closes, so an in-flight patch stream is
                        // not cut mid-frame during a rollout.
                        lifecycle: Some(Lifecycle {
                            pre_stop: Some(LifecycleHandler {
                                sleep: Some(SleepAction { seconds: 5 }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// §3.5's "least-privilege infra, computed": the rules are the effect row, plus the two the
/// platform layer owes every pod.
///
/// # Three kinds of peer, and only two of them are the program's
///
/// * **In-cluster** — the log store. A pod selector and the port it listens on, which is exactly
///   what the `durable` effect implies and nothing more.
/// * **DNS** — not derived from anything the program says, and added anyway. A `NetworkPolicy`
///   with `policyTypes: [Ingress, Egress]` denies everything it does not name, *including port 53*.
///   Forgetting it is the classic generated-policy bug: the manifests look strict, the pod comes
///   up, and nothing resolves. Phase 0 knew this (§6.5) and the Phase 1 emitter did not.
/// * **External hosts** — and this is where the derivation stops being able to say what it means.
///
/// # The honest limit on external egress
///
/// A core `NetworkPolicy` egress peer is an `ipBlock`, a namespace selector or a pod selector. It
/// **cannot name a DNS host**. Phase 1 emitted `podSelector: {app: payments.example.com}` for a
/// `net.out(payments.example.com)` — a selector matching no pod, so the rule granted nothing, so
/// the program's own network call was denied by the policy derived from it. It rendered as YAML
/// that looked exactly like the feature working.
///
/// What is emitted instead is the tightest thing the API can actually express: egress on 443 to
/// everything *except* the cluster's own address space and the cloud metadata endpoint — the
/// standard SSRF target, and the one address a workload with a `net.out` effect most wants to be
/// unable to reach. The host list itself is recorded in a `beck.dev/egress-hosts` annotation,
/// because a CNI that does understand names (Cilium's `toFQDNs`, Calico's `NetworkSet`) can enforce
/// it exactly — and that is a `Platform` implementation's job, not core Kubernetes'.
///
/// So the claim this object supports is: *the program's `net.out` atoms are what open egress at
/// all, and removing one removes a rule.* The claim it does not support is: *only those hosts are
/// reachable.* The difference is written here rather than left in a slide.
fn policy(
    app: &str,
    name: &str,
    allow_ingress_from: &[String],
    allow_egress_to: &[crate::Peer],
    allow_egress_hosts: &[String],
) -> NetworkPolicy {
    let mut egress = vec![dns_egress()];
    for peer in allow_egress_to {
        egress.push(NetworkPolicyEgressRule {
            to: Some(vec![NetworkPolicyPeer {
                pod_selector: Some(LabelSelector {
                    match_labels: Some(BTreeMap::from([("app".to_string(), peer.app.clone())])),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ports: Some(vec![tcp(i32::from(peer.port))]),
        });
    }
    if !allow_egress_hosts.is_empty() {
        egress.push(external_egress());
    }

    let mut metadata = meta(app, name);
    if !allow_egress_hosts.is_empty() {
        // Provenance in the object, the same idea as `beck explain` (§4.7): a reviewer can see
        // which hosts the rule below is standing in for, and a `Platform` that can enforce names
        // has them without re-deriving anything.
        metadata.annotations = Some(BTreeMap::from([(
            "beck.dev/egress-hosts".to_string(),
            allow_egress_hosts.join(","),
        )]));
    }

    NetworkPolicy {
        metadata,
        spec: Some(NetworkPolicySpec {
            pod_selector: Some(LabelSelector {
                match_labels: Some(BTreeMap::from([("app".to_string(), app.to_string())])),
                ..Default::default()
            }),
            // Both listed, so everything not named above is denied.
            policy_types: Some(vec!["Ingress".to_string(), "Egress".to_string()]),
            ingress: (!allow_ingress_from.is_empty()).then(|| {
                vec![NetworkPolicyIngressRule {
                    from: Some(
                        allow_ingress_from
                            .iter()
                            .map(|p| NetworkPolicyPeer {
                                namespace_selector: Some(LabelSelector {
                                    match_labels: Some(BTreeMap::from([(
                                        "kubernetes.io/metadata.name".to_string(),
                                        p.clone(),
                                    )])),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            })
                            .collect(),
                    ),
                    ports: Some(vec![tcp(APP_PORT)]),
                }]
            }),
            egress: Some(egress),
        }),
    }
}

/// The rule no effect implies and every pod needs.
fn dns_egress() -> NetworkPolicyEgressRule {
    NetworkPolicyEgressRule {
        to: Some(vec![NetworkPolicyPeer {
            namespace_selector: Some(LabelSelector {
                match_labels: Some(BTreeMap::from([(
                    "kubernetes.io/metadata.name".to_string(),
                    "kube-system".to_string(),
                )])),
                ..Default::default()
            }),
            pod_selector: Some(LabelSelector {
                match_labels: Some(BTreeMap::from([(
                    "k8s-app".to_string(),
                    "kube-dns".to_string(),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        }]),
        ports: Some(vec![port(53, "UDP"), port(53, "TCP")]),
    }
}

/// Public egress on 443, with the cluster's own address space and the metadata endpoint removed.
///
/// The exclusions are the point. `0.0.0.0/0` would let a workload with one outbound API call reach
/// every other pod in the cluster and read the node's cloud credentials from `169.254.169.254`.
fn external_egress() -> NetworkPolicyEgressRule {
    NetworkPolicyEgressRule {
        to: Some(vec![NetworkPolicyPeer {
            ip_block: Some(IPBlock {
                cidr: "0.0.0.0/0".to_string(),
                except: Some(vec![
                    "10.0.0.0/8".to_string(),
                    "172.16.0.0/12".to_string(),
                    "192.168.0.0/16".to_string(),
                    "169.254.0.0/16".to_string(),
                ]),
            }),
            ..Default::default()
        }]),
        ports: Some(vec![tcp(443)]),
    }
}

fn tcp(number: i32) -> NetworkPolicyPort {
    port(number, "TCP")
}

fn port(number: i32, protocol: &str) -> NetworkPolicyPort {
    NetworkPolicyPort {
        port: Some(IntOrString::Int(number)),
        protocol: Some(protocol.to_string()),
        ..Default::default()
    }
}

/// Gateway API, by hand.
///
/// §6.3 chooses Gateway API over `Ingress` deliberately — websockets and timeouts are expressible
/// there and are annotations everywhere else. The cost is that these are CRDs, so this is the one
/// place in the emitter where a field name is checked by review rather than by the compiler. The
/// structs are `Deserialize` as well as `Serialize` so `tests/manifests.rs` can at least prove the
/// document reads back as the same object.
pub mod gateway {
    use serde::{Deserialize, Serialize};

    pub const API_VERSION: &str = "gateway.networking.k8s.io/v1";

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct HttpRoute {
        pub api_version: String,
        pub kind: String,
        pub metadata: Metadata,
        pub spec: HttpRouteSpec,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Metadata {
        pub name: String,
        pub namespace: String,
        #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty", default)]
        pub annotations: std::collections::BTreeMap<String, String>,
        #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty", default)]
        pub labels: std::collections::BTreeMap<String, String>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct HttpRouteSpec {
        pub parent_refs: Vec<ParentRef>,
        pub hostnames: Vec<String>,
        pub rules: Vec<Rule>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ParentRef {
        pub name: String,
        pub namespace: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Rule {
        pub matches: Vec<Match>,
        pub backend_refs: Vec<BackendRef>,
        /// `"0s"` means no request timeout, which is what a long-lived websocket needs.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        pub timeouts: Option<Timeouts>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Timeouts {
        pub request: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Match {
        pub path: PathMatch,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PathMatch {
        #[serde(rename = "type")]
        pub type_: String,
        pub value: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct BackendRef {
        pub name: String,
        pub port: i32,
    }

    /// The route the `ingress` effect implies: a websocket path, then everything else.
    ///
    /// The websocket rule is first because Gateway API matches most-specific-first only within a
    /// rule; two rules are tried in order, and `PathPrefix: /` would otherwise swallow `/socket`.
    pub fn http_route(app: &str, name: &str, host: &str, websocket: bool) -> HttpRoute {
        let backend = vec![BackendRef {
            name: app.to_string(),
            port: super::APP_PORT,
        }];
        let mut rules = Vec::new();
        if websocket {
            rules.push(Rule {
                matches: vec![Match {
                    path: PathMatch {
                        type_: "Exact".to_string(),
                        value: "/socket".to_string(),
                    },
                }],
                backend_refs: backend.clone(),
                timeouts: Some(Timeouts {
                    request: "0s".to_string(),
                }),
            });
        }
        rules.push(Rule {
            matches: vec![Match {
                path: PathMatch {
                    type_: "PathPrefix".to_string(),
                    value: "/".to_string(),
                },
            }],
            backend_refs: backend,
            timeouts: None,
        });
        HttpRoute {
            api_version: API_VERSION.to_string(),
            kind: "HTTPRoute".to_string(),
            metadata: Metadata {
                name: name.to_string(),
                namespace: app.to_string(),
                annotations: std::collections::BTreeMap::from([(
                    "beck.dev/websocket".to_string(),
                    websocket.to_string(),
                )]),
                labels: super::labels(app),
            },
            spec: HttpRouteSpec {
                parent_refs: vec![ParentRef {
                    name: "beck-gateway".to_string(),
                    namespace: crate::GATEWAY_NAMESPACE.to_string(),
                }],
                hostnames: vec![host.to_string()],
                rules,
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The small pieces every object shares
// ---------------------------------------------------------------------------------------------

fn to_value<T: serde::Serialize>(object: &T) -> Value {
    serde_json::to_value(object).expect("kubernetes objects are serialisable")
}

fn meta(app: &str, name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        namespace: Some(app.to_string()),
        labels: Some(labels(app)),
        ..Default::default()
    }
}

fn labels(app: &str) -> BTreeMap<String, String> {
    labels_with(app, [])
}

fn labels_with(
    app: &str,
    extra: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::from([
        ("app.kubernetes.io/name".to_string(), app.to_string()),
        (
            "app.kubernetes.io/managed-by".to_string(),
            "beck".to_string(),
        ),
    ]);
    out.extend(extra);
    out
}

/// The `matchLabels` of a workload and the `labels` of the pods it makes are the same map, built
/// once. Written twice, they drift, and a Deployment whose selector matches none of its own pods is
/// accepted by the API server and never becomes ready.
fn pod_labels(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("app".to_string(), name.to_string())])
}

fn selector(name: &str) -> LabelSelector {
    LabelSelector {
        match_labels: Some(pod_labels(name)),
        ..Default::default()
    }
}

fn pod(name: &str, spec: PodSpec) -> PodTemplateSpec {
    PodTemplateSpec {
        metadata: Some(ObjectMeta {
            labels: Some(pod_labels(name)),
            ..Default::default()
        }),
        spec: Some(spec),
    }
}

fn credentials(app: &str) -> String {
    format!("{app}-log-credentials")
}

fn log_url(app: &str) -> String {
    SUBSTRATE.url(&format!("{app}-log.{app}.svc"))
}

fn from_secret(var: &str, secret: &str, key: &str) -> EnvVar {
    EnvVar {
        name: var.to_string(),
        value_from: Some(EnvVarSource {
            secret_key_ref: Some(SecretKeySelector {
                name: secret.to_string(),
                key: key.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn http_probe(path: &str) -> Probe {
    Probe {
        http_get: Some(HTTPGetAction {
            path: Some(path.to_string()),
            port: IntOrString::Int(APP_PORT),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn slug(n: &Node) -> String {
    match n {
        Node::Namespace { .. } => "namespace".into(),
        Node::Image { .. } => "image".into(),
        Node::Workload { .. } => "workload".into(),
        Node::Route { .. } => "route".into(),
        Node::LogStore { .. } => "log-store".into(),
        Node::IdentityProvider { .. } => "identity".into(),
        Node::SnapshotSchedule { .. } => "snapshots".into(),
        Node::Secret { .. } => "secret".into(),
        Node::Policy { .. } => "policy".into(),
        Node::Grant { .. } => "grants".into(),
        Node::Service { headless: true, .. } => "log-service".into(),
        Node::Service { .. } => "service".into(),
    }
}

/// The image, declaratively — no Dockerfile, no daemon, no build-time package manager.
///
/// §6.2: because an apko build performs no arbitrary execution, the same config and package
/// versions yield the same digest on any machine. That property is measured — two builds of this
/// config produce a bit-identical digest (`docs/19-phase-1-report.md` §19.5).
///
/// It is also the reason for the shape below. **apko has no way to copy a file from the host**: an
/// image's contents come from packages and from nothing else, which is exactly what "performs no
/// arbitrary execution" buys. So the service binary cannot be dropped into the image — it has to
/// *be* a package, and [`melange`] is the config that makes one. Phase 0's hand-written apko config
/// assumed otherwise, hardlinking `/usr/bin/beck-p0` to a `/beck-p0` that nothing ever put there,
/// and the mistake was invisible until the build was run for the first time.
pub fn apko(graph: &InfraGraph) -> String {
    format!(
        "# Generated by `beck build` from {app}. Do not edit.\n\
         #\n\
         # The binary arrives as an APK, because apko copies nothing from the host — see\n\
         # `image.melange.yaml`, and build in that order:\n\
         #\n\
         #   melange keygen local.rsa\n\
         #   melange build image.melange.yaml --arch x86_64 --signing-key local.rsa \\\n\
         #       --source-dir . --out-dir ./packages\n\
         #   apko build image.apko.yaml {app}:dev {app}.tar\n\
         #\n\
         # Reproducibility is the property this file exists for; check it with two builds:\n\
         #   apko build … a.tar && apko build … b.tar && cmp a.tar b.tar\n\
         contents:\n  repositories:\n    - https://packages.wolfi.dev/os\n    \
         - '@local ./packages'\n  keyring:\n    \
         - https://packages.wolfi.dev/os/wolfi-signing.rsa.pub\n    - ./local.rsa.pub\n  \
         packages:\n{packages}\n\
         entrypoint:\n  command: /usr/bin/beck\n\n\
         cmd: run {APP_SOURCE} --store {store} --addr 0.0.0.0:{APP_PORT}\n\n\
         # Non-root, matching the generated pod's securityContext exactly. A mismatch here is the\n\
         # classic \"works locally, CrashLoopBackOff in the cluster\".\n\
         accounts:\n  groups:\n    - groupname: nonroot\n      gid: 65532\n  users:\n    \
         - username: nonroot\n      uid: 65532\n      gid: 65532\n  run-as: 65532\n\n\
         archs:\n  - x86_64\n  - aarch64\n\n\
         annotations:\n  org.opencontainers.image.description: >-\n    \
         The {app} application, compiled by Beck.\n",
        app = graph.app,
        store = SUBSTRATE.store,
        // One list, shared with the SBOM: an inventory assembled beside the image is an inventory
        // that can be wrong about it (`sbom`).
        packages = crate::sbom::packages(graph)
            .iter()
            .map(|p| format!("    - {}", p.apko_name()))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The package the service binary ships in.
///
/// There is no build step here worth the name — the binary is already compiler output — so the
/// pipeline is one `install`. That is the point: melange exists to turn an artefact into an APK,
/// not to build it, and keeping the pipeline trivial keeps §6.2's "no arbitrary execution" true of
/// the whole chain rather than only of the last link.
pub fn melange(graph: &InfraGraph) -> String {
    format!(
        "# Generated by `beck build` from {app}. Do not edit.\n\
         package:\n  name: {app}\n  version: 0.1.0\n  epoch: 0\n  \
         description: The {app} application, compiled by Beck\n\n\
         environment:\n  contents:\n    repositories:\n      \
         - https://packages.wolfi.dev/os\n    keyring:\n      \
         - https://packages.wolfi.dev/os/wolfi-signing.rsa.pub\n    packages:\n      - busybox\n\n\
         pipeline:\n  - runs: |\n      mkdir -p \"${{{{targets.destdir}}}}/usr/bin\" \"${{{{targets.destdir}}}}/app\"\n      \
         install -m755 beck \"${{{{targets.destdir}}}}/usr/bin/beck\"\n      \
         install -m644 app.beck \"${{{{targets.destdir}}}}{APP_SOURCE}\"\n",
        app = graph.app
    )
}
