# 28 — Releases and deployment pipelines

Three pipelines, kept separate: **releasing Beck itself**, **deploying what `beck build` emits**
for a user's application, and **deploying the project's own applications** — which per
[`10`](10-decisions.md) D15 are Beck apps and ride the second pipeline rather than getting one of
their own.

One discipline governs all three: a pipeline is an artefact, and an artefact nobody has executed
is a design document ([`19`](19-phase-1-report.md) §19.4 item 10, [`20`](20-phase-2-report.md)
§20.4 item 8). Every step is run by hand once before it is trusted, and every gate must be
demonstrably able to fail ([`22`](22-phase-3-report.md) §22.8).

## 28.1 What exists today

Continuous integration, and nothing downstream of it. Three workflows:

- **`compiler.yml`** — format, clippy, the full suite (differential and replay harnesses
  included), the sketch compiled/served/replayed, `beck test` over sketch + corpus + SICP with a
  deliberately-failing negative control, the slicer and incremental-analysis assertions,
  annotation-free placement, wire-compat, the object graph, a k3d admission job, a compose
  parity job, the thin-client size budget, the full `cargo-deny` check ([ADR-0004](adr/0004-full-cargo-deny-gate.md)), a Postgres
  service behind the log-contract test, an image built from the real package repository and signed
  and verified ([`99`](99-supply-chain-report.md)), and a release-profile measurements lane ([ADR-0006](adr/0006-ci-measurements-lane.md)).
- **`phase0.yml`** — the same discipline over the frozen baseline.
- **`docs.yml`** — every relative link in tracked markdown resolves.

Each workflow parses every workflow file, so a broken one is reported by another ([ADR-0005](adr/0005-workflows-cross-check.md)).
Branch-protection note: required checks must name jobs that actually run on a docs-only PR —
the paths filters mean the compiler jobs do not start there.

Not existing: no release has been cut, no artefact is published, no version number is meaningful
(`0.1.0` on `publish = false` crates), nothing built here is deployed. Phase 1 proved a Beck app
deploys to a real cluster ([`19`](19-phase-1-report.md) §19.5); the pipeline that would do it on
every merge is this document's subject, not its claim.

## 28.2 Releasing the compiler

The Phase 3 bullet that owned this — "`beck init ci`, apko image build in-process, cosign signing,
SBOM" ([`08`](08-roadmap.md)) — **is built** ([`92`](92-sbom-report.md),
[`99`](99-supply-chain-report.md)): there is a command that builds the image, a command that signs
it and a command that verifies the signature. What is left of this section is the *pipeline*, and
that is the harder half. Plus Phase 4's multi-arch and air-gapped items. In landing order:

1. **A release is a tag on a commit that passed the whole matrix.** No release-only build steps:
   the same `cargo build --release`, pinned toolchain and locked graph CI uses. A tag-triggered
   workflow builds `beck` for `x86_64/aarch64-linux-musl` and `aarch64-darwin`, re-runs the
   suite on the tagged commit, and refuses to publish on any red.
2. **The artefact set**: the static binary per target; ~~an apko-built~~ **a `beck`-built** OCI
   image ([`06`](06-kubernetes-and-packaging.md) §6.2, [`99`](99-supply-chain-report.md)); the SBOM
   — which `beck build` writes and which nothing yet **attaches** to the image; a cosign signature
   per artefact, **built** (§99.5), and a provenance attestation, **not**; checksums in the release
   notes. GitHub Releases first; §6.7's OCI/ORAS route in Phase 4. The Mere serves packages, not the
   compiler.
3. ~~**Reproducibility as a gate**: build the image twice, `diff` the digests~~ — **built, and per
   commit rather than per release** ([`99`](99-supply-chain-report.md) §99.4). Its limit is now
   written down too: the digest is stable for one package set, and the resolver takes the highest
   version the repository serves today, so it is not stable across weeks.
4. **Versioning before 1.0**: `0.x` minor per phase-sized increment; no stability promise until
   Phase 5's specification and deprecation policy. Until the wire-format commitment lands there,
   release notes state which `.becki`/envelope formats a release reads; the log format stamp
   ([`log.rs`](../compiler/crates/beck-rt/src/log.rs)) refuses silent misreads at runtime.

**The commands exist; the pipeline does not.** No tag has been cut, nothing is published, and the
one thing that would make a signature reachable by a consumer — a push, so cosign can find
`sha256-<digest>.sig` in a registry — is [`99`](99-supply-chain-report.md) §99.7's first row. The
first slice is still tag-triggered signed binaries for one target, and it is now a smaller job than
it was: the signing is a command.

## 28.3 The pipeline a user's application gets

`beck build` output is already deployment-shaped: the manifest directory applies cleanly
(`kubectl apply --dry-run=server` is a CI gate), grants derive from effects, images are
digest-pinned. Missing is the pipeline around it:

