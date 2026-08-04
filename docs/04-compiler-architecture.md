# 04 — Compiler architecture

## 4.1 Pipeline

```
  .beck / .sx
      │
      ▼
 ┌──────────────┐
 │ 1 Lex+Layout │  logos + hand-written INDENT/DEDENT
 ├──────────────┤
 │ 2 Parse      │  recursive descent + Pratt  ──▶  Node (homoiconic AST, §2.2)
 ├──────────────┤
 │ 3 Expand     │  hygienic macro expansion, fixpoint, Salsa-cached
 ├──────────────┤
 │ 4 Resolve    │  modules, imports, name binding, hygiene scopes → resolved AST
 ├──────────────┤
 │ 5 Typecheck  │  HM + rows + effect rows + capabilities → typed AST
 ├──────────────┤
 │ 6 Lower      │  desugar to CORE: typed SSA-ish ANF, explicit effects,
 │              │  closures explicit, no surface sugar left
 ├──────────────┤
 │ 7 PLACE      │  ◀── the product.  constraint solve → every Core node carries a tier
 ├──────────────┤
 │ 8 Split      │  partition Core into per-tier programs; SYNTHESISE boundaries
 │              │  (signal slicing, patch/command channels, serialisers, migrations)
 ├──────────────┤
 │ 9 Optimise   │  per-tier: inline, specialise, DCE (aggressive on client), fuse queries
 ├──────────────┤
 │10 Codegen    │  4 backends, see 05-tier-lowering.md
 ├──────────────┤
 │11 Assemble   │  OCI images (apko), k8s object graph, asset manifest, SBOM, signatures
 └──────────────┘
      │
      ▼
  DeploymentPlan  ──▶  beck deploy  ──▶  server-side apply / Beck operator
```

Stages 1–6 are a conventional modern compiler front end; do not innovate there. Stages 7–8 are novel
and are where the engineering budget goes. Stages 10–11 are integration work against the dependencies
in [`07-dependencies.md`](07-dependencies.md).

## 4.2 Intermediate representations

Three, and no more — every extra IR is a tax on every later feature.

| IR | Shape | Purpose |
|---|---|---|
| **`Node`** | Untyped/typed tree, homoiconic, spans | Surface, macros, tooling, formatter, LSP |
| **`Core`** | Typed ANF/SSA hybrid; explicit closures, explicit effect operations, explicit tier annotation per node; `Query` sub-language kept *symbolic* | Typechecked semantics, placement, splitting, optimisation. The load-bearing IR |
| **`Target`** | Per-backend: Cranelift IR / LLVM IR / WASM / dataflow+SQL plans / k8s object graph | Codegen only |

Two deliberate choices inside `Core`:

- **Views/queries stay symbolic** until stage 10. If you lower a view to loops early, you can never
  compile it to an incremental dataflow plan or a SQL read model. Keep the view sub-language as a
  first-class `Core` node with its own typing rules.
- **UI trees stay symbolic** too, for the same reason: a component tree that has already become DOM
  mutation calls cannot be server-side rendered or pre-rendered at build time.

### On MLIR

MLIR is the obvious-looking fit — dialects per tier, progressive lowering, exactly our shape. I still
recommend **against** it for v1:

- It is C++ with a fast-moving API; the FFI surface from Rust is large and the build becomes an LLVM
  build. Contributor accessibility drops sharply.
- Our optimisation needs are *not* the ones MLIR excels at (loop nests, tensors, polyhedral). Ours are
  inlining, specialisation, DCE, and relational pushdown — all easier in a bespoke typed IR with our
  effect information attached.
- Revisit if and when a numeric/tensor tier appears; the `Core` → `Target` seam is where MLIR would
  slot in without disturbing anything upstream.

## 4.3 The splitting stage in detail

Given placed `Core`, stage 8 does five things:

1. **Partition** into one `Core` program per tier per service. Multi-placed functions are duplicated
   (and their identity recorded, so `beck explain` can say "compiled into 2 tiers").
2. **Synthesise boundary stubs.** For each cross-tier call, emit a caller stub and a callee entry:
   - a stable, content-derived operation id (`blake3(module, structure-of-command, structure-of-event,
     structure-of-state)[..16]`) — *not* a URL a human maintains, and stable across refactors that
     don't change the signature. **Content means structure, not names**: Phase 1 hashed the *names*
     of those three types, so the id was stable across adding a field to a command — which is
     precisely the change that breaks every open tab. It now hashes them transitively through every
     field of every variant they reach ([`20`](20-phase-2-report.md) §20.4 item 4);
   - serialiser/deserialiser pairs generated from the types (§4.4);
   - an authorisation check derived from the callee's `cap.*` effects;
   - request batching and coalescing by default (one round trip per event-loop turn, Haxl-style),
     because tierless code makes fine-grained calls *look* free and they are not;
   - idempotency by envelope identity on the command channel, so client retries are safe.
