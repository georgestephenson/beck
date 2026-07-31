# 06 — Kubernetes, containers and packaging

> **Your question:** *"I think we should probably also tap into containerisation and use kubernetes
> under the hood too. This is one language for: frontend, backend, database, IaC, containerisation."*

Agreed — with one strong condition, stated first because it determines whether the language is
adoptable.

## 6.1 The condition: Kubernetes is a backend, never a semantics

If `beck run hello.beck` requires a cluster, a registry, or a running container daemon, the language is
dead on arrival for the Python audience you're targeting. Kubernetes must be *one implementation* of the
`Platform` trait (§5.4), selected by a single line in a `deployment` block.

```
beck run          → single process, in-memory/embedded store, no container, no cluster
beck run --docker → single OCI container, for parity checks
beck up           → local k3d/kind cluster, real manifests, real operator
beck deploy       → remote cluster
```

The language's *semantics* — placement, effects, boundaries — are identical in all four. Only the
`Platform` differs. Everything that follows is about the two rightmost columns.

Corollary: **no Kubernetes vocabulary in the language surface.** The user writes `service`,
`autoscale`, `expose`, `store`, `job`, `queue`, `cron` — domain concepts. `Deployment`,
`HorizontalPodAutoscaler` and `HTTPRoute` are compiler output, in the same way `mov` is compiler output.

## 6.2 Container images: daemonless, reproducible, distroless

Beck's server artefacts are statically linked binaries (or WASM components). That is the easiest
possible input to a container image, and it means we never need a Dockerfile, a build daemon, or a
package manager at image-build time.

