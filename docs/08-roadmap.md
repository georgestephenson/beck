# 08 — Roadmap

## 8.0 Sizing, honestly

A language with five backends is a **multi-year** project. Comparable efforts: Elm reached usable in
~2 years with 1–2 people (one tier); Rust took ~9 years to 1.0 with a team; Eliom took a research group
a decade to industrial quality (two tiers). Per George's directive ([`10`](10-decisions.md) D8), the
plan optimises for **completeness, clean architecture and performance, not schedule**: nothing is
descoped to fit a team size, and durations below are sequencing calibration, not commitments. What
survives from schedule discipline is *ordering* — the walking-skeleton rule below — and budgets as
CI gates, because "optimal performance" is only real if it is measured from Phase 0.

The sequencing rule that matters more than any estimate: **build one thin vertical slice through all
five tiers before making any single tier good.** The risk in this project is not "can we write a good
type checker" — it is "does the tierless partition idea survive contact with a real deployment." Answer
that in month 3, not year 2.

## Phase 0 — Prove the premise (4–6 weeks, 1–2 people)

No compiler. Hand-write, in Rust, the *output* the compiler will eventually generate for the todo
sketch ([`00`](00-original-idea.md)): ingress + envelope stamping, a durable fold over a Postgres
log (and redb embedded), server-side `view` + structural diff, the thin patch-interpreter client,
`(subscription, seq)` resumption, an apko image, k8s manifests, a kube-rs operator stub, deployed
to k3d. Then kill the process mid-stream and replay the log.

**Exit criteria** — stated from evidence, not opinion:
- interaction latency p50/p99 (click → command → event → fold → patch → DOM) on a realistic RTT,
  Mode A;
- events/s through a single Postgres-backed sequencer, and fold throughput on replay;
- per-idle-session server memory with 1k and 10k connected subscribers of a per-session view
  (the fanout number — this is the one that kills LiveView-shaped systems);
- thin-client payload (target < 10 KB brotli) and time-to-first-paint (SSR);
- reconnect-after-deploy behaviour: does resumption actually replay the gap;
- whether apko builds our artefact reproducibly; image size;
- a written list of everything that turned out harder than expected.

**Kill/pivot gates.** Interaction p99 > ~150 ms on realistic RTT ⇒ Mode B (client-side view) moves
from Phase 3 into the core plan. Per-idle-session memory that can't be brought under ~50 KB ⇒
redesign the session/subscription representation before any compiler work builds on it.

## Phase 1 — Walking skeleton (3–4 months)

The narrowest possible compiler that takes the todo sketch from source to a running k3d deployment.
Deliberately bad at everything, complete end-to-end.

- Lexer, layout, parser (Python surface + S-expression reader), `Node`, pretty-printer, `beck fmt`.
- Macro expander with hygiene — **from the start** (§2.4); retrofitting hygiene is a rewrite.
- Modules, name resolution, HM typechecker: ADTs, records, traits, `Stream`/`Signal`/`fold`/
  `durable` typed; no effect inference yet.
- `Core` IR; **manual placement only** via `@on(...)` — exactly the original sketch's annotations.
- Signal-graph slicing + command channel + envelope/patch serialisers (§4.3). Views are **full
  recompute per event** — semantically final, later made incremental.
- Backends: Cranelift (server), the thin patch client (plain JS, ~KBs — no WASM in Phase 1),
  Postgres/redb log engine, k8s object graph.
- `beck run` (single process) and `beck up` (k3d).
- Salsa from commit one; `insta` diagnostic snapshots from week two; the **differential
  single-process vs split harness** and the **replay-determinism harness** (§4.8) from the moment
  each is expressible.

**Exit**: `git clone && beck up` yields the working todo app in a local cluster, CI-asserted;
differential and replay harnesses green; `beck replay` reproduces state from a recorded log.

## Phase 2 — Effects and placement (3–4 months)

The moat.

