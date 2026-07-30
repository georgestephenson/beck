# 05 — Tier lowering

Four backends behind one front end. Each section states the target, the runtime we must ship, the
dependency choice, and the hard problems.

## 5.1 Browser tier — the browser is a patch interpreter (with an upgrade path)

The original sketch's model ([`00`](00-original-idea.md)): the browser's default job is
`fold(apply_patch, initial_html, patch_stream)` — "a couple-kilobyte patch interpreter." The sketch
also marks `page` as `@client`, running `view` locally. Both readings are correct, for different
components — and because `view` is unplaced-pure, **where it runs is a placement decision**
([`03`](03-type-and-effect-system.md) §3.4), made per component by the solver or by annotation.
Either way, the full-circle property holds: hand-written JavaScript never appears in the source —
"it's compiler residue: the patch interpreter plus the compiled view. You stopped writing it the
moment the page became a function." Two rendering modes, one source:

| | **Mode A — thin (default)** | **Mode B — local** |
|---|---|---|
| `view` runs on | server | client |
| Wire carries | DOM patches | data patches (state diffs) |
| Client payload | **~5–10 KB JS** patch interpreter + input capture | WASM (or JS) build of the component's `view` + `apply_event` + signal runtime |
| First paint | free — it *is* the SSR output | SSR + hydrate signals |
| Optimistic UI | no (round trip per interaction) | yes — same fold runs locally, reconciled by `seq` |
| Offline | no | possible (local log, replay on reconnect) |
| Server cost | per-session view state (the LiveView cost) | per-session subscription only |
| Precedent | Phoenix LiveView, Electric | Elm/Lamdera, Leptos |

Defaults that fall out: content-shaped components (lists, dashboards, forms-that-submit) are Mode A;
components with `optimistic` interactions, latency-sensitive input (typeahead, drag), or an
`offline` requirement get Mode B — inferred from those requirements, overridable with
`@render(server)` / `@render(client)`. A page mixes modes freely; the boundary is per component
subtree. **v0.1 ships Mode A only** — it makes the walking skeleton drastically smaller (no GC in
WASM, no size crisis, trivially good Lighthouse scores) — with Mode B in Phase 3
([`08`](08-roadmap.md)).

**Runtime we must write:**

- **The patch protocol**: a compact DOM-diff format (keyed children, text/attr ops, component
  boundaries as stable frames) applied by the thin client; *the same format* is the SSR hydration
  seed. Server-side, `Html` values diff structurally — and because views are signal-derived, the
  differ knows which subtrees *can't* have changed and skips them (this is where fine-grained
  signals beat blind vdom diffing even server-side).
- **Input capture**: event handlers in `view` compile to declarative attributes
  (`on_click → send(Toggle(id))` becomes a serialized command constructor); the thin client posts
  commands up the same websocket. No user JS runs in Mode A at all — which is also a CSP story:
  `script-src` can be near-empty.
- **Mode B kernel** (Phase 3): the component's pure code compiled to WASM (GC proposal where
  available; Perceus-style refcounting fallback — the Roc/Koka route, given our immutable data),
  fine-grained signal graph, local speculative fold + `seq`-based reconciliation
  ([`03`](03-type-and-effect-system.md) §3.7). Size budgets enforced in CI: < 150 KB brotli for a
  typical Mode-B component bundle; `wasm-opt -Oz` (Binaryen) in the release path.
- **Connection layer**: one websocket (WebTransport later) multiplexing patch-streams down and
  commands up; resumable by `(subscription id, last seq)` so a dropped connection or a deploy
  replays the gap instead of re-rendering the world; sticky-session friendly but not dependent
  (resume works cross-replica because subscriptions are keyed by log position, not socket).
- **Router** derived from route declarations; navigation is just another command; scroll/focus
  preservation handled by frame identity in the patch protocol.
- **Progressive enhancement**: Mode A degrades to plain forms + full-page responses under
  `noscript` — generated from the same `view`, since the server can always render.

**Debugging**: source maps from patch frames back to `view` expressions; a devtools extension
showing the signal graph, patch traffic, and pending (optimistic) state; DWARF for Mode B WASM.

## 5.2 Service tier → native code (dual backend)

The server partition — ingress, `validate`, folds, view evaluation for Mode A sessions, boundary
endpoints — compiles to native binaries, statically linked, one per `service`.

**Dual codegen**, as rustc does, because the evidence points both ways:

| Mode | Backend | Evidence |
|---|---|---|
| `tier dev`, hot reload | **Cranelift** | ~40% faster whole-compiles; codegen step ~an order of magnitude faster than LLVM |
| `tier build --release` | **LLVM** (inkwell) | Cranelift output ~14% slower; Perry's 2026 Cranelift→LLVM move turned a deficit into 1.7–24.6× wins over Node.js |

The two must agree observably — enforced by differential tests ([`04`](04-compiler-architecture.md)
§4.8). The `Core → Target` seam stays narrow so a third backend (or MLIR) can slot in later.

**Runtime we must ship** (the "Roc platform" of Tier — an effectful Rust host owning I/O, scheduling
and memory, executing the pure program):

- **Tokio** executor; **Hyper** HTTP/1+2; **quinn** HTTP/3 optional; **rustls** TLS.
- The **log engine client**: append at ingress (envelope stamping, `seq` assignment), fold
  execution, snapshot writing, subscription fan-out. This component is small but is *the* hot path;
  it is where [`07`](07-dependencies.md) §7.4's storage choices land.
- Structured concurrency mapped onto tasks; the compiler inserts awaits — no `async` colouring in
  user code.
- Generated boundaries get pooling, retries with jitter, deadlines, idempotency by envelope
  identity; **OpenTelemetry spans at every synthesized boundary** — one trace from browser click →
  command → event → fold → patch, no manual instrumentation. Make this the second demo.
