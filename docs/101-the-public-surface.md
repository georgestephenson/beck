# 101 — The public surface

The boundary between a Beck backend and a consumer that is not Beck. This document says what the
`@public` family is, what a consumer gets to configure, why Beck itself never transports over a
public contract, and how a consumer who does not trust the claims can check them. Nothing in it is
built except where a section says so; [`12`](12-standards-and-conformance.md) §12.1's rule — a
claim is a test or it is marketing — governs every row here.

## 101.1 The premise: opt-in, derived, and never the transport

A Beck program's inbound surface today is one page and one websocket command channel: the entire
write surface is `send(cmd)` against a `Command` union whose schema is compiled into both ends
([`03`](03-type-and-effect-system.md) §3.5 — mass assignment and over-posting have no
representation). That seam is Beck-to-Beck, and it is the right one for Beck's own client.

A third party does not want a Beck client. It wants the consensus the industry already has:
OpenAPI, gRPC, CloudEvents, OTLP — and, for agents, MCP. The design position, stated once in
[`04`](04-compiler-architecture.md) §4.4 and expanded here, is:

- **A public contract is a rendering of the internal contract, not a second contract.** The types,
  the command union, and the views already exist; an emitter renders them into somebody else's
  standard artefact. This is the pattern the rest of the project already runs on
  ([`92`](92-supply-chain-and-release-report.md) §92.2): emit a standard artefact, gate it with a
  reader that is not the writer. The SBOM is CycloneDX read by `supply_chain.rs`; the read models
  speak the Postgres wire protocol read by `tokio-postgres`
  ([`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md)); telemetry renders as OTLP/HTTP JSON. The public
  API is the conspicuous empty row in that table, and this document is the plan for filling it.
- **Every form is opt-in.** A program with no `@public` annotation has no public surface, emits no
  spec, runs no extra listener. The default Beck application is exactly what it is today.
- **The emitter is derived, so it cannot drift** — the same argument as `beck doc reference`
  ([`adr/0009`](adr/0009-generated-reference-documentation.md)): the artefact is generated
  from the compiler's own tables, checked in, and a `--check` mode regenerates and fails on
  difference.

## 101.2 The family

`@public(rest)` implies the annotation is a family, and it is. One member per consumer kind, all
rendering the same typed contract:

| Form | Contract artefact | Consumer | Status |
|---|---|---|---|
| `@public(rest)` | OpenAPI 3.1 + JSON Schema 2020-12; errors as RFC 9457 Problem Details | any HTTP client | designed here; chartered in §4.4 and [`12`](12-standards-and-conformance.md) §12.3 |
| `@public(mcp)` | an MCP server: tools, resources, and their schemas | an AI agent | designed here (§101.5) |
| `@public(grpc)` | `.proto` + a gRPC listener | polyglot RPC consumers | chartered in §4.4; staged after `rest` |
| `@public(events)` | AsyncAPI 3 channel description; events as CloudEvents 1.0 | webhook/bus subscribers | designed here; the inbound half is [`30`](30-bounded-contexts-and-microservices.md) §30.4's `ingest(source)` |
| `@public(sql)` | the PostgreSQL wire protocol over read models | any Postgres driver or BI tool | **built** — `beck run --pgwire`, [`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md), gated by `tokio-postgres`; named into the family retroactively |
| *(rung B)* | an exchanged `.becki` | another Beck project | the Beck-to-Beck public contract, [`30`](30-bounded-contexts-and-microservices.md) §30.4; `--wire-compat` gates both sides |

Two deliberate absences, in [`35`](35-standards-landscape.md)'s vocabulary:

- **GraphQL: decline, for now.** A GraphQL surface accepts query shapes the server did not choose,
  so its cost is an emergent property of the consumer's imagination; a public surface whose worst
  case is unbounded is a denial-of-service surface with a schema. Beck's premise makes cost part of
  correctness ([`01`](01-vision-and-premise.md)), so the form is declined until it can carry a cost
  model, not adopted and rate-limited after the fact. The verdict is recorded so reversing it is a
  decision rather than a drift.
- **A bespoke Beck public protocol: decline, permanently.** §4.4 already says the internal format
  is never a public contract. The public surface exists to meet consumers where they are; inventing
  a format would recreate the problem the family solves.

## 101.3 What the consumer configures, and what they cannot

A public surface serves an external consumer's needs, not Beck's opinions about API design. The
opinionated defaults exist so that the zero-config emission is respectable, and everything about
the *shape* is configurable — per surface, in the annotation's block or `beck.toml`'s
`[public.<form>]` table:

- **Naming and routes**: which commands and views are exposed, under what paths and operation ids;
  casing conventions (a consumer's codegen may demand `camelCase` where Beck writes `snake_case`).
- **Versioning**: path-versioned (`/v1/`) or header-versioned; the deprecation window; what
  `beck check --wire-compat` holds the surface to (the asymmetry from
  [`04`](04-compiler-architecture.md) §4.3 applies unchanged — a new accepted command is
  compatible, a new emitted event is breaking for a consumer's exhaustive handler).
- **Errors**: RFC 9457 by default; the `type` URI namespace and any extension members are the
  consumer's to choose.
- **Auth**: bearer against the identity subsystem ([`10`](10-decisions.md) D6) by default; API
  keys or mTLS where a consumer's platform demands them.
- **Pagination, filtering, content types** for view endpoints.

What is **not** configurable is the properties, because they are why the surface is derived rather
than hand-written: the schema cannot drift from the types; a `secret[.]` cannot appear in a
response type (the same discipline that keeps it out of the log); validation is the same
`validate` the command channel runs — a public request is a command, not a second code path into
the fold; and the surface appears in the derived infrastructure like everything else — a
`@public` listener is an ingress the object graph knows about, not a port somebody opened.

The line to keep: **the consumer chooses the shape; the compiler keeps the properties.**

## 101.4 Beck never uses the public contract internally

The question was asked directly: once the public contract exists, does Beck's own frontend-backend
seam move onto it? **No, and the reasons are load-bearing:**

1. **The internal seam carries what a public contract cannot.** Placement, effect rows, patch
   streams tagged with the `seq` they bring a subscriber up to, resumption, optimism
   reconciliation. Flattening `send(cmd)` + patch subscription into request/response REST would
   forfeit the incremental patch stream and the §3.5 property in the same move.
2. **Public contracts are versioned for strangers.** A public surface owes its consumers a
   deprecation policy and a stability window. The internal seam is rebuilt from source on both
   sides at every deploy and owes nobody anything between versions. Routing Beck's own traffic
   through the public contract would couple internal evolution to the external deprecation
   clock — inverting the reason both are derived from one set of types.
3. **§4.4 states the converse already**: the internal format is never a public contract. This
   section completes the sentence — the public contract is never the internal transport.

What keeps the two honest is not shared transport but **shared derivation and a drift gate**: both
render the same types, the emitted spec is checked in, `--check` regenerates and diffs it in CI,
and the surface's harness drives the running server with a *foreign* client (§101.7) — so the
public surface is exercised on every CI run even before it has an external consumer. Where
dogfooding is honest, it happens at the tooling layer, which is where it already happens: the
dashboard reads the OTLP endpoints; an agent working on a Beck program is `@public(mcp)`'s first
consumer, the way an editor was `beck lsp`'s.

## 101.5 MCP: the agent as a consumer

Nothing in this repository mentioned MCP before this document; it is a white space, not a refused
idea. Beck is unusually well-shaped for it, because the mapping is nearly one-to-one rather than a
retrofit:

- **A tool is a public command.** An MCP tool call is a named operation with a JSON Schema for its
  arguments — which is what `send(cmd)` already is. A REST API has to be bent into tool shape;
  Beck's write surface starts there.
- **A resource is a view.** Read models already render over a foreign protocol at `@public(sql)`;
  MCP resources are the same move for agents.
- **The effect system answers the question an agent's operator actually asks** — *what can this
  tool touch?* A tool's effect row and placement are static facts, so the emitted tool listing can
  say, from the compiler's own analysis rather than from prose, whether a tool writes, what hosts
  it reaches (`net.out` is call-site literal, [`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md)),
  and that a destructive operation is marked so (MCP's annotation hints, populated from analysis
  instead of hope). No other emitter in the family gets a differentiator this large from Beck's
  semantics.

What MCP needs that the type system cannot derive, stated rather than glossed:

- **Descriptions are prose for a model, and they have consequences.** A tool description is the
  interface an agent reasons over; a bad one produces wrong calls. They come from doc comments —
  which the language already treats as surviving, first-class artefacts
  ([`34`](34-generated-documentation-report.md)) — and a `@public(mcp)` export without one is a
  diagnostic, not a silent empty string.
- **Sessions, streaming, and elicitation** are MCP's own protocol machinery, implemented once in
  the runtime, not derived per program.
- **Auth is the identity subsystem's problem** (D6): MCP's OAuth story maps onto the existing
  relying-party work, and an MCP consumer is an *actor* the runtime decides
  ([`48`](48-identity-report.md) §48.2), subject to the same quotas and audit as any other.
- **Evolution maps cleanly**: the wire-compat gate's verdict on a surface change is exactly what
  decides whether the server sends `tool-list-changed` or the change was breaking.

## 101.6 `@public(events)`: the enterprise bus, in both directions

Beck is event-sourced at its semantic core; enterprise event-driven architecture is the same
commitment at organisational scale, so the fit is native — but the honest version of that sentence
says which of EDA's standing problems *dissolve* on Beck's semantics and which are merely imported.

**The outbox problem does not exist here.** The best-known defect in enterprise event publishing
is the dual write — commit to the database, publish to the broker, and the crash between them —
patched industry-wide with the transactional-outbox pattern and a relay process. Beck has no dual
write to patch: the log is primary, so a public event stream is a *projection of the log*, the
outbox pattern with nothing bolted on. Publish order is the log's order, and redelivery is
re-reading, not re-remembering.

**Delivery is at-least-once, and the contract says so.** Exactly-once delivery across a network
boundary is a claim no broker can cash; what enterprises actually run is at-least-once plus
consumer-side deduplication. The emitted envelope makes that cheap instead of bespoke: events are
CloudEvents 1.0 with the identity derived from `(context, seq)`, so the dedupe key and the resume
cursor are *given* by the contract rather than invented per consumer — a consumer that tracks its
high-water `seq` gets resumption the same way Beck's own thin client does.

**Ordering is per-context, stated rather than discovered.** One log per context
([`10`](10-decisions.md) D20) means the published stream promises the log's total order within a
context and promises nothing across contexts — the same shape as a broker's per-partition
guarantee, except declared in the AsyncAPI document instead of learned in an incident review.

**Schema evolution is the wire-compat gate's hard half, at the producer.** §4.3's asymmetry — a
new emitted event variant is breaking for a consumer's exhaustive handler — is exactly the
compatibility check an enterprise schema registry performs, done here by the compiler before the
producer can deploy. The checked-in AsyncAPI document plus its JSON Schemas *is* the registry
artefact; interoperating with a Confluent-class registry means publishing schemas into it, not
running one. Within a version channel, evolution is additive; a breaking change is a new channel
plus an `upcast`-shaped translation — the same machinery deploys already demand.

**The contract is the envelope, not the broker.** The first transport is webhooks — HTTP push,
HMAC-signed with `digest_keyed`, the one declassifier already built for signing
([`adr/0014`](adr/0014-a-keyed-digest-is-the-one-declassifier.md)) — with broker bridges (Kafka;
NATS, which Phase 4's internal fabric brings anyway) behind the same envelope. One tension is
stated rather than glossed: [`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md)
makes egress hosts call-site literals so the NetworkPolicy is derivable, and a
subscriber-registered URL has no literal. The resolution is that a surface's subscriber endpoints
are constrained to a **declared host set** in its configuration — enterprises name their partners —
and an unconstrained subscriber set requires an explicit wildcard egress declaration that
`beck explain` reports loudly: the forfeit is available, and it is never quiet.

**Delivery state is a fold.** Retries, backoff, dead-lettering after N attempts: the delivery
ledger is ordinary Beck state — inspectable, replayable, and testable in `test` blocks — rather
than a broker's opaque DLQ. **The subscribe half already has its design**:
[`30`](30-bounded-contexts-and-microservices.md) §30.4's `ingest(source)` takes CloudEvents in,
with `translate` as the demanded anti-corruption layer. `@public(events)` is the publish half;
together they put a Beck context on an enterprise bus in both directions, speaking the same
envelope each way.

## 101.7 What the edge imports: the obligations a public surface brings

Internally, Beck *deleted* a whole problem class by construction: one compiled client, one
channel, both ends rebuilt from the same source at every deploy. A public surface un-deletes it —
strangers' clients, hostile input, unbounded consumers, a deprecation clock owed to people who
cannot be recompiled. None of that may become a program author's problem, and none of it may reach
the fold. The organising rule:

**Every imported obligation lives at the runtime edge, configured per surface, and the fold never
sees it.** A rejected request — over quota, unauthenticated, oversized, malformed — is a
*non-event* in D17's exact sense: it belongs to telemetry, not to the log, and the edge absorbs it
the way [`82`](82-the-edge-report.md) already absorbs the uninvited.

The ledger, each obligation mapped to derived, built, or prerequisite:

| Obligation | Where it lands |
|---|---|
| **Authentication** | D6's machinery; a public consumer is an *actor the runtime decides* ([`48`](48-identity-report.md) §48.2). New work: machine-to-machine credentials — OAuth client-credentials, API keys, mTLS — as configurable shapes per §101.3 |
| **Authorisation** | scopes map onto the exposure list itself — which commands and views a surface exports *is* the scope vocabulary, so it is derived, not maintained |
| **Per-actor write quotas** | **built** — [`82`](82-the-edge-report.md) §82.4's quota at the merge point; a public consumer rides it as an actor. §82.5's finding governs the anonymous tier: the bound is worth what the actor is worth, so unauthenticated read surfaces get the table-bounded treatment |
| **Rate limits; connection and subscription quotas** | **prerequisite** — F15 is a pending absence ([`43`](43-threat-model.md) §43.4, asserted in `pending_security.rs`). Shipping any `@public` form converts it from pending to blocking; the vocabulary is 429 + `Retry-After` |
| **Inbound TLS posture** | **prerequisite decision** — today `beck run` serves plaintext and the gateway terminates ([`43`](43-threat-model.md) §43.4). The surface must state which posture it ships under; building direct TLS means correcting §43.4 and `pending_security.rs` in the same change, which is what the red test is for |
| **Validation of hostile input** | the same `validate` — a public request is a command, not a second path — plus the bounds a hostile client makes load-bearing: body size, nesting depth, content-type strictness; the grammar-fuzz suite's method extended to every public decoder |
| **Response codes** | the full RFC 9110 vocabulary, mapped from facts the runtime already knows: `validate` failure → 422 with an RFC 9457 body; auth → 401/403; quota → 429; optimistic conflict → 409; **deploy drain → 503 + `Retry-After`** — the quiesce window of deploys-ride-the-stream, given its HTTP meaning |
| **Idempotency** | `Idempotency-Key` on commands, with the dedupe ledger as a fold — natural here, because commands are logged |
| **Caching** | ETags derived from `seq` — strong validators for free, `If-None-Match` answered by comparison, because a state *has* a name |
| **Pagination** | cursors over ordered read models — [`54`](54-ordering.md)'s value order is what makes a cursor stable across requests |
| **CORS and preflight** | per-surface configuration, for the browser-based consumer |
| **Deprecation lifecycle** | `Sunset` (RFC 8594) and `Deprecation` headers driven by §101.3's versioning configuration, so the clock is machine-readable |
| **Surface observability** | per-operation counters and histograms in the existing telemetry; W3C `traceparent` accepted at the edge and joined to `beck.seq`, the two correlation schemes meeting at the boundary where D17 puts all correlation |

Read down the right column and the shape is consistent: most rows are **derived** from machinery
that exists because of what Beck is, two rows are **already built**, and exactly two are genuine
prerequisites — F15's quotas and the inbound TLS posture — which is why the roadmap orders them
*before* the first `@public` form ships rather than after its first incident. This section is
§101.3's line cashed at the edge: the consumer chooses the shape; the compiler keeps the
properties.

## 101.8 The trust surface: verifying Beck's claims, and maximalist telemetry

A user who demands evidence for Beck's claims about itself already has three rungs, and it is
worth stating them as one ladder because they were built separately:

1. **Claims are tests.** The measurement suites are runnable by the doubter
   (`cargo test --release --test <suite> -- --nocapture`); every number in a report names its
   command; what is *absent* is asserted absent in `pending_security.rs`
   ([`43`](43-threat-model.md) §43.4), so a missing control cannot be quietly built or quietly
   claimed.
2. **The artefacts carry provenance.** The released binary is verifiable back to the build
   ([`92`](92-supply-chain-and-release-report.md)); the generated reference cannot drift from the
   compiler's tables; `beck explain` says which boundaries are proved and which are rented.
3. **The running system serves evidence.** OTLP/HTTP JSON at `/_beck/otlp/*` — built, narrow by
   design ([`10`](10-decisions.md) D17): the log is the trace; telemetry carries only what cannot
   replay.

The demand this document adds is the fourth rung: an operator who wants *maximalist* telemetry —
full traces, span-per-operation, the depth a service fleet drowns in — without Beck paying for it
in production. The Beck-shaped answer is that **maximal telemetry is a derived artefact of the
log, not a production cost**:

- Because `state = fold(f, init, log[..seq])` is total and replay-pure, a replay of any range under
  an instrumenting backend can emit full-resolution OTLP spans — inside the fold, per event, per
  view recomputation, to whatever depth is demanded — *after the fact*, from a process that is not
  serving traffic. Working name: `beck trace --from <seq> --to <seq> --otlp <endpoint>`. The
  serving fold stays uninstrumented and replay-pure; D17's boundary is not weakened, it is what
  makes the feature possible. No refactor touches the hot path, because the hot path is not where
  the telemetry comes from.
- **The honest caveat, stated up front**: replay-derived spans carry the replay machine's
  durations, not production's. What the derived trace verifies *exactly* is causality, ordering,
  counts, and state; what it verifies *representatively* is time. Production wall-clock stays where
  D17 put it — recorded at the boundaries, correlated by `beck.seq`, which is precisely the join
  key between the two: a boundary record names a `seq`, and `beck trace` expands that `seq` to any
  depth on demand.
- The two runtime gaps worth closing are small and additive: a **push exporter** (an optional
  `BECK_OTLP_ENDPOINT` that ships the same OTLP JSON to a collector on an interval — the doc
  comment that claimed this exists has been corrected to say it does not), and **OpenMetrics
  exposition** for Prometheus scrapers ([`12`](12-standards-and-conformance.md) §12.8's unticked
  row). Both export what is already recorded; neither adds a measurement to the fold's path.
- Cross-tier boundary spans on by default remain Phase 4's item ([`08`](08-roadmap.md)),
  unchanged.

## 101.9 The gates

Per §92.2, each emitter is gated by a reader that is not the writer, and the gate drives the
*running surface*, not only the artefact:

| Surface | Artefact gate | Behaviour gate |
|---|---|---|
| `@public(rest)` | a third-party OpenAPI 3.1 validator accepts the emitted spec | a client *generated from the spec by someone else's generator* round-trips commands and reads against `beck run` |
| `@public(mcp)` | — | the official MCP SDK, as the client, lists tools, calls one, reads a resource, and observes `tool-list-changed` across a surface change |
| `@public(grpc)` | `protoc` accepts the emitted `.proto` | a foreign gRPC client round-trips (dev-dependency only; D17's refusal of `tonic`/`prost` in the *runtime* tree stands) |
| `@public(events)` | an AsyncAPI validator accepts the channel description | a consumer asserts CloudEvents 1.0 envelopes on the wire |
| `@public(sql)` | — | **built**: `tokio-postgres` drives the read models |

Plus, for every form: the checked-in artefact has a `--check` drift gate; wire-compat holds the
surface across changes; and a `secret[.]`-bearing type in a public signature is a diagnostic with
a test asserting so.

## 101.10 Where this lands in the plan

The dependency is not `rest` → `mcp`; both hang off the one thing that must come first — the
**`@public` boundary itself**: which declarations are exposed, the versioning and auth semantics of
§101.3, and the diagnostics for what may not cross (secrets, non-serialisable types, effectful
views). Beside it, and before any form ships, §101.7's two genuine prerequisites: F15's quotas and
the inbound TLS posture — the edge obligations land before the first public consumer exists, not
after its first incident. Then, in order of consumer demand: `rest` (with RFC 9457, closing
[`35`](35-standards-landscape.md) §35.5's blocked item), `mcp`, then `events` and `grpc`.
[`08`](08-roadmap.md) carries the phase placement; [`30`](30-bounded-contexts-and-microservices.md)
§30.4 continues to own the *inbound* foreign-system story (`ingest`, `translate`), which this
document leans on and does not restate.

## 101.11 What this document does not claim

No `@public` annotation exists in the compiler; no OpenAPI, MCP, gRPC or AsyncAPI artefact has
ever been emitted; `beck trace` and the push exporter are designs. What is built and gated today:
the pgwire read models, the OTLP pull endpoints, the `.becki` interface and its wire-compat gate.
Each row of §101.2, §101.7 and §101.8 turns true the way everything here does — a test using
somebody else's reader goes green, and [`12`](12-standards-and-conformance.md) §12.3 gets its row
ticked in the same change; the two §101.7 rows that are pending-security absences additionally turn
their `pending_security.rs` test red, and correcting [`43`](43-threat-model.md) §43.4 in the same
change is what the red test is for.
