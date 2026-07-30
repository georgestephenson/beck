# 03 — Type and effect system

This is where the idea either becomes real or becomes Meteor. Two things must be *checked properties
of the program*, not conventions: **where code runs** (placement) and **where time enters**
(determinism). The first makes tier-splitting sound; the second makes the database-as-fold sound.

## 3.1 Base type system

The program shape these types serve (details in §3.7): values flow as `Stream[T]` (discrete
occurrences) and `Signal[T]` (a value over time); state is a `durable` fold of a stream; the UI is a
signal of `Html`. Around that core, a conventional modern checker:

- **Hindley–Milner inference with bidirectional checking.** Full inference inside bodies; mandatory
  annotations on public signatures. The standard modern compromise (Rust, Swift, OCaml + mli), and
  what makes separate compilation possible.
- **Algebraic data types** + records + **row polymorphism** on records. Rows matter doubly here:
  client components need *projections* of server state without hand-written DTOs, and the byte-size
  of a crossing row feeds the placement cost model (§3.4).
- **Traits/typeclasses** with coherence (orphan rule): `Json`, `Sendable`, `Storable`, `Eq`, `Hash`,
  `Display` — mostly `derive`d by macros ([`02`](02-syntax.md) §2.4).
- **Zero-cost nominal newtypes**: `type CustomerId = newtype[u64]`. Ids of different entities must
  not be interchangeable — the premise is that boundaries stop lying.
- **`Option`/`Result`, no null**; `match` with exhaustiveness (a fold over a `union Event` that
  misses a case is a compile error — this single check carries the migration story, §3.9).
- **No subtyping** beyond effect-row subsumption. Subtyping + inference + effects is where type
  systems go to die.
- **Immutable by default**; `var` for local mutation only. Required for code to be placeable and
  folds replayable.
- **Memory**: GC'd surface. Per-tier strategy in [`05`](05-tier-lowering.md); region/escape analysis
  as optimisation, never in the surface.

## 3.2 Effects as rows

Every function type carries an inferred, row-polymorphic effect row:

```
apply_event : (Map[Id,Todo], Event) -> Map[Id,Todo]   ! {}            # pure — placeable anywhere
validate    : (Session, Command) -> Option[Event]     ! { cap.session }
view        : (Map[Id,Todo], int) -> Html             ! {}
merge_clients : () -> Stream[(Session, Command)]      ! { ingress }   # nondeterminism lives here
durable     : Signal[S] -> Signal[S]                  ! { durable(S) }
charge      : (Card, Money) -> Result[Receipt, Declined] ! { net.out(payments.example.com) }
```

Effect atoms: `ingress` (a merge point — arbitrary interleaving), `durable(S)` (persistent
accumulator), `dom`, `net.out(host)`, `net.in`, `fs(path)`, `env`, `spawn`, `cap.X` (capabilities),
`partial` (may diverge/panic), `external.read/write(store)` (escape-hatch stores, §3.8). An
`ambient` set (`log`, `time`, `rand`, `metrics`) is implicitly available *outside folds* and elided
from signatures — except where determinism forbids it (§3.7); a strict mode surfaces it.

Effect polymorphism is what keeps one standard library:
`map : (list[a], (a -> b ! e)) -> list[b] ! e`. This machinery is validated by Koka, Unison
abilities, and OCaml 5 — we are assembling, not inventing.

## 3.3 Placement is derived from effects

**Placement is not a primitive annotation; it is a constraint solution** — with explicit `@on(...)`
always available and always winning. Purity means *unplaced*: legal on every tier, compiled to each
tier that needs it. That duplication is the payoff, not waste — it is why the client can run the
same `apply_event`/`view` the server runs (optimism, §3.7), and why validation logic cannot drift
between tiers.

Each tier is defined by the effects it can discharge:

| Tier | Discharges | Cannot |
|---|---|---|
| `client` | `dom`, `net.out(own-origin)`, local storage, ambient | `ingress`, `durable`, `cap.*` needing server secrets, `fs`, `env` |
| `server` | `ingress`, `durable`, `net.*`, `fs`, `env`, `spawn`, `cap.*` | `dom` |
| `data` (the fold/view engine) | pure computation over streams/signals; `durable` | `dom`, `net`, ambient time/rand (§3.7) |
| `edge` (optional) | `net`, cache, ambient | `durable` direct, `dom` |
| `build` (macro phase) | pure + module-graph reads | everything else |

