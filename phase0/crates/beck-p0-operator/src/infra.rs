//! The `InfraGraph`: infrastructure as a function of the program (§5.4, §6.3, §6.5).
//!
//! The original sketch's claim is that a deploy "sees one durable fold, so it provisions one volume
//! plus snapshotting; sees `merge-clients`, so it provisions a websocket ingress". This module is
//! that sentence, executable. Nothing here is a template: [`InfraGraph::derive`] takes the
//! program's effect row and a service declaration and produces typed Kubernetes objects, so
//! adding a network call to the program changes the NetworkPolicy in the same commit.
//!
//! Phase 0 writes the effect row down by hand (`InfraGraph::todo_app`) because there is no effect
//! inference until Phase 2. Everything downstream of that row is real.

use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, StatefulSet, StatefulSetSpec};
use k8s_openapi::api::batch::v1::{CronJob, CronJobSpec, JobSpec, JobTemplateSpec};
use k8s_openapi::api::core::v1::{
    Capabilities, ConfigMap, Container, ContainerPort, EnvVar, EnvVarSource, HTTPGetAction,
    Lifecycle, LifecycleHandler, Namespace, PersistentVolumeClaim, PersistentVolumeClaimSpec,
    PodSecurityContext, PodSpec, PodTemplateSpec, Probe, SeccompProfile, SecretKeySelector,
    SecurityContext, Service, ServiceAccount, ServicePort, ServiceSpec, SleepAction,
    TopologySpreadConstraint, Volume, VolumeMount, VolumeResourceRequirements,
};
use k8s_openapi::api::networking::v1::{
    IPBlock, NetworkPolicy, NetworkPolicyEgressRule, NetworkPolicyIngressRule, NetworkPolicyPeer,
    NetworkPolicyPort, NetworkPolicySpec,
};
use k8s_openapi::api::policy::v1::{PodDisruptionBudget, PodDisruptionBudgetSpec};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use serde_json::{json, Value};

use crate::yaml;

/// A `service` declaration — domain vocabulary only. No Kubernetes nouns appear in the language
/// surface (§6.1).
#[derive(Clone, Debug)]
pub struct ServiceDecl {
    pub name: String,
    pub namespace: String,
    /// Pinned by digest. `beck deploy` never resolves a tag (§6.2).
    pub image: String,
    pub replicas: i32,
    pub port: i32,
}

/// A durable fold, with the retention and snapshot policy that hangs off the `durable` effect.
#[derive(Clone, Debug)]
pub struct DurableFold {
    pub name: String,
    pub snapshot_every: u64,
    pub retain_days: u32,
}

/// Where an outbound connection goes. The compiler knows this from `net.out(...)`; the platform
/// layer adds the infrastructural egress (DNS) that a hand-written policy always forgets.
#[derive(Clone, Debug)]
pub enum EgressTarget {
    ClusterDns,
    ClusterService {
        app: String,
        namespace: String,
    },
    /// `net.out("payments.example.com")` — the todo program has none, so this variant is only
    /// constructed by the tests that check the derivation reacts to a new network effect.
    #[allow(dead_code)]
    External {
        host: String,
        cidr: String,
    },
}

#[derive(Clone, Debug)]
pub struct Egress {
    pub name: String,
    pub target: EgressTarget,
    pub port: i32,
}

