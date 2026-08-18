# 01 — Vision and premise

*Premise source: the original conversation, preserved in [`00-original-idea.md`](00-original-idea.md).*

## 1.1 The seed, and the three moves

The idea began with SICP: what if HTML, CSS and JavaScript acted as both data and functions, so that
a working website is just `(my-javascript (my-css (my-html)))`? Three moves take that seed to a full
language, and each move is a claim the rest of these documents must cash:

1. **A website is an evaluation.** Pages, styles and components are ordinary values built by
   ordinary functions (the Hiccup/X-expression lineage). Rendering is function application.
2. **Every tier is a pure function or a fold.** JavaScript's impurity — the page evolving over
   time — generalises: the *database* is a durable fold over an event stream; a *query* is a pure
   function of that fold's value; *infrastructure* is a function of the program; a *deploy* is an
   event on the same stream it deploys. The whole system is
   `(deploy (infra (backend (page))))` folded over an event stream.
3. **Time enters at declared merge points.** SICP's unresolved wound — merging streams from
   independent sources smuggles in nondeterminism — becomes a language construct: the program has
   explicit ingress points (`merge_clients()`), and *only there* does arbitrary interleaving exist.
   Everything downstream of the merge is deterministic.

Move 3 is the deep one. It means the entire backend is a deterministic function of the event log —
which buys replay, time-travel debugging, principled optimistic UI, and audit-by-construction. It
also concentrates all the hard distributed-systems problems into one named place, where the language
can be honest about them (see [`09`](09-risks-and-open-questions.md) on ordering and scale).

## 1.2 The problem being solved

A single feature — "show the customer their todos and let them add one" — currently requires
hand-writing and hand-synchronising:

| Artefact | Language | Failure mode when it drifts |
|---|---|---|
| React component | TypeScript/JSX | renders stale shape, `undefined is not a function` |
| REST/GraphQL endpoint | TS/Go/Python | 500s, over-fetching |
| Request/response DTOs | TS types + JSON Schema + Pydantic | silent field mismatch |
| ORM model + cache invalidation | Python/TS | N+1 queries, stale caches |
| Migration | SQL | deploy-order coupling |
| Dockerfile | Dockerfile DSL | 900 MB image, CVE surface |
| Helm chart / Terraform | YAML/HCL | `imagePullBackOff` at 2 a.m. |
| CI pipeline | more YAML | works on my machine |

Eight representations of one idea, six languages, every boundary a place where types are re-declared
by hand and disagree. The industry's response is better glue (codegen, GraphQL, tRPC, CDKs). Beck's
response is to **delete the boundaries and make them compiler output**.

## 1.3 What "one language" means precisely — and the canonical example

Not "a language you can also use for infrastructure" (Pulumi/CDK — a general language scripting a
foreign model). Rather:

> There is **one program**, **one module graph**, **one type checker**, and **one AST**. Execution
> tier is a property the compiler checks (and can infer), and each tier is a *lowering target*, the
> way x86 and ARM are lowering targets for a C compiler.

```
                    ┌─────────────────────────────────┐
   one program ───▶ │  parse → expand → typecheck     │
                    │  → Core IR (placed, effectful)  │
                    │  → PARTITION                    │
                    └──┬────┬────┬────┬────┬──────────┘
                       │    │    │    │    │
              browser ─┘    │    │    │    └─ cluster  (k8s object graph)
                  service ──┘    │    └────── container (OCI image)
                     log+folds ──┘            (durable state + read models)
```

### The canonical example: the original sketch, in the Python surface

This is the todo program from [`00`](00-original-idea.md) §Exchange 4, translated construct-for-
construct into the surface syntax that [`02-syntax.md`](02-syntax.md) motivates. Nothing is added or
dropped; only notation changes.