- Graceful drain (finish folds, snapshot, hand off subscriptions), readiness/liveness, config from
  env + mounted secrets — all generated, because [`06`](06-kubernetes-and-packaging.md) depends on
  them.

**Optional WASM server target**: compile the service partition to a **WASI Preview 2 component**
for edge/serverless/multi-tenant placement. Runtime: **Wasmtime** (leads cold start and steady
state — 2.41× native in 2026 benchmarks, from 2.67× in 2024). Target the component-model boundary,
not any runtime's API — WasmEdge's AOT (1.74× native) and Wasmer's LLVM backend win other workload
shapes, and the deployment should be free to choose.

## 5.3 Data tier — the log is the database

The semantic model is fixed by [`03`](03-type-and-effect-system.md) §3.7: an append-only,
totally-ordered log of envelopes; `durable` folds as state; views as incrementally-maintained pure
functions. The lowering question is substrates. **We do not write a storage engine**
([`01`](01-vision-and-premise.md) §1.5) — we write a small log engine *on top of* proven storage:

| Concern | v1 substrate | Why |
|---|---|---|
| The log | **PostgreSQL** (append-only table per app; `seq` from a sequence; logical decoding for tailing) | Boring, transactional, operable everywhere, PITR for free; licence-clean. A dedicated log store (e.g. NATS JetStream, Apache-2.0) is a post-1.0 option when fan-out demands it — Kafka's JVM weight is unnecessary; Redpanda is BSL, excluded |
| Snapshots | object storage (S3-compatible) or Postgres large objects | Cheap, versioned, feeds `tier fork` |
| Read models | generated tables in the same Postgres | One-shot queries and **pgwire access for the outside world**: `psql`, BI tools, DBeaver see materialized views as ordinary tables — the single cheapest trust-builder for adopting teams |
| Dev (rung 0) | in-memory folds + embedded append-only log (**redb**, MIT/Apache) | `tier run` needs no server; the log file is still replayable |
| Analytical stores | **Apache DataFusion** over Arrow/Parquet (log archives Parquet-partitioned) | Fastest single-node Parquet engine (ClickBench); designed to be embedded/extended — the right shape for our plans, where DuckDB is a complete system designed to be used as-is |

**Incremental view maintenance** — "keeping it incremental is the compiler's job":

- Subscribed/materialized views compile to **differential-dataflow-style incremental plans**
  (timely/differential are MIT-licensed Rust; DBSP/Feldera is the maintained modern embodiment —
  [`07`](07-dependencies.md) §7.4). `remaining` updates by ±1 per event; a joined read model
  updates by delta, not by re-join. Materialize itself validates the approach commercially but is
  BUSL — excluded as a dependency by the open-source constraint.
- **Per-session fanout is the scaling problem to design for** ([`03`](03-type-and-effect-system.md)
  §3.8): a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one*
  shared dataflow whose final per-session operators (filter, project, diff) run per subscriber —
  the differential "arrangement" sharing model — not a thousand plans. Subscription count, shared-
  prefix hit rate, and per-session memory are metrics the runtime exports from day one, because
  this is where a naive implementation quietly becomes Meteor-at-scale.
- v0.1 does **not** need the dataflow engine: full recompute per event on in-memory folds is
  semantically identical and fine at todo-app scale; the incremental plan is an optimisation with
  an exact correctness oracle (recompute) to test against — a luxurious position for CI
  ([`04`](04-compiler-architecture.md) §4.8).
- The comprehension surface also lowers to SQL for one-shot reads against read models, with
  compile-checked columns; `@sql_pure` user functions push into SQL expressions.
- Query fusion still matters (a `for` over a view of a view should become one plan, not N+1
  lookups); it is a plan-rewrite on symbolic `Query` nodes, kept symbolic in `Core` precisely for
  this ([`04`](04-compiler-architecture.md) §4.2).

**External stores** (`external store`) — existing databases the team already owns — get generated
typed access with `external.*` effects, no fold guarantees, and honest documentation that they are
the adoption ramp, not the model.

## 5.4 Infrastructure tier → object graph

No YAML text, no `kubectl` shelling. The compiler builds a typed `InfraGraph` — nodes like `Image`,
`Workload`, `Route`, `LogStore`, `SnapshotSchedule`, `Secret`, `Policy`, `Grant` — **derived from
program analysis**, exactly as the original sketch demands: a `durable` fold ⇒ a `LogStore` +
volume + snapshot schedule; `merge_clients()` ⇒ a websocket ingress route; `net.out(host)` ⇒ an
egress policy entry; a `cron` declaration ⇒ a schedule.

```
service/deployment declarations  +  effect rows of the placed program
                    │
                    ▼
            InfraGraph  (typed, diffable, testable — an ordinary Core value)
                    │
        ┌───────────┴────────────┐
        ▼                        ▼
  KubernetesPlatform      SingleProcessPlatform     (+ later: Nomad, serverless…)
```

- The `Platform` trait keeps orchestrators out of language semantics
  ([`06`](06-kubernetes-and-packaging.md) §6.1). Kubernetes and single-process are the two v1
  implementations.
- Off-cluster resources (managed Postgres, buckets, DNS, queues) render as **Crossplane** claims —
  one control plane, no second state engine; an **OpenTofu** emitter is the escape hatch for
  estates Crossplane can't reach. (Terraform is BUSL — excluded.)
- `import infra` reads live cluster objects or OpenTofu state as *typed, read-only facts* (a VPC
  id, a DB endpoint) so Tier can be adopted beside an existing estate rather than instead of it.

Details of images, manifests, the operator, migrations-in-deploys and the dev→prod ladder:
[`06-kubernetes-and-packaging.md`](06-kubernetes-and-packaging.md).