- Effect rows, inference, effect polymorphism (§3.2).
- Placement *verification* against effects (reject `@on(client)` + `durable`; reject impure folds —
  the determinism rule that makes replay exact, §3.7) — valuable on its own.
- Placement *inference* + the cost solver, with determinism and stability guarantees (§3.4).
- `beck explain place` / `flow` / `wire` (§4.7).
- `secret[T]`, `Sendable`, capability effects; the §3.5 security property suite as executable tests.
- `.becki` published signatures and separate compilation (§3.6) — **do not slip this**; it is the
  historical failure mode (§1.6).
- Boundary versioning + `beck check --wire-compat` (§4.3).

**Exit**: on a corpus of 20+ programs, placement is inferred with no annotations for the common cases;
every §3.5 property is a passing test; a 3-module project rebuilds incrementally without recompiling
dependencies whose signatures didn't change.

## Phase 3 — Make it real for developers (4–5 months)

- LLVM release backend + differential tests against Cranelift (§5.2).
- **Incremental views**: compile subscribed/materialized views to differential-dataflow plans with
  arrangement sharing (per-session fanout, §5.3); recompute stays as the CI oracle; SQL read models
  + pgwire exposure; query fusion on symbolic plans.
- **Mode B client**: per-component WASM (view + fold + signal kernel), optimistic application with
  `seq` reconciliation, freshness-typed pending state; size budget CI gate (< 150 KB brotli per
  component bundle).
- Client polish for both modes: router, forms, lazy routes, focus/scroll preservation, devtools
  extension showing signal graph, patch traffic and pending state.
- Structured concurrency, `Result`/error rows, `match` exhaustiveness, pattern matching completion.
- Standard library v1: collections, strings, time, money/decimal, HTTP client, JSON, UUID, crypto
  primitives (delegated to `ring`/`aws-lc-rs`, not hand-rolled).
- **Identity**: OIDC relying-party runtime, `identity = managed()` provisioning (Keycloak/Ory),
  claims → `Session` capability mapping, dev-mode identity for rung 0, presence as a first-class
  signal ([`10`](10-decisions.md) D6).
