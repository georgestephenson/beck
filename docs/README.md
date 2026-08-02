# Beck — implementation plan

One language for the frontend, the backend, the database, the container, and the cluster — where
every tier is a pure function or a fold, and time enters at declared merge points.

## Provenance

The plan derives from George's original conversation, preserved in
[`00-original-idea.md`](00-original-idea.md) (the share link wasn't machine-readable; the transcript
was supplied directly and is now the source of truth). A first draft of this plan was written before
the transcript was available; checking it against the original changed three load-bearing things:

- **The data tier is event-sourced, not relational-first**: the database is a `durable` fold over
  an event stream; queries are pure functions of that state, kept incremental by the compiler.
  Relational storage is a *substrate* (read models in Postgres), not the semantic model.
- **The browser is a patch interpreter by default** (~KBs of compiler residue), with a per-component
  upgrade to client-side rendering — not a fat-WASM-client-first design.
- **Deploys ride the stream**: typed `migrate`/`upcast` functions demanded at deploy, drain/resume
  choreography, replay as a feature.

## The three questions from the brief, answered

| Question | Short answer | Where |
|---|---|---|
| Python-like without losing the Lisp power? | **Yes — solved problem.** Homoiconicity is a property of the core AST, not the notation (Elixir/Julia/Nim prove it). One canonical `Node` AST, two faithful surfaces; the missing Python piece is a block-passing call form so macros receive indented bodies. The S-expression surface stays as the spec and macro-debugging notation | [`02`](02-syntax.md) |
| Dependencies best-in-class *and* open source? | Rust host; Salsa; dual Cranelift/LLVM codegen; thin-client first, Wasmtime for server-side WASM; **Postgres as durable substrate + MIT-licensed differential dataflow for incremental views** (Materialize-class capability without the BUSL licence); DataFusion for analytics; apko for reproducible daemonless images; kube-rs; Crossplane (Terraform is BUSL — excluded). Licences, benchmarks, swap costs per dependency | [`07`](07-dependencies.md) |
| Kubernetes under the hood? | Yes — as a **compiler backend** behind a `Platform` trait, never as language semantics. Infra is derived from program analysis: a `durable` fold ⇒ volume + snapshots; `merge_clients()` ⇒ websocket ingress; effect rows ⇒ least-privilege NetworkPolicy/RBAC/grants. `beck run` needs no cluster, container, or registry | [`06`](06-kubernetes-and-packaging.md) |

## Documents

