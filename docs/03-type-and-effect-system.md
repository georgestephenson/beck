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
accumulator), `dom`, `net.out(host)`, `net.in`, `fs.read(path)`, `fs.write(path)`, `env`,
`spawn`, `cap.X` (capabilities), `partial` (may diverge/panic), `raises(E)` (may fail with a value
of type `E`), `external.read/write(store)` (escape-hatch stores, §3.8). An
`ambient` set — **`log` and `metrics`** — is implicitly available on every tier, elided from
signatures, and never a reason to place anything.

**Failure is one of those atoms, and `Result` is what a handler produces from it.** `raise e`
performs `raises(E)`; `try: block` is the handler, and it yields `Result[T, E]` having discharged
exactly that label. The consequence is the point: a function that can fail says so in its
*signature* whether or not its author thought about it, because the row is inferred; a `uses`
clause is an upper bound on it, as it is on every other atom; and `--wire-compat` calls a function
that starts being able to fail a breaking change, in the same sentence it already used for a
library that starts phoning home. Several labels in one row get a name —
`row Fallible = raises(Refusal), log` — because rows of five and six labels are ordinary and a
signature nobody reads is not a contract.
[`27`](27-the-walls-come-down-report.md) is what was built and what was not.

> **Correction, Phase 2** ([`20`](20-phase-2-report.md) §20.4 item 3). This paragraph originally put
> `time` and `rand` in the ambient set as well, "implicitly available *outside folds* … except where
> determinism forbids it". They cannot be there. §3.3's table makes the fold engine a **tier**, and
> whether a tier discharges time and randomness is precisely §3.7's determinism rule — so an effect
> that decides a placement cannot also be elided from the signature the placement is derived from.
> They are one atom, `nondet`, which `client` and `server` discharge and `data` refuses.
>
> The correction pays for itself: `Tier::Any` is then definable as the *intersection* of the
> concrete tiers, and "an ambient effect never forces a placement" stops being a special case.

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