3. **Slice the signal graph.** Every signal edge that crosses tiers becomes a subscription: the
   server side gets a diff operator (DOM patches for Mode-A components, data patches for Mode-B —
   [`05`](05-tier-lowering.md) §5.1), the client side a resumable `(subscription, seq)` consumer;
   `send` becomes the upstream command channel into the ingress. There is no cache-invalidation
   wiring to synthesise — views are downstream of the log by construction (§3.8).

   *Built* — `beck-core/src/{signal,split}.rs`, and "every … edge" is literal: the crossings are
   enumerated with a content-derived id each, which `beck explain flow` prints
   ([`23`](23-general-slicer-report.md) §23.3). The todo sketch has three, and both earlier reports
   said it had one.
4. **Emit state artefacts.** Log-store DDL, read-model DDL, snapshot schedules, and — when
   accumulator or event types changed against the previously deployed signature — the demand for
   `migrate`/`upcast` functions, refusing to build a deployable plan without them (§3.9).
5. **Emit the infra object graph.** `service`/`deployment` declarations, plus everything inferred from
   effects: RBAC verbs, network policy peers, volume claims, secret references.

**Boundary versioning** is a hard requirement, not a nicety: during a rolling deploy, old clients talk
to new servers. Rules: operation ids are content-derived; a removed operation is retained as a
deprecated shim for N releases (declared in `beck.toml`); the wire format is field-tagged and
tolerates unknown fields; `beck check --wire-compat <previous-release>` runs in CI and fails on a
breaking change without an explicit `@breaking` marker. Getting this wrong produces the failure that
kills adoption — "the deploy worked but every open browser tab broke."

## 4.4 Wire format

- **Internal (Beck↔Beck)**: a compact, field-tagged binary encoding generated from types — schema
  known on both sides at build time, so no self-describing overhead. `postcard`-class efficiency with
  tags for compatibility. Zero-copy on read where the type allows. Envelopes, commands, data
  patches and DOM patches all ride this one encoding; every patch is tagged with the `seq` it
  brings the subscriber up to, which is what makes resumption and optimism reconciliation cheap.
- **Bulk/columnar** (query results, analytics, anything > ~1000 rows): **Apache Arrow** IPC. Zero-copy
  into DataFusion on the server, and Arrow decoding in WASM is fast enough for real data tables.
- **External (public API)**: generate **OpenAPI + JSON** and **gRPC/Protobuf** from the same types, on
  request (`@public(rest)`, `@public(grpc)`). Beck's internal format is never a public contract.
- **Transport**: HTTP/2 (h2 via `hyper`) with HTTP/3 (`quinn`) optional; WebSocket or WebTransport for
  subscriptions; TLS via `rustls`.

## 4.5 Error messages as a first-class subsystem

For a language whose main feature is inference, error quality *is* the product. Concretely:

- Every `Node` carries a span; every `Core` node carries provenance back to a `Node` (and, for
  macro-generated code, the *expansion chain*: "in `derive(Json)` expanded at orders.beck:12").
- Diagnostics are structured values (code, primary span, secondary spans, notes, fix-its), rendered by
  one renderer shared by CLI and LSP. Model on rustc/Elm.
- Placement errors get a dedicated explainer that prints the constraint derivation (§3.4).
- A `tests/ui/` snapshot suite (rustc-style): every diagnostic has a committed expected rendering, so
  regressions in error quality fail CI. Use `insta` for snapshots. Start this in week two, not year two.

## 4.6 Incrementality and the IDE

Use **Salsa** (the incremental query framework behind rust-analyzer) as the compiler's spine from the
first commit, not as a later retrofit. Everything is a memoised query:

```
parse(file)                → Node
expand(module)             → Node
signature(item)            → Signature          ◀── the separate-compilation firewall (§3.6)
typecheck_body(item)       → TypedBody
core(item)                 → Core
placement(component)       → Placement
artifact(tier, service)    → Bytes
```

Because §3.6 makes signatures the module firewall, editing a function body invalidates
`typecheck_body` and `core` for that item and nothing upstream — the property that makes both
sub-second IDE feedback and fast CI builds possible. **One binary** serves `beck build`, `beck check`,
`beck lsp` and `beck explain`; there is no separate language server implementation to drift.

