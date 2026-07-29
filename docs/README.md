# Tier — implementation plan

One language for the frontend, the backend, the database, the container, and the cluster.

## ⚠️ Read this first: premise reconstruction

The source conversation for this idea (a `claude.ai/share/...` link) is **not machine-readable** —
shared conversation URLs return the single-page-app shell to any fetcher, and the underlying API
path answers `403`. So the design premise below was **reconstructed** from three signals:

1. The repository is named **`tier`**.
2. The stated goal: *"one language for: frontend, backend, database, IaC, containerisation."*
3. The note that *"this lisp type syntax illustrates my idea in the best way."*

That points squarely at **tierless (multitier) programming**: you write one program, and the
compiler — not you — decides and emits which parts run in the browser, which run on the server,
which become SQL, and which become infrastructure. A Lisp surface illustrates it best because
homoiconicity makes the *code-as-data* moves (quoting a fragment of the program and shipping it
to another tier) look trivial, which is exactly the trick at the heart of the idea.

**If that is not your idea**, the pieces most likely to need rework are
[`01-vision-and-premise.md`](01-vision-and-premise.md) and [`03-type-and-effect-system.md`](03-type-and-effect-system.md);
paste the transcript and those two can be rewritten without disturbing the dependency, packaging,
or roadmap docs. Everything in [`07-dependencies.md`](07-dependencies.md),
[`06-kubernetes-and-packaging.md`](06-kubernetes-and-packaging.md) and
[`08-roadmap.md`](08-roadmap.md) holds for any language project of this shape.

## The three questions you asked, answered

| Your question | Short answer | Where |
|---|---|---|
| Can it be Python-like without losing Lisp power? | **Yes, and this is a solved problem** — homoiconicity is a property of the *core AST*, not the surface. Elixir, Julia and Nim all have non-Lisp syntax and full hygienic macros. The one thing you must add that Python lacks is a **block-passing call form** (`name(args):` + indented body → macro receives the body as AST). Keep the S-expression notation as a first-class *second surface* that prints from the same AST. | [`02-syntax.md`](02-syntax.md) |
| Dependencies: best-in-class *and* open source? | Rust host; Salsa incremental core; **dual codegen** (Cranelift for dev speed, LLVM for release throughput); Wasmtime for server-side WASM; DataFusion + PostgreSQL for the data tier; **apko/melange** for bit-reproducible daemonless OCI images; **kube-rs** for the operator; **Crossplane** (not BUSL Terraform) for cloud resources. Full table with licences, benchmark citations, and rejected alternatives. | [`07-dependencies.md`](07-dependencies.md) |
| Kubernetes under the hood? | Yes — as a **compiler backend**, behind a `Platform` trait, never as language semantics. `tier build` emits OCI images + a Kubernetes object graph; a Tier operator reconciles it. Critically: `tier dev` must run the whole program in **one process with zero Kubernetes**, or the language dies on first contact with a beginner. | [`06-kubernetes-and-packaging.md`](06-kubernetes-and-packaging.md) |

## Documents

| # | Document | What it covers |
|---|---|---|
| 01 | [Vision and premise](01-vision-and-premise.md) | What "one language" means precisely, the worked example used throughout, non-goals, prior art and what killed it |
| 02 | [Syntax: Python surface, Lisp core](02-syntax.md) | Dual-surface design, the macro system, block-passing, hygiene, what you genuinely give up |
| 03 | [Type and effect system](03-type-and-effect-system.md) | Placement-as-effect, the placement solver, capability-based security, relational types |
| 04 | [Compiler architecture](04-compiler-architecture.md) | Pipeline, IRs, the partitioning pass, RPC synthesis, incrementality |
| 05 | [Tier lowering](05-tier-lowering.md) | Per-tier backends: browser/WASM, server/native, data/SQL, infra/object graph |
| 06 | [Kubernetes and packaging](06-kubernetes-and-packaging.md) | Images, manifests, the operator, the dev→prod parity ladder |
| 07 | [Dependencies](07-dependencies.md) | Every third-party choice, with rationale, licence, benchmark evidence, and the alternative rejected |
| 08 | [Roadmap](08-roadmap.md) | Phased plan, walking skeleton first, milestone exit criteria, team shape |
| 09 | [Risks and open questions](09-risks-and-open-questions.md) | Honest failure modes, mitigations, and the decisions that need you |

## The one-paragraph version

`tier` is a statically typed, homoiconic language with a Python-like default surface syntax. A
program declares data models, functions, UI components, services and deployments in one module
graph. Placement across execution tiers is part of the **type system** — an effect row — so it can
be inferred, checked, and used to prove security properties (a `Secret[T]` cannot reach the browser
tier because the type system forbids the flow, not because a reviewer noticed). The compiler
partitions the typed core IR by tier and lowers each partition through a different backend: WASM
for the browser, native code for services, relational plans for the database, and an OCI-image +
Kubernetes object graph for deployment. The result of `tier build` is not a binary — it is a
deployable system, and `tier deploy` applies it.