| Beck | Discharges | Cannot |
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
  one-line edit must not re-place unrelated code; previous solution persisted in `beck.lock`,
  churn reported in CI), **explainability** (`beck explain place`, §4.7), **assertability**
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
| Secrets cannot reach the browser | Boundary crossings require `Sendable`; `secret[T]` isn't. The leak is a compile error naming the flow (`beck explain flow ApiKey`) |
| Clients can only *propose* | The client's entire write surface is `send(cmd)` into a typed `Command` union. There is no other mutation path — mass assignment and over-posting have no representation |
| Authority is one chokepoint | Only `validate` (the `ingress` consumer, holding `Session` capabilities) turns commands into events; forgetting an auth check means the `cap.*` effect goes undischarged — a compile error, not a pentest finding |
| The log and rules never ship to clients | `ingress`/`durable` are undischargeable on `client`; DCE strips server-only code from client artefacts, verified |
| No injection / no XSS | `sql"..."`/`html"..."` typed literals: interpolation is bind-parameters / escaped by type (Ur/Web's guarantee) |
| Least-privilege infra, computed | Effect rows → NetworkPolicy, RBAC, store grants ([`06`](06-kubernetes-and-packaging.md) §6.5) |
| No arbitrary build-time code | Macro phase is capability-restricted ([`02`](02-syntax.md) §2.4) |
| Tamper-evident history | State is a fold over an append-only log: "how did this row get here" is `beck replay`, not forensics |

"Your compiler proves the API key can't reach the browser, and your audit log is your database" is
the claim nothing mainstream can make. Lead with it.

### 3.5.1 `Sendable` and `Storable` are two axes, not one

*Added in Phase 2 ([`20`](20-phase-2-report.md) §20.4 item 12), after the question "what if you
want the table but not the data object" made the gap visible.*

The table above uses `Sendable` for crossing and `Storable` for the log without ever saying they are
independent. They are, and the four combinations all occur:

| | `Sendable` | `Storable` | |
|---|---|---|---|
| ordinary data | ✓ | ✓ | a `str`, a model of them |
| `Html`, `Attr` | ✓ | ✗ | a patch stream crosses; replay recomputes a view rather than reading one back |
| **`internal[T]`** | ✗ | ✓ | why an account was suspended: recorded forever, never rendered |
| `secret[T]`, a function | ✗ | ✗ | a token must reach neither the browser nor the log (§3.7 F5) |

`internal[T]` is the row `secret[T]` alone leaves empty, and an event-sourced system needs it: the
reason a moderator suspended an account is the fact an appeal is decided against six months later,
so it belongs in the log — and it must never be rendered into a page. Typing it `str` leaves that to
review; typing it `secret[str]` refuses to write it down at all, which is an event that does not say
why it happened.

```python
model Suspension:
    account: str
    reason: internal[str]      # storable, and no view can print it

internal_of : (a) -> internal[a]
reveal      : (internal[a]) -> a ! {cap.internal}
```

Reading one back is a capability, so §3.5's chokepoint rule does the enforcement rather than a
second mechanism: `validate` may hold `cap.internal` and decide *on* the reason, and a
`Signal[Html]` is the browser's because of its type, so a view that calls `reveal` is a placement
error. The corpus program is
[`20-moderation.beck`](../compiler/corpus/20-moderation.beck) and the tests are the `1b` section of
`security.rs`.

## 3.6 Modularity and separate compilation (do not defer)

The historical killer of tierless languages ([`01`](01-vision-and-premise.md) §1.6). The rule:

> **Placement, effects, and event/command types are part of a module's published signature.**
> Inference is intra-module; boundaries are declared.

```python
# orders.becki — generated by `beck iface`, checked in, reviewed like a .mli

model Order:
    id: OrderId
    total: Money

union Event:
    OrderPlaced(id: OrderId, customer: Str, total: Money, at: Int)

@on(any)
def recent(customer: Str, limit: Int) -> list[Order]

@signal
@on(server)
def orders() -> Signal[Map[OrderId, Order]] uses durable
```

> **Correction, Phase 2** ([`20`](20-phase-2-report.md) §20.4 item 6). This block originally used a
> notation of its own — `orders : Signal[…] ! { durable } @on(server)` — which would have meant a
> second grammar to keep in step with the first, for a file whose whole purpose is to be read by
> people who already read Beck. A `.becki` is **ordinary Beck**: the module's types verbatim, and a
> **bodyless `def`** per published name carrying its row and its placement. A bodyless `def` is a
> *declaration*, which the language wanted anyway — a trait's method signature is one — and the
> lexer, parser, printer, formatter and diagnostics all came free.
>
> One thing the file must do that is not obvious: **published rows are closed**. A row variable's
> number comes from the order the checker minted it in, so publishing one would make two
> compilations of an unchanged module disagree and the firewall would never hold. The cost is that
> effect polymorphism does not cross a module boundary — an exported higher-order function publishes
> its parameter's row as closed — which rejects programs that could have been accepted and never the
> reverse.

Consequences: modules compile against signatures (true separate compilation, parallel builds); body
edits don't invalidate downstream modules; **effect widening is a breaking API change** flagged by
`beck check --api` — a command that arrives with the package system (Phase 4;
[`12`](12-standards-and-conformance.md) §12.2 records that today only `--wire-compat` exists) — so
a library that starts phoning home cannot do so silently, a novel supply-chain
property; and **event types in signatures are the wire/log compatibility surface** — changing one
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
    actor: ActorId      # stable authenticated identity — NEVER the live Session capability or token
    body: T
```

*There is one other source, and it is a `Signal` rather than a `Stream`.* `presence()` is who is
connected now ([`10`](10-decisions.md) D6, built in [`48`](48-identity-report.md)) — the only input
to a view that **moves without an event**. It performs `cap.presence`, which is what places it on
the server; the chokepoint may not read it, because an event whose existence depended on who
happened to be connected would not survive a replay.

Two rules the review pass made explicit ([`14`](14-review-findings.md) F5, F3): the envelope
records a durable *identity*, never the `Session` capability itself — tokens and live capabilities
must not be persisted into an immutable log; and **only validated events are durably logged** — raw
command envelopes are transient (retained briefly for idempotency de-duplication, then discarded),
so an attacker's rejected traffic never becomes permanent storage.

`Session` is minted by the identity subsystem — a bundled OSS IdP or an external OIDC issuer, never
our own auth code — with verified claims mapped to typed capabilities
([`10`](10-decisions.md) D6).

> It carries a third field that **nothing verifies and nothing should**: `path`, the route the
> client says it is on ([`94`](94-the-client-report.md)). `actor` and `claims` say *who* is
> asking and are a provider's answer; `path` says *where* they are and is the browser's own
> statement about itself. The two halves are told apart structurally where it matters — a Mode B
> page may read the second and not the first (§94.3) — and the route cannot reach a **fold**,
> because an `Envelope` carries the actor's name and nothing else, so no replay depends on where
> anybody was browsing.

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

A program may declare **several** `durable` folds, and that is not several logs. The rule above is
one totally-ordered log per application, so several folds are several *projections* of it: the
compiler fuses them into one accumulator with a field per fold, and the runtime persists and
snapshots exactly what it did before ([`23`](23-incremental-views-report.md) §23.3). A fold may also
read a slice of the log rather than all of it, by naming a `filter_map` between the chokepoint and
itself; the filter is compiled into the step, because replay has to stay a function of the log.

**The signal graph is a graph, not a pipeline.** This section reads top-to-bottom, and the programs
it describes do not: `events` is decided from the state, and the state is folded from `events`. The
cycle is real and it is sound — validation reads the accumulator under the same lock as the append —
so a checker that resolves declarations in order rejects working programs
([`19`](19-phase-1-report.md) §19.4 item 4). Signal names are pre-registered and unified afterwards,
and a condensation exists where an ordering does not.

**`decide` and `per_session` are constructs, not prose.** Two names below were originally described
rather than defined, and Phase 1 found that the description could not be implemented as written:

```
decide      : (Stream[Proposal], Signal[S], (S, Proposal) -> Result[list[E], R] ! e) -> Stream[E] ! e
per_session : (Signal[A], (A, Session) -> B ! e) -> Signal[B] ! e
```

`decide` threads the **accumulator** through validation, which the `filter_map` shape below cannot:
F2's obligations — a client-minted id accepted only if *fresh*, and ownership checked against the
actor — both require reading current state ([`19`](19-phase-1-report.md) §19.4 item 5). It makes
"authority is one chokepoint" a node in the graph rather than a convention, which is what §3.5's
security property rests on.

`per_session` makes the fanout point first-class ([`19`](19-phase-1-report.md) §19.4 item 6). §3.8
says per-session views are the norm and [`05`](05-tier-lowering.md) §5.3 says they are where a naive
implementation becomes Meteor-at-scale; a construct is what lets Phase 3 share arrangements across
one.

**Commands vs events (CQRS as types).** Clients propose `Command`s; the server's `validate` — the
sole consumer of ingress, holding the `Session` capability — decides what becomes an `Event`.
Events are facts: past-tense, immutable, the only input to folds. The general signature is
`validate : (Session, Command) -> list[Event]` — a command may legitimately yield several events
(PlaceOrder → OrderPlaced + StockReserved), and the batch is appended **atomically**: contiguous
`seq`s, all-or-nothing, so no fold ever observes half a command's consequences. The sketch's
`Option[Event]` is the single-event special case. Two obligations sit on `validate` that the todo
sketch (deliberately auth-free) does not model, and real programs must
([`14`](14-review-findings.md) F2): client-minted ids are accepted only if *fresh* (first-writer
wins — a colliding `Add` is rejected, never an overwrite), and commands referencing existing
entities check ownership against `actor` — the `Ref[T] requires owns(...)` pattern the orders
example uses.

**Optimism, derived.** For a command `c`, the client may speculatively apply
`validate_optimistic(c)`'s expected event to its local view of state **iff** the fold and view
involved are unplaced-pure and the state slice is client-visible. Both tiers run *the same
function* and merely disagree briefly about the fold's order; reconciliation is by `seq` — when the
authoritative patch arrives, speculative state is confirmed or discarded. This is also why clients
mint entity ids (`uuid7()` in the sketch): they must refer to a todo before the server confirms it
exists. Browsers here are **replicas, not terminals**. `Signal[T]` carries a freshness dimension
(`confirmed | pending(n)`) that UI code can render ("saving…") — staleness is typed, not pretended
away. *Built ([`94`](94-the-client-report.md)), and as a **source** rather than as a
dimension on every signal's type: `freshness()` joins `presence()` in this section's vocabulary and
answers `Confirmed | Pending(n)` for the render it is part of. What that gives a page is "is any of
this a guess", which is what "saving…" means; what it does not give is **which part** — a per-signal
dimension would let a page say that one list is speculative while the header is not, and §94.15 says
plainly that this is a coarser thing than the sentence above describes.* The concession the original makes explicitly and we inherit: for *concurrent edits to the
same value* (two users in one text field), speculative-then-reconcile is not enough — you need
CRDTs or OT, "and no type system absolves you"
([`09`](09-risks-and-open-questions.md) §9.5).

**Replay, forked worlds.** Because everything downstream of ingress is deterministic:
`beck replay --to <seq|time>` reproduces any historical state; `beck fork --from prod --at
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
  analysis allows, recomputed where not — `beck explain incremental <view>` shows which, and why.

  *The command is built* — `beck-core/src/incremental.rs`, and it is the **analysis only**: it says
  which views a plan could maintain and which the rules cannot reach, over a program whose views
  are all still full recompute per event ([`23`](23-incremental-views-report.md) §23.15).
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
- Doctrine ([`10`](10-decisions.md) D3, decided): **the ledger is the truth.** By default,
  `retain=forever` — replay from the first event, through the full upcast chain, must always
  reproduce state (a CI-gated invariant, [`04`](04-compiler-architecture.md) §4.8); snapshots are
  pure optimisation. A store may opt down to bounded retention (`retain=90.days`), making snapshots
  authoritative beyond the window and its upcaster obligations finite.
- Rollout choreography — drain, snapshot, migrate, resume; two versions live simultaneously — is
  the operator's job ([`06`](06-kubernetes-and-packaging.md) §6.4). Wire compatibility of
  command/event types across versions is checked by `beck check --wire-compat` against the
  previously deployed signature ([`04`](04-compiler-architecture.md) §4.3).

## 3.10 Staged sequence of work

1. HM + ADTs + traits; `Stream`/`Signal`/`fold`/`durable` typed but **placement fully manual**
   (`@on`), matching the original sketch exactly. — **Phase 1**, except traits, which parsed and
   warned until Phase 3 checked them: declarations and impls ([`27`](27-the-walls-come-down-report.md)), bounds
   on a type parameter ([`27`](27-the-walls-come-down-report.md)), the `.becki` boundary
   ([`27`](27-the-walls-come-down-report.md)) and the arithmetic operators
   ([`27`](27-the-walls-come-down-report.md)). Dispatch is static and an impl desugars to ordinary
   definitions, so the IR and the evaluator are unchanged.
2. Effect rows inferred; placement manual but **verified** (reject `@on(client)` + `durable`;
   reject impure folds). Already novel, already shippable. — **Phase 2**.
3. Placement inference for the unannotated middle; `beck explain place`; freshness-typed optimism.
   — **Phase 2**, except freshness-typed optimism, which needed Mode B and is now built
   ([`94`](94-the-client-report.md)): `freshness()` is a signal source, `Freshness`
   is `Confirmed | Pending(n)`, and a page that reads it may only render on the client — because a
   server holds the log and its answer would be `Confirmed` at every position of it.
4. The cost model: rendering placement (DOM-patch vs data-patch, [`05`](05-tier-lowering.md) §5.1)
   chosen per component. — **the cost model is Phase 2; the rendering choice is not.** Mode B does
   not exist, so there is one way to render and a model with one option is not choosing. What Phase 2
   does is charge a crossing the *smaller* of its two ends, which is that choice expressed as a cost
   and ready for the second lowering ([`20`](20-phase-2-report.md) §20.4 item 1).
5. Incremental view compilation (differential lineage); materialized read models; pgwire exposure.
   — **all three are Phase 3** ([`23`](23-incremental-views-report.md),
   [`23`](23-incremental-views-report.md), [`23`](23-incremental-views-report.md)). The read
   models are not *materialized* in the sense this line assumed: they are the arrangements, served
   as relations rather than projected into tables ([`10`](10-decisions.md) D26). **Query fusion is
   built too** ([`23`](23-incremental-views-report.md)), on the dataflow plan rather than on §4.2's
   `Query` sub-language, which is still symbolic and still unwritten — so this item is complete.
6. Migrations/upcasters + operator choreography; replay/fork tooling.

Stages 1–2 are a "typed tierless framework" someone could adopt. Stages 3–6 are the moat.