The solver then chooses, per function, among the tiers that can discharge its row, minimising
boundary-crossing cost, subject to annotations and the security rules of §3.5. In the canonical
example nothing needs annotating beyond the original's `@on(server)` markers — and even those are
recoverable: `merge_clients` carries `ingress`, the fold carries `durable`, both server-only, and
everything pure is unplaced. v0.1 ships **explicit placement, verified against effects** (reject
`@on(client)` on anything whose row the client cannot discharge); inference of the unannotated
middle ground follows (§3.10).

## 3.4 The placement/cost solver

Min-cut / small-MILP over the (monomorphised) call-and-signal graph, the classic program-
partitioning shape:

- **Node costs**: ∞ for forbidden tiers; tier-specific compute cost otherwise (client CPU is
  expensive and untrusted; fold-engine compute is cheap and adjacent to state).
- **Edge costs**: for each *signal edge or call* that crosses tiers, `latency + bytes × unit`,
  bytes estimated from row types. Signal edges are the interesting ones: placing `view`
  server-side means the crossing carries **DOM patches**; placing it client-side means it carries
  **data patches**. Rendering placement is just another placement decision — this resolves the
  original sketch's deliberate ambiguity about where `view` runs
  ([`05`](05-tier-lowering.md) §5.1).
- Guardrails, non-negotiable: **determinism** (same input, same solution), **stability** (a
  one-line edit must not re-place unrelated code; previous solution persisted in `tier.lock`,
  churn reported in CI), **explainability** (`tier explain place`, §4.7), **assertability**
  (`assert place(view, OrderPanel) == client` in tests; latency budgets on `service` blocks the
  solver must respect).
- Ambiguity that survives defaults is a **compile error with a suggested annotation** — never a
  silent guess. The Meteor lesson, made mechanical.

## 3.5 Security properties for free (the selling point)

Placement-as-type makes vulnerability classes *unrepresentable*:

```python
type ApiKey = secret[str]          # secret[T] is not Sendable
```

