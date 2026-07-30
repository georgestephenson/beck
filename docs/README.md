# Tier — implementation plan

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
| Kubernetes under the hood? | Yes — as a **compiler backend** behind a `Platform` trait, never as language semantics. Infra is derived from program analysis: a `durable` fold ⇒ volume + snapshots; `merge_clients()` ⇒ websocket ingress; effect rows ⇒ least-privilege NetworkPolicy/RBAC/grants. `tier run` needs no cluster, container, or registry | [`06`](06-kubernetes-and-packaging.md) |

## Documents

| # | Document | What it covers |
|---|---|---|
| 00 | [The original idea](00-original-idea.md) | The seed conversation — sketch verbatim, requirements extracted |
| 01 | [Vision and premise](01-vision-and-premise.md) | The three moves from SICP to a language; the canonical example in the Python surface; prior art and lessons |
| 02 | [Syntax: Python surface, Lisp core](02-syntax.md) | Dual-surface design, macros, block-passing, hygiene, honest losses |
| 03 | [Type and effect system](03-type-and-effect-system.md) | Streams/signals/folds, the merge point, determinism, placement-as-effect, security proofs, migrations, modularity |
| 04 | [Compiler architecture](04-compiler-architecture.md) | Pipeline, IRs, signal-graph slicing, wire format, incrementality, `tier explain`, testing |
| 05 | [Tier lowering](05-tier-lowering.md) | Patch-interpreter client + Mode B, dual-backend server, log-as-database, infra object graph |
| 06 | [Kubernetes and packaging](06-kubernetes-and-packaging.md) | Images, derived manifests, the operator, deploys-ride-the-stream, dev→prod ladder |
| 07 | [Dependencies](07-dependencies.md) | Every third-party choice: rationale, licence, benchmarks, rejected alternatives, swap cost |
| 08 | [Roadmap](08-roadmap.md) | Phase 0 premise-proof, walking skeleton, phased plan, exit criteria, team shape |
| 09 | [Risks and open questions](09-risks-and-open-questions.md) | Ranked risks, decisions taken, remaining opens |
| 10 | [Decision log](10-decisions.md) | George's decisions D1–D10, all settled — including the Git/local-first analysis, the language-not-framework rationale, and the name: **Beck** |
| 11 | [Language tour](11-language-tour.md) | What Tier looks like, construct by construct — types, traits, macros, queries, UI, services, in-language tests |
| 12 | [Standards and conformance](12-standards-and-conformance.md) | The rulesets Tier conforms to, layer by layer — and the test-linked spec discipline that makes conformance real |
| 13 | [Testing strategy](13-testing.md) | Every applicable kind of test and why — built around the free oracles Tier's semantics manufacture (DST, differentials, TLA+, Jepsen) |

## The one-paragraph version

`tier` is a statically typed, homoiconic language with a Python-like surface in which the original
`(my-javascript (my-css (my-html)))` becomes literal: the page is a pure function of state, state is
a durable fold over an event stream, and hand-written JavaScript disappears — it's compiler residue.
Clients propose typed commands; one declared merge point admits time and nondeterminism; everything
downstream is deterministic, so replay, time-travel debugging and optimistic UI come from the
semantics rather than from frameworks. Placement is typed and checkable (secrets provably cannot
reach the browser), views are kept incremental by the compiler, and `tier build` emits not a binary
but a deployable system — reproducible OCI images plus a Kubernetes object graph derived from the
program's own effects — which `tier deploy` applies with typed migrations gating the rollout.