/// The effect row of the placed program, as §3.2 will compute it.
#[derive(Clone, Debug, Default)]
pub struct Effects {
    /// `merge_clients()` — the one nondeterministic stream constructor. Provisions the websocket
    /// route, and nothing else does.
    pub ingress: bool,
    pub durable: Vec<DurableFold>,
    pub net_out: Vec<Egress>,
    /// `cap.k8s.*`. Empty for this program, so it gets no Role and no mounted token.
    pub kube_api: Vec<String>,
    /// Not yet: Phase 0 ships no OpenTelemetry, so the collector egress is not invented here.
    pub telemetry: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Substrate {
    /// The log lives in PostgreSQL (§5.3 v1). Storage is a claim, not a volume on this pod.
    Postgres,
    /// The log lives in an embedded redb file — rung 0, and rung 3 without a database.
    Embedded,
}

pub struct InfraGraph {
    pub service: ServiceDecl,
    pub effects: Effects,
    pub substrate: Substrate,
}

impl InfraGraph {
    /// The derivation for *this* program: the todo sketch, deployed.
    ///
    /// Read it as the compiler would: `merge_clients()` ⇒ ingress; one `durable` fold ⇒ a log
    /// store, a snapshot schedule and (on the embedded substrate) a volume; a Postgres substrate ⇒
    /// one egress rule to 5432 and nothing else; no `cap.*` effects ⇒ no RBAC and no service
    /// account token.
    pub fn todo_app(substrate: Substrate) -> InfraGraph {
        let mut net_out = vec![Egress {
            name: "dns".into(),
            target: EgressTarget::ClusterDns,
            port: 53,
        }];
        if substrate == Substrate::Postgres {
            net_out.push(Egress {
                name: "log".into(),
                target: EgressTarget::ClusterService {
                    app: "beck-postgres".into(),
                    namespace: "beck-todo".into(),
                },
                port: 5432,
            });
        }

        InfraGraph::derive(
            ServiceDecl {
                name: "beck-todo".into(),
                namespace: "beck-todo".into(),
                // A digest, because a tag is not a deployment. Replace with the digest `apko` +
                // `cosign` produced for this build.
                image: "ghcr.io/beck/phase0-todo@sha256:0000000000000000000000000000000000000000000000000000000000000000".into(),
                replicas: 2,
                port: 8080,
            },
            Effects {
                ingress: true,
                durable: vec![DurableFold {
                    name: "todos".into(),
                    snapshot_every: 1000,
                    retain_days: 90,
                }],
                net_out,
                kube_api: vec![],
                telemetry: false,
            },
            substrate,
        )
    }

    pub fn derive(service: ServiceDecl, effects: Effects, substrate: Substrate) -> InfraGraph {
        InfraGraph {
            service,
            effects,
            substrate,
        }
    }

