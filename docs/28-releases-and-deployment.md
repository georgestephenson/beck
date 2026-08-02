# 28 — Releases and deployment pipelines

This document is a plan, in a repository whose rule is that plans are stated from evidence
([`AGENTS.md`](../AGENTS.md)): each section says what exists today, with the gate that proves it,
before it says what comes next. It covers three pipelines that are usually conflated and should
not be — **releasing Beck itself**, **deploying what `beck build` emits** for a user's
application, and **deploying the project's own applications** (the playground, beck.dev, the
Mere), which per [`10`](10-decisions.md) D15 are Beck apps and therefore ride the second pipeline
rather than getting one of their own.

One discipline governs all three, learned the expensive way: **a pipeline is an artefact, and an
artefact nobody has executed is a design document** ([`19`](19-phase-1-report.md) §19.4 item 10;
[`20`](20-phase-2-report.md) §20.4 item 8, where the Phase 1 CI workflow turned out to have been
invalid YAML for an entire phase). Every step added to any pipeline below is run by hand once
before it is trusted, and every gate must be demonstrably able to fail
([`22`](22-phase-3-report.md) §22.8).

## 28.1 What exists today

Continuous integration, and nothing downstream of it. Three workflows:

- **`compiler.yml`** — format, clippy, the full test suite including the differential and replay
  harnesses, the sketch compiled/served/replayed, `beck test` over the sketch, the corpus and the
  SICP chapters (with a deliberately failing test proving the gate can fail), the slicer and
  incremental-analysis assertions, annotation-free placement, wire-compat, the object graph, a
  k3d admission job, a Docker Compose parity job, the thin-client size budget, and the full
  `cargo-deny` check. A live Postgres service backs the log-contract test, and a reported (never
  thresholded, per [`13`](13-testing.md) §13.7) measurements lane re-runs the release-profile
  suites the phase reports quote.
- **`phase0.yml`** — the same discipline over the frozen baseline.
- **`docs.yml`** — every relative link in tracked markdown resolves.

Both compiler and phase0 workflows carry a YAML-validity job over *every* workflow file, so
whichever file is broken, the other reports it — closing the §20.4 hole at last.

What does **not** exist: no release has ever been cut, no artefact is published anywhere, no
version number is meaningful (`0.1.0` is a placeholder on `publish = false` crates), and nothing
built from this repository is deployed. Phase 1 proved a Beck app deploys to a real cluster
([`19`](19-phase-1-report.md) §19.5); the *pipeline* that would do that on every merge does not
exist, and this document is the plan for it rather than a claim that it does.

## 28.2 Releasing the compiler

Owned by the Phase 3 bullet that is currently untouched — "`beck init ci`, apko image build
in-process, cosign signing, SBOM" ([`08`](08-roadmap.md)) — plus Phase 4's multi-arch and
air-gapped items. The plan, in the order the pieces should land:

1. **A release is a tag on a commit that passed the whole matrix.** No release-only build steps:
   the binary shipped is built by the same `cargo build --release` the sketch job already runs,
   on the same pinned toolchain (`rust-toolchain.toml`), from the same locked dependency graph
   (`Cargo.lock`, gated by `cargo-deny` — including advisories, and the licence policy of
   [`07`](07-dependencies.md)). A tag-triggered workflow builds `beck` for
   `x86_64/aarch64-linux-musl` and `aarch64-darwin`, runs the full suite once more on the tagged
   commit, and refuses to publish on any red.
