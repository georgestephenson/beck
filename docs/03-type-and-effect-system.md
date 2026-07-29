# 03 — Type and effect system

This is where the idea either becomes real or becomes Meteor. Placement must be a **checked property
of the program**, not a heuristic in a bundler.

## 3.1 Base type system

- **Hindley–Milner style inference with bidirectional checking.** Full inference inside function
  bodies; mandatory annotations on public signatures (§2.6). This is the standard modern compromise
  (Rust, Swift, OCaml-with-mli) and it is what makes separate compilation possible.
- **Algebraic data types** + records + **row polymorphism** on records. Row polymorphism matters more
  than usual here: a client component often needs *a projection* of a server-side model, and rows let
  `{name, total | r}` be typed without generating a DTO type per endpoint.
- **Traits/typeclasses** with coherence rules (orphan rule), for `Json`, `Sendable`, `Storable`,
  `Eq`, `Hash`, `Display` — mostly `derive`d by macros (§2.4).
- **Nominal newtypes with zero cost**: `type CustomerId = newtype[u64]`. `OrderId` and `CustomerId`
  must not be interchangeable — the entire premise is that boundaries stop lying.
- **No subtyping** beyond effect-row and lifetime subsumption. Subtyping plus inference plus effects
  is where type systems go to die.
- **Memory**: garbage collected by default. Recommendation: precise tracing GC per tier
  (browser: WASM GC proposal or our own in linear memory; server: our own). Do **not** put ownership
  in the surface language (§1.5); *do* run escape analysis and region inference as an optimisation,
  because it is what will let the server tier compete with Go.

## 3.2 Effects as rows

Every function's type carries an effect row:

```
recent : (Ref[Customer], int) -> list[Order]  ! { db.read(orders) }
place  : (Ref[Customer], Money) -> OrderId    ! { db.write(orders), cap.auth }
render : (list[Order]) -> Dom                 ! { dom }
main   : () -> ()                             ! { io, net, dom, db.read(*), db.write(*) }
```

Effects are **row-polymorphic and inferred**: `map : (list[a], (a -> b ! e)) -> list[b] ! e` — one
`map`, usable in pure, DOM, and DB contexts, with the effect flowing through. This is the machinery
that Koka, Unison ("abilities") and OCaml 5 effects validate; we are not inventing it.

Effect atoms we need at minimum: `db.read(t)`, `db.write(t)`, `dom`, `net.out(host)`, `net.in`,
`fs(path)`, `time`, `rand`, `env`, `log`, `spawn`, `cap.X` (capabilities), `partial` (may diverge/panic).

## 3.3 Placement is derived from effects

**Key design decision: placement is not a primitive annotation, it is a constraint solution.**

Each tier is defined by the effect set it can *discharge*:

| Tier | Can discharge | Cannot |
|---|---|---|
| `client` | `dom`, `net.out(own-origin)`, `time`, `rand`, local storage | `db.*`, `fs`, `env`, `cap.*` requiring server secrets |
| `server` | `db.*`, `net.*`, `fs`, `env`, `spawn`, `cap.*` | `dom` |
| `data` | `db.read/write` on its own tables, pure computation only | `dom`, `net`, `io`, non-total functions |
| `edge` (optional) | `net`, `time`, cache; a restricted server | `db.*` direct |
| `build` (macro phase) | pure + module graph reads | everything else |

Placement then falls out of a constraint problem:

```
for each function f:  place(f) ∈ { t : effects(f) ⊆ discharge(t) }
for each call f → g:  either place(f) = place(g)
                      or a REMOTE BOUNDARY exists between place(f) and place(g)
                         and all crossing values are Sendable
minimise:             Σ (boundary crossings × estimated cost)
subject to:           explicit @on(...) annotations, security constraints (§3.5)
```

- Explicit annotation always wins: `@on(server)`, `@on(client)`, `@on(data)`, `@on(any)`.
- Multi-placement is allowed and normal: a pure validation function is compiled **into both** the
  client and server artefacts (validate on the client for latency, re-validate on the server for
  trust — and the compiler guarantees they cannot diverge, which no current stack does).
- Ambiguity is resolved by declared defaults per module (`#[default_placement(server)]`), then by
  cost, and any *remaining* ambiguity is a **compile error with a suggested annotation** — never a
  silent guess. This is the Meteor lesson made mechanical.

## 3.4 The placement/cost solver

Formulate as a min-cut / integer-programming problem over the call graph, which is the same shape as
classic program-partitioning work:

- **Nodes**: functions and data values (post-monomorphisation, post-inlining of small functions).
- **Node cost**: forbidden placements are ∞; allowed placements carry a tier-specific compute cost
  (client CPU is expensive and untrusted, data-tier compute is cheap and local to the rows).
- **Edge cost**: for each call that crosses tiers, `latency(edge) + bytes_crossing × unit`, with
  `bytes_crossing` estimated from types (this is where row types pay off: a projection crosses fewer
  bytes than a whole model).
- **Solve** with min-cut for the two-tier case and a small MILP/greedy-then-local-search for the
  general case; cache the solution and only re-solve dirty components.

Practical guardrails, learned from every "clever compiler" that failed:

1. **Determinism.** Same inputs ⇒ same placement, always. No timing- or hash-order-dependent results.
2. **Stability.** A one-line edit must not re-place unrelated code. Prefer the previous solution
   (stored in `tier.lock`) when costs are within a tolerance band; report placement *churn* in CI.
3. **Explainability.** `tier explain place` prints the derivation: effects → allowed tiers → binding
   constraint → chosen tier, with the alternative and its cost.
