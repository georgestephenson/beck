//! The seam between an [`InfraGraph`] and a thing that can run it.
//!
//! # Why this exists
//!
//! [`docs/06-kubernetes-and-packaging.md`](../../../../docs/06-kubernetes-and-packaging.md) §6.1:
//! "Kubernetes under the hood? Yes — as a **compiler backend** behind a `Platform` trait, never as
//! language semantics." [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.4
//! draws the same picture with three boxes under it — `KubernetesPlatform`,
//! `SingleProcessPlatform`, and "(+ later: Nomad, serverless…)".
//!
//! Ten places in the design documents promised this trait and no code declared it. `emit()` called
//! `k8s::render()` by name, so a second target was a refactor rather than an addition — the exact
//! criticism [`crate::backend`-style seams exist to prevent, and the one
//! [`beck_core::backend`](../../beck_core/backend/index.html)'s own header levels at Phase 1:
//! "that is not a narrow Phase 1 — it is a Phase 1 whose successor is a refactor rather than an
//! addition".
//!
//! What made it cheap to fix is that the *boundary* was already in the right place. [`crate::Node`]
//! has never contained a Kubernetes noun: it is `Workload`, `Route`, `Service`, `LogStore`,
//! `Secret`, `Policy`, `Grant`, `Peer`, and [`crate::derive`] produces them from an effect row
//! without knowing what a Deployment is. Only the emitter knew. So this module is a trait
//! declaration and a second implementation, not a rewrite.
//!
//! # A trait with one implementation is a claim, not a fact
//!
//! [`crate::compose`] is the second, and it is deliberately *not* a variation on the first: Docker
//! Compose has no namespaces, no label selectors, no CRDs and no network policy. A trait shaped so
//! that only Kubernetes could satisfy it would have looked fine with one implementation. Three
//! things had to change to fit both, and each was a place Kubernetes had leaked:
//!
//! 1. **The output directory is the platform's**, not a crate constant. `k8s/` and `compose/` are
//!    different because `kubectl apply -f <dir>` and `docker compose -f <file>` want different
//!    things.
//! 2. **Applying is the platform's**, not a `kubectl` call in [`crate::up`].
//! 3. **Some objects have no rendering on some platforms**, and that has to be *sayable* rather
//!    than silently skipped — see [`Platform::unsupported`], which is what makes "Compose cannot
//!    express an egress policy" appear in the output instead of being lost.
//!
//! # What a platform is not allowed to do
//!
//! Derive anything. The graph is the whole input, and it is produced from the program's effects
//! before any platform is chosen; a platform that decided *which* objects exist would be making a
//! deployment decision the program did not ask for, and §5.4's "infrastructure is a function of the
//! program" would stop being true. A platform renders, and reports what it cannot render.

use std::path::Path;

use anyhow::Result;

use crate::InfraGraph;

/// One file a platform wants written, relative to the output root.
pub type Artefact = (String, String);

/// A deployment target.
pub trait Platform: Send + Sync {
    /// What to call this on a command line and in a report.
    fn name(&self) -> &'static str;

    /// The subdirectory holding **only** the files an apply consumes, and nothing else.
    ///
    /// It exists because both consumers of this output — a person running `kubectl apply -f` and a
    /// GitOps controller watching a path — read *every* file in a directory. Mixing image configs
    /// in meant the API server was handed an apko file as an object with no `apiVersion`
    /// (docs/20 §20.4 item 14).
    fn manifest_dir(&self) -> &'static str;

    /// The files that describe the running system to this target.
    fn manifests(&self, graph: &InfraGraph, wire_id: &str) -> Vec<Artefact>;

    /// Build inputs — image configs and the like — written beside the manifest directory rather
    /// than inside it. Empty for a platform that builds nothing.
    fn build_inputs(&self, _graph: &InfraGraph) -> Vec<Artefact> {
        Vec::new()
    }

    /// The nodes this platform cannot express, with the reason.
    ///
    /// **Not a lint.** A platform that silently drops a `Policy` produces a deployment that looks
    /// like the one the effects asked for and enforces nothing, which is the failure mode
    /// [`docs/06`](../../../../docs/06-kubernetes-and-packaging.md) §6.5.1 records for
    /// FQDN egress. Whatever a platform returns here is written into the output and printed by
    /// `beck build`, so a gap is something a reader sees rather than something they infer.
    fn unsupported(&self, _graph: &InfraGraph) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Apply the emitted manifests to a live target — rung 3 of §6.6's parity ladder.
    fn apply(&self, manifests: &Path) -> Result<()>;
}

/// Every platform this build knows, in the order a `--platform` list should print them.
pub fn all() -> Vec<Box<dyn Platform>> {
    vec![
        Box::new(crate::k8s::Kubernetes),
        Box::new(crate::compose::Compose),
    ]
}

/// Look one up by name, for `--platform`.
pub fn by_name(name: &str) -> Option<Box<dyn Platform>> {
    all().into_iter().find(|p| p.name() == name)
}

/// The default target.
///
/// Kubernetes, because that is what §6.1 chose and what the operator (Phase 4) will drive. The
/// point of the trait is not that the default is in doubt; it is that the default is *a choice*
/// and lives in one place.
pub fn default_platform() -> Box<dyn Platform> {
    Box::new(crate::k8s::Kubernetes)
}
