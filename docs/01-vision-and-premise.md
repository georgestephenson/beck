# 01 — Vision and premise

## 1.1 The problem being solved

A single feature — "show the customer their recent orders and let them place a new one" — currently
requires the developer to hand-write and hand-synchronise:

| Artefact | Language | Failure mode when it drifts |
|---|---|---|
| React component | TypeScript/JSX | renders stale shape, `undefined is not a function` |
| REST/GraphQL endpoint | TypeScript/Go/Python | 500s, over-fetching |
| Request/response DTOs | TS types + JSON Schema + Pydantic | silent field mismatch |
| ORM model | Python/TS | N+1 queries |
| Migration | SQL | deploy-order coupling |
| Dockerfile | Dockerfile DSL | 900 MB image, CVE surface |
| Helm chart / Terraform | YAML/HCL | `imagePullBackOff` at 2 a.m. |
| CI pipeline | more YAML | works on my machine |

Eight representations of **one** idea, six languages, and every boundary between them is a place
where types are re-declared by hand and can disagree. The industry's response has been better glue
(codegen, GraphQL, tRPC, CDKs). The tierless response is to **delete the boundaries** and make them
compiler output.

## 1.2 What "one language" means precisely

It does **not** mean "a language you can also use for infrastructure" (that's Pulumi/CDK — a general
language *scripting* a foreign model). It means:

> There is **one program**, **one module graph**, **one type checker**, and **one AST**. Execution
> tier is a property the compiler *infers and checks*, and each tier is a *lowering target*, the way
> x86 and ARM are lowering targets for a C compiler.

Five tiers, five backends, one front end:

```
                    ┌─────────────────────────────────┐
   one program ───▶ │  parse → expand → typecheck     │
                    │  → Core IR (tier-annotated)     │
                    │  → PARTITION                    │
                    └──┬────┬────┬────┬────┬──────────┘
                       │    │    │    │    │
              browser ─┘    │    │    │    └─ cluster  (k8s object graph)
                  service ──┘    │    └────── container (OCI image)
                          data ──┘            (SQL / relational plan)
```

The partition pass is the whole product. Everything else is table stakes.

## 1.3 The running example

Used verbatim across all following documents. This is the Python-surface form; see
[`02-syntax.md`](02-syntax.md) for the identical S-expression core.

```python
# orders.tier
model Customer:
    id:    CustomerId = key(auto)
    email: Email                      @unique
    name:  str

model Order:
    id:        OrderId = key(auto)
    customer:  Ref[Customer]
    total:     Money
    placed_at: Instant = now()
    @index(customer, placed_at.desc)

store orders:    Table[Order]
store customers: Table[Customer]


def recent(c: Ref[Customer], limit: int = 20) -> list[Order]:
    return from o in orders
           where o.customer == c and o.placed_at > now() - 7.days
           order by o.placed_at desc
           take limit

def place(c: Ref[Customer], total: Money) -> OrderId requires auth(c):
    return orders.insert(Order(customer=c, total=total))


component OrderPanel(c: Ref[Customer]):
    rows = live recent(c)             # subscribes; re-renders on write to `orders`
    ui:
        h2: "Recent orders"
        table:
            for o in rows:
                tr:
                    td: o.id
                    td: money(o.total)
        button(on_click=lambda: place(c, Money("9.99"))):
            "Buy again"


service api:
    entry     = OrderPanel
    expose    = http(route="/", tls=auto)
    autoscale = between(2, 50, on=[cpu(70), p99_latency(50.ms)])

deployment prod:
    platform = kubernetes(context="prod-eu")
    data     = postgres(managed, ha=True, backups=daily)
    include  = [api]
```

Note what is **absent**: no `@server`/`@client` on `recent` or `place`, no endpoint declaration, no
fetch call, no DTO, no SQL, no `Dockerfile`, no `Deployment` YAML, no migration file. The compiler
derives all of it:

- `recent` touches `orders` ⇒ it *may* run on server or data tier. The solver pushes it **into the
  database** as a single SQL statement (it is expressible relationally), and synthesises a typed,
  batched, cacheable server endpoint for the client to call.
- `place` is guarded by `requires auth(c)` ⇒ it is **forbidden** from the client tier (the capability
  cannot be forged there), so it becomes a server-tier mutation with a generated endpoint; the
  `on_click` closure becomes a client-side call to it.
- `live` makes `rows` a subscription — the compiler knows `place` writes `orders` and `recent` reads
  it, so it emits invalidation without the developer wiring a cache key.