4. **Budgets, not vibes.** Let the developer assert `assert place(recent) == data` in tests, and let
   `service` declarations carry latency budgets the solver must respect.

## 3.5 Security properties you get for free (the real selling point)

Because placement is typed, several classes of vulnerability become *unrepresentable*, which is a
stronger claim than "linted":

```python
type ApiKey = secret[str]          # `secret[T]` is not `Sendable`
```

| Property | Mechanism |
|---|---|
| Secrets cannot reach the browser | `secret[T] : !Sendable`; crossing a boundary requires `Sendable`. A leak is a type error naming the exact flow |
| No SQL injection | Interpolation into `sql"..."` produces bind parameters; string→query coercion does not exist (Ur/Web's guarantee) |
| No XSS | `html"..."` interpolation escapes by type; raw insertion requires `Html.trusted(...)` which requires the `unsafe_html` capability |
| Authorisation cannot be forgotten | `place` requires `cap.auth`; `cap.*` is undischargeable on the client, so any client call to it *must* pass through the generated server endpoint, which checks the capability. Forgetting the check is a compile error, not a pentest finding |
| Least-privilege infrastructure, automatically | Effects → generated RBAC and `NetworkPolicy`. A service whose code only reads `orders` gets exactly that grant (§6.5) |
| Supply-chain: no arbitrary build-time code | Macro phase is capability-restricted (§2.4) |
| Mass-assignment / over-posting | Endpoints are generated from types; there is no request-body-to-model reflection |

Write these down as **the** marketing claims. "One language for everything" is a productivity pitch
that CTOs have heard before; "your framework cannot leak your API key to the browser, and the compiler
proves it" is a claim nothing in the mainstream stack can make.

## 3.6 Modularity and separate compilation (do not defer this)

§1.6 flagged this as the historical killer. The rule:

> **Placement and effects are part of a module's published signature.** Inference is *intra-module*;
> module boundaries are *declared*.

```python
# orders.tieri  — the published interface (generated, then checked in)
def recent(c: Ref[Customer], limit: int = 20) -> list[Order]
    ! { db.read(orders) }  @on(data | server)
def place(c: Ref[Customer], total: Money) -> OrderId
    ! { db.write(orders), cap.auth }  @on(server)
```

Consequences, all good:

- A module compiles against its dependencies' **signatures**, not their bodies ⇒ true separate
  compilation, parallel builds, and cached artefacts.
- Changing a body without changing placement/effects does not invalidate downstream modules.
- Effect widening is a **breaking API change**, surfaced by `tier check --api` in CI. A library that
  starts phoning home cannot do so silently. (This is a genuinely novel supply-chain property.)
- Generic/polymorphic code stays placement-polymorphic (`@on(any)` + effect row variable) and is
  specialised per instantiation, so the standard library is not duplicated per tier.
- `tier api generate` writes the `.tieri`; review it in PRs the way you'd review a `.h` or `.mli`.

## 3.7 Relational types for the data tier

The data tier needs a type-level distinction between "code that can be pushed into the database" and
ordinary code:

- A `Query[T]` is a **first-class value** with a typed logical plan inside (`Node`, again — the
  homoiconic core paying off), not a string.
- The comprehension syntax (`from o in orders where ... order by ... take n`) is a macro that builds
  `Query[T]`. It is checked against `store` declarations, so column typos and type mismatches are
  compile errors.
- A lambda inside a query is admitted only if its effect row is `⊆ {}` (pure) and it uses only the
  *pushable* fragment — comparisons, arithmetic, string ops, aggregates, window functions, and
  user-defined functions marked `@sql_pure`. Anything else is a compile error suggesting the code be
  moved outside the query, with a fix-it.
- **Query fusion.** `for o in recent(c): show(o.customer.name)` — the compiler sees the traversal and
  fuses it into a join rather than N+1 selects. The classic ORM disease is a *compiler* problem and we
  should treat it as one; where fusion is impossible, batch (Haxl-style) and emit a warning with the
  query count.
- **Migrations from types.** The store declarations are the schema; `tier migrate plan` diffs the
  declared schema against the deployed one and emits reversible steps, refusing destructive changes
  without an explicit `@allow_destructive` marker. Migrations are artefacts, reviewed in PRs, applied
  by the operator as a pre-upgrade hook (§6.4).

## 3.8 Reactivity is a typed dataflow, not a framework

`live recent(c)` in §1.3 works because the read effect `db.read(orders)` and the write effect
`db.write(orders)` are visible in the types:

- `live e` has type `Signal[T]` where `T` is `e`'s type, and it records `e`'s read set.
- Any committed transaction publishes its write set; the runtime intersects write sets with
  subscription read sets to decide invalidation (table-level at v0.1, predicate/row-level later).
- The client-side runtime is **fine-grained/signals**, not a virtual DOM (§5.1), so an invalidation
  updates exactly the affected DOM nodes.
- The same `Signal` abstraction serves DOM updates, server-side caches, and derived materialised
  views — one concept, three tiers. That unification is a strong argument that the tierless framing
  is the *right* framing, not just a packaging trick.

## 3.9 Staged sequence of work

Do not build all of this at once. Order matters, because each stage is checkable end-to-end:

1. HM inference + ADTs + traits, **no effects** — placement fully manual via `@on`.
2. Effect rows, inferred; placement still manual but now *verified* against effects (error if a
   `@on(client)` function performs `db.write`). Already valuable, already novel.
3. Placement inference for the unannotated cases; `tier explain place`.
4. The cost model and the solver's optimisation objective.
5. Query pushdown and fusion.
6. Row-level invalidation, region inference, escape analysis.

Stages 1–2 are shippable as a "typed tierless framework". Stages 3+ are the moat.
