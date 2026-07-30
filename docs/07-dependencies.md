# 07 — Dependencies

> **Your constraint:** *"any dependencies should be both: the highest performing in their class, and
> open source."*

Adopted, with three additions that experience says you want alongside it:

1. **Permissive or weak-copyleft licences only.** Nothing BUSL/SSPL/proprietary-relicensable. This
   rules out post-1.6 Terraform (BUSL-1.1) — see §7.6 — and it is why Redis-lineage and Elastic-lineage
   components are absent throughout.
2. **Governance matters as much as benchmarks.** A single-vendor project that could relicense is a
   liability at the *foundation* layer of a language. Prefer CNCF/ASF/Rust-Foundation-adjacent
   projects for anything load-bearing. This is a real cost of the "one language for everything" scope:
   we are taking dependencies in five domains, and a relicensing event in any of them is our problem.
3. **A stated exit path** for every dependency. Written below as "swap cost".

Licences below were correct at time of writing (July 2026) — verify at adoption, and put a
`cargo-deny`/`cargo-about` licence gate in CI from the first commit.

## 7.1 Compiler core

| Need | Choice | Licence | Why it wins | Rejected |
|---|---|---|---|---|
| Host language | **Rust** | MIT/Apache-2.0 | Only language with best-in-class libraries across *all five* of our domains (WASM, k8s, query engines, async servers, codegen); memory safety in a compiler that will run as a long-lived language server; single static binary distribution | OCaml (best compiler ergonomics, but the infra/k8s/WASM ecosystem is absent — we'd write it all); Zig (immature ecosystem for this breadth); Go (weak type system for an ADT-heavy compiler; GC pauses in the LSP); C++ (no) |
| Incremental engine | **Salsa** | MIT/Apache-2.0 | The framework behind rust-analyzer; gives IDE-grade incrementality for free if adopted from day one (§4.6) | Hand-rolled dirty-bit invalidation (always becomes wrong); `adapton` (research-grade) |
| Lexer | **logos** | MIT/Apache-2.0 | Derive-macro DFA generation, near-optimal token throughput | hand-written (fine, but logos is faster and less code) |
| Parser | **hand-written** RD + Pratt | — | Error messages and recovery are the top UX property of a new language, and generated parsers are worst at exactly that (§2.8) | `chumsky`/`nom`/LALRPOP/tree-sitter-as-compiler-parser |
| Editor grammar | **tree-sitter** | MIT | De facto standard for editor highlighting/structure; error-tolerant and fast | TextMate grammars (unstructured) |
| Release codegen | **LLVM** via `inkwell` | Apache-2.0 with LLVM exception | Best peak code quality available in open source; Cranelift measured ~14% slower, and Perry's 2026 Cranelift→LLVM migration turned a deficit into 1.7×–24.6× wins over Node.js | GCC (GPL + no usable library API); TPDE (promising fast back-end research, too new) |
| Dev codegen / JIT | **Cranelift** | Apache-2.0 with LLVM exception | ~40% faster whole-compile and ~10× faster codegen step than LLVM; makes `tier dev` hot reload feel instant | LLVM `-O0` (still far slower); interpreter only (too slow for realistic dev) |
| WASM optimisation | **Binaryen** (`wasm-opt`) | Apache-2.0 | Standard, and `-Oz` is decisive for the client size budget (§5.1) | LLVM's WASM backend alone (leaves size on the table) |
| Diagnostics rendering | `ariadne` or `codespan-reporting` | MIT/Apache-2.0 | rustc-quality rendering without writing it | own renderer (do this only if neither fits) |

**Swap cost**: LLVM↔Cranelift is already dual, so low. Salsa is the highest-swap-cost choice here
(pervasive), which is why the query decomposition in §4.6 should be designed as *our* interface with
Salsa behind it.

## 7.2 Client tier

| Need | Choice | Licence | Why | Rejected |
|---|---|---|---|---|
| Client target | **WebAssembly** | W3C standard | Shares one code generator and one semantics with the server partition — essential to the tierless guarantee (§5.1) | JS-only (kept as an optional second backend, `--client-backend=js`) |
| Reactivity model | **fine-grained signals** (own implementation, SolidJS/Leptos model) | — | No diffing cost, no reconciler heuristics, and it is the *same* abstraction as server-side invalidation (§3.8) | Virtual DOM (extra runtime weight + heuristics); dirty-checking (Angular-era pathologies) |
| Browser automation for tests | **Playwright** | Apache-2.0 | Best-in-class; already installed in this environment (Chromium at `/opt/pw-browsers`) | Selenium (slower, flakier) |

We write the reactive runtime and DOM layer ourselves. There is nothing to depend on: existing Rust/WASM
UI frameworks (Leptos, Dioxus, Sycamore) are *for Rust programmers*, and taking one would put a foreign
component model at the centre of our language. Read their source; don't link them.

## 7.3 Service tier

| Need | Choice | Licence | Why | Rejected |
|---|---|---|---|---|
| Async runtime | **Tokio** | MIT | Highest-performing, most-battle-tested async runtime in any systems language; the ecosystem gravity is decisive | `async-std` (effectively unmaintained); `smol` (smaller ecosystem); `glommio` (io_uring-per-core, faster in narrow cases, Linux-only, niche) |
| HTTP/1+2 | **Hyper** | MIT | The reference-quality Rust HTTP implementation; everything else builds on it | Actix-web internals (framework, not a library at our layer) |
| HTTP/3 / QUIC | **quinn** | MIT/Apache-2.0 | Leading pure-Rust QUIC | `s2n-quic` (good; Apache-2.0; single-vendor) |
| TLS | **rustls** (+ `aws-lc-rs`) | Apache/MIT/ISC | Faster than OpenSSL in benchmarks, memory-safe, no C build dependency | OpenSSL (CVE history, build pain) |
| Server-side WASM | **Wasmtime** | Apache-2.0 with LLVM exception | 2026 benchmarks: leads cold start (Winch baseline compiler) *and* steady state, at 2.41× native (from 2.54× in 2025, 2.67× in 2024); 1.12× native on 1 GB SHA-256; reference implementation of the component model/WASI-P2 | **WasmEdge** (AOT reached 1.74× native, 1.08× on SHA-256 — better on long compute; keep as an option); **Wasmer** (LLVM backend wins peak throughput on long-running compute); WAMR (embedded focus) |
| Tracing | **OpenTelemetry** Rust SDK | Apache-2.0 | The only vendor-neutral standard; automatic cross-tier traces are a headline feature (§5.2) | vendor SDKs (lock-in) |
| Metrics | **Prometheus** exposition | Apache-2.0 | Universal | statsd (legacy) |
| Identity provider (`identity = managed()`) | **Keycloak** (default) / **Ory Kratos** (lighter alternative) | Apache-2.0 (both; Keycloak is CNCF) | Never roll our own auth: passkeys, MFA, social login, admin UI inherited from a hardened IdP, provisioned by the InfraGraph ([`10`](10-decisions.md) D6) | Authentik/SuperTokens (open-core ambiguity); Zitadel (verify current licence); hand-rolled auth (never) |
| OIDC relying party | **`openidconnect`** crate | MIT/Apache-2.0 | Audited, standard code-flow implementation; Tier's runtime does only token verification and typed claims→`Session`-capability mapping | hand-rolled OIDC (a CVE farm) |

Note the deliberate hedge on server-side WASM: because the runtimes trade places depending on workload
shape, target the **component model / WASI-P2 interface** rather than any runtime's API, and let the
deployment pick. That costs little now and avoids a rewrite later.

## 7.4 Data tier

The semantic model is fixed ([`03`](03-type-and-effect-system.md) §3.7): append-only log, `durable`
folds, incrementally-maintained views. These are the substrates:

| Need | Choice | Licence | Why | Rejected |
|---|---|---|---|---|
| Durable substrate v1 (log, snapshots, read models) | **PostgreSQL** | PostgreSQL License | Boring, transactional, operable everywhere; PITR for free; read models as ordinary tables makes Tier legible to DBAs and BI. "Your data is in Postgres; Tier is how it got there" | MySQL/MariaDB (weaker SQL surface); CockroachDB (BUSL — excluded); bespoke log-structured store (a storage company's worth of work) |
| Incremental view maintenance | **timely + differential dataflow** (the Naiad lineage) | MIT | "Keeping it incremental is the compiler's job" made real: views compile to incremental plans with arrangement *sharing* — the mechanism behind per-session fanout at scale (§5.3). **DBSP/Feldera** is the maintained modern embodiment of the same theory — evaluate at adoption (verify its current licence) | **Materialize** (validates the approach commercially; BUSL — excluded); hand-rolled IVM (subtly wrong forever); recompute-always (correct — kept as the CI oracle and the v0.1 semantics, not the endgame) |
| Dedicated log transport (post-1.0, if fan-out demands) | **NATS JetStream** | Apache-2.0 | Single-binary, small, at-least-once persistent streams | Kafka (Apache-2.0 but JVM-heavy for our need); Redpanda (BSL — excluded) |
| Embedded dev log (rung 0) | **redb** | MIT/Apache-2.0 | Pure-Rust embedded ordered KV; `tier run` needs no server and the log file still replays | SQLite (fine fallback; C dependency); sled (unmaintained) |
| Analytical query engine | **Apache DataFusion** | Apache-2.0 | *Designed to be embedded and extended* — replaceable table providers, optimiser rules, UDFs: the right shape for our symbolic plans; fastest single-node Parquet engine in ClickBench (ahead of DuckDB, chDB, ClickHouse). Runs analytics over Parquet-partitioned log archives | **DuckDB** (MIT; faster on its own storage format and the better pick if you only need to *query*; but a complete system designed to be used as-is — wrong shape to embed a compiler into). Polars (DataFrame-shaped, not plan-shaped) |
| Columnar interchange | **Apache Arrow** | Apache-2.0 | Zero-copy across the wire and into DataFusion; the industry format (§4.4) | bespoke format (no) |
| Postgres client | **tokio-postgres** | MIT/Apache-2.0 | Lowest-overhead async Postgres in Rust; we generate all SQL, so no ORM and no macro-checked SQL needed | `sqlx`, Diesel (we *are* the ORM) |
| External-tool compatibility | **pgwire** protocol server | — | `psql`/BI/DBeaver see read models as tables (§5.3) — the cheapest trust-builder for adopting teams | none |
| CRDT-valued types (post-1.0, collaborative text / local-first) | **automerge** (or **loro**) | MIT | The original's concession — concurrent edits to one value need CRDTs, "no type system absolves you" — met with a typed, library-backed value type rather than folklore | OT server (operationally heavier); hand-rolled CRDTs (research trap) |

## 7.5 Containers and registry

| Need | Choice | Licence | Why | Rejected |
|---|---|---|---|---|
| Image build | **apko** (+ **melange** for native deps) | Apache-2.0 | Declarative, no shell execution, therefore **bit-for-bit reproducible** — same config + package versions ⇒ identical digest on any machine; distroless Wolfi base; SBOM covering complete contents; daemonless and unprivileged. Ideal for our static-binary artefacts (§6.2) | **BuildKit** (Apache-2.0; best general builder — keep as `builder = buildkit` escape hatch); Docker build (daemon, root, non-reproducible); Kaniko/Buildah (Dockerfile-shaped, which we don't need); Nix (best-in-class reproducibility, but its learning curve becomes ours); ko/Jib (language-specific) |
| OCI read/write/push | `oci-spec` + `oci-client` crates | Apache-2.0/MIT | Removes the last external binary from `tier build` | shelling out permanently |
| Signing / provenance | **Sigstore** (`cosign`, `sigstore-rs`) | Apache-2.0 | Keyless signing is the emerging default; pairs with reproducible builds for a real supply-chain story | GPG (key management burden); notation (smaller ecosystem) |
| Package distribution | **ORAS** / OCI artefacts | Apache-2.0 | Reuse registries users already run; air-gap support on day one; no package host to operate (§6.7) | bespoke registry (a company's worth of work) |

## 7.6 Kubernetes and infrastructure

| Need | Choice | Licence | Why | Rejected |
|---|---|---|---|---|
| Orchestrator | **Kubernetes** | Apache-2.0 | Its API server is a general-purpose, extensible desired-state store with a mature reconciler ecosystem — we get the IaC state engine for free (§1.4) | Nomad (BUSL — excluded); ECS/Cloud Run (proprietary, single-cloud); own orchestrator (absurd) |
| K8s client + controller runtime | **kube-rs** | Apache-2.0 | CNCF Sandbox; client + `kube-runtime` controller abstractions + CRD derive (the `client-go`/`controller-runtime`/`kubebuilder` trio in one crate); production reports show large reliability/footprint wins for Rust operators (one: 94% fewer crashes, 68% less resource use) | Go + controller-runtime (would mean a second language in the toolchain — directly against the project's thesis); raw REST calls (reinventing informers and caches) |
| Typed API objects | **k8s-openapi** | Apache-2.0 | Generated, versioned, exhaustive typed structs — no YAML templating anywhere | string templates (this is what we're replacing) |
| Ingress | **Gateway API** | Apache-2.0 | The successor to Ingress; portable traffic splitting for canaries (§6.4) | Ingress (frozen, annotation soup); vendor CRDs (lock-in) |
| Event autoscaling | **KEDA** | Apache-2.0 | CNCF graduated; scale on queue depth/external metrics, which HPA can't do natively | custom metrics adapters (more moving parts) |
| Progressive delivery | **Argo Rollouts** | Apache-2.0 | Mature canary/blue-green with metric analysis; don't reimplement | Flagger (fine alternative); own implementation (no) |
| Cloud resources beyond the cluster | **Crossplane** | Apache-2.0 | Keeps one control plane and one reconciler; managed Postgres/buckets/DNS become the same reconciliation as workloads | **Terraform** (BUSL-1.1 — **excluded by your open-source constraint**); **OpenTofu** (MPL-2.0 — the licence-clean fork; keep as an emitter escape hatch for estates Crossplane can't reach); Pulumi (Apache-2.0, but its SDK model duplicates what our compiler already does) |
| Certificates | **cert-manager** | Apache-2.0 | CNCF graduated, universal | bespoke ACME client |
| Secrets | **External Secrets Operator** | Apache-2.0 | Broad backend support; `secret[T]` types make the wiring safe (§3.5) | Sealed Secrets (weaker rotation story) |
| Local cluster | **k3s** / **k3d** | Apache-2.0 | Single binary, fast, small — the right rung-3 dev cluster (§6.6) | kind (fine; support both); minikube (heavier) |
| Network policy / mTLS at scale | **Cilium** | Apache-2.0 | eBPF dataplane, best-in-class policy performance; identity-based policy pairs naturally with effect-derived rules (§6.5) | Istio ambient (Apache-2.0, capable, heavier); Linkerd (Rust dataplane, appealing, but verify current stable-release distribution terms before depending on it) |
| GitOps interop | **Argo CD** / **Flux** | Apache-2.0 | Emit manifests they can consume; don't compete (§6.3) | requiring `tier deploy` (loses half the market) |

## 7.7 Development, testing and release

| Need | Choice | Licence |
|---|---|---|
| Property testing | `proptest` | MIT/Apache-2.0 |
| Snapshot testing (diagnostics, manifests, expansions) | `insta` | Apache-2.0 |
| Fuzzing (parser, macro expander) | `cargo-fuzz` / libFuzzer, AFL++ | MIT/Apache-2.0 / Apache-2.0 |
| Benchmarking with statistics | `criterion` / `divan` | Apache-2.0/MIT |
| Licence + advisory gating in CI | `cargo-deny`, `cargo-about` | MIT/Apache-2.0 |
| Optional formal verification of core invariants | `Kani` (model checking) | Apache-2.0/MIT |
| Docs site | `mdBook` | MPL-2.0 |
| Playground | our WASM build of the compiler | — |

The playground deserves a line of its own: the compiler is in Rust, so `tier` compiles to WASM and
runs **entirely in the browser**. A zero-install playground that type-checks, shows placement, and
displays the generated SQL and Kubernetes objects side by side is the single most persuasive artefact
this project can produce for adoption. Budget it into Phase 3, not "someday".

## 7.8 Total dependency risk summary

| Domain | Load-bearing dependency | If it disappeared tomorrow |
|---|---|---|
| Codegen | LLVM | Fall back to Cranelift (already built) — lose ~14% runtime perf |
| Incrementality (compiler) | Salsa | Rewrite behind our own query interface — weeks, not months, if the seam is respected |
| Incrementality (views) | differential dataflow | Fall back to full recompute per event — semantically identical (it is the CI oracle), slower; the seam is the symbolic plan, not the engine |
| WASM host | Wasmtime | Swap for WasmEdge/Wasmer behind the component-model boundary |
| Durable substrate | PostgreSQL | Realistically permanent as default; the log-engine interface admits other substrates (the dev rung already runs on redb) |
| Images | apko | Fall back to BuildKit — lose bit-reproducibility |
| Cluster | Kubernetes | The `Platform` trait means single-process still works; a new platform is a bounded project |
| Cloud resources | Crossplane | OpenTofu emitter escape hatch |

No single dependency is unrecoverable. That is the point of writing this table.

## 7.9 Versioning and upgrade policy

Many open-source dependencies means version management is a designed system, not a habit.

**Classes.** Every dependency is assigned a class in `deps.toml`, reviewed in PRs:

| Class | Examples | Upgrade rules |
|---|---|---|
| **1 — load-bearing** (semantics or security ride on it) | LLVM, Cranelift, Salsa, Wasmtime, timely/differential, Tokio, rustls, kube-rs, Keycloak, Postgres | Dedicated PR per upgrade; full differential + DST + perf suites must pass; changelog reviewed by a named owner; rollback plan noted; majors on a deliberate cadence (LLVM ~annually, Kubernetes per its own N-2 window) |
| **2 — replaceable library** | logos, insta, ariadne, oci-spec | Batched automated PRs, normal CI |
| **3 — dev/CI only** | cargo-mutants, criterion | Batched, relaxed |

**Pinning — everything, always.** `Cargo.lock` committed for every workspace; Rust toolchain pinned
via `rust-toolchain.toml` with an explicit MSRV policy; container base packages pinned by apko to
exact versions and images by digest (never tags); k3d/Postgres/browser versions in the CI matrix
pinned and bumped by PR like any dependency. A build that cannot state exactly what it contains
cannot claim reproducibility ([`12`](12-standards-and-conformance.md) §12.6).

**Provenance and review.**
- `cargo-deny` (licences, advisories, duplicate-version bans) and `cargo-audit` (RUSTSEC) gate
  every CI run — the licence policy of §7 is enforced by machine, not memory.
- **`cargo-vet`**: every new dependency and every upgrade of a Class-1/2 crate carries a recorded
  human audit (importing the shared Mozilla/Google audit sets where available). This is the
  supply-chain answer that scales past "we trust crates.io".
- **No git dependencies** in released artefacts — registry releases only. Needed patches go
  upstream first; unavoidable forks live under our org, carry a tracking issue with an exit
  criterion, and are treated as Class 1.
- SBOM + SLSA provenance regenerated per build (§12.6), so "which versions are in production" is a
  query, not an investigation.

**Cadence and automation.** Renovate opens batched weekly PRs for Classes 2–3 and individual PRs
for Class 1; a monthly dependency-review session triages anything unmerged; security advisories
preempt all cadence (patch within 48h for Critical, with the embargoed-response process documented
in `SECURITY.md`). A quarterly `cargo update --dry-run` + `-Z minimal-versions` CI job catches both
over-fresh and under-specified constraints.

**Version skew is abolished internally.** Compiler, runtime, thin client, operator, and stdlib
version together on a single release train — one version number, tested as one artefact
([`13`](13-testing.md) §13.3 covers the *user-facing* skew: old deployed apps vs new servers).
Externally, supported matrices are explicit and CI-enforced: Kubernetes N-2, the two newest
Postgres majors, evergreen browsers + last-2 Safari.

**Air-gap**: `tier vendor` produces a complete offline dependency set (crates, base packages,
images) — falls out of the pinning discipline, and regulated adopters will ask on day one.

## 7.10 Sources

Benchmark and licensing claims above draw on:

- [Cranelift code generation comes to Rust — LWN.net](https://lwn.net/Articles/964735/)
- [Cranelift project site](https://cranelift.dev/) · [Cranelift — Wikipedia](https://en.wikipedia.org/wiki/Cranelift)
- [From Cranelift to LLVM: How Perry Got 24x Faster](https://perryts.com/en/blog/cranelift-to-llvm)
- [TPDE: A Fast Adaptable Compiler Back-End Framework](https://arxiv.org/pdf/2505.22610)
- [Performance of WebAssembly runtimes in 2026 — Frank Denis](https://00f.net/2026/06/23/webassembly-runtimes-2026/)
- [WebAssembly Runtime Benchmarks 2026](https://wasmruntime.com/en/benchmarks) · [wasm-runtime-comparison](https://github.com/wasmruntime-io/wasm-runtime-comparison)
- [Apache DataFusion is now the fastest single node engine for querying Apache Parquet files](https://datafusion.apache.org/blog/2024/11/18/datafusion-fastest-single-node-parquet-clickbench/)
- [Apache DataFusion vs DuckDB — Spice AI](https://spice.ai/learn/apache-datafusion-vs-duckdb) · [Comparing DuckDB and Apache DataFusion](https://buremba.com/blog/duckdb-vs-apache-datafusion)
- [apko — Build OCI images from APK packages directly without Dockerfile](https://github.com/chainguard-dev/apko) · [apko overview](https://rawkode.academy/technology/apko)
- [kube-rs](https://kube.rs/) · [kube on GitHub](https://github.com/kube-rs/kube) · [Kubernetes Operators in Rust](https://dev.to/speed_engineer/kubernetes-operators-in-rust-control-loops-that-behave-3b86)
- Tierless-language prior art: [Eliom: A Language for Modular Tierless Web Programming (arXiv 1901.11411)](https://arxiv.org/abs/1901.11411) · [Tierless Web Programming in the Large](https://dl.acm.org/doi/fullHtml/10.1145/3184558.3185953) · [Multitier programming — Wikipedia](https://en.wikipedia.org/wiki/Multitier_programming)
- Streams/folds lineage: [differential dataflow](https://github.com/TimelyDataflow/differential-dataflow) · [Turning the Database Inside-Out — Kleppmann](https://martin.kleppmann.com/2015/03/04/turning-the-database-inside-out.html) · [Out of the Tar Pit — Moseley & Marks](https://curtclifton.net/papers/MoseleyMarks06a.pdf)
