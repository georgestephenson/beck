//! The operator: a kube-rs control loop over `BeckApplication`.
//!
//! Deliberately limited to what needs a cluster-side control loop (§6.4): reconcile the object
//! graph, order the deploy, gate on wire compatibility, and report status with provenance. It does
//! not schedule, autoscale, route, store state, or manage certificates — Kubernetes, KEDA, the
//! Gateway implementation and cert-manager do those.
//!
//! Phase 0 ships the loop and the *decision* (`crd::plan`, which is pure and tested); the
//! individual choreography steps — quiesce at the gateway, drain folds, snapshot, run `migrate`,
//! resume — are Phase 4 work and are logged rather than performed. That boundary is stated in the
//! status message the operator writes, so nobody can mistake a stub for a rollout.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use kube::api::{Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::runtime::{watcher, Controller};
use kube::{Api, Client, ResourceExt};
use serde_json::json;

use crate::crd::{plan, BeckApplication, Step};
use crate::yaml;

struct Context {
    client: Client,
    /// Server-side apply field manager: Beck owns only the fields it sets, and coexists with other
    /// controllers (§6.3).
    field_manager: String,
}

pub async fn run() -> Result<()> {
    let client = Client::try_default().await?;
    let applications: Api<BeckApplication> = Api::all(client.clone());
    let context = Arc::new(Context {
        client,
        field_manager: "beck-operator".to_string(),
    });

    tracing::info!("watching BeckApplications");
    Controller::new(applications, watcher::Config::default())
        .shutdown_on_signal()
        .run(reconcile, error_policy, context)
        .for_each(|outcome| async move {
            match outcome {
                Ok((object, _)) => tracing::debug!(application = %object.name, "reconciled"),
                Err(e) => tracing::warn!(error = %e, "reconcile failed"),
            }
        })
        .await;
    Ok(())
}

async fn reconcile(
    application: Arc<BeckApplication>,
    context: Arc<Context>,
) -> Result<Action, kube::Error> {
    let name = application.name_any();
    let namespace = application.namespace().unwrap_or_else(|| "default".into());
    let generation = application.metadata.generation.unwrap_or(0);
    let status = application.status.clone().unwrap_or_default();

    // Phase 0 cannot know whether the accumulator's type changed: that answer comes from comparing
    // published signatures (§3.6), which needs a compiler. Until then, the presence of a migration
    // in the spec is the only signal, and the operator says so rather than guessing.
    let types_changed = application.spec.migration.is_some();
    let step = plan(&application.spec, &status, generation, types_changed);

    let (phase, message) = match &step {
        Step::Steady => ("Serving", "up to date".to_string()),
        Step::RefuseIncompatibleWire { serving, proposed } => (
            "Refused",
            format!(
                "wire operations {proposed} are incompatible with the serving version {serving}; \
                 supply a migration or mark the change @breaking"
            ),
        ),
        Step::RefuseMissingMigration => (
            "Refused",
            "the accumulator or event types changed and no migrate function was supplied"
                .to_string(),
        ),
        Step::Quiesce => (
            "Quiescing",
            "buffering commands at the gateway (Phase 4: perform)".to_string(),
        ),
        Step::DrainAndSnapshot => (
            "Drained",
            "draining in-flight folds and snapshotting (Phase 4: perform)".to_string(),
        ),
        Step::Migrate { migrate } => (
            "Migrated",
            format!("running {migrate} against the snapshot (Phase 4: perform)"),
        ),
        Step::Resume => (
            "Serving",
            "new version folding from migrated snapshot + tail; ingress re-opened \
             (Phase 4: perform)"
                .to_string(),
        ),
    };

    tracing::info!(%name, %namespace, ?step, "reconcile");

    let applications: Api<BeckApplication> = Api::namespaced(context.client.clone(), &namespace);
    let patch = json!({
        "apiVersion": "beck.dev/v1alpha1",
        "kind": "BeckApplication",
        "status": {
            "observedGeneration": generation,
            "phase": phase,
            "servingWireOpsDigest": application.spec.wire_ops_digest,
            "message": message,
            "source": "phase0/crates/beck-p0-operator/src/controller.rs",
        }
    });
    applications
        .patch_status(
            &name,
            &PatchParams::apply(&context.field_manager),
            &Patch::Apply(&patch),
        )
        .await?;

    Ok(match step {
        Step::Steady => Action::requeue(Duration::from_secs(300)),
        Step::RefuseIncompatibleWire { .. } | Step::RefuseMissingMigration => {
            Action::requeue(Duration::from_secs(60))
        }
        // A rollout in progress is re-examined promptly: the operator's whole value is ordering.
        _ => Action::requeue(Duration::from_secs(5)),
    })
}

fn error_policy(
    _application: Arc<BeckApplication>,
    error: &kube::Error,
    _context: Arc<Context>,
) -> Action {
    tracing::warn!(%error, "reconcile error; backing off");
    Action::requeue(Duration::from_secs(30))
}

/// The operator's own manifests. Its RBAC is the one place Beck *does* need Kubernetes API access,
/// and it is scoped to the objects it owns.
pub fn rbac() -> String {
    let namespace = "beck-system";
    let name = "beck-operator";
    yaml::documents(&[
        json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {"name": namespace}
        }),
        json!({
            "apiVersion": "v1",
            "kind": "ServiceAccount",
            "metadata": {"name": name, "namespace": namespace}
        }),
        json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "ClusterRole",
            "metadata": {"name": name},
            "rules": [
                {
                    "apiGroups": ["beck.dev"],
                    "resources": ["beckapplications", "beckapplications/status"],
                    "verbs": ["get", "list", "watch", "patch", "update"]
                },
                {
                    "apiGroups": ["apps"],
                    "resources": ["deployments", "statefulsets"],
                    "verbs": ["get", "list", "watch", "create", "patch"]
                },
                {
                    "apiGroups": [""],
                    "resources": ["services", "configmaps"],
                    "verbs": ["get", "list", "watch", "create", "patch"]
                },
                {
                    "apiGroups": ["gateway.networking.k8s.io"],
                    "resources": ["httproutes"],
                    "verbs": ["get", "list", "watch", "create", "patch"]
                },
                {
                    "apiGroups": ["batch"],
                    "resources": ["jobs", "cronjobs"],
                    "verbs": ["get", "list", "watch", "create", "patch"]
                }
            ]
        }),
        json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "ClusterRoleBinding",
            "metadata": {"name": name},
            "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": name},
            "subjects": [{"kind": "ServiceAccount", "name": name, "namespace": namespace}]
        }),
        json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {"name": name, "namespace": namespace},
            "spec": {
                "replicas": 1,
                "selector": {"matchLabels": {"app.kubernetes.io/name": name}},
                "template": {
                    "metadata": {"labels": {"app.kubernetes.io/name": name}},
                    "spec": {
                        "serviceAccountName": name,
                        "securityContext": {
                            "runAsNonRoot": true,
                            "runAsUser": 65532,
                            "seccompProfile": {"type": "RuntimeDefault"}
                        },
                        "containers": [{
                            "name": name,
                            "image": "ghcr.io/beck/phase0-operator@sha256:0000000000000000000000000000000000000000000000000000000000000000",
                            "args": ["run"],
                            "resources": {
                                "requests": {"cpu": "50m", "memory": "64Mi"},
                                "limits": {"memory": "128Mi"}
                            },
                            "securityContext": {
                                "allowPrivilegeEscalation": false,
                                "readOnlyRootFilesystem": true,
                                "capabilities": {"drop": ["ALL"]}
                            }
                        }]
                    }
                }
            }
        }),
    ])
}
