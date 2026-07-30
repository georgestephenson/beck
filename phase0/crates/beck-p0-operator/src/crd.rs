//! `BeckApplication` — the whole deployment plan as one object for the operator to reconcile
//! (§6.3's last row).
//!
//! The fields are exactly the ones the operator needs in order to do its two real jobs: ordering
//! (§6.4's quiesce → drain → snapshot → migrate → resume choreography) and provenance (refusing a
//! rollout whose wire operations are incompatible with what is currently serving, §4.3).

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "beck.dev",
    version = "v1alpha1",
    kind = "BeckApplication",
    namespaced,
    shortname = "beckapp",
    status = "BeckApplicationStatus",
    printcolumn = r#"{"name":"Phase","type":"string","jsonPath":".status.phase"}"#,
    printcolumn = r#"{"name":"Head","type":"integer","jsonPath":".status.headSeq"}"#,
    printcolumn = r#"{"name":"Image","type":"string","jsonPath":".spec.image"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct BeckApplicationSpec {
    /// Pinned by digest, never by tag: `beck build` is a pure function from source to digest
    /// (§6.2), and a deploy that cannot say exactly what it is running is not a deploy.
    pub image: String,
    pub replicas: i32,
    /// `postgres` or `embedded` — which substrate the durable fold lives on (§5.3).
    pub substrate: String,
    /// Content-derived digest of the program's wire operations (§4.3). The operator refuses a
    /// rollout that is incompatible with the version currently serving, unless overridden.
    pub wire_ops_digest: String,
    /// Present when accumulator or event types changed since the deployed signature. The build
    /// refuses to produce a plan without it (§3.9); the operator refuses to roll without running
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration: Option<Migration>,
    pub log: LogSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Migration {
    /// The `migrate : OldState -> NewState` function this deploy demands.
    pub migrate: String,
    /// Upcasters registered for events already in the log (§3.9).
    #[serde(default)]
    pub upcasters: Vec<String>,
    pub from_wire_ops_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LogSpec {
    /// `durable(retain=..., snapshot=...)` — the retention and snapshot policy hanging off the
    /// durable effect.
    pub retain_days: u32,
    pub snapshot_every: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BeckApplicationStatus {
    pub observed_generation: Option<i64>,
    /// Where in §6.4's choreography this application is.
    pub phase: Option<String>,
    /// The log position the serving version has folded to — `beck status`'s headline number.
    pub head_seq: Option<u64>,
    pub serving_wire_ops_digest: Option<String>,
    pub message: Option<String>,
    /// Source provenance for whatever went wrong (§6.4 responsibility 5).
    pub source: Option<String>,
}

/// The steps of a deploy that rides the stream (§6.4). Ordering is the operator's whole job, so it
/// is a value, and the decision that produces it is a pure function (see [`plan`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do: the observed generation matches and the workload is healthy.
    Steady,
    /// Refuse: the new version's wire operations are incompatible with what is serving.
    RefuseIncompatibleWire { serving: String, proposed: String },
    /// Refuse: types changed and no `migrate` was supplied.
    RefuseMissingMigration,
    /// Stop admitting commands at the gateway; buffer them there.
    Quiesce,
    /// Finish in-flight folds and snapshot the accumulator.
    DrainAndSnapshot,
    /// Run `migrate` against the snapshot and register upcasters for the log tail.
    Migrate { migrate: String },
    /// Start the new version folding from migrated snapshot + tail, then re-open ingress.
    Resume,
}

/// Decide the next step. Pure, and therefore testable without a cluster — which matters, because
/// the ordering rules here are the part of the operator that must never be wrong.
pub fn plan(
    spec: &BeckApplicationSpec,
    status: &BeckApplicationStatus,
    generation: i64,
    types_changed: bool,
) -> Step {
    let serving = status.serving_wire_ops_digest.as_deref();
    let up_to_date = status.observed_generation == Some(generation);

    if up_to_date {
        return Step::Steady;
    }

    if types_changed && spec.migration.is_none() {
        return Step::RefuseMissingMigration;
    }

    if let Some(serving) = serving {
        if serving != spec.wire_ops_digest {
            let compatible = spec
                .migration
                .as_ref()
                .is_some_and(|m| m.from_wire_ops_digest == serving);
            if !compatible {
                return Step::RefuseIncompatibleWire {
                    serving: serving.to_string(),
                    proposed: spec.wire_ops_digest.clone(),
                };
            }
        }
    }

    match status.phase.as_deref() {
        None | Some("Serving") => Step::Quiesce,
        Some("Quiescing") => Step::DrainAndSnapshot,
        Some("Drained") => match &spec.migration {
            Some(migration) => Step::Migrate {
                migrate: migration.migrate.clone(),
            },
            None => Step::Resume,
        },
        Some("Migrated") => Step::Resume,
        Some(_) => Step::Steady,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> BeckApplicationSpec {
        BeckApplicationSpec {
            image: "ghcr.io/beck/todo@sha256:0000".into(),
            replicas: 2,
            substrate: "postgres".into(),
            wire_ops_digest: "wire-v2".into(),
            migration: None,
            log: LogSpec {
                retain_days: 90,
                snapshot_every: 1000,
            },
        }
    }

    #[test]
    fn an_up_to_date_application_is_left_alone() {
        let status = BeckApplicationStatus {
            observed_generation: Some(7),
            ..Default::default()
        };
        assert_eq!(plan(&spec(), &status, 7, false), Step::Steady);
    }

    #[test]
    fn a_deploy_walks_quiesce_drain_migrate_resume_in_order() {
        let mut spec = spec();
        spec.migration = Some(Migration {
            migrate: "migrate_v1_to_v2".into(),
            upcasters: vec!["upcast_Added_v1".into()],
            from_wire_ops_digest: "wire-v1".into(),
        });
        let mut status = BeckApplicationStatus {
            observed_generation: Some(6),
            phase: Some("Serving".into()),
            serving_wire_ops_digest: Some("wire-v1".into()),
            ..Default::default()
        };

        assert_eq!(plan(&spec, &status, 7, true), Step::Quiesce);
        status.phase = Some("Quiescing".into());
        assert_eq!(plan(&spec, &status, 7, true), Step::DrainAndSnapshot);
        status.phase = Some("Drained".into());
        assert_eq!(
            plan(&spec, &status, 7, true),
            Step::Migrate {
                migrate: "migrate_v1_to_v2".into()
            }
        );
        status.phase = Some("Migrated".into());
        assert_eq!(plan(&spec, &status, 7, true), Step::Resume);
    }

    #[test]
    fn a_wire_incompatible_rollout_is_refused() {
        // "The deploy worked but every open browser tab broke" is the failure this exists to
        // prevent (§4.3).
        let status = BeckApplicationStatus {
            observed_generation: Some(6),
            phase: Some("Serving".into()),
            serving_wire_ops_digest: Some("wire-v1".into()),
            ..Default::default()
        };
        assert_eq!(
            plan(&spec(), &status, 7, false),
            Step::RefuseIncompatibleWire {
                serving: "wire-v1".into(),
                proposed: "wire-v2".into()
            }
        );
    }

    #[test]
    fn changed_types_without_a_migration_are_refused() {
        let status = BeckApplicationStatus {
            observed_generation: Some(6),
            phase: Some("Serving".into()),
            serving_wire_ops_digest: Some("wire-v2".into()),
            ..Default::default()
        };
        assert_eq!(
            plan(&spec(), &status, 7, true),
            Step::RefuseMissingMigration
        );
    }
}