**It does not compile, and that is deliberate rather than rot.** It is a faithful translation of the
*sketch*, so it keeps the sketch's notation where the language later chose differently: method calls
(`todos.map(…)`, `.values().sort_by(…)`), `f"…"` interpolation, a handler written as a `lambda`
rather than a command constructor, `cls=` where the attribute is `class` (now `B0218`), a `css:`
block that has no parser, a `validate` returning `Option[Event]` where it returns
`Result[list[Event], Rejection]`, and an `input` with no label (now `B0221`). Rewriting it would
make this section claim something it does not: that the sketch and the language are the same text.
**[`examples/todo.beck`](../compiler/examples/todo.beck) is the same program in the language**, it
compiles, and a test runs it. Read this one for the shape and that one for the syntax.

```python
# todo.beck — one file: client, server, database, wire, deploy

type Id = Uuid
model Todo:
    id: Id; text: str; done: bool

union Command:                      # what clients may ASK
    Add(id: Id, text: str)          # client names the id — see "optimism"
    Toggle(id: Id)
    Delete(id: Id)

union Event:                        # what the server RECORDS
    Added(id: Id, text: str)
    Toggled(id: Id)
    Deleted(id: Id)

# ---- the merge point: where time enters. exactly one of these. ----
@on(server)
commands: Stream[(Session, Command)] = merge_clients()

@on(server)
def validate(sess: Session, cmd: Command) -> Option[Event]:
    match cmd:
        case Add(id, text): Some(Added(id, text)) if text.strip() else None
        case Toggle(id):    Some(Toggled(id))
        case Delete(id):    Some(Deleted(id))

events: Stream[Event] = commands.filter_map(validate)

# ---- the database is a fold ----
def apply_event(todos: Map[Id, Todo], e: Event) -> Map[Id, Todo]:   # pure, unplaced
    match e:
        case Added(id, text): todos.set(id, Todo(id, text, done=False))
        case Toggled(id):     todos.update(id, lambda t: t.with(done=not t.done))
        case Deleted(id):     todos.remove(id)

todos: Signal[Map[Id, Todo]] = durable(fold(apply_event, {}, events))

# a "query" is just another pure function of the signal;
# keeping it incremental is the compiler's job, not yours.
remaining: Signal[int] = todos.map(lambda ts: ts.values().count(lambda t: not t.done))

# ---- the page is a pure function of state ----
def view(todos: Map[Id, Todo], remaining: int) -> Html:             # pure, unplaced
    ui:
        main:
            h1: "todos"
            input(placeholder="what needs doing?",
                  on_enter=lambda text: send(Add(uuid7(), text)))
            ul:
                for t in todos.values().sort_by(lambda t: t.text):
                    li(key=t.id, cls="done" if t.done else ""):
                        span(on_click=lambda: send(Toggle(t.id))): t.text
                        button(on_click=lambda: send(Delete(t.id))): "×"
            footer: f"{remaining} remaining"

styles = css:
    main:  {max_width: 40.ch, margin: "0 auto", font: "16px system-ui"}
    .done: {text_decoration: line_through, opacity: 0.5}

page: Signal[Html] = map2(view, todos, remaining)
app = document(styles, page)
```

The canonical core is the S-expression form — one definition, to show the two surfaces are one AST:

```clojure
(def todos (: (Signal (Map Id Todo)))
  (durable (fold apply-event (map) events)))
```

**What is absent** — the original's checklist, now our acceptance criteria: no HTTP routes, no JSON,
no SQL, no schema migration files, no fetch calls, no Dockerfile. What the compiler derives instead:

- `validate`, `events` and the durable fold are `@on(server)`: the log and the business rules
  provably never ship to a browser.
- `apply_event` and `view` are pure and unplaced, so they **compile twice** — client and server —
  "and that's not waste, it's the payoff": the client can apply an expected event speculatively and
  reconcile when the authoritative patch arrives (Meteor's latency compensation, made principled,
  because both tiers run *the same fold* — the guarantee hand-written stacks cannot make).
- `page` consumes server signals from the client: the compiler slices the signal graph at that edge
  and streams patches over one websocket; the browser's default job is
  `fold(apply_patch, initial_html, patches)` — a few-kilobyte patch interpreter
  ([`05-tier-lowering.md`](05-tier-lowering.md) §5.1).
- First paint is free server-side rendering: evaluate `view` against the current accumulator, ship
  HTML.