Targets to hold yourself to: keystroke→diagnostics **< 100 ms** on a 50 kLOC project; incremental
`beck build` for a one-line change **< 2 s** to a running dev process (hot reload); clean release
build of 50 kLOC **< 60 s** including WASM and image assembly.

*Status ([`65`](65-lsp-report.md)). `beck lsp` is built — diagnostics, hover, go-to-definition and
document symbols — and the "no separate implementation to drift" claim is now a harness: the
server's answers are compared to `compile_or_library_str`'s own diagnostics and to
`iface::render_item`, rather than to strings written in a test. **The Salsa query graph above is
not built**; the server re-checks the whole buffer, which [`64`](64-compile-speed-report.md) §64.6
is the argument for. Measured end to end through the protocol, the 100 ms target holds to about
**13,000 lines in one module** — 0.84 ms at 59 lines, 7.37 ms at 914, 88 ms at 12,899 — so a 50 kLOC
project of ordinary modules is inside the budget and a 50 kLOC module is not. That gap is exactly
what per-item invalidation is for, and §65.4 says so with the numbers.*

## 4.7 `beck explain` — shipped in v0.1

Non-negotiable, per §1.6's Meteor lesson. Every inferred decision must be interrogable:

```console
$ beck explain place recent
recent  →  data tier (incremental view over `orders` fold)

  effects    : {}  (pure; reads signal `orders`)
  candidates : data (cost 1.0), server (cost 3.1), client (cost 44.0 — full state crossing)
  chosen     : data
  because    : incrementalizable (filter+order+take over a keyed fold);
               subscribed by OrderPanel, so maintained, not recomputed
  emitted as : dataflow plan recent_v1; one-shot form: SELECT ... FROM rm_orders
               WHERE customer = $1 ORDER BY at DESC LIMIT $2
  subscribers: OrderPanel (Mode A) → DOM-patch stream, shared prefix with 2 other views
```

```console
$ beck explain flow ApiKey
ApiKey (secret[str]) declared at config.beck:8
  reaches: charge()          server   ok
           audit_log()       server   ok
  BLOCKED: OrderPanel        client   secret[T] is not Sendable
           └─ would cross boundary at orders.beck:41
```

Also ship `beck explain wire <op>`, `beck explain query <fn>`, `beck explain deploy <service>`
(the full object graph and its provenance) and `beck explain cost <fn>`.

*Built*: `place`, `flow`, `wire`, `deploy`, and — with the general slicer, which gave it a plan to
read — `beck explain incremental <view>`, which [`03`](03-type-and-effect-system.md) §3.8 asks for
([`23`](23-general-slicer-report.md) §23.8). `query` and `cost` are not, and
[`23`](23-general-slicer-report.md) §23.9 says why each is still waiting.

## 4.8 Testing strategy for the compiler itself

*(Summary table — the full strategy, including deterministic simulation, Jepsen-style consistency
testing, TLA+ specs, mutation testing and the meta-testing policies, is
[`13-testing.md`](13-testing.md).)*

| Layer | Technique | Tool |
|---|---|---|
| Lexer/parser | Round-trip property: `parse(print(parse(src))) == parse(src)`; corpus tests shared with tree-sitter | `proptest`, `insta` |
| Macro expansion | Golden expansion dumps; hygiene test suite (capture must fail) | `insta` |
| Typechecker | Positive/negative suites; **principality** property tests; random well-typed program generation | `proptest` |
| Placement | Property: *no valid program is rejected*; *no `secret` reaches client* (assert over generated programs); determinism and stability properties | `proptest` |
| Splitting | Differential execution: run the whole program single-process vs. split across tiers, assert identical observable behaviour. **This is the highest-value test in the project** | custom harness |
| Determinism/replay | Fold the same recorded log twice (and across dev/release backends); assert bit-identical states and patch streams. For `retain=forever` stores: **genesis replay** — an archived corpus through the full upcast chain, asserting state equality ([`10`](10-decisions.md) D3) | harness |
| Codegen | Execution tests per backend; WASM vs native differential | harness |
| Data tier | Incremental-vs-oracle: every incrementalized view checked against full recompute over the same log; SQL read-model forms checked against both | harness + Postgres |
| Infra | Golden manifests (`insta`), then apply to ephemeral `k3d` clusters in CI and assert reachability/policy | k3s in CI |
| End-to-end | The running example, deployed to a kind/k3d cluster, driven by a browser | Playwright (pre-installed here) |
| Fuzzing | Parser and macro expander, continuously | `cargo-fuzz` / AFL++ |

The differential-execution harness (single-process vs split) deserves emphasis: it is the mechanised
form of the language's central promise. If it is green on a large corpus, the idea works.
