//! The single-host target: Docker Compose.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.4 draws
//! `SingleProcessPlatform` beside `KubernetesPlatform`, and
//! [`docs/06-kubernetes-and-packaging.md`](../../../../../docs/06-kubernetes-and-packaging.md) §6.6
//! rung 2 is `beck run --docker` — "one container, image sanity, works outside my machine". This is
//! that rung as an artefact rather than a flag, and it is the reason [`crate::platform::Platform`]
//! is a fact rather than a claim.
//!
//! # It exists to be unlike Kubernetes
//!
//! A second implementation that agreed with the first about everything would prove nothing. Compose
//! has **no namespaces, no label selectors, no CRDs, and no network policy**, and each absence
//! caught a place where Kubernetes had leaked into a supposedly neutral interface:
//!
//! * the manifest *directory* was a crate constant (`k8s`), because only one target had ever wanted
//!   one — Compose wants a single file;
//! * `up()` shelled out to `kubectl` from the generic layer;
//! * a [`crate::Node`] with no rendering here had nowhere to be reported, so a `Policy` would have
//!   been silently dropped and the deployment would have looked like the one the effects asked for
//!   while enforcing nothing. [`crate::platform::Platform::unsupported`] exists because of this
//!   file.
//!
//! # What it does and does not carry
//!
//! | node | Compose | faithful? |
//! |---|---|---|
//! | `Workload` | a service, with the same args, ports and healthcheck | yes |
//! | `LogStore` | a service plus a named volume | yes |
//! | `Service` | a published port on the service it selects | yes, for one host |
//! | `Secret` | environment variables on the service that reads them | **weaker** — a dev default, in the file |
//! | `Route` | the app's published port; there is no gateway | **weaker** — no hostname routing |
//! | `SnapshotSchedule` | an environment variable | yes |
//! | `Grant` | an init SQL file the log store runs at first start | yes |
//! | `Policy` | nothing — Compose cannot express egress rules | **no**, and it says so |
//! | `Namespace` | the project name | n/a |
//! | `Image` | the build context | n/a |
//!
//! The two "weaker" rows and the one "no" are the honest content of §6.6's ladder: rung 2 is for
//! image sanity and SQL fidelity, not for testing the policies. Rung 3 is where policies become
//! real, and that is what the rung exists for.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::platform::{Artefact, Platform};
use crate::substrate::DEFAULT as SUBSTRATE;
use crate::yaml;
use crate::{InfraGraph, Node};

/// Where the program is mounted into the container.
///
/// Compose *can* copy from the host — a bind mount — which apko deliberately cannot (§6.2). So rung
/// 2 mounts the source rather than baking it, which is why it starts in seconds and why it is not
/// the thing to trust about an image. The *path* is [`crate::APP_SOURCE`]'s parent, because the
/// binary is told to run that path whatever put it there.
const APP_MOUNT: &str = "/app";

pub struct Compose;

impl Platform for Compose {
    fn name(&self) -> &'static str {
        "compose"
    }

    fn manifest_dir(&self) -> &'static str {
        "compose"
    }

    fn manifests(&self, graph: &InfraGraph, wire_id: &str) -> Vec<Artefact> {
        let mut out = vec![(
            "compose.yaml".to_string(),
            yaml::to_yaml(&file(graph, wire_id)),
        )];
        if let Some(sql) = grants_sql(graph) {
            // Compose has no ConfigMap, so the grants become a file the log store's image runs on
            // first start — the same SQL, delivered the way this platform delivers things.
            out.push(("initdb/01-grants.sql".to_string(), sql));
        }
        out
    }

    fn unsupported(&self, graph: &InfraGraph) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for d in &graph.nodes {
            match &d.node {
                Node::Policy { name, .. } => out.push((
                    format!("Policy/{name}"),
                    "Docker Compose has no egress or ingress policy: the effect row's \
                     least-privilege rules (§3.5, §6.5) are not enforced on this platform. Use \
                     `--platform kubernetes` to test them."
                        .to_string(),
                )),
                Node::Route { name, host, .. } => out.push((
                    format!("Route/{name}"),
                    format!(
                        "there is no gateway on a single host, so `{host}` does not route: the app \
                         is published on a port instead."
                    ),
                )),
                Node::Secret { name, .. } => out.push((
                    format!("Secret/{name}"),
                    "Compose has no secret store: the development credentials are written into \
                     `compose.yaml` as environment variables, which is fine for rung 2 and is not \
                     a way to hold a real one."
                        .to_string(),
                )),
                _ => {}
            }
        }
        out
    }

    fn apply(&self, manifests: &std::path::Path) -> anyhow::Result<()> {
        use anyhow::Context as _;
        let file = manifests.join("compose.yaml");
        let status = std::process::Command::new("docker")
            .arg("compose")
            .arg("-f")
            .arg(&file)
            .arg("up")
            .arg("-d")
            .status()
            .context(
                "running docker — `beck up --platform compose` needs a container runtime; \
                 `beck run` deliberately needs nothing",
            )?;
        if !status.success() {
            anyhow::bail!("docker compose up failed");
        }
        Ok(())
    }
}