**Recommendation: [apko](https://github.com/chainguard-dev/apko) plus
[melange](https://github.com/chainguard-dev/melange) — always both.** Rationale:

- **Declarative, no shell.** An apko config is a package list plus metadata; because the build performs
  no arbitrary execution, images are **bit-for-bit reproducible** — the same config and package
  versions yield the same image digest on any machine. Measured, not assumed: two builds of Phase 1's
  config produced one digest ([`19`](19-phase-1-report.md) §19.5). That property is worth a great deal
  here: `beck build` becomes a pure function from source to digest, which is what makes the deployment
  plan cacheable, auditable and trustworthy.
- **…and melange is what that property makes mandatory.** This section originally read "(+ melange
  when native deps are needed)", which is wrong in a way that cannot be seen by reading: **apko copies
  nothing from the host.** That absence *is* the reproducibility story, so a config that hardlinks a
  binary from a path no package creates cannot build — it fails with `file does not exist` the first
  time it is run, and both Phase 0's config and the one `beck build` first emitted had exactly that
  shape. The binary has to *be* a package, and melange is the tool that makes one. `beck build` emits
  both configs, in build order ([`19`](19-phase-1-report.md) §19.5).
- **Distroless by default** (Wolfi-based): no shell, no package manager, tiny attack surface. A Beck
  service image should be ~10–20 MB, not ~900 MB.
- **SBOM generated automatically**, covering the complete contents.
- **No daemon, runs unprivileged** — works in CI and in a pod without Docker-in-Docker.

**The OS is substrate too — already.** A Beck service image contains, in effect, no operating
system: the compiler emits a statically linked binary (no libc dependency), and apko wraps it in a
distroless Wolfi base — no shell, no package manager, no init system, just CA certificates and
tzdata. There is no distro to choose, patch, or harden, because the userland *is* compiler output;
the kernel is the platform's job (the Kubernetes node, or the local machine at rung 0). This is the
container-shaped version of the MirageOS idea the seed conversation cited — app and OS compiled
together — without unikernel exoticism. The remaining rung, a **microVM/unikernel `Platform`**
(Firecracker-class isolation, boot-in-milliseconds), is a clean post-1.0 fit precisely because our
artefact is already a single static binary with a declared effect surface; nothing in v1 needs to
change to allow it. Likewise the WASI server target ([`05`](05-tier-lowering.md) §5.2) is the
zero-OS limit of the same idea.

Implementation: shell out to `apko` initially; move to writing the OCI layout directly from Rust
(`oci-client`/`oci-spec` crates) once the format is settled, so `beck build` is one process with no
external tools. Push with the registry API; **sign with Sigstore/cosign** and attach the SBOM and a
provenance attestation. `beck deploy` then pins by digest, never by tag.

**Rejected alternatives**: BuildKit (excellent, and the right answer *if* users need arbitrary build
steps — keep it as `builder = buildkit` for escape-hatch cases, e.g. FFI to a C library needing a
compile step); Docker/`docker build` (daemon, root, non-reproducible); Kaniko/Buildah (fine, but still
Dockerfile-shaped, which we don't need); Nix (superb reproducibility — and the original sketch's
`beck deploy` "emits a Nix closure" — but its learning curve would become *our* learning curve;
apko's bit-reproducible builds plus digest-pinned, content-addressed OCI artefacts deliver the
property the sketch wanted, referentially transparent deploys, with mainstream tooling. A
`NixPlatform` emitter remains a reasonable community contribution).

Also emit, from the same `InfraGraph`:

- a **static asset bundle** for the client tier (WASM + assets, content-hashed), served either from the
  service image or pushed to object storage/CDN;
- a **multi-arch** image index (`amd64` + `arm64`) — trivial for us since we cross-compile, and it
  removes a whole category of user pain.

## 6.3 Kubernetes object generation

From `service api` in §1.3, plus the effect information from §3, the compiler emits:

| Object | Derived from |
|---|---|
| `Deployment` / `StatefulSet` | `service` declaration; image digest; replica bounds (`StatefulSet` where a service hosts `durable` folds with local state) |
| `PersistentVolumeClaim` + snapshot `CronJob` | **the `durable` effect** — the original's "sees one durable fold, so it provisions one volume plus snapshotting", verbatim |
| `Service` | exposed ports |
| `HTTPRoute` + websocket route (**Gateway API**, not `Ingress`) | `expose = http(route=...)`; **the `ingress` effect** (`merge_clients()`) is what provisions the websocket path; TLS via cert-manager |
| `HorizontalPodAutoscaler` / **KEDA** `ScaledObject` | `autoscale = between(...)`; KEDA when the trigger is a queue depth or external metric |
| `ConfigMap` / `ExternalSecret` | config declarations; `secret[T]` values are *never* inlined into a manifest |
| `NetworkPolicy` | **the `net.*` effect set** — a service that only talks to Postgres gets exactly that egress rule |
| `ServiceAccount` + `Role` + `RoleBinding` | **the `cap.*`/k8s effect set** — least privilege, computed |
| `PodDisruptionBudget`, `topologySpreadConstraints` | replica count and HA settings |
| `Job` (pre-upgrade hook) | pending `migrate`/`upcast` plan ([`03`](03-type-and-effect-system.md) §3.9) |
| `CronJob` | `cron` declarations |
| Keycloak/Ory deployment, or external-issuer OIDC wiring | `identity = managed()` / `identity = external(issuer=...)` ([`10`](10-decisions.md) D6) |
| `BeckApplication` CR | the whole plan, for the operator to reconcile |

Non-obvious defaults that should be *unavoidable*, because they are what separates "generated YAML" from
"production-grade generated YAML": non-root + read-only root filesystem + dropped capabilities +
`seccomp: RuntimeDefault`; resource requests set and limits set for memory only (CPU limits cause
throttling pathologies); `revisionHistoryLimit`; anti-affinity across zones; `preStop` sleep for
connection draining; probes wired to the generated readiness endpoints.

**Resource requests** are a genuinely hard inference problem. Plan: v1 uses a per-language-construct
heuristic plus explicit override; v1.x records actual usage via the operator and OpenTelemetry and
proposes updated requests (`beck tune`), which is a right-sizing feature nothing in the ecosystem does
from source knowledge.

Rendering: build objects via **typed Rust structs** (`k8s-openapi`), apply with **server-side apply**
and a field manager, so Beck owns only the fields it sets and coexists with other controllers. Emit the
plain YAML too (`beck build --emit=manifests`) — teams with GitOps (Argo CD/Flux) must be able to commit
it, and refusing them is refusing half the market.

## 6.4 The Beck operator

Written with **[kube-rs](https://kube.rs/)** — the Rust Kubernetes client + controller runtime + CRD
derive macro, a CNCF Sandbox project, modelled on `client-go`/`controller-runtime`/`kubebuilder`.
Rationale beyond language consistency: reported production migrations to Rust operators show large
reliability and footprint wins (one report: 94% fewer operator crashes, 68% less resource
consumption). Our operator is also *small*, which is the main thing.

Responsibilities of the operator — deliberately limited to what needs a cluster-side control loop:

1. **Reconcile `BeckApplication`** → owned workloads/routes/policies (server-side apply, ownership
   references so deletion cascades).
2. **Deploys ride the stream** ([`03`](03-type-and-effect-system.md) §3.9): the rollout is the
   original's choreography made mechanical — quiesce ingress on the old version (commands buffer at
   the gateway), drain in-flight folds, snapshot, run `migrate` against the snapshot (and register
   `upcast`ers for the log tail), start the new version folding from migrated snapshot + tail,
   re-open ingress, let subscribers resume by `(subscription, seq)`. Old and new versions coexist
   only in the read path during the switch; the write path has exactly one owner at all times. For
   `external store`s (plain relational tables), fall back to classic expand → migrate → contract.
3. **Progressive delivery**: canary by traffic percentage using Gateway API weights, promoting on SLO
   metrics; delegate to **Argo Rollouts** rather than reimplementing analysis.
4. **Boundary-compatibility gate**: refuse a rollout whose wire operations are incompatible with the
   currently-serving version (§4.3), unless overridden. This is the operator earning its existence.
5. **Status back to the developer**: `beck status` shows real reconciliation state and, on failure, the
   *source location* that produced the failing object.
6. **Right-sizing telemetry** for `beck tune` (§6.3).

Explicit non-responsibilities: it does not schedule, autoscale, route, store state, or manage
certificates. Kubernetes, KEDA, the Gateway implementation, and cert-manager do those. The operator's
job is *ordering and provenance*.

## 6.5 Effects → policy (the strongest infra story)

Worth restating on its own because it is the part no existing tool can do. Because §3.2 gives us a
precise, checked effect set per service, the generated policy is exact rather than aspirational:

```
service api effects: { ingress, durable(orders), net.out(payments.example.com), log }

⇒ NetworkPolicy egress: [ kube-dns:53, postgres:5432, payments.example.com:443,
                          otel-collector:4317 ]                              DENY all else
                        (infrastructural egress — DNS, telemetry — is added by the
                         platform layer; forgetting DNS is the classic generated-policy bug)
⇒ Gateway:              websocket route (the ingress effect), rate-limited at the edge
⇒ Postgres grants:      INSERT on the log; ALL on api's own read models; nothing else —
                        no generic UPDATE/DELETE exists anywhere, because state changes
                        are events by construction
⇒ Volume + snapshots:   from durable(orders); retention per its declared policy
⇒ Role:                 [ ] (no Kubernetes API access needed)
⇒ No filesystem mounts beyond the volume, readOnlyRootFilesystem: true
```

Add a network call to the code and the policy changes with it, in the same commit, reviewable in the
same diff. Today those two facts live in different repositories, owned by different teams, and drift
within a week. **Effect-derived least privilege is the feature to lead with when selling this to a
platform team.**

### 6.5.1 What Kubernetes cannot express, and what is emitted instead

*Corrected in Phase 2 ([`20`](20-phase-2-report.md) §20.4 item 13).*

The sketch above writes `payments.example.com:443` as an egress peer. **A core `NetworkPolicy`
cannot say that.** Its egress peers are an `ipBlock`, a namespace selector or a pod selector; there
is no DNS name anywhere in the type. The Phase 1 emitter took the line above literally and produced
`podSelector: {app: payments.example.com}` — a selector matching no pod, so the rule granted
nothing, so the policy derived from the program denied the program's own network call. It rendered
as YAML that looked exactly like the feature working.

What is emitted now, and the claim each part supports:

| the effect says | the object says | enforced by Kubernetes? |
|---|---|---|
| `durable` | `podSelector: {app: <log>}`, port 5432 | yes, exactly |
| `ingress` | ingress from the gateway namespace, port 8080 | yes, exactly |
| *(nothing — the platform owes it)* | egress to `kube-dns`, port 53 UDP and TCP | yes; forgetting it denies name resolution and is the classic generated-policy bug |
| `net.out(h)` for any `h` | egress on 443 to `0.0.0.0/0` **except** `10/8`, `172.16/12`, `192.168/16` and `169.254/16`, plus the host list in a `beck.dev/egress-hosts` annotation | partly — see below |

So the supported claim is *the program's `net.out` atoms are what open outbound access at all, and
removing one removes a rule*. The unsupported claim is *only those hosts are reachable*. The
exclusions are what makes the weaker version still worth having: a workload with one outbound API
call cannot reach the rest of the cluster, and cannot reach `169.254.169.254` — the cloud metadata
endpoint, and the thing an SSRF is usually after.

Enforcing the host list exactly needs a CNI that understands names — Cilium's `toFQDNs`, Calico's
`NetworkSet` — which is a **`Platform` implementation's** decision and not core Kubernetes'. The
annotation exists so that such an implementation has the list without re-deriving it. §6.1's
`Platform` trait is where that belongs; [`20`](20-phase-2-report.md) §20.5 records that the trait
does not exist yet.

## 6.6 The dev→prod parity ladder

| Rung | Command | Runs | Storage | Startup | Purpose |
|---|---|---|---|---|---|
| 0 | `beck run` | one native process; client served from memory | in-memory folds + embedded append-only log (replayable) | < 1 s | daily development, hot reload on save |
| 1 | `beck run --pg` | one process | local Postgres (container or existing) | ~2 s | SQL-fidelity checks |
| 2 | `beck run --docker` | one container | ditto | ~5 s | image sanity, "works outside my machine" |
| 3 | `beck up` | local **k3s/k3d** cluster, real operator, real manifests | Postgres in-cluster | ~30 s | test the infra tier, policies, migrations |
| 4 | `beck deploy --to staging` | remote cluster | managed Postgres via Crossplane | minutes | pre-production |
| 5 | `beck deploy --to prod` | remote cluster | ditto, HA | minutes | production |

Rungs 0 and 3 are the ones that must be excellent. Rung 0 is where 99% of developer-minutes are spent;
rung 3 is what makes the Kubernetes claim credible. Use **k3s** (Apache-2.0, single binary, small) via
`k3d` for rung 3, in CI too.

Hot reload at rung 0 is a compiler feature, not a file watcher: Salsa (§4.6) recomputes only the dirty
queries, the client partition is swapped over a WebSocket preserving signal state where types are
unchanged, and the server partition is re-JITted with Cranelift. Target < 2 s edit→visible.

## 6.7 Packages distributed as OCI artefacts

A small unification worth taking: use **OCI registries as the package registry** via **ORAS**, rather
than building a bespoke package host.

- A Beck package version — a **tarn** ([`16`](16-packages-and-ecosystem.md) §16.2) — is an OCI
  artefact: compiled signatures (`.becki`), the source, prebuilt per-tier artefacts, and an SBOM —
  content-addressed by digest.
- Users authenticate to registries they already run (GHCR, ECR, Harbor, Artifactory); air-gapped and
  regulated environments are supported on day one, which is otherwise a multi-year problem.
- Signing and provenance reuse Sigstore/cosign — the same machinery as images (§6.2), so supply-chain
  verification is one code path rather than two.
- `beck.lock` pins digests. Combined with the reproducible image builds and the capability-restricted
  macro phase (§2.4), the whole pipeline from source to running pod is verifiable — a genuinely
  differentiated position.

## 6.8 What *not* to build

- **Not** our own scheduler, service mesh, ingress controller, or certificate manager.
- **Not** a Helm chart generator as the primary interface (emit manifests + a chart *if* asked; Helm's
  templating model is precisely what we're replacing).
- **Not** a CI system. Emit a GitHub Actions / GitLab CI workflow (`beck init ci`) that calls
  `beck build`/`beck deploy`, and stop there.
- **Not** a secrets store. Integrate External Secrets Operator / cloud KMS; `secret[T]` types make the
  integration safe (§3.5).
- **Not** a multi-cloud abstraction layer. Crossplane already is one; wrapping it would add a leak.