- `remaining` is not recomputed per event; the fold and its derived signals lower onto an
  incremental dataflow core ([`05`](05-tier-lowering.md) §5.3).
- `durable` ⇒ a persisted log + snapshots ⇒ a volume and a snapshot schedule in the deployment;
  `merge_clients()` ⇒ a websocket ingress route; the whole thing ⇒ OCI image + Kubernetes object
  graph ([`06`](06-kubernetes-and-packaging.md)).
- If `Todo` changed shape since the last deploy, `beck deploy` **refuses to ship** until a
  `migrate: OldState -> NewState` exists; the deploy rides the stream — old fold drains, new fold
  resumes from snapshot + log ([`06`](06-kubernetes-and-packaging.md) §6.4).

### The same shape at "week two" scale

The todo program answers "is it coherent"; the questions in later documents (auth, money, external
calls, read models) use an orders variant in the same shape:

```python
command PlaceOrder(customer: Ref[Customer], total: Money) requires auth(customer)

event OrderPlaced:
    id: OrderId; customer: Ref[Customer]; total: Money; at: Instant   # stamped at ingress

orders: Signal[Map[OrderId, Order]] = durable(fold(apply_order, {}, order_events))

def recent(c: Ref[Customer], limit: int = 20) -> list[Order]:        # a read model
    return from o in orders.values()
           where o.customer == c and o.at > clock() - 7.days
           order by o.at desc
           take limit
```

Note `at: Instant` on the event, not `now()` in the fold: folds must be deterministic, so **time is
data, captured at the merge point** ([`03`](03-type-and-effect-system.md) §3.7). `clock()` in a read
model makes it a function of the clock *signal* — legal, and visibly time-varying in its type.

## 1.4 Design principles

1. **The surface is negotiable; the core is not.** One canonical homoiconic AST, multiple faithful
   printers ([`02`](02-syntax.md)).
2. **Every tier is a pure function or a fold; time enters at declared merge points.** This is the
   semantic bet of the whole language (§1.1). Purity is not a style preference — it is what makes
   code placeable, optimism sound, and replay exact.
3. **Placement is typed.** Explicit `@on(...)` always available; purity means "unplaced — compiles
   anywhere"; what is security-relevant is *proved* (secrets cannot cross to the client)
   ([`03`](03-type-and-effect-system.md)).
4. **Zero-config must actually be zero.** `beck run app.beck` starts a working app — one process,
   embedded log, no daemon, no cluster, no YAML. Kubernetes appears only when a `deployment` block
   names it ([`06`](06-kubernetes-and-packaging.md) §6.1).
5. **No new state engine.** The event log is Beck's semantic core, but its *substrate* is boring,
   proven storage (Postgres, object stores); cluster desired-state lives in the Kubernetes API
   server, extended by Crossplane. We write neither a storage engine nor a Terraform.
6. **Escape hatches at every layer, typed.** External relational stores, raw SQL, raw HTML, raw k8s
   patches, FFI — available, type-checked at the boundary, never required.
7. **Interop or die** — right about the stakes, wrong about the verb. A language that cannot reach
   the Python and npm ecosystems is a research artefact ([`09`](09-risks-and-open-questions.md)
   §9.2). But the libraries that most expand a language's utility — NumPy, pandas, `serde`, LINQ —
   are **notations**, and a notation cannot be called across a boundary at all: its whole value is
   that it composes with the code around it, which is exactly what an RPC hop destroys
   ([`105`](105-the-ecosystem-answer.md) §102.5). So most of the ecosystem question is not an
   interop question but a question about Beck's own means of combination. What must genuinely be
   *callable* is the capability half — GPUs, codecs, cloud APIs — and it is narrower than the
   slogan and still existential.
8. **Errors and explanations are the product.** `beck explain` answers "why did this land there /
   re-render / re-provision" in prose, with a trace ([`04`](04-compiler-architecture.md) §4.7).

## 1.5 Non-goals

- **Not** a systems language — GC'd surface, no ownership in user code. Rust is the engine room,
  not the surface.
- **Not** a numeric/ML language — FFI to that world instead.
- **Not** a storage engine — `durable` folds ride on proven substrates (§1.4.5).
- **Not** a general-purpose IaC tool for pre-existing estates (it can *reference* them,
  [`05`](05-tier-lowering.md) §5.4).