/// The whole `compose.yaml`, as a value.
fn file(graph: &InfraGraph, wire_id: &str) -> Value {
    let app = &graph.app;
    let mut services = serde_json::Map::new();
    let mut volumes = serde_json::Map::new();

    let log = log_store(graph);
    if let Some((name, volume_gb)) = &log {
        let _ = volume_gb;
        services.insert(name.clone(), log_service(graph, name));
        volumes.insert(format!("{name}-data"), json!({}));
    }
    services.insert(
        app.clone(),
        app_service(graph, log.as_ref().map(|(n, _)| n.as_str())),
    );

    let mut out = serde_json::Map::new();
    out.insert("name".to_string(), json!(app));
    out.insert("services".to_string(), Value::Object(services));
    if !volumes.is_empty() {
        out.insert("volumes".to_string(), Value::Object(volumes));
    }
    // The same provenance the Kubernetes namespace carries, in the only place Compose has for it.
    out.insert(
        "x-beck".to_string(),
        json!({
            "wire-id": wire_id,
            "generated-by": "beck build --platform compose",
            "note": "derived from the program's effects; see explain.txt. Do not edit.",
        }),
    );
    Value::Object(out)
}

/// The service partition.
fn app_service(graph: &InfraGraph, log: Option<&str>) -> Value {
    let app = &graph.app;
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let mut depends: Vec<String> = Vec::new();

    if let Some(log) = log {
        // The one place the two platforms must agree about a string: the URL's host is the log
        // store's address, which is a Service name there and a service name here.
        env.insert(SUBSTRATE.url_var.to_string(), SUBSTRATE.url(log));
        depends.push(log.to_string());
    }
    for d in &graph.nodes {
        if let Node::SnapshotSchedule { every_events, .. } = &d.node {
            env.insert(
                "BECK_SNAPSHOT_EVERY_EVENTS".to_string(),
                every_events.to_string(),
            );
        }
    }

    let mut svc = serde_json::Map::new();
    svc.insert("image".to_string(), json!(format!("{app}:dev")));
    svc.insert(
        "command".to_string(),
        json!([
            "run",
            crate::APP_SOURCE,
            "--store",
            if log.is_some() {
                SUBSTRATE.store
            } else {
                "memory"
            },
            "--addr",
            format!("0.0.0.0:{}", crate::APP_PORT),
        ]),
    );
    svc.insert(
        "ports".to_string(),
        json!([format!("{}:{}", crate::APP_PORT, crate::APP_PORT)]),
    );
    svc.insert("volumes".to_string(), json!([format!("..:{APP_MOUNT}:ro")]));
    debug_assert!(crate::APP_SOURCE.starts_with(APP_MOUNT));
    if !env.is_empty() {
        svc.insert("environment".to_string(), json!(env));
    }
    if !depends.is_empty() {
        svc.insert(
            "depends_on".to_string(),
            json!(depends
                .iter()
                .map(|d| (d.clone(), json!({"condition": "service_healthy"})))
                .collect::<serde_json::Map<_, _>>()),
        );
    }
    svc.insert(
        "healthcheck".to_string(),
        json!({
            "test": ["CMD", "wget", "-qO-", format!("http://127.0.0.1:{}/healthz", crate::APP_PORT)],
            "interval": "2s",
            "retries": 30,
        }),
    );
    Value::Object(svc)
}