1. **`beck init ci`** — **built** ([`99`](99-supply-chain-report.md)) — emits a workflow mirroring
   our discipline:
   `beck check`, `beck test`, `beck check --wire-compat` against the deployed interface, then
   `beck build` and an image push on merge. Wire-compat is the load-bearing step — the §3.6/§4.3
   firewall placed where it stops a bad deploy rather than reporting one.
2. **GitOps is the default contract**: `beck build --out` writes manifests-only directories (a
   CI gate asserts it), so a controller pointed at the directory is already a deployment
   pipeline. `beck deploy` stays the imperative form for rungs 0–2 of §6.6's ladder.
3. **The operator carries the choreography** (Phase 4): quiesce → drain → snapshot →
   `migrate`/`upcast` → resume (§6.4); canaries via Gateway API + Argo Rollouts. The pipeline
   ends at "the desired state names a new digest and a migration"; the operator owns the stream
   surviving the transition. Until then, deploys are rolling-update only, and docs say so.

## 28.4 The project's own deployments

D15's dogfood, in adoption order ([`17`](17-playground.md) §17.6): playground **rung A** (a
static CDN artefact built by CI — the first thing this project deploys continuously), **rung B**
(riding Mode B's kernel work), then **rung C**, **beck.dev** and the **Mere** (Phase 4/5,
deployed by §28.3's pipeline — the registry entering production is the exit criterion). Each
gets its pipeline when it gets built, not before.

## 28.5 The gate schedule

Gates deliberately not added yet, and when each becomes due — so absence stays a decision:

| Gate | Due | Why not now |
|---|---|---|
| Compile-speed budgets (§13.7, rustc-perf model) | Phase 3, per §8.4 | Needs the harness; timings stay reported-only on shared runners, so any gate is on counts (the firewall number `beck-db` already asserts) |
| A browser job driving the compiled sketch | Phase 3, with client polish | phase0 holds this gate; add for the compiler when client work makes it more than a re-test of phase0's DOM |
| Image size / cold start (§8.3 item 4) | ~~With §28.2's release workflow~~ — **due now**: CI builds an image ([`99`](99-supply-chain-report.md)) | The reason for the deferral is gone — "nothing builds an image in CI" is no longer true. What the budget still waits on is the *right* binary: the image ships whichever `beck` built it, and until that is the static musl build §28.2 item 1 describes, a size number would be about the wrong artefact (§99.8) |
| Mutation testing ≥85% on checker/solver/splitter (§13.8) | Phase 3 tail, nightly lane | Too slow for PRs |
| Docs-as-tests: Beck blocks in docs checked (§13.6) | With the tutorial | Today's doc snippets are design sketches, deliberately ahead of the language |
| Per-session memory / arrangement-entry gates | Needs an owner | [`26`](26-arrangement-sharing-report.md) §26.10: exported, ungated, deferred by four reports; the value that should fail a build has to be defended, not guessed |

## 28.6 The review's ledger

Debts found by the Phase 3 review (2026-08) that no report names, smallest first:

- `engine.rs` and `plan.rs` (~2,600 lines, the newest code) have no in-module unit tests; the
  whole gate lives two crates downstream in end-to-end suites. `Arrangement`, `same()`,
  `Upstream`'s history-window fallback and `inner_key` are untestable in isolation today.
- `incremental.rs::RULES` and `plan.rs::Builder::prim` are two tables of "which primitives are
  incremental" with no test linking them; they disagree on `MapLen`/`MapGet`/`MapContains`
  (documented as stated-not-measured, but drift should be visible, not archaeological).
- `engine.rs` should split: the footprint/measurement block and `SharedDataflow` are separable
  modules, and the measurement-only public API (`footprint`, `fanout_footprint`, `work`) should
  be marked as such — nothing outside the release measurement suites uses it.
- Four hand-written `Core` child-traversals exist (`place.rs`, `incremental.rs`, `graph.rs`,
  `check.rs::resolve_types`); the `With.base` omission fixed in `graph.rs` argues for one shared
  `Core::children`.
- `main.rs` (1,241 lines): clap definitions and the explain/graph reporters belong in modules;
  three near-identical compile-and-report helpers should be one.
- redb 2→4: [ADR-0003](adr/0003-redb-held-at-2.md).
- `check.rs` (3,012 lines) and the remaining walls are already on the record
  ([`22`](22-phase-3-report.md) §22.6, [`27`](27-walls-report.md) §27.7,
  [`25`](25-benchmarks-and-expressiveness.md) §25.7) and are not repeated here.

## 28.7 What this document refuses

No timing thresholds on shared runners (§13.7; a gate that flakes gets deleted). No release
cadence promises: releases are cut when a phase report can stand behind them. No pipeline for an
artefact not yet executed by hand — this file inherits the discipline it documents.