- **Not** multi-orchestrator on day one — Kubernetes + single-process; others are post-1.0
  `Platform` implementations.
- **Not** Python-compatible — Python-*shaped*. No `pip install` promises.

## 1.6 Prior art, and the lessons taken

The source conversation ([`00`](00-original-idea.md)) identified most of this map; consolidated with
what each system teaches us:

| System | Contribution | Lesson for Beck |
|---|---|---|
| **Links / Ur/Web / ML5** | Typed tierless client+server+SQL; placement in the type system | The type-level placement machinery works; the audiences were too narrow. Syntax and ecosystem are adoption problems, not afterthoughts |
| **Eliom (Ocsigen)** | Industrial multitier OCaml, separate compilation, module system | Signatures must carry placement/effects, or modularity dies ([`03`](03-type-and-effect-system.md) §3.6) |
| **Meteor** | One JS codebase, latency compensation, live queries | The dream sells. Magic without types becomes distrust at scale; `oplog` tailing without incremental semantics hits a wall |
| **Electric Clojure** | Reactive tier-splitting compiler; `e/server`/`e/client` at expression granularity | Signal-graph slicing across the network is implementable today |
| **Lamdera** | Elm on both tiers; **typed state migrations gate deploys** | Refuse-to-ship-without-migration is the correct deploy semantics ([`06`](06-kubernetes-and-packaging.md) §6.4) |
| **Phoenix LiveView** | Server-driven UI; browser as patch applier | The thin-client model scales to real products; per-connection server state is the cost to manage |
| **React Server Components** | Server functions evaluating to serialized UI | The mainstream is converging on our shape, piecemeal |
| **Elm / Redux** | `view : Model -> Html`, `update : Msg -> Model -> Model` | The page is a fold; the architecture is teachable to millions |
| **Datomic / event sourcing / Kleppmann** | Database as a value; state = fold over log; views = derived | The data tier's semantic model ([`03`](03-type-and-effect-system.md) §3.7) |
| **Materialize / Naiad / differential dataflow / DDlog** | Incremental view maintenance as a compilation target | "Keeping queries incremental is the compiler's job" is buildable — and DD is MIT-licensed Rust ([`07`](07-dependencies.md) §7.4) |
| **Rama / Convex** | Whole backend as functions over an event log; reactive re-evaluated queries | Commercial validation of log-centric backends |
| **Nix / NixOS / MirageOS** | Infrastructure as evaluation of a pure function | Referential transparency of deploys; we deliver the property via reproducible builds + content-addressed artefacts rather than Nix itself ([`06`](06-kubernetes-and-packaging.md) §6.2) |
| **Unison / Darklang** | Content-addressed code; deployless | Radical cousins; their friction was ecosystem exit, not the idea |
| **Roc (platforms) / Koka** | Pure app handed to an effectful Rust host; Perceus refcounting | The compile-to-Rust-host pattern for our server runtime |
| **Erlang/OTP / Gleam** | Deploys arrive in-band; `code_change` migrates state | Deploy-as-stream-event is proven at telecom scale |
| **Obelisk (Haskell/reflex)** | One FRP codebase + Nix deploys, shipping | Closest single artefact to the whole idea; ergonomics kept it niche |
| **Out of the Tar Pit** | Essential state + pure logic, everything else derived | The manifesto; Beck is an attempt to build it |

Two recurring failure modes, now designed against: **(a)** poor modularity/separate compilation in
tierless languages — countered by placement-and-effects in published module signatures
([`03`](03-type-and-effect-system.md) §3.6), non-negotiable from Phase 2; **(b)** magic that cannot
be inspected (Meteor) — countered by `beck explain` shipped in v0.1
([`04`](04-compiler-architecture.md) §4.7).

## 1.7 What success looks like at 1.0

A developer who knows Python but has never touched Kubernetes can, in one afternoon:

