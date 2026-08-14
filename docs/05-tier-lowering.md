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

> **Built** ([`94`](94-the-client-report.md)), and three lines above are now corrections rather than
> plans. `@render(client)` is the whole surface: **the mode is declared, never inferred**, because a
> wrong inference ships a state to a browser. A page mixes modes freely is still unbuilt, because a
> program has one `page`. And the promotion carries a **refusal this section did not anticipate** —
> a component whose view reads the session cannot render on the client, since Mode B hands the
> browser the state a per-session view was filtering (§94.2). *That refusal is narrower than it was
> written: it is about **who** is asking, and reading `session.path` — where the browser is, which
> the browser chose — is allowed ([`94`](94-the-client-report.md) §94.3).* The kernel interprets rather than
> compiles ([`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md)), which is what the size
> budget below has to be read against: 179,195 bytes brotli once per application, 1,753 per
> component.

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
  typical Mode-B component bundle; `wasm-opt -Oz` (Binaryen) in the release path. — **Built, with
  the first clause deferred** ([`94`](94-the-client-report.md)): the local fold and the reconciliation
  are here, and the WASM is the *evaluator* rather than the component compiled. The budget is
  answered in two parts, because a shared kernel and a per-component payload are different
  questions; `wasm-opt` is not run, so the kernel's number is a ceiling. *The budget is **enforced**
  now rather than reported ([`94`](94-the-client-report.md) §94.11): `beck bundle`
  writes the artefact, a `budgets` job weighs every Mode B example against 150 KB brotli, and a
  shape gate under `cargo test` says the thing the threshold cannot — that a bundle is a function of
  the component's slice and not of the program around it.*
- **Connection layer**: one websocket (WebTransport later) multiplexing patch-streams down and
  commands up; resumable by `(subscription id, last seq)` so a dropped connection or a deploy
  replays the gap instead of re-rendering the world; sticky-session friendly but not dependent
  (resume works cross-replica because subscriptions are keyed by log position, not socket).
- **Router** derived from route declarations; navigation is just another command; scroll/focus
  preservation handled by frame identity in the patch protocol.
- **Progressive enhancement**: Mode A degrades to plain forms + full-page responses under
  `noscript` — generated from the same `view`, since the server can always render.

> **The router is built** ([`94`](94-the-client-report.md)), and this line is a correction rather
> than a plan in two places. There are **no route declarations**: a route is a field of `Session`,
> so the page is a pure function of it and "which page is this" is written in the program. And
> navigation is **not** a command — a command is a proposal that becomes an event and reaches the
> log, and where a browser is is neither. It is a `Session` that moved, which is why Mode A answers
> a navigation with the difference between two pages and Mode B answers it with nothing at all.
> Focus and scroll are kept by the patch interpreter rather than by frame identity (§94.8), at a
> cost proportional to the patch. **Lazy routes are not built** and §94.15 says what they wait on.
> Progressive enhancement is not built either — but a route *is* a URL the server renders, which is
> the half of it that mattered most.

**Debugging**: source maps from patch frames back to `view` expressions; a devtools extension
showing the signal graph, patch traffic, and pending (optimistic) state; DWARF for Mode B WASM.

> The **panel** is built ([`94`](94-the-client-report.md) §94.9) — all three of the things this
> line names — and it is a page the server serves rather than an extension, for a reason written out
> there. Source maps and DWARF are not. What a page could already be asked is whether its client is
> live — `data-b-ready` carries the mode's letter and `beck:ready`, `beck:rejected` and `beck:error`
> are bubbling events on the frame root ([`94`](94-the-client-report.md) §94.13) — and the panel is
> attached to exactly that.

## 5.2 Service tier → native code (dual backend)

The server partition — ingress, `validate`, folds, view evaluation for Mode A sessions, boundary
endpoints — compiles to native binaries, statically linked, one per `service`.

**Dual codegen**, as rustc does, because the evidence points both ways:

| Mode | Backend | Evidence |
|---|---|---|
| `beck dev`, hot reload | **Cranelift**, as a crate, emitting an object a linker turns into a program — **built** for the scalar subset and for records and unions ([`93`](93-the-native-backends-report.md), [`93`](93-the-native-backends-report.md), [`adr/0024`](adr/0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md)) | ~40% faster whole-compiles; codegen step ~an order of magnitude faster than LLVM |
| `beck build --release` | **LLVM**, as textual IR through the host's `clang` — **built** for the scalar subset and for records and unions ([`93`](93-the-native-backends-report.md), [`93`](93-the-native-backends-report.md)), not via `inkwell` ([`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)) | Cranelift output ~14% slower; Perry's 2026 Cranelift→LLVM move turned a deficit into 1.7–24.6× wins over Node.js |