2. **The artefact set**: the static binary per target; an apko-built OCI image of it
   ([`06`](06-kubernetes-and-packaging.md) §6.2 — daemonless, distroless, bit-reproducible: two
   builds of Phase 1's config produced one digest, [`19`](19-phase-1-report.md) §19.5); an SBOM
   (apko generates it); a cosign signature and provenance attestation per artefact. Checksums in
   the release notes. GitHub Releases is the first distribution channel; the OCI registry route
   of §6.7 (ORAS) arrives with Phase 4, and the Mere ([`16`](16-packages-and-ecosystem.md))
   serves *packages*, not the compiler, so it is not on this path.
3. **Reproducibility is a gate, not a hope**: the release workflow builds the image twice and
   `diff`s the digests, mechanising the §6.2 claim on every release rather than once in a report.
4. **Versioning before 1.0**: `0.x` minor per phase-sized increment, patch as needed; no
   stability promise is implied or stated until Phase 5's specification and deprecation policy
   ([`08`](08-roadmap.md) Phase 5). The wire-format stability commitment lands there too, and
   until it does, release notes must say which `.becki`/envelope formats a release reads —
   the log format stamp ([`log.rs`](../compiler/crates/beck-rt/src/log.rs)) already refuses silent misreads at runtime.

None of this is built. The first slice — a tag-triggered workflow producing signed binaries for
one target — is deliberately small and should land alongside the remaining Phase 3 supply-chain
work, because `beck init ci` (below) wants to emit for users what we already run for ourselves.

## 28.3 The pipeline a user's application gets

The output of `beck build` is already shaped for deployment — the manifest directory applies
cleanly (`kubectl apply --dry-run=server` is a CI gate), grants are derived from effects, images
are digest-pinned by construction (§6.2). What is missing is the pipeline around it:

1. **`beck init ci`** (Phase 3, untouched) emits a workflow for the user's repository that
   mirrors our own discipline: `beck check`, `beck test`, `beck check --wire-compat` against the
   published interface of what is currently deployed, then `beck build` and an image push on
   merge. The wire-compat step is the load-bearing one — it is the §3.6/§4.3 firewall placed
   where it stops a bad deploy instead of reporting one.
2. **GitOps is the default contract**: `beck build --out` writes manifests-only directories on
   purpose (a CI gate asserts it), so a controller pointed at that directory is already a
   deployment pipeline. `beck deploy` remains the imperative alternative for rung 0–2 of the
   parity ladder (§6.6).
3. **The operator carries the choreography** (Phase 4): quiesce → drain → snapshot →
   `migrate`/`upcast` → resume (§6.4), with canaries via Gateway API + Argo Rollouts. The
   pipeline's job ends at "the desired state names a new digest and a migration"; the operator's
   job is that the stream survives the transition. Until the operator exists, deploys are
   rolling-update only, and honest documentation says exactly that.

## 28.4 The project's own deployments

Per D15 these are dogfood, in adoption order ([`17`](17-playground.md) §17.6): the **playground
rung A** (static compile-to-WASM page — a CDN artefact built by CI, the first thing this project
will ever deploy continuously); **rung B** (the whole app in the tab, riding Mode B's kernel
work); **rung C** and **beck.dev** with the **Mere** (Phase 4/5 — operator workloads, deployed by
the §28.3 pipeline, which is the point: the registry entering production *is* the exit criterion,
[`08`](08-roadmap.md) Phase 4–5). Each of these gets its pipeline when it gets built, not before;
a deployment pipeline for an application that does not exist would be this document's own §20.4
violation.

## 28.5 The gate schedule

Gates this review chose *not* to add yet, and when they become due — kept here so their absence
stays a decision rather than an oversight:

| Gate | Due | Why not now |
|---|---|---|
| Compile-speed budgets (§13.7, rustc-perf model) | Phase 3, per §8.4's own table | Needs the harness stood up; timings on shared runners stay reported-only, so the gate is on *counts* (modules re-checked — the firewall number already asserted in `beck-db`) and the durations are published curves |
| A browser job driving the compiled sketch | Phase 3, with client polish | phase0 already holds this gate (`browser` job); the compiler regressed on it. Add when the client work makes it more than a re-test of phase0's DOM |
| Image size / cold start budgets (§8.3 item 4) | With §28.2's release workflow | Nothing builds an image in CI yet; the budget lands with the artefact |
| Mutation testing ≥85% on checker/solver/splitter (§13.8) | Phase 3 tail, nightly lane | Too slow for PRs; belongs beside the measurements lane, tracked not aspirational |
| Docs-as-tests: Beck code blocks in docs extracted and checked (§13.6) | With the tutorial (§8.3 item 6) | The docs' Beck snippets today are design sketches, many deliberately ahead of the language; gating them would freeze the design docs rather than the tutorial |
| Per-session memory / arrangement-entry gates | When a value can be defended | [`26`](26-arrangement-sharing-report.md) §26.10 names this the deferred half: the metrics are exported; the number that should fail a build has been deferred by four reports and needs an owner, not a guess |

## 28.6 What this document refuses

No timing thresholds on shared runners, ever — §13.7's rule has held through five phases and a
gate that flakes gets deleted. No release cadence promises: releases are cut when a phase report
can stand behind them. And no pipeline for an artefact that has not been executed by hand first —
this file inherits the discipline it documents.
