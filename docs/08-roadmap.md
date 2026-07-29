# 08 — Roadmap

## 8.0 Sizing, honestly

A language with five backends is a **multi-year** project. Comparable efforts: Elm reached usable in
~2 years with 1–2 people (one tier); Rust took ~9 years to 1.0 with a team; Eliom took a research group
a decade to industrial quality (two tiers). Estimates below assume **3–6 experienced engineers**;
halve the team and roughly triple the calendar. Durations are for calibration, not commitment.

The sequencing rule that matters more than any estimate: **build one thin vertical slice through all
five tiers before making any single tier good.** The risk in this project is not "can we write a good
type checker" — it is "does the tierless partition idea survive contact with a real deployment." Answer
that in month 3, not year 2.

## Phase 0 — Prove the premise (4–6 weeks, 1–2 people)

No compiler. Hand-write, in Rust, the *output* the compiler will eventually generate for §1.3's example:
a WASM client with a signal runtime, a native service, generated SQL, an apko image, k8s manifests,
a kube-rs operator stub, deployed to k3d.

**Exit criteria** — you can state, from evidence, not opinion:
- the client WASM payload for a table-and-button app, brotli-compressed (target < 150 KB);
- the p50/p99 latency of a generated boundary call, client→server→Postgres;
- the image size and whether apko builds reproducibly for our artefact shape;
- edit→visible time for the Cranelift hot-reload path (prototype);
- a written list of everything that turned out harder than expected.

**Kill/pivot gate.** If the client payload is > 500 KB or hot reload > 10 s, reconsider WASM-primary
(fall back to JS as the primary client backend, §5.1) *before* building the compiler around it.

## Phase 1 — Walking skeleton (3–4 months)

The narrowest possible compiler that takes §1.3's example from source to a running k3d deployment.
Deliberately bad at everything, complete end-to-end.

- Lexer, layout, parser (Python surface + S-expression reader), `Node`, pretty-printer, `tier fmt`.
- Macro expander with hygiene — **from the start** (§2.4); retrofitting hygiene is a rewrite.
- Modules, name resolution, HM typechecker: ADTs, records, traits, no effects yet.
- `Core` IR; **manual placement only** via `@on(...)`.
- Splitting + RPC synthesis + generated serialisers.
- Backends: Cranelift (server), WASM (client, minimal signal runtime), SQL (a *tiny* query fragment:
  filter/project/order/limit on one table), k8s object graph.
- `tier run` (single process) and `tier up` (k3d).
- Salsa from commit one; `insta` diagnostic snapshots from week two; the **differential single-process
  vs split harness** (§4.8) from the moment splitting exists.

**Exit**: `git clone && tier up` yields the working example in a local cluster, with a CI job that
asserts it, plus the differential harness green.

## Phase 2 — Effects and placement (3–4 months)

The moat.

- Effect rows, inference, effect polymorphism (§3.2).
- Placement *verification* against effects (reject `@on(client)` + `db.write`) — valuable on its own.
- Placement *inference* + the cost solver, with determinism and stability guarantees (§3.4).
- `tier explain place` / `flow` / `wire` (§4.7).
- `secret[T]`, `Sendable`, capability effects; the §3.5 security property suite as executable tests.
- `.tieri` published signatures and separate compilation (§3.6) — **do not slip this**; it is the
  historical failure mode (§1.6).
- Boundary versioning + `tier check --wire-compat` (§4.3).

**Exit**: on a corpus of 20+ programs, placement is inferred with no annotations for the common cases;
every §3.5 property is a passing test; a 3-module project rebuilds incrementally without recompiling
dependencies whose signatures didn't change.

## Phase 3 — Make it real for developers (4–5 months)

- LLVM release backend + differential tests against Cranelift (§5.2).
- Real query compiler: joins, aggregates, window functions, **query fusion / N+1 elimination** (§3.7),
  migration planning and the pre-upgrade Job.
- Client tier for real: SSR/hydration, router, forms, lazy route chunks, size budget CI gate, devtools
  extension with signal-graph view.
- Structured concurrency, `Result`/error rows, `match` exhaustiveness, pattern matching completion.
- Standard library v1: collections, strings, time, money/decimal, HTTP client, JSON, UUID, crypto
  primitives (delegated to `ring`/`aws-lc-rs`, not hand-rolled).
- LSP: completion, hover with *inferred placement*, go-to-def, rename, inline diagnostics.
- **The in-browser playground** (§7.7) — highest-leverage adoption artefact.
- `tier init ci`, apko image build in-process, cosign signing, SBOM.

**Exit**: an outside developer builds a non-trivial app from documentation alone, without asking the
team a question. Track this literally as the acceptance test.

## Phase 4 — Production readiness (4–5 months)

- Tier operator: ordered rollouts, migration hooks, canaries via Gateway API + Argo Rollouts,
  wire-compat gate, `tier status` with source provenance (§6.4).
- Effect-derived NetworkPolicy/RBAC/DB grants (§6.5) — the platform-team sales pitch.
- Crossplane emitter for managed Postgres/buckets/DNS; OpenTofu escape hatch; `import infra`.
- OpenTelemetry cross-tier tracing on by default; `tier tune` right-sizing.
- Multi-arch images; air-gapped install; OCI package registry via ORAS (§6.7).
- **FFI**: C ABI both directions; JS interop for the client tier; a Python bridge (§9.2) — the
  ecosystem-access question is existential, so give it real headcount.
- Security review: hygiene escapes, macro sandbox, deserialisation of untrusted wire data, generated
  SQL, the `Sendable`/`secret` proofs. Get external eyes on this.

**Exit**: one real production application — ideally yours — running on it, with an on-call rotation and
a postmortem or two.

## Phase 5 — 1.0 (3–4 months)

- Language specification written against the S-expression core (this is where that surface earns its
  keep, §2.2).
- Stability guarantees and a deprecation policy; wire-format stability commitment.
- Editor support beyond VS Code; debugger integration (DAP) with cross-tier stepping.
- Package ecosystem seeding; documentation, book, tutorials, and 5–10 non-trivial example apps.
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
| Data-tier engineer | 1 | §3.7, §5.3 — query compilation, migrations, pushdown |
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
7. **Dogfood.** Build the playground, the docs site, and the project's own dashboards in Tier as soon
   as it can express them.