The two must agree observably — enforced by differential tests ([`04`](04-compiler-architecture.md)
§4.8). The `Core → Target` seam stays narrow so a third backend (or MLIR) can slot in later.

> **Built, half of it.** [`93`](93-the-native-backends-report.md): the LLVM row exists over the **scalar
> subset** — `Int`, `Float` and `Bool` — and the differential is against the *evaluator*, since
> Cranelift is still not built. There is no heap in the emitted code, so a fold over a record, a
> view that builds `Html` and every effect in this section still run on the tree-walker, and this
> section's "compiles to native binaries, statically linked, one per `service`" remains a design.
> The seam held: not one line of `beck-rt` changed.
>
> *Both rows exist now* ([`93`](93-the-native-backends-report.md)), over the same scalar subset and held to
> the same programs: `beck native --backend cranelift|llvm`, and "the two must agree observably" is
> a **three-way** differential — the tree-walker, LLVM and Cranelift on every call. The heap is
> what still bounds them, and it bounds both equally, so the sentence above about records, `Html`
> and effects is unchanged. What *is* new is that the second implementation exists to disagree:
> §93.8 is what holding two emitters to one subset found.
>
> *And the heap has its first floor* ([`93`](93-the-native-backends-report.md)): a `model`, a `union` and a
> `newtype` compile on both, over one layout shared with the host, in an arena of offsets
> ([`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md)). So "a fold over a record" is
> half-true rather than false — the *record* compiles and the fold's `Map` does not. Text,
> collections, closures and every effect are still the tree-walker's, and this section's "compiles
> to native binaries, statically linked, one per `service`" remains a design for that reason.
> §93.15 is the list, each row with a test that goes red the day it stops being true.
>
> *And that list is now two rows long* ([`93`](93-the-native-backends-report.md),
> [`93`](93-the-native-backends-report.md), [`93`](93-the-native-backends-report.md),
> [`93`](93-the-native-backends-report.md), [`93`](93-the-native-backends-report.md)): text, a
> `list`, a `Map`, a closure and — since [`93`](93-the-native-backends-report.md) — **the
> view** all compile on both, so "a view that builds `Html`" above is no longer among what runs on
> the tree-walker, and 21 of the 32 corpus programs have a `view` that compiles. The paragraph before
> this one is therefore corrected in its last sentence: what is still the tree-walker's is **every
> effect** and every operation that *grows* a collection — the second being a decision rather than a
> gap ([`93`](93-the-native-backends-report.md) §93.7), which is why "a fold over a record"
> stays half-true and this section's "compiles to native binaries, statically linked, one per
> `service`" stays a design. A view compiles as the **call** that builds the page rather than as the
> page, so §93.5 is explicit that it buys no speed: it removes a prerequisite, and the one it
> removes is Mode B's codegen ([`94`](94-the-client-report.md) §94.15).
>
> *And the list is empty of classes* ([`93`](93-the-native-backends-report.md),
> [`93`](93-the-native-backends-report.md), [`93`](93-the-native-backends-report.md),
> [`93`](93-the-native-backends-report.md), [`93`](93-the-native-backends-report.md)): growing a
> `list` and a `Map` compile, a generic definition compiles by being specialised, a list pattern
> comes apart, and **the four primitives that ask the host** — `now()`, `uuid()`, `secret_env` and
> `http_fetch` — compile by *asking*, on a worker protocol that grew a second direction. So the
> paragraph above is corrected twice over: neither "every effect" nor "every operation that grows a
> collection" is the tree-walker's any more. What is left is not a class but three names
> ([`93`](93-the-native-backends-report.md) §93.15) — the signal vocabulary, which the splitter
> reads rather than a body calling it; a **bounded** definition, whose dictionary is a function
> value; and a worker that can answer two calls at once. This section's "compiles to native
> binaries, statically linked, one per `service`" is *still* a design, and now for one reason rather
> than a list: a compiled definition is called through a pipe rather than linked into a server.
>
> *And "statically linked" is half-true now* ([`93`](93-the-native-backends-report.md) §93.12): a
> compiled program that reaches a digest, an encoding, a case mapping, `str_to_int` or the calendar
> **links a static library** — `beck-prim`, the same crate the tree-walker calls, so the two cannot
> disagree about a Unicode table. What is still a design is the sentence's other half: the linked
> thing is a runtime library rather than the server, and the definition is still reached across a
> pipe.

**Runtime we must ship** (the "Roc platform" of Beck — an effectful Rust host owning I/O, scheduling
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
| Snapshots | object storage (S3-compatible) or Postgres large objects | Cheap, versioned, feeds `beck fork` |
| Read models | ~~generated tables in the same Postgres~~ — **the arrangement itself** ([`23`](23-incremental-views-report.md), [`10`](10-decisions.md) D26) | One-shot queries and **pgwire access for the outside world**: `psql`, BI tools, DBeaver see materialized views as ordinary tables — the single cheapest trust-builder for adopting teams. Built, and the first half of this row is what changed: a read model is not a second copy written on the append path, it is the collection the fold holds and the arrangement the view engine maintains, projected. Nothing is written per event, and nothing can drift from the page |
| Dev (rung 0) | in-memory folds + embedded append-only log (**redb**, MIT/Apache) | `beck run` needs no server; the log file is still replayable |
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

  *Both halves exist.* The signal graph is a graph and a computation read by two consumers is
  identified as one ([`23`](23-incremental-views-report.md) §23.3), which is what an arrangement needs
  to be shareable; the dataflow engine that shares it is built
  ([`23`](23-incremental-views-report.md), [`23`](23-incremental-views-report.md)), and the
  operators above the session cut are held once for the whole fanout rather than once per
  subscriber. What that is worth is a property of the program: 55× less work per event on a public
  feed, 1.3× on the todo sketch, because where a program reads the session decides its fanout cost.
  Per-session memory is exported ([`23`](23-incremental-views-report.md) §23.14). **The read
  models and the pgwire exposure below are built too** ([`23`](23-incremental-views-report.md)),
  and on the same cut: a table is a view that does not depend on who is asking, which is the
  `per_session` boundary this paragraph draws, used a second time. **Query fusion is built too**
  ([`23`](23-incremental-views-report.md)), and the condition that turned out to matter is this
  paragraph's: a rewrite may not fuse a shared operator into a per-session one, because the smaller
  plan is the slower program.

  *The cut has a second thing on the per-subscriber side of it, and it is not the session.*
  `presence()` ([`48`](48-identity-report.md)) is the same value for everybody and still runs per
  subscriber, because the shared dataflow is versioned by the log's `seq` and the roster moves when
  `seq` does not: two subscribers at one version must be handed one input, and there is no version
  at which they can be. Sharing it needs a second clock, which is §48.13's first unbuilt item.
- v0.1 does **not** need the dataflow engine: full recompute per event on in-memory folds is
  semantically identical and fine at todo-app scale; the incremental plan is an optimisation with
  an exact correctness oracle (recompute) to test against — a luxurious position for CI
  ([`04`](04-compiler-architecture.md) §4.8).
- The comprehension surface also lowers to SQL for one-shot reads against read models, with
  compile-checked columns; `@sql_pure` user functions push into SQL expressions.
- Query fusion still matters (a `for` over a view of a view should become one plan, not N+1
  lookups); it is a plan-rewrite on symbolic `Query` nodes, kept symbolic in `Core` precisely for
  this ([`04`](04-compiler-architecture.md) §4.2).
  *Built* ([`23`](23-incremental-views-report.md)), and on the **dataflow** plan rather than on the
  `Query` sub-language this line names: `for t in todos:` decomposes to a `map_list` under a
  `flatten` and fuses to one operator, so the arrangement between them is never built. Five local
  rewrites, each sound against the change semantics; a rewrite is refused when the operator it would
  absorb is read twice, is named by a signal, or is shared while its consumer is per session.
  `beck explain query` prints both what fused and what did not, with the condition that refused it.

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

  *Built* — `beck-infra/src/platform.rs`, with `k8s::Kubernetes` and `compose::Compose` behind it
  and `--platform` on `beck build`/`beck up`. It was promised in ten places and declared in none
  until [`20`](20-phase-2-report.md) §20.4 item 15; what made it a day's work rather than a rewrite
  is that `InfraGraph` never contained a Kubernetes noun, so only the emitter had to move. The
  second implementation is the point: Compose has no namespaces, no selectors, no CRDs and no
  network policy, and each absence found somewhere the first target had leaked into the interface.
- Off-cluster resources (managed Postgres, buckets, DNS, queues) render as **Crossplane** claims —
  one control plane, no second state engine; an **OpenTofu** emitter is the escape hatch for
  estates Crossplane can't reach. (Terraform is BUSL — excluded.)
- `import infra` reads live cluster objects or OpenTofu state as *typed, read-only facts* (a VPC
  id, a DB endpoint) so Beck can be adopted beside an existing estate rather than instead of it.

Details of images, manifests, the operator, migrations-in-deploys and the dev→prod ladder:
[`06-kubernetes-and-packaging.md`](06-kubernetes-and-packaging.md).

## 5.5 Future surfaces: is this extensible to native mobile? (not v1)

Yes — by design, and cheaply, because of three properties v1 already has:

1. **`view` emits a typed semantic tree, not HTML strings.** The `ui:` vocabulary (input, button,
   list, section…) is meaning, not markup; the DOM is merely its first renderer. A mobile backend
   maps the same tree onto **Jetpack Compose** (Android) and **SwiftUI** (iOS) — both declarative
   frameworks whose recomposition model matches our signals one-to-one. Native look, native
   accessibility, same source. (A self-rendered Flutter-style canvas is the rejected default:
   pixel-consistent but alien to each platform.)
2. **The pure core needs no browser.** Folds, views, the signal kernel and the wire client compile
   natively to arm64 through the *existing* LLVM backend — no WASM required on mobile, and the
   one-semantics-everywhere guarantee (what makes optimism sound) carries over unchanged.
3. **Mode B is already the mobile shape.** Local state, queued commands, offline tolerance,
   `seq`-reconciled optimism ([`10`](10-decisions.md) D7) — designed for flaky networks before a
   phone ever entered the picture. Identity rides the system-browser OIDC pattern; push
   notifications become one more typed ingress.

The honest new problem is **deploys**: app-store review breaks "deploys ride the stream" — old
clients live for months, so the wire-compat matrix and upcasters ([`04`](04-compiler-architecture.md)
§4.3) become mobile-critical, and server-driven (Mode A-style) screens regain value as the
store-policy-friendly way to change UI without a release.

Cost now, so this stays cheap later: keep the `ui:` core vocabulary free of HTML-isms, and keep the
renderer behind a `Surface` trait (Dom is implementation #1). Both are v1 disciplines we want
anyway. Everything else waits for post-1.0.