| Property | Mechanism |
|---|---|
| Secrets cannot reach the browser | Boundary crossings require `Sendable`; `secret[T]` isn't. The leak is a compile error naming the flow (`tier explain flow ApiKey`) |
| Clients can only *propose* | The client's entire write surface is `send(cmd)` into a typed `Command` union. There is no other mutation path — mass assignment and over-posting have no representation |
| Authority is one chokepoint | Only `validate` (the `ingress` consumer, holding `Session` capabilities) turns commands into events; forgetting an auth check means the `cap.*` effect goes undischarged — a compile error, not a pentest finding |
| The log and rules never ship to clients | `ingress`/`durable` are undischargeable on `client`; DCE strips server-only code from client artefacts, verified |
| No injection / no XSS | `sql"..."`/`html"..."` typed literals: interpolation is bind-parameters / escaped by type (Ur/Web's guarantee) |
| Least-privilege infra, computed | Effect rows → NetworkPolicy, RBAC, store grants ([`06`](06-kubernetes-and-packaging.md) §6.5) |
| No arbitrary build-time code | Macro phase is capability-restricted ([`02`](02-syntax.md) §2.4) |
| Tamper-evident history | State is a fold over an append-only log: "how did this row get here" is `tier replay`, not forensics |

"Your compiler proves the API key can't reach the browser, and your audit log is your database" is
the claim nothing mainstream can make. Lead with it.

## 3.6 Modularity and separate compilation (do not defer)

The historical killer of tierless languages ([`01`](01-vision-and-premise.md) §1.6). The rule:

> **Placement, effects, and event/command types are part of a module's published signature.**
> Inference is intra-module; boundaries are declared.

```python
# orders.tieri — generated, checked in, reviewed like a .mli
command PlaceOrder(customer: Ref[Customer], total: Money) requires auth(customer)
event   OrderPlaced { id: OrderId, customer: Ref[Customer], total: Money, at: Instant }  # v1
orders : Signal[Map[OrderId, Order]]  ! { durable }   @on(server)
recent : (Ref[Customer], int) -> list[Order]  ! {}    @on(any)
```

Consequences: modules compile against signatures (true separate compilation, parallel builds); body
edits don't invalidate downstream modules; **effect widening is a breaking API change** flagged by
`tier check --api` (a library that starts phoning home cannot do so silently — a novel supply-chain
property); and **event types in signatures are the wire/log compatibility surface** — changing one
without a migration is unshippable (§3.9).

## 3.7 Time, streams, and folds (the semantic core)

The constructs from the original sketch, given precise typing rules.

**`Stream[T]`** — discrete occurrences. **`Signal[T]`** — a value defined at all times (Elliott's
events vs behaviors). Core operations: `map`, `filter_map`, `merge` (of *already-ordered* streams),
`fold(f, init, s) : Signal[S]`, `map2`, `sample`, `window`. Backpressure is in the runtime contract
of every stream edge, not user-visible in v1.

**Ingress — where time enters.** `merge_clients()` (and `ingest(source)` for webhooks/timers/
external feeds) are the only constructors of nondeterministically-ordered streams, and they carry
the `ingress` effect. At ingress the runtime stamps each occurrence into an envelope:

```python
model Envelope[T]:
    seq: Seq            # position in the total order — assigned here, nowhere else
    at: Instant         # wall-clock, captured as data
    actor: Session      # authenticated origin capability
    body: T
```

v1 semantics: **one totally-ordered log per application**. That is a real throughput/geography
ceiling, chosen consciously — the honest options at scale (per-entity sharding, logical timestamps,
CRDTs) are [`09`](09-risks-and-open-questions.md) §9.6's first question.

**Folds and determinism.** `fold`'s function must be *replay-pure*: effect row `⊆ {}`. The checker
therefore rejects `now()`, `rand()`, `uuid()` and any I/O **inside a fold** — time is data on the
envelope (`e.at`), identity is minted at the edge (client-side `uuid7()` in the sketch — legal,
it's just data by the time it reaches the fold). This one rule is what makes the following true and
testable: *replaying the log reproduces the state, bit for bit*.

**`durable`.** `durable(fold(f, init, s))` marks the accumulator as surviving restarts: the runtime
persists the input stream (the log) and snapshots the accumulator. `durable` is "the entire database
administration story" — and it is an *effect*, so infrastructure derivation
([`06`](06-kubernetes-and-packaging.md) §6.5) and RBAC can see it. Retention/snapshot policy hangs
off it: `durable(retain=90.days, snapshot=hourly)` — defaults sane, overridable.

**Commands vs events (CQRS as types).** Clients propose `Command`s; the server's `validate` — the
sole consumer of ingress, holding the `Session` capability — decides what becomes an `Event`.
Events are facts: past-tense, immutable, the only input to folds.

**Optimism, derived.** For a command `c`, the client may speculatively apply
`validate_optimistic(c)`'s expected event to its local view of state **iff** the fold and view
involved are unplaced-pure and the state slice is client-visible. Both tiers run *the same
function* and merely disagree briefly about the fold's order; reconciliation is by `seq` — when the
authoritative patch arrives, speculative state is confirmed or discarded. This is also why clients
mint entity ids (`uuid7()` in the sketch): they must refer to a todo before the server confirms it
exists. Browsers here are **replicas, not terminals**. `Signal[T]` carries a freshness dimension
(`confirmed | pending(n)`) that UI code can render ("saving…") — staleness is typed, not pretended
away. The concession the original makes explicitly and we inherit: for *concurrent edits to the
same value* (two users in one text field), speculative-then-reconcile is not enough — you need
CRDTs or OT, "and no type system absolves you"
([`09`](09-risks-and-open-questions.md) §9.5).

**Replay, forked worlds.** Because everything downstream of ingress is deterministic:
`tier replay --to <seq|time>` reproduces any historical state; `tier fork --from prod --at
yesterday` starts a local instance folded from a production snapshot; property tests run over
recorded logs; the time-travel debugger scrubs `seq`. These are features of the *semantics* — they
cost almost nothing to build once the determinism rule holds, and they are demos nobody else can
give.

## 3.8 Queries, views and reactivity

A "query" is a pure function of signal values — `remaining` in the sketch. The compiler's job is
keeping them incremental:

- **Subscribed views** (anything feeding a live `page`, or marked `materialized`) compile to
  **incremental dataflow plans** — the differential-dataflow lineage
  ([`05`](05-tier-lowering.md) §5.3): `remaining` updates by ±1 per event, never by recount.
- The relational comprehension surface (`from o in orders.values() where ... order by ... take n`)
  is sugar for pure functions that are *guaranteed* incrementalizable and also lower to SQL against
  materialized read models for one-shot reads. Arbitrary pure code is incrementalized where
  analysis allows, recomputed where not — `tier explain incremental <view>` shows which, and why.
- **Invalidation does not exist as a concept.** There are no caches to invalidate — only views
  downstream of the log. What ships to a subscribed client is the patch stream of its view
  ([`05`](05-tier-lowering.md) §5.1). This subsumes the entire cache-key/TTL discipline of
  conventional stacks.
- **Per-session views are the norm, not the exception.** Real auth means
  `mine: Signal[...] = todos.map(filter_by(session.user))` — a view *parameterised by the session*,
  which turns one broadcast into per-client fanout. Placement types are what make that safe to
  express (the filter provably runs server-side; the client never receives unfiltered state), and
  the fanout cost becomes a first-class engineering concern: shared-prefix plans — one dataflow
  with per-session final operators — not N independent recomputations
  ([`05`](05-tier-lowering.md) §5.3, [`09`](09-risks-and-open-questions.md) §9.1).
- Reads of `clock()` inside a view make it a function of the clock signal — a typed, visible
  time-dependence (re-evaluated on a declared tick), never a hidden `now()`.
- **External stores** (existing Postgres/APIs the team already has) enter as
  `external store legacy: ...` with `external.read/write` effects — honest escape hatches with none
  of the fold guarantees, so adoption doesn't require migration day one.

## 3.9 Evolution: migrations as typed functions

State schema evolution is a *language* concern, not an ops concern (Lamdera's proof, Erlang's
`code_change` lineage):

- Changing an accumulator type `S → S'` requires `migrate : S -> S'` or the deploy refuses to ship.
  Snapshots are migrated; the new fold resumes from migrated snapshot + tail of the log.
- Changing an **event** type requires an **upcaster** `upcast : OldEvent -> Event` (events are
  immutable facts; the log is never rewritten). Exhaustiveness checking on `union Event` is what
  makes a missed case a compile error rather than a 3 a.m. page.
- Doctrine (the fork is argued in [`09`](09-risks-and-open-questions.md) §9.5): **snapshots are
  authoritative for liveness; upcasters are required only within the declared log retention
  window.** Full-history replay from genesis is a per-store opt-in (`retain=forever`), with its
  cost stated, not the default everyone silently inherits.
- Rollout choreography — drain, snapshot, migrate, resume; two versions live simultaneously — is
  the operator's job ([`06`](06-kubernetes-and-packaging.md) §6.4). Wire compatibility of
  command/event types across versions is checked by `tier check --wire-compat` against the
  previously deployed signature ([`04`](04-compiler-architecture.md) §4.3).

## 3.10 Staged sequence of work

1. HM + ADTs + traits; `Stream`/`Signal`/`fold`/`durable` typed but **placement fully manual**
   (`@on`), matching the original sketch exactly.
2. Effect rows inferred; placement manual but **verified** (reject `@on(client)` + `durable`;
   reject impure folds). Already novel, already shippable.
3. Placement inference for the unannotated middle; `tier explain place`; freshness-typed optimism.
4. The cost model: rendering placement (DOM-patch vs data-patch, [`05`](05-tier-lowering.md) §5.1)
   chosen per component.
5. Incremental view compilation (differential lineage); materialized read models; pgwire exposure.
6. Migrations/upcasters + operator choreography; replay/fork tooling.

Stages 1–2 are a "typed tierless framework" someone could adopt. Stages 3–6 are the moat.