- `service api` ⇒ one OCI image + `Deployment`, `Service`, `HTTPRoute`, `HorizontalPodAutoscaler`,
  `NetworkPolicy`, `ServiceAccount`+`Role` scoped to exactly the effects the code performs.
- `store` declarations ⇒ table DDL, and a migration diffed against the deployed schema.

## 1.4 Design principles

1. **The surface is negotiable; the core is not.** One canonical AST, multiple faithful printers.
   (This is what makes Q1 answerable — see [`02-syntax.md`](02-syntax.md).)
2. **Placement is a type, not a comment.** If it is inferable, infer it; if it is security-relevant,
   *prove* it; always let the human override it explicitly.
3. **Zero-config must actually be zero.** `tier run app.tier` starts a working app — single process,
   embedded storage, no daemon, no cluster, no YAML. Kubernetes appears only when you say
   `platform = kubernetes(...)`.
4. **No new state engine.** Kubernetes' API server *is* the desired-state store; Crossplane extends
   it beyond the cluster. We do not write a Terraform.
5. **Escape hatches at every layer, typed.** Raw SQL, raw HTML, raw k8s patches, raw FFI — all
   available, all type-checked at the boundary, none required.
6. **Interop or die.** A language that cannot call the Python and npm ecosystems is a research
   artefact. See [`09-risks-and-open-questions.md`](09-risks-and-open-questions.md) §9.2.
7. **Errors are the product.** For an inference-heavy language, "why did this land on the client?"
   must be answerable by the compiler in prose, with a trace. Budget real engineering for this.

## 1.5 Non-goals

- **Not** a systems language. No manual memory management in the surface language; GC (or region
  inference) is fine. Do not compete with Rust.
- **Not** a numeric/ML language. Provide FFI to that world instead.
- **Not** a general-purpose IaC tool. Tier deploys *Tier programs*. It does not aspire to manage
  your pre-existing 400-module Terraform estate (it can *reference* it — §5.4).
- **Not** multi-orchestrator on day one. Kubernetes plus single-binary. Nomad/ECS/serverless are
  post-1.0 `Platform` implementations.
- **Not** dynamically typed. Python-*like*, not Python-compatible. Do not promise `pip install`.

## 1.6 Prior art, and what actually went wrong

| System | Contribution | Why it didn't take over |
|---|---|---|
| **Ur/Web** (2010s) | Statically typed tierless web, SQL in the type system, provably no injection/XSS | Type system too alien; tiny ecosystem; no story beyond a single web app |
| **Eliom / Ocsigen** | The most industrially serious tierless language; typed client/server sections with *separate compilation* and ML modules | OCaml-shaped audience; no data/infra tier |
| **Links** (Edinburgh) | Research foundation for client/server/database in one language | Research scope by design |
| **Hop.js** | Multitier JavaScript with staging | Dynamically typed; boundaries still visible |
| **Meteor** | Huge developer excitement, JS everywhere, live queries | Not a language — a framework over JS; magic without types; scaling and ops pain |
| **GWT / Vaadin** | Compile Java to browser | Compile times; leaky abstraction over the DOM |
| **Pulumi / CDK / cdk8s** | Real language for infra, real ecosystem | Infra only; still a foreign object model; no tierless application semantics |
| **Unison** | Content-addressed code, typed distributed abilities | Radical storage model; distinct problem (durable computation) but *the closest living relative* to placement-as-effect |
| **Dark / Darklang** | Deployless full-stack | Proprietary editor + hosting; no exit |

The recurring lesson from the literature is specific and worth internalising: the Eliom papers found
that *"most tierless languages offer very poor support for modularity and separate compilation."*
Whole-program placement inference is the enemy of separate compilation and of fast incremental
builds. **This is the single biggest technical risk in the project**, and it is why
[`03-type-and-effect-system.md`](03-type-and-effect-system.md) §3.6 requires placement to be part of
a module's *published signature* — inferred within a module, declared at its boundary. Do not defer
this decision; it is not retrofittable.

The second lesson, from Meteor: magic that cannot be inspected becomes hatred at scale. Hence
`tier explain` (§4.7) as a first-class, shipped-in-v0.1 tool.

## 1.7 What success looks like at 1.0

A developer who knows Python but has never touched Kubernetes can, in one afternoon:

```console
$ tier new shop && cd shop
$ tier run                       # working app on localhost:3000, no container, no cluster
$ tier deploy --to prod          # images built, cluster reconciled, TLS live, migration applied
$ tier explain place             # "runs on: server (auth(c) capability unavailable on client)"
```

…and a staff engineer can read [`03-type-and-effect-system.md`](03-type-and-effect-system.md) and
believe the placement guarantees.
