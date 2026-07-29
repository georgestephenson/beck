# 05 — Tier lowering

Four backends behind one front end. Each section states the target, the runtime we must ship, the
dependency choice, and the hard problems.

## 5.1 Browser tier → WebAssembly + fine-grained reactivity

**Target**: WASM (with the GC and reference-types proposals where available; a fallback bump allocator
otherwise) plus a small JS shim for DOM/host access.

**Runtime we must write** (this is real work, ~15–25 kLOC):

- **Signals-based reactive core**, not a virtual DOM. A `Signal[T]` is a node in a dependency graph;
  reading inside an effect subscribes; writing marks dependents dirty; a scheduler flushes once per
  microtask. This is the SolidJS/Leptos model. Rationale: no diffing work proportional to tree size,
  no reconciler heuristics, and — decisively — it is *the same abstraction* as §3.8's server-side
  invalidation, so one concept spans tiers.
- **DOM binding layer**: generated from the WebIDL surface we support, so it stays honest and small.
  Direct `externref` calls where available, otherwise a batched command buffer to amortise the JS
  boundary (many small `wasm→js` calls are the classic WASM-UI performance trap).
- **Client-side data cache** keyed by generated operation id + argument hash, with the subscription
  wiring from §4.3.
- **Router** derived from `service.expose` routes, with typed params.

**Size is the make-or-break metric.** WASM's reputation problem in the browser is payload size. Budget
and enforce: **< 150 KB brotli** for a hello-world app, **< 400 KB** for the running example. Levers:

- Aggressive whole-program DCE after splitting (the client partition is usually a small slice of the
  program — this is a structural advantage over hand-written SPAs where the whole bundle ships).
- No reflection, no dynamic dispatch unless used, monomorphise then merge identical functions.
- GC design that doesn't drag a large runtime in; prefer the WASM GC proposal when the browser matrix
  allows, since it moves the collector out of our payload entirely.
- `wasm-opt` from **Binaryen** at `-Oz`, plus `wasm-snip`, in the release pipeline.
- Split the module: eager core + lazily fetched route chunks, driven by the router.
- A CI gate that fails the build on a size regression > 2%.

**Also required**:

- **SSR / prerender**: run the client partition's component tree on the server (it is the same `Core`;
  place the render on the server, hydrate signals on the client). Needed for first-paint and SEO —
  and it comes almost free from the tierless design, which is a genuine advantage to advertise.
- **Progressive enhancement fallback**: forms that work without WASM, for the `noscript` and
  slow-network cases. Generated from the same component tree.
- **Debugging**: DWARF in the WASM, source maps to `.tier` files, and a browser devtools extension
  that shows the signal graph. Without this, nobody debugs the frontend.

**Rejected alternative**: compile the client tier to JavaScript instead of WASM. Cheaper to start
(smaller payloads today, no GC problem, trivial JS interop) and *worth keeping as a second client
backend* for size-critical apps — but WASM is the right primary target because it lets the client and
server partitions share one code generator, one numeric semantics, and one set of guarantees. Diverging
semantics between tiers would undermine the entire premise. Recommendation: WASM primary, and
`--client-backend=js` as a supported option from v0.4.

## 5.2 Service tier → native code (dual backend)

**Target**: native binaries, statically linked, one per `service` declaration.

Codegen strategy — **two backends**, as rustc does, because the evidence is unambiguous in both
directions:

| Mode | Backend | Evidence |
|---|---|---|
| `tier dev`, `tier check`, hot reload | **Cranelift** | Cranelift compiled a Rust workload in 125 CPU-seconds vs LLVM's 211 (≈40% faster), and the code-generation step alone can be ~an order of magnitude faster than an LLVM-based equivalent |
| `tier build --release` | **LLVM** (via `inkwell`) | Cranelift output measured ~14% slower than LLVM-generated code; and the Perry language's 2026 migration from Cranelift to LLVM took it from behind to beating Node.js by 1.7×–24.6× across benchmarks |

The two backends must agree observably; enforce with a differential test suite (§4.8). Keep the
`Core → Target` interface narrow so a third backend (or MLIR, §4.2) is possible later.

**Runtime we must ship**:

- **Tokio** for the async executor, **Hyper** for HTTP/1+2, **quinn** for HTTP/3 if enabled, **rustls**
  for TLS. Structured concurrency is surfaced in the language (§2.6) and mapped onto Tokio tasks; the
  compiler inserts awaits, so there is no `async` colouring in user code.
- Connection pooling, retries with jitter, circuit breaking, and deadline propagation on generated
  boundaries — defaults, not options.