/// The log store.
fn log_service(graph: &InfraGraph, name: &str) -> Value {
    let has_grants = graph
        .nodes
        .iter()
        .any(|d| matches!(d.node, Node::Grant { .. }));
    let mut volumes = vec![json!(format!("{name}-data:{}", SUBSTRATE.data_dir))];
    if has_grants {
        volumes.push(json!("./initdb:/docker-entrypoint-initdb.d:ro"));
    }
    json!({
        "image": SUBSTRATE.image,
        "environment": {
            "POSTGRES_PASSWORD": SUBSTRATE.dev_password(),
            "PGDATA": SUBSTRATE.pgdata(),
        },
        "volumes": volumes,
        "healthcheck": {
            "test": ["CMD-SHELL", "pg_isready -U postgres"],
            "interval": "1s",
            "retries": 30,
        },
    })
}

fn log_store(graph: &InfraGraph) -> Option<(String, u32)> {
    graph.nodes.iter().find_map(|d| match &d.node {
        Node::LogStore { name, volume_gb } => Some((name.clone(), *volume_gb)),
        _ => None,
    })
}

/// The same grants the Kubernetes ConfigMap carries, as an init script.
fn grants_sql(graph: &InfraGraph) -> Option<String> {
    let mut out = String::new();
    for d in &graph.nodes {
        if let Node::Grant {
            role,
            on,
            privileges,
        } = &d.node
        {
            out.push_str(
                "-- derived from the program's effects: it appends and reads, so it may not \
                 update or delete\n",
            );
            out.push_str(&format!(
                "GRANT {} ON {on} TO \"{role}\";\n",
                privileges.join(", ")
            ));
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_core::Effect;

    fn graph() -> InfraGraph {
        crate::derive(
            "app",
            &[
                (Effect::Ingress, "proposals".to_string()),
                (Effect::Durable, "st".to_string()),
                (
                    Effect::NetOut("payments.example.com".into()),
                    "charge".to_string(),
                ),
            ],
            true,
        )
    }

    #[test]
    fn the_app_reaches_the_log_store_by_the_name_the_file_gives_it() {
        // The single cross-object fact this platform has, and the one that breaks first: the URL's
        // host has to be a service the same file defines.
        let f = file(&graph(), "id");
        let services = f["services"].as_object().expect("services");
        let url = f["services"]["app"]["environment"]["BECK_POSTGRES_URL"]
            .as_str()
            .expect("a url");
        let host = url
            .rsplit_once('@')
            .and_then(|(_, rest)| rest.split(':').next())
            .expect("a host");
        assert!(
            services.contains_key(host),
            "the app points at `{host}`, which this file does not define: {:?}",
            services.keys().collect::<Vec<_>>()
        );
        assert!(f["services"]["app"]["depends_on"][host].is_object());
    }

    #[test]
    fn a_program_with_no_durable_fold_gets_no_log_store_and_no_credentials() {
        let g = crate::derive("app", &[(Effect::Ingress, "proposals".to_string())], true);
        let f = file(&g, "id");
        assert!(f["volumes"].is_null());
        assert_eq!(f["services"].as_object().expect("services").len(), 1);
        assert!(f["services"]["app"]["environment"]["BECK_POSTGRES_URL"].is_null());
        assert!(f["services"]["app"]["command"]
            .as_array()
            .expect("command")
            .contains(&json!("memory")));
    }

    #[test]
    fn what_this_platform_cannot_do_is_reported_rather_than_dropped() {
        // The reason `Platform::unsupported` exists. A Compose file that silently omits the
        // NetworkPolicy looks exactly like one where the policy is working.
        let gaps = Compose.unsupported(&graph());
        assert!(
            gaps.iter().any(|(what, _)| what.starts_with("Policy/")),
            "the egress policy is not enforceable here and must say so: {gaps:?}"
        );
        assert!(gaps.iter().any(|(what, _)| what.starts_with("Route/")));
        for (_, why) in &gaps {
            assert!(!why.is_empty());
        }
    }

    #[test]
    fn the_grants_are_the_same_grants() {
        // Two platforms, one derivation: the SQL is the program's, not the emitter's.
        let sql = grants_sql(&graph()).expect("a durable fold implies a grant");
        assert!(sql.contains("GRANT SELECT, INSERT"), "{sql}");
        assert!(!sql.contains("DELETE"), "{sql}");
    }
}