- LSP: completion, hover with *inferred placement*, go-to-def, rename, inline diagnostics.
- **The playground** ([`17`](17-playground.md)) — highest-leverage adoption artefact: rung A
  (compile-time, static) and rung B (the whole app in the tab — the worker-server is the rung-0
  platform compiled to WASM, riding Mode B's kernel work; `seq` scrubber and two-client demos).
- `beck init ci`, apko image build in-process, cosign signing, SBOM.

**Exit**: an outside developer builds a non-trivial app from documentation alone, without asking the
team a question. Track this literally as the acceptance test.

## Phase 4 — Production readiness (4–5 months)

- Beck operator: the deploy-rides-the-stream choreography — quiesce, drain, snapshot, `migrate`/
  `upcast`, resume (§6.4); canaries via Gateway API + Argo Rollouts; wire-compat gate; `beck
  status` with source provenance.
- Replay tooling as product: `beck replay`, `beck fork --from prod --at <time>` (privileged,
  audited, default-redacted — F4), log-backed property tests; structural shredding worked
  end-to-end per D14 (log, snapshots, read models, backups).
- Package system UX ([`16`](16-packages-and-ecosystem.md)): `beck add`/`publish`/`why` with the
  effect-diff prompt, namespaces, registry index + transparency log, generated docs site.
- `process` (sagas) and non-durable folds hardened; internal fan-out fabric (NATS) when
  multi-node demands it ([`15`](15-scale-and-distribution.md)).
- Playground rung C ([`17`](17-playground.md) §17.3): ephemeral TTL'd cloud environments with the
  compiler-as-first-sandbox effect budget — an operator workload, built alongside the operator.
- Effect-derived NetworkPolicy/RBAC/DB grants (§6.5) — the platform-team sales pitch.
- Crossplane emitter for managed Postgres/buckets/DNS; OpenTofu escape hatch; `import infra`.
- OpenTelemetry cross-tier tracing on by default; `beck tune` right-sizing.
- Multi-arch images; air-gapped install; OCI package registry via ORAS (§6.7).
- **FFI**: C ABI both directions; JS interop for the client tier; a Python bridge (§9.2) — the
  ecosystem-access question is existential, so give it real headcount.
- Security review: hygiene escapes, macro sandbox, deserialisation of untrusted wire data, generated
  SQL, the `Sendable`/`secret` proofs. Get external eyes on this.

**Exit**: one real production application running on it, with an on-call rotation and a postmortem
or two — per [`10`](10-decisions.md) D15 this is **beck.dev**, with the registry entering
production hardening.

## Phase 5 — 1.0 (3–4 months)

- Language specification written against the S-expression core (this is where that surface earns its
  keep, §2.2).
- Stability guarantees and a deprecation policy; wire-format stability commitment.
- **CRDT-valued types**: `Text` and friends (automerge/loro-backed) — concurrent edits merge within
  the value while the log still orders the updates around it ([`10`](10-decisions.md) D7).
- Editor support beyond VS Code; debugger integration (DAP) with cross-tier stepping.
- Package ecosystem seeding; documentation, book, tutorials, and 5–10 non-trivial example apps.
- **The registry in production, on Beck, serving real packages** — the D15 exit criterion: the
  flagship dogfood (event-sourced by domain, saga-driven publish pipeline, counter folds, genesis
  replay as an operational tool) proving the backend and data tier in public.
- Performance: a published benchmark suite versus a hand-written React+FastAPI+Postgres+Helm baseline —
  latency, payload size, image size, build time, *and* lines of code. That last number is the one that
  travels.

## 8.1 Milestone dependency graph

```
P0 prove ──▶ P1 skeleton ──┬─▶ P2 effects+placement ──┬─▶ P4 production ──▶ P5 1.0
                           │                          │
                           └─▶ P3 developer UX ───────┘
                                    │
                                    └─▶ playground (adoption flywheel, can ship early)
```

P2 and P3 can overlap with separate owners; P4 needs both. The playground can and should ship as soon
as P2's `explain` output exists — a page that shows source on the left and *generated SQL + generated
Kubernetes objects + inferred placement* on the right is the demo that explains this project in
15 seconds.

## 8.2 Team shape

| Role | Count | Focus |
|---|---|---|
| Language/type-system engineer | 1–2 | §2, §3 — the front end and effect system |
| Compiler backend engineer | 1 | §5.2 codegen, WASM, optimisation, size budget |
| Data-tier engineer | 1 | §3.7–3.9, §5.3 — log engine, folds, incremental views, migrations |
| Platform/Kubernetes engineer | 1 | §6 — images, operator, policy generation |
| Developer-experience engineer | 1 | errors, LSP, playground, docs — **not** a junior role; this is where adoption is won or lost |

## 8.3 Cross-cutting practices from day one

1. **Every phase ships a demo that runs.** No phase completes on a design document.
2. **The differential harness (§4.8) is the project's conscience.** It is the mechanised statement of
   the central promise; keep it green.
3. **Error-message snapshots in CI** from week two (§4.5).
4. **Size and latency budgets are CI gates**, not aspirations: client payload, image size, cold start,
   incremental build time, keystroke→diagnostic.
5. **`cargo-deny` licence gate** from the first commit, given §7's open-source constraint.
6. **Write the tutorial as you build**, and treat any sentence that requires an apology ("for now you
   have to…") as a bug report against the design.
7. **Dogfood, per D15.** The playground, beck.dev, and above all the package registry are built in
   Beck — the registry's domain is a homomorphism of the semantics (immutable versions = events
   forever; transparency log = the log; yank = an event), so building it tests exactly the backend
   and data tier claims, in production, in public.