- **OpenTelemetry** spans emitted automatically at every synthesised boundary. One program ⇒ one
  distributed trace spanning browser click → server → SQL, with no manual instrumentation. This is a
  very strong demo; make it the second slide.
- Graceful shutdown, readiness/liveness endpoints, config from env + mounted secrets — all generated,
  because they are what makes the k8s tier work (§6).

**Optional WASM server target**: compile the service partition to a **WASI Preview 2 component**
instead of a native binary, for edge/serverless deployment. Runtime: **Wasmtime** — in 2026 benchmarks
it leads on cold start (its Winch baseline compiler is built for exactly that) and holds a steady-state
advantage, reaching 2.41× native (improving from 2.54× in 2025 and 2.67× in 2024); on a 1 GB SHA-256
benchmark it was 1.12× slower than native Rust. For long-running compute-heavy workloads, Wasmer's
LLVM backend wins peak throughput and WasmEdge's AOT mode reached 1.74× native — so keep the host
interface behind the **component model / WASI-P2** boundary rather than a Wasmtime-specific API, and
let the deployment choose. Native remains the default for the `service` tier; WASM is for the `edge`
tier and for plugin/multi-tenant isolation.

## 5.3 Data tier → relational plans

Two backends, chosen per store declaration.

**Transactional (default): PostgreSQL.**

- The `Query[T]` logical plan (§3.7) lowers to SQL. Postgres because it is the best open-source
  transactional engine, licence-clean (PostgreSQL License), and universally operable.
- Access via `tokio-postgres`; prepared statements cached per operation id; `sqlx`-style compile-time
  verification is unnecessary because *we* generate the SQL from checked types.
- DDL and migrations generated from `store` declarations (§3.7), applied by the operator (§6.4).
- Pushdown of user code: functions marked `@sql_pure` compile to SQL expressions; where a function is
  not pushable, we can optionally emit it as a **WASM UDF** loaded into the database extension —
  ambitious, post-1.0, but it is the natural endgame of "one language for the database" and worth
  designing the seam for now.

**Analytical / embedded: Apache DataFusion.**

- Chosen over DuckDB deliberately. DataFusion is *a query engine framework designed to be embedded and
  extended* — every major component (table providers, optimiser rules, UDFs) is replaceable, which is
  exactly what we need to plug in our own logical plans. DuckDB is a complete system designed to be
  used as-is. On performance, DataFusion is now the fastest single-node engine for querying Parquet in
  ClickBench — ahead of DuckDB, chDB and ClickHouse on the same hardware — while DuckDB still leads on
  its own native storage format.
- Roles: (a) the **embedded store for `tier dev`**, so local development needs no database server at
  all; (b) the analytical/columnar store for `store x: Analytics[T]`; (c) the evaluator for the
  differential query test harness (§4.8).
- Arrow throughout, so results cross the wire zero-copy (§4.4).

**Also**: expose a **pgwire**-compatible endpoint on Tier's own stores so that `psql`, BI tools and
DBeaver work against a Tier application without special support. Cheap to build, disproportionately
reassuring to buyers.

## 5.4 Infrastructure tier → object graph

The infra tier does **not** generate YAML text and shell out to `kubectl`. It builds a typed object
graph and applies it through the API.

```
service/deployment declarations  +  effects of the placed program
                    │
                    ▼
        InfraGraph  (typed nodes: Image, Workload, Route, Store, Secret, Policy, Grant)
                    │
        ┌───────────┴────────────┐
        ▼                        ▼
  KubernetesPlatform      SingleProcessPlatform      (+ later: NomadPlatform, …)
  k8s objects + CRs        one binary, embedded store
```

- `InfraGraph` is a `Core` value like any other, so it is type-checked, diffable, and testable.
- A `Platform` trait renders the graph to a concrete target. Kubernetes and single-process are the two
  v1 implementations. Keeping this seam is what stops Kubernetes from leaking into language semantics
  (§1.5, §6.1).
- Resources *outside* the cluster (managed Postgres, object storage, DNS, queues) are expressed as
  `Store`/`Resource` nodes and rendered as **Crossplane** claims — one control plane, one reconciler,
  no second state engine (§7.6). An **OpenTofu** emitter exists as an escape hatch for estates
  Crossplane can't reach.
- **Referencing existing infrastructure**: `import infra` reads a cluster's live objects or an
  OpenTofu state as *typed, read-only* facts (a VPC id, a DB endpoint), so Tier can be adopted
  incrementally beside an existing estate rather than demanding a greenfield.

Details of image building, manifest generation, the operator and the dev/prod ladder are in
[`06-kubernetes-and-packaging.md`](06-kubernetes-and-packaging.md).