| # | Document | What it covers |
|---|---|---|
| 00 | [The original idea](00-original-idea.md) | The seed conversation — sketch verbatim, requirements extracted |
| 01 | [Vision and premise](01-vision-and-premise.md) | The three moves from SICP to a language; the canonical example in the Python surface; prior art and lessons |
| 02 | [Syntax: Python surface, Lisp core](02-syntax.md) | Dual-surface design, macros, block-passing, hygiene, honest losses |
| 03 | [Type and effect system](03-type-and-effect-system.md) | Streams/signals/folds, the merge point, determinism, placement-as-effect, security proofs, migrations, modularity |
| 04 | [Compiler architecture](04-compiler-architecture.md) | Pipeline, IRs, signal-graph slicing, wire format, incrementality, `beck explain`, testing |
| 05 | [Tier lowering](05-tier-lowering.md) | Patch-interpreter client + Mode B, dual-backend server, log-as-database, infra object graph |
| 06 | [Kubernetes and packaging](06-kubernetes-and-packaging.md) | Images, derived manifests, the operator, deploys-ride-the-stream, dev→prod ladder |
| 07 | [Dependencies](07-dependencies.md) | Every third-party choice: rationale, licence, benchmarks, rejected alternatives, swap cost |
| 08 | [Roadmap](08-roadmap.md) | Phase 0 premise-proof, walking skeleton, phased plan, exit criteria, team shape |
| 09 | [Risks and open questions](09-risks-and-open-questions.md) | Ranked risks, decisions taken, remaining opens |
| 10 | [Decision log](10-decisions.md) | George's decisions D1–D18, all settled — including the Git/local-first analysis, the language-not-framework rationale, and the name: **Beck** |
| 11 | [Language tour](11-language-tour.md) | What Beck looks like, construct by construct — types, traits, macros, queries, UI, services, in-language tests |
| 12 | [Standards and conformance](12-standards-and-conformance.md) | The rulesets Beck conforms to, layer by layer — and the test-linked spec discipline that makes conformance real |
| 13 | [Testing strategy](13-testing.md) | Every applicable kind of test and why — built around the free oracles Beck's semantics manufacture (DST, differentials, TLA+, Jepsen) |
| 14 | [Review findings](14-review-findings.md) | The adversarial review: ranked security/design findings — all resolved (D14) |
| 15 | [Scale and distribution](15-scale-and-distribution.md) | The DDIA checklist against Beck's semantics; the scaling ladder (partitioned logs, typed sharding), sagas, and why Beck needs a fabric, not a Redis — with that checklist made executable as a four-verdict conformance matrix (§15.6) |
| 16 | [Packages and ecosystem](16-packages-and-ecosystem.md) | The Rails/npm lessons; Beck's own package system — **tarns** (packages), **forces** (vertical-slice feature packages), **the Mere** (index); effect-transparent dependencies, no install hooks |
| 17 | [The playground](17-playground.md) | The whole stack in a browser tab (the DST convergence), the cloud rung with the compiler as first sandbox, content-addressed sharing, playground-as-Beck-app |
| 18 | [Phase 0 report](18-phase-0-report.md) | **Built.** What [`phase0/`](../phase0/) implements, the measured exit criteria, the kill/pivot gates, and what turned out harder than expected |
| 19 | [Phase 1 report](19-phase-1-report.md) | **Built.** The compiler in [`compiler/`](../compiler/): what it compiles, the harnesses it passes, what it deliberately does not do, and the corrections Phase 1 makes to these documents |
| 20 | [Phase 2 report](20-phase-2-report.md) | **Built.** The moat: effect rows and their inference, placement solved from a cost model, `secret[T]`/`Sendable` with §3.5 as tests, `.becki` and separate compilation, `--wire-compat` — and the corrections Phase 2 makes to these documents |
| 21 | [Tests in Beck, and proof](21-tests-in-beck-and-proof.md) | **§21.2–§21.3 built** (see 22). `test` blocks that cross the boundary without a network, mocks derived from the effect row instead of written out — and the six-rung ladder that says how we know a generated manifest is right, with what a proof would and would not add |
| 22 | [Phase 3 report, part 1](22-phase-3-report.md) | **Built.** Tests written in Beck: `test`/`property` blocks, `beck test`, effect-atom stubs nobody writes, one type-directed generator — one bullet of Phase 3's twelve, with the other eleven named rather than implied |
| 23 | [Phase 3 report, part 2](23-general-slicer-report.md) | **Built.** The general slicer: the signal graph as a graph, several durable folds fused into one accumulator, shared computations bound once, every tier crossing enumerated — the debt two phases carried, and the silent mis-slice it was hiding. Plus `beck explain incremental`, the analysis §3.8 asks for and the plan is the input to — with the view engine still unbuilt, and the report saying so first |
| 24 | [Phase 3 report, part 3](24-incremental-views-report.md) | **Built.** The incremental view engine: a view compiled to a dataflow of operators and maintained from the change, `remaining` by ±1 per event at any size, recompute promoted to the CI oracle it was always supposed to be — with the page's children still assembled in full, a 3–5× constant factor rather than a new asymptote, and four times the per-subscriber memory, all measured |
| 25 | [Benchmarks and expressiveness](25-benchmarks-and-expressiveness.md) | The third-party performance suites, layer by layer, and which would measure a placeholder today — plus SICP as the expressiveness benchmark the premise has never had, with chapter 1 running and the six walls beyond it measured. Nand2Tetris, LeetCode, DDIA and the curriculum behind them assessed: which yield a test, which a lesson |
| 26 | [Phase 3 report, part 4](26-arrangement-sharing-report.md) | **Built.** One shared dataflow: the operators that do not read the session held once for every subscriber rather than once each, advanced per event rather than per connection, with a bounded change history for subscribers that render late and a rebuild for ones that render very late. 55× less work per event on a public feed and 1.3× on the sketch — because where a program reads the session is what decides its fanout cost. Plus §5.3's per-session memory, exported at last, and a stale-flag defect in `24`'s engine that only a fanout could show |
| 27 | [Phase 3 report, part 5](27-walls-report.md) | **Built.** Three of the six walls [`25`](25-benchmarks-and-expressiveness.md) measured between Beck and the rest of SICP: a library runs its own tests at last — so chapter 1's five-declaration wrapper is gone and every domain module can be unit-tested; a type may mention itself or anything declared later, with a comment thread added to the corpus to prove every pass survives one; and the `if` that typed one branch as the other's expectation, which cost exercise 1.43. Plus `sicp/ch2.beck`, which needed two of the three and could not have been written for either alone |
| 28 | [Releases and deployment](28-releases-and-deployment.md) | The three pipelines — releasing the compiler, deploying what `beck build` emits, and the project's own dogfood deployments — with what exists today (CI and nothing downstream of it), the signed-and-reproducible release plan, and the schedule of gates deliberately not added yet |

## The one-paragraph version

`beck` is a statically typed, homoiconic language with a Python-like surface in which the original
`(my-javascript (my-css (my-html)))` becomes literal: the page is a pure function of state, state is
a durable fold over an event stream, and hand-written JavaScript disappears — it's compiler residue.
Clients propose typed commands; one declared merge point admits time and nondeterminism; everything
downstream is deterministic, so replay, time-travel debugging and optimistic UI come from the
semantics rather than from frameworks. Placement is typed and checkable (secrets provably cannot
reach the browser), views are kept incremental by the compiler, and `beck build` emits not a binary
but a deployable system — reproducible OCI images plus a Kubernetes object graph derived from the
program's own effects — which `beck deploy` applies with typed migrations gating the rollout.