    fn labels(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "app.kubernetes.io/name".to_string(),
                self.service.name.clone(),
            ),
            (
                "app.kubernetes.io/managed-by".to_string(),
                "beck".to_string(),
            ),
        ])
    }

    fn meta(&self, name: &str) -> ObjectMeta {
        ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(self.service.namespace.clone()),
            labels: Some(self.labels()),
            ..Default::default()
        }
    }

    /// The emitted object graph, one file per concern, in apply order.
    pub fn files(&self) -> Vec<(String, String)> {
        vec![
            (
                "00-namespace.yaml".into(),
                yaml::documents(&[self.namespace()]),
            ),
            (
                "10-identity.yaml".into(),
                yaml::documents(&[self.service_account(), self.config()]),
            ),
            (
                "20-log-store.yaml".into(),
                yaml::documents(&self.log_store()),
            ),
            (
                "30-workload.yaml".into(),
                yaml::documents(&[self.workload(), self.service()]),
            ),
            ("40-route.yaml".into(), yaml::documents(&self.routes())),
            (
                "50-policy.yaml".into(),
                yaml::documents(&[self.network_policy(), self.disruption_budget()]),
            ),
            (
                "60-snapshots.yaml".into(),
                yaml::documents(&self.snapshot_schedules()),
            ),
            (
                "70-application.yaml".into(),
                yaml::documents(&[self.beck_application()]),
            ),
        ]
    }

    fn namespace(&self) -> Value {
        to_value(&Namespace {
            metadata: ObjectMeta {
                name: Some(self.service.namespace.clone()),
                labels: Some(self.labels()),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    /// No `cap.*` effects ⇒ no Role, no RoleBinding, and no mounted token. The absence is the
    /// feature (§6.5).
    fn service_account(&self) -> Value {
        assert!(
            self.effects.kube_api.is_empty(),
            "a program with cap.k8s effects would also emit a Role and RoleBinding here"
        );
        to_value(&ServiceAccount {
            metadata: self.meta(&self.service.name),
            automount_service_account_token: Some(false),
            ..Default::default()
        })
    }

    fn config(&self) -> Value {
        let mut data = BTreeMap::new();
        data.insert("BECK_STORE".to_string(), self.substrate_flag().to_string());
        data.insert("RUST_LOG".to_string(), "info".to_string());
        to_value(&ConfigMap {
            metadata: self.meta(&format!("{}-config", self.service.name)),
            data: Some(data),
            ..Default::default()
        })
    }

    fn substrate_flag(&self) -> &'static str {
        match self.substrate {
            Substrate::Postgres => "postgres",
            Substrate::Embedded => "redb",
        }
    }

    /// The `durable` effect's storage. On Postgres that is a database (in-cluster here, a
    /// Crossplane claim in a real estate — §5.4); on the embedded substrate it is a volume, which
    /// is why the workload becomes a StatefulSet.
    fn log_store(&self) -> Vec<Value> {
        match self.substrate {
            Substrate::Embedded => vec![json!({
                "apiVersion": "v1",
                "kind": "ConfigMap",
                "metadata": {"name": "beck-log-store", "namespace": self.service.namespace},
                "data": {
                    "note": "The embedded substrate stores the log on the workload's own volume; \
                            see the volumeClaimTemplate in 30-workload.yaml."
                }
            })],
            Substrate::Postgres => {
                let labels = BTreeMap::from([(
                    "app.kubernetes.io/name".to_string(),
                    "beck-postgres".to_string(),
                )]);
                let statefulset = StatefulSet {
                    metadata: ObjectMeta {
                        name: Some("beck-postgres".into()),
                        namespace: Some(self.service.namespace.clone()),
                        labels: Some(labels.clone()),
                        ..Default::default()
                    },
                    spec: Some(StatefulSetSpec {
                        service_name: Some("beck-postgres".into()),
                        replicas: Some(1),
                        selector: LabelSelector {
                            match_labels: Some(labels.clone()),
                            ..Default::default()
                        },
                        template: PodTemplateSpec {
                            metadata: Some(ObjectMeta {
                                labels: Some(labels.clone()),
                                ..Default::default()
                            }),
                            spec: Some(PodSpec {
                                containers: vec![Container {
                                    name: "postgres".into(),
                                    image: Some("postgres:17-alpine".into()),
                                    ports: Some(vec![ContainerPort {
                                        container_port: 5432,
                                        name: Some("postgres".into()),
                                        ..Default::default()
                                    }]),
                                    env: Some(vec![
                                        EnvVar {
                                            name: "POSTGRES_DB".into(),
                                            value: Some("beck".into()),
                                            ..Default::default()
                                        },
                                        EnvVar {
                                            name: "POSTGRES_PASSWORD".into(),
                                            value_from: Some(EnvVarSource {
                                                secret_key_ref: Some(SecretKeySelector {
                                                    name: "beck-postgres".into(),
                                                    key: "password".into(),
                                                    ..Default::default()
                                                }),
                                                ..Default::default()
                                            }),
                                            ..Default::default()
                                        },
                                        EnvVar {
                                            name: "PGDATA".into(),
                                            value: Some("/var/lib/postgresql/data/pgdata".into()),
                                            ..Default::default()
                                        },
                                    ]),
                                    volume_mounts: Some(vec![VolumeMount {
                                        name: "data".into(),
                                        mount_path: "/var/lib/postgresql/data".into(),
                                        ..Default::default()
                                    }]),
                                    ..Default::default()
                                }],
                                ..Default::default()
                            }),
                        },
                        volume_claim_templates: Some(vec![PersistentVolumeClaim {
                            metadata: ObjectMeta {
                                name: Some("data".into()),
                                ..Default::default()
                            },
                            spec: Some(PersistentVolumeClaimSpec {
                                access_modes: Some(vec!["ReadWriteOnce".into()]),
                                resources: Some(VolumeResourceRequirements {
                                    requests: Some(BTreeMap::from([(
                                        "storage".to_string(),
                                        Quantity("8Gi".into()),
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
                };

                let service = Service {
                    metadata: ObjectMeta {
                        name: Some("beck-postgres".into()),
                        namespace: Some(self.service.namespace.clone()),
                        labels: Some(labels.clone()),
                        ..Default::default()
                    },
                    spec: Some(ServiceSpec {
                        selector: Some(labels),
                        ports: Some(vec![ServicePort {
                            port: 5432,
                            target_port: Some(IntOrString::Int(5432)),
                            name: Some("postgres".into()),
                            ..Default::default()
                        }]),
                        cluster_ip: Some("None".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                vec![to_value(&statefulset), to_value(&service)]
            }
        }
    }

    fn container(&self) -> Container {
        let mut args = vec![
            "run".to_string(),
            "--store".to_string(),
            self.substrate_flag().to_string(),
            "--addr".to_string(),
            format!("0.0.0.0:{}", self.service.port),
        ];
        if let Some(fold) = self.effects.durable.first() {
            args.push("--snapshot-every".to_string());
            args.push(fold.snapshot_every.to_string());
        }
        if self.substrate == Substrate::Embedded {
            args.push("--redb-path".to_string());
            args.push("/var/lib/beck/log.redb".to_string());
        }

        let mut env = vec![EnvVar {
            name: "RUST_LOG".into(),
            value: Some("info".into()),
            ..Default::default()
        }];
        if self.substrate == Substrate::Postgres {
            // `secret[T]` values are never inlined into a manifest (§6.3): the connection string
            // arrives from a Secret, which External Secrets Operator populates.
            env.push(EnvVar {
                name: "BECK_PG".into(),
                value_from: Some(EnvVarSource {
                    secret_key_ref: Some(SecretKeySelector {
                        name: "beck-postgres".into(),
                        key: "url".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }

        Container {
            name: self.service.name.clone(),
            image: Some(self.service.image.clone()),
            args: Some(args),
            env: Some(env),
            ports: Some(vec![ContainerPort {
                container_port: self.service.port,
                name: Some("http".into()),
                ..Default::default()
            }]),
            // Probes wired to the generated endpoints (§6.3).
            liveness_probe: Some(probe("/healthz", self.service.port, 10)),
            readiness_probe: Some(probe("/readyz", self.service.port, 5)),
            startup_probe: Some(Probe {
                failure_threshold: Some(30),
                period_seconds: Some(2),
                http_get: Some(HTTPGetAction {
                    path: Some("/readyz".into()),
                    port: IntOrString::Int(self.service.port),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            resources: Some(k8s_openapi::api::core::v1::ResourceRequirements {
                requests: Some(BTreeMap::from([
                    ("cpu".to_string(), Quantity("100m".into())),
                    ("memory".to_string(), Quantity("64Mi".into())),
                ])),
                // Memory only. CPU limits cause throttling pathologies, so they are deliberately
                // absent (§6.3).
                limits: Some(BTreeMap::from([(
                    "memory".to_string(),
                    Quantity("256Mi".into()),
                )])),
                ..Default::default()
            }),
            security_context: Some(SecurityContext {
                allow_privilege_escalation: Some(false),
                read_only_root_filesystem: Some(true),
                capabilities: Some(Capabilities {
                    drop: Some(vec!["ALL".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            // A distroless image has no `sleep`, so the drain delay uses the native sleep action
            // rather than an exec probe — the sort of detail that only shows up when you actually
            // ship the distroless image (§6.2).
            lifecycle: Some(Lifecycle {
                pre_stop: Some(LifecycleHandler {
                    sleep: Some(SleepAction { seconds: 5 }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            volume_mounts: match self.substrate {
                Substrate::Embedded => Some(vec![VolumeMount {
                    name: "log".into(),
                    mount_path: "/var/lib/beck".into(),
                    ..Default::default()
                }]),
                Substrate::Postgres => None,
            },
            ..Default::default()
        }
    }

    fn pod_spec(&self) -> PodSpec {
        PodSpec {
            service_account_name: Some(self.service.name.clone()),
            automount_service_account_token: Some(false),
            termination_grace_period_seconds: Some(30),
            security_context: Some(PodSecurityContext {
                run_as_non_root: Some(true),
                run_as_user: Some(65532),
                run_as_group: Some(65532),
                fs_group: Some(65532),
                seccomp_profile: Some(SeccompProfile {
                    type_: "RuntimeDefault".into(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            topology_spread_constraints: Some(vec![TopologySpreadConstraint {
                max_skew: 1,
                topology_key: "topology.kubernetes.io/zone".into(),
                when_unsatisfiable: "ScheduleAnyway".into(),
                label_selector: Some(LabelSelector {
                    match_labels: Some(self.labels()),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            containers: vec![self.container()],
            volumes: match self.substrate {
                Substrate::Embedded => None,
                Substrate::Postgres => Some(vec![Volume {
                    name: "tmp".into(),
                    empty_dir: Some(Default::default()),
                    ..Default::default()
                }]),
            },
            ..Default::default()
        }
    }

    /// A `StatefulSet` when the service hosts a durable fold with *local* state, a `Deployment`
    /// when the log lives elsewhere (§6.3). The choice is derived, not configured.
    fn workload(&self) -> Value {
        let template = PodTemplateSpec {
            metadata: Some(ObjectMeta {
                labels: Some(self.labels()),
                ..Default::default()
            }),
            spec: Some(self.pod_spec()),
        };
        let selector = LabelSelector {
            match_labels: Some(self.labels()),
            ..Default::default()
        };

        match self.substrate {
            Substrate::Postgres => to_value(&Deployment {
                metadata: self.meta(&self.service.name),
                spec: Some(DeploymentSpec {
                    replicas: Some(self.service.replicas),
                    revision_history_limit: Some(3),
                    selector,
                    template,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            Substrate::Embedded => to_value(&StatefulSet {
                metadata: self.meta(&self.service.name),
                spec: Some(StatefulSetSpec {
                    service_name: Some(self.service.name.clone()),
                    // One writer: the log has exactly one sequencer (§3.7).
                    replicas: Some(1),
                    revision_history_limit: Some(3),
                    selector,
                    template,
                    volume_claim_templates: Some(vec![PersistentVolumeClaim {
                        metadata: ObjectMeta {
                            name: Some("log".into()),
                            ..Default::default()
                        },
                        spec: Some(PersistentVolumeClaimSpec {
                            access_modes: Some(vec!["ReadWriteOnce".into()]),
                            resources: Some(VolumeResourceRequirements {
                                requests: Some(BTreeMap::from([(
                                    "storage".to_string(),
                                    Quantity("10Gi".into()),
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
            }),
        }
    }

    fn service(&self) -> Value {
        to_value(&Service {
            metadata: self.meta(&self.service.name),
            spec: Some(ServiceSpec {
                selector: Some(self.labels()),
                ports: Some(vec![ServicePort {
                    name: Some("http".into()),
                    port: 80,
                    target_port: Some(IntOrString::Int(self.service.port)),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    /// The `ingress` effect is what provisions the websocket path — Gateway API, not `Ingress`
    /// (§6.3). Gateway API types are CRDs, so they are emitted as plain objects.
    fn routes(&self) -> Vec<Value> {
        if !self.effects.ingress {
            return vec![];
        }
        vec![json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "name": self.service.name,
                "namespace": self.service.namespace,
                "labels": self.labels(),
            },
            "spec": {
                "parentRefs": [{"name": "beck-gateway", "namespace": "gateway-system"}],
                "hostnames": ["todo.beck.localhost"],
                "rules": [
                    {
                        // The websocket path exists because the program calls merge_clients().
                        "matches": [{"path": {"type": "Exact", "value": "/socket"}}],
                        "backendRefs": [{"name": self.service.name, "port": 80}],
                        "timeouts": {"request": "0s"}
                    },
                    {
                        "matches": [{"path": {"type": "PathPrefix", "value": "/"}}],
                        "backendRefs": [{"name": self.service.name, "port": 80}]
                    }
                ]
            }
        })]
    }

    /// Effect-derived least privilege (§6.5): exactly the egress the effect row implies, and
    /// nothing else.
    fn network_policy(&self) -> Value {
        let mut egress = Vec::new();
        let mut rules: Vec<Egress> = self.effects.net_out.clone();
        if self.effects.telemetry {
            // Infrastructural egress the platform layer adds, not the program (§6.5). Phase 0
            // exports no traces, so this is off and the collector rule does not appear.
            rules.push(Egress {
                name: "telemetry".into(),
                target: EgressTarget::ClusterService {
                    app: "otel-collector".into(),
                    namespace: "observability".into(),
                },
                port: 4317,
            });
        }

        for rule in &rules {
            let (peers, ports) = match &rule.target {
                EgressTarget::ClusterDns => (
                    vec![NetworkPolicyPeer {
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
                    }],
                    // Forgetting DNS is the classic generated-policy bug, so the platform layer
                    // adds it rather than the program having to ask (§6.5).
                    vec![port(rule.port, "UDP"), port(rule.port, "TCP")],
                ),
                EgressTarget::ClusterService { app, namespace } => (
                    vec![NetworkPolicyPeer {
                        namespace_selector: Some(LabelSelector {
                            match_labels: Some(BTreeMap::from([(
                                "kubernetes.io/metadata.name".to_string(),
                                namespace.clone(),
                            )])),
                            ..Default::default()
                        }),
                        pod_selector: Some(LabelSelector {
                            match_labels: Some(BTreeMap::from([(
                                "app.kubernetes.io/name".to_string(),
                                app.clone(),
                            )])),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    vec![port(rule.port, "TCP")],
                ),
                EgressTarget::External { cidr, .. } => (
                    vec![NetworkPolicyPeer {
                        ip_block: Some(IPBlock {
                            cidr: cidr.clone(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    vec![port(rule.port, "TCP")],
                ),
            };
            egress.push(NetworkPolicyEgressRule {
                to: Some(peers),
                ports: Some(ports),
            });
        }

        let ingress = if self.effects.ingress {
            vec![NetworkPolicyIngressRule {
                from: Some(vec![NetworkPolicyPeer {
                    namespace_selector: Some(LabelSelector {
                        match_labels: Some(BTreeMap::from([(
                            "kubernetes.io/metadata.name".to_string(),
                            "gateway-system".to_string(),
                        )])),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]),
                ports: Some(vec![port(self.service.port, "TCP")]),
            }]
        } else {
            vec![]
        };

        // Provenance, so a reviewer can see which effect produced which rule — the same idea as
        // `beck explain` (§4.7), carried into the generated object.
        let mut metadata = self.meta(&self.service.name);
        metadata.annotations = Some(BTreeMap::from([(
            "beck.dev/derived-from".to_string(),
            rules
                .iter()
                .map(|rule| format!("net.out({})", rule.name))
                .chain(self.effects.ingress.then(|| "merge_clients()".to_string()))
                .collect::<Vec<_>>()
                .join(", "),
        )]));

        to_value(&NetworkPolicy {
            metadata,
            spec: Some(NetworkPolicySpec {
                pod_selector: Some(LabelSelector {
                    match_labels: Some(self.labels()),
                    ..Default::default()
                }),
                // Both listed, so everything not named above is denied.
                policy_types: Some(vec!["Ingress".into(), "Egress".into()]),
                ingress: Some(ingress),
                egress: Some(egress),
            }),
        })
    }

    fn disruption_budget(&self) -> Value {
        to_value(&PodDisruptionBudget {
            metadata: self.meta(&self.service.name),
            spec: Some(PodDisruptionBudgetSpec {
                min_available: Some(IntOrString::Int(if self.service.replicas > 1 {
                    1
                } else {
                    0
                })),
                selector: Some(LabelSelector {
                    match_labels: Some(self.labels()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    /// One `CronJob` per durable fold — "sees one durable fold, so it provisions one volume plus
    /// snapshotting", verbatim.
    fn snapshot_schedules(&self) -> Vec<Value> {
        self.effects
            .durable
            .iter()
            .map(|fold| {
                let name = format!("{}-snapshot-{}", self.service.name, fold.name);
                to_value(&CronJob {
                    metadata: self.meta(&name),
                    spec: Some(CronJobSpec {
                        schedule: "0 * * * *".into(),
                        concurrency_policy: Some("Forbid".into()),
                        successful_jobs_history_limit: Some(3),
                        failed_jobs_history_limit: Some(3),
                        job_template: JobTemplateSpec {
                            spec: Some(JobSpec {
                                backoff_limit: Some(2),
                                template: PodTemplateSpec {
                                    metadata: Some(ObjectMeta {
                                        labels: Some(self.labels()),
                                        ..Default::default()
                                    }),
                                    spec: Some(PodSpec {
                                        restart_policy: Some("OnFailure".into()),
                                        service_account_name: Some(self.service.name.clone()),
                                        automount_service_account_token: Some(false),
                                        security_context: self.pod_spec().security_context,
                                        containers: vec![Container {
                                            name: "snapshot".into(),
                                            image: Some(self.service.image.clone()),
                                            args: Some(vec![
                                                "snapshot".into(),
                                                "--store".into(),
                                                self.substrate_flag().to_string(),
                                            ]),
                                            env: self.container().env,
                                            security_context: self.container().security_context,
                                            ..Default::default()
                                        }],
                                        ..Default::default()
                                    }),
                                },
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            })
            .collect()
    }

    /// The whole plan, for the operator to reconcile.
    fn beck_application(&self) -> Value {
        let fold = self.effects.durable.first();
        json!({
            "apiVersion": "beck.dev/v1alpha1",
            "kind": "BeckApplication",
            "metadata": {"name": self.service.name, "namespace": self.service.namespace},
            "spec": {
                "image": self.service.image,
                "replicas": self.service.replicas,
                "substrate": self.substrate_flag(),
                // Phase 0 has no compiler, so this stands in for sha256(module, name, signature)
                // over the program's wire operations (§4.3).
                "wireOpsDigest": "phase0-todo-v1",
                "log": {
                    "retainDays": fold.map_or(90, |f| f.retain_days),
                    "snapshotEvery": fold.map_or(1000, |f| f.snapshot_every),
                }
            }
        })
    }
}

fn probe(path: &str, port: i32, period: i32) -> Probe {
    Probe {
        http_get: Some(HTTPGetAction {
            path: Some(path.to_string()),
            port: IntOrString::Int(port),
            ..Default::default()
        }),
        period_seconds: Some(period),
        timeout_seconds: Some(2),
        ..Default::default()
    }
}

fn port(number: i32, protocol: &str) -> NetworkPolicyPort {
    NetworkPolicyPort {
        port: Some(IntOrString::Int(number)),
        protocol: Some(protocol.to_string()),
        ..Default::default()
    }
}

fn to_value<T: serde::Serialize>(object: &T) -> Value {
    serde_json::to_value(object).expect("kubernetes objects are serialisable")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
        &files
            .iter()
            .find(|(file, _)| file == name)
            .unwrap_or_else(|| panic!("no file {name}"))
            .1
    }

    #[test]
    fn a_durable_fold_provisions_a_snapshot_schedule() {
        let graph = InfraGraph::todo_app(Substrate::Postgres);
        let files = graph.files();
        assert!(find(&files, "60-snapshots.yaml").contains("kind: CronJob"));

        let mut without = InfraGraph::todo_app(Substrate::Postgres);
        without.effects.durable.clear();
        assert!(!without
            .files()
            .iter()
            .any(|(name, body)| name == "60-snapshots.yaml" && body.contains("CronJob")));
    }

    #[test]
    fn the_ingress_effect_is_what_provisions_the_websocket_route() {
        let graph = InfraGraph::todo_app(Substrate::Postgres);
        let route = find(&graph.files(), "40-route.yaml").to_string();
        assert!(route.contains("HTTPRoute"));
        assert!(route.contains("/socket"));

        let mut without = InfraGraph::todo_app(Substrate::Postgres);
        without.effects.ingress = false;
        assert_eq!(find(&without.files(), "40-route.yaml"), "");
    }

    #[test]
    fn the_policy_contains_exactly_the_declared_egress() {
        let graph = InfraGraph::todo_app(Substrate::Postgres);
        let policy = find(&graph.files(), "50-policy.yaml").to_string();
        assert!(policy.contains("kube-dns"));
        assert!(policy.contains("beck-postgres"));
        assert!(policy.contains("5432"));
        // Nothing was invented: Phase 0 exports no telemetry, so no collector egress appears.
        assert!(!policy.contains("4317"));

        // Add a network effect, and the policy changes with it — in the same commit.
        let mut with_payments = InfraGraph::todo_app(Substrate::Postgres);
        with_payments.effects.net_out.push(Egress {
            name: "payments".into(),
            target: EgressTarget::External {
                host: "payments.example.com".into(),
                cidr: "203.0.113.0/24".into(),
            },
            port: 443,
        });
        let policy = find(&with_payments.files(), "50-policy.yaml").to_string();
        assert!(policy.contains("203.0.113.0/24"));
        assert!(policy.contains("443"));
    }

    #[test]
    fn local_state_makes_it_a_statefulset_and_a_remote_log_makes_it_a_deployment() {
        assert!(find(
            &InfraGraph::todo_app(Substrate::Embedded).files(),
            "30-workload.yaml"
        )
        .contains("kind: StatefulSet"));
        assert!(find(
            &InfraGraph::todo_app(Substrate::Postgres).files(),
            "30-workload.yaml"
        )
        .contains("kind: Deployment"));
    }

    #[test]
    fn the_generated_pod_is_locked_down_by_default() {
        let workload = find(
            &InfraGraph::todo_app(Substrate::Postgres).files(),
            "30-workload.yaml",
        )
        .to_string();
        for required in [
            "runAsNonRoot: true",
            "readOnlyRootFilesystem: true",
            "allowPrivilegeEscalation: false",
            "type: RuntimeDefault",
            "automountServiceAccountToken: false",
            "revisionHistoryLimit: 3",
        ] {
            assert!(workload.contains(required), "missing `{required}`");
        }
        // No secret is ever inlined into a manifest.
        assert!(workload.contains("secretKeyRef"));
        assert!(!workload.contains("postgres://"));
    }

    #[test]
    fn images_are_pinned_by_digest() {
        let workload = find(
            &InfraGraph::todo_app(Substrate::Postgres).files(),
            "30-workload.yaml",
        )
        .to_string();
        let image_line = workload
            .lines()
            .find(|line| line.trim_start().starts_with("image: ghcr.io/beck"))
            .expect("an image line");
        assert!(image_line.contains("@sha256:"), "{image_line}");
    }
}