```console
$ beck new shop && cd shop
$ beck run                    # working app on localhost:3000 — no container, no cluster
$ beck deploy --to prod       # images built, cluster reconciled, TLS live, state migrated
$ beck explain place validate # "server: consumes Session capability; feeds the event log"
$ beck replay --at yesterday  # the whole backend, re-evaluated deterministically
```

…and a staff engineer can read [`03`](03-type-and-effect-system.md) and believe the guarantees; and
a platform engineer can read the generated policies and find them *stricter* than the hand-written
ones they replaced.

## 1.8 The competitive claim, stated honestly

Asked directly — will this plan give Beck better performance and features than any alternative? —
the defensible answer has three parts, and the marketing must never blur them.

**Features: no alternative has the combination; most have one row of this table.**

| Capability | Who else has it |
|---|---|
| Placement proofs (secrets provably never reach the client) | Ur/Web (research); no mainstream system |
| One program → app + database + infra, all derived | Darklang (proprietary, faded), Encore/Winglang (backend/infra only — no frontend or data semantics) |
| Event-sourced core with compiler-maintained incremental views | Materialize (SQL only, BUSL), Rama (JVM, backend only), Convex (no infra tier, no proofs) |
| Replay / fork-from-prod / deterministic simulation as *user-facing* features | FoundationDB internally; Antithesis as a paid service; nobody as a language feature |
| Typed migrations gating deploys | Lamdera (Elm-niche, two tiers) |
| Least-privilege infrastructure derived from effects | nobody |
| ~10 KB thin client with per-component local upgrade, solver-chosen | LiveView (thin only), Elm/Lamdera (fat only) |
| WCAG as a compile-time property | nobody |

Every row exists somewhere; no system has more than two rows. Within its scope, Beck's feature set
is a strict superset of every alternative. The honest boundary is that scope ([`01`](#15-non-goals)):
ML/numeric work, systems programming, and ecosystem breadth are conceded and bridged (FFI, sidecar),
not contested.

**That concession is narrower than it has been read**, and the distinction is
[`105`](105-the-ecosystem-answer.md) §102.1's. Conceding *solvers, GPU kernels and numerical
methods* — SciPy, PyTorch, CUDA — is sound and stands. Conceding *arrays and dataframes* is a
different and much larger concession that was never argued: the Stack Overflow survey puts NumPy at
21.2% and pandas at 20.7% of **all** developers in **all** languages, second and third among every
library it lists, which is general-purpose vocabulary rather than a numerical speciality. Aggregation
over collections is [`99`](99-the-data-tier-means-of-combination.md)'s territory and this language's
own, and [`08`](08-roadmap.md) §8.6.2 now records a verdict for each rather than letting one sentence
concede both.

**Performance: win where it is structural, concede where it is artisanal.**

1. **Versus what teams actually deploy** (Node/Next, Python, Rails, typical JVM): large, structural
   wins — LLVM-native services (the Perry precedent: 1.7–24.6× over Node), no interpreter or GIL,
   kilobytes of client instead of megabytes, incremental views instead of cache-invalidate-recompute,
   fused queries instead of N+1, boundaries that don't exist and therefore cost nothing.
2. **Versus the best hand-written artefact at any single layer** (hand-tuned Rust service, artisan
   bundle, DBA-tuned SQL): a narrow loss, and the plan does not claim otherwise — the escape
   hatches (FFI for hot paths, raw SQL, `external store`) exist precisely so the 1% can be
   hand-fed.
3. **The claim we actually make and benchmark**: real systems lose their performance at the
   boundaries — serialisation, over-fetch, chattiness, stale-cache recomputation, bundle bloat.
   Beck deletes the boundaries, so the *shipped system* outperforms the composite that real teams
   actually produce. Phase 5 publishes the suite against a named baseline stack, with lines of
   code alongside latency ([`08`](08-roadmap.md)).

**The enforcement**: none of this is self-congratulation — the feature claims are conformance and
security tests, and the performance claims are CI-gated budgets from Phase 0
([`12`](12-standards-and-conformance.md) §12.1, [`13`](13-testing.md) §13.7). The features are
structural moat (a competitor cannot bolt on placement proofs without becoming a language); the
performance is earned by executing the roadmap, and the gates are what keep it earned.
