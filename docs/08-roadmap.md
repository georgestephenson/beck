# 08 — Roadmap

## 8.0 Sizing, honestly

A language with five backends is a **multi-year** project. Comparable efforts: Elm reached usable in
~2 years with 1–2 people (one tier); Rust took ~9 years to 1.0 with a team; Eliom took a research group
a decade to industrial quality (two tiers). Per George's directive ([`10`](10-decisions.md) D8), the
plan optimises for **completeness, clean architecture and performance, not schedule**: nothing is
descoped to fit a team size, and durations below are sequencing calibration, not commitments. What
survives from schedule discipline is *ordering* — the walking-skeleton rule below — and budgets as
CI gates, because "optimal performance" is only real if it is measured from Phase 0.

The sequencing rule that matters more than any estimate: **build one thin vertical slice through all
five tiers before making any single tier good.** The risk in this project is not "can we write a good
type checker" — it is "does the tierless partition idea survive contact with a real deployment." Answer
that in month 3, not year 2.

## Phase 0 — Prove the premise (4–6 weeks, 1–2 people) — **BUILT**

> Implemented in [`phase0/`](../phase0/); measurements, gate verdicts and the harder-than-expected
> list in [`18-phase-0-report.md`](18-phase-0-report.md). The premise holds: interaction p99 is
> ~1 ms of server work (34 ms at a 25 ms RTT) against the ~150 ms gate, per-idle-session memory is
> ~5 KB against the ~50 KB tripwire, and replay after SIGKILL is bit-identical. Two items are
> unproven rather than proven: the apko image and the k3d deployment were never executed, because
> the environment has no container runtime.

No compiler. Hand-write, in Rust, the *output* the compiler will eventually generate for the todo
sketch ([`00`](00-original-idea.md)): ingress + envelope stamping, a durable fold over a Postgres
log (and redb embedded), server-side `view` + structural diff, the thin patch-interpreter client,
`(subscription, seq)` resumption, an apko image, k8s manifests, a kube-rs operator stub, deployed
to k3d. Then kill the process mid-stream and replay the log.

**Exit criteria** — stated from evidence, not opinion:
- interaction latency p50/p99 (click → command → event → fold → patch → DOM) on a realistic RTT,
  Mode A;
- events/s through a single Postgres-backed sequencer, and fold throughput on replay;
- per-idle-session server memory with 1k and 10k connected subscribers of a per-session view
  (the fanout number — this is the one that kills LiveView-shaped systems);
- thin-client payload (target < 10 KB brotli) and time-to-first-paint (SSR);
- reconnect-after-deploy behaviour: does resumption actually replay the gap;
- whether apko builds our artefact reproducibly; image size;
- a written list of everything that turned out harder than expected.

**Kill/pivot gates.** Interaction p99 > ~150 ms on realistic RTT ⇒ Mode B (client-side view) moves
from Phase 3 into the core plan. Per-idle-session memory that can't be brought under ~50 KB ⇒
redesign the session/subscription representation before any compiler work builds on it.

## Phase 1 — Walking skeleton (3–4 months) — **BUILT**

> Implemented in [`compiler/`](../compiler/); what it does and does not do, the measurements, and
> the harder-than-expected list are in [`19-phase-1-report.md`](19-phase-1-report.md). The sketch
> compiles and runs: `beck run examples/todo.beck` serves it, `beck replay --verify` reproduces its
> state, and the differential and replay-determinism harnesses are green. Three items are **not**
> done and are named as such: native codegen (a `Core` evaluator stands in for Cranelift) and effect
> inference (effects are declared and collected, not inferred). Deployment **is** done: the app runs
> in a real cluster, serving the page from a pod with its events in the Postgres its own `durable`
> effect provisioned, and a killed pod recovers by folding the log
> ([`19`](19-phase-1-report.md) §19.5).

The narrowest possible compiler that takes the todo sketch from source to a running k3d deployment.
Deliberately bad at everything, complete end-to-end.

- Lexer, layout, parser (Python surface + S-expression reader), `Node`, pretty-printer, `beck fmt`.
- Macro expander with hygiene — **from the start** (§2.4); retrofitting hygiene is a rewrite.
- Modules, name resolution, HM typechecker: ADTs, records, traits, `Stream`/`Signal`/`fold`/
  `durable` typed; no effect inference yet.
- `Core` IR; **manual placement only** via `@on(...)` — exactly the original sketch's annotations.
- Signal-graph slicing + command channel + envelope/patch serialisers (§4.3). Views are **full
  recompute per event** — semantically final, later made incremental.
- Backends: Cranelift (server), the thin patch client (plain JS, ~KBs — no WASM in Phase 1),
  Postgres/redb log engine, k8s object graph.
- `beck run` (single process) and `beck up` (k3d).
- Salsa from commit one; `insta` diagnostic snapshots from week two; the **differential
  single-process vs split harness** and the **replay-determinism harness** (§4.8) from the moment
  each is expressible.

**Exit**: `git clone && beck up` yields the working todo app in a local cluster, CI-asserted;
differential and replay harnesses green; `beck replay` reproduces state from a recorded log.
**Met** ([`19`](19-phase-1-report.md) §19.3, §19.5).

## Phase 2 — Effects and placement (3–4 months) — **BUILT**

> Implemented in [`compiler/`](../compiler/); what it does, the measurements, the corrections it
> makes to these documents, and what it deliberately does not do are in
> [`20-phase-2-report.md`](20-phase-2-report.md). The exit criteria are met: the todo sketch with
> **every `@on(...) removed`** compiles, places and runs; all eight of §3.5's properties are passing
> tests; and a three-module project re-checks **one** module after a body edit and three after a
> signature change. Across a 22-program corpus, 44% of everything placed is unplaced-pure.
>
> Three items are *not* what the section below implies, and are named rather than implied. **The
> general slicer was assigned to this phase and was not built** —
> [`19`](19-phase-1-report.md) §19.9 put it beside placement inference, and the splitter still
> understands one topology (it refuses anything else by name rather than mis-slicing it). Native
> codegen is still not done, unchanged from Phase 1. And **rendering placement is not a decision
> yet**: Mode B does not exist, so the cost model is ready for the Mode A/B choice and does not make
> it ([`20`](20-phase-2-report.md) §20.4 item 1). §20.5 has the rest.
>
> *The slicer was built in Phase 3 — [`23`](23-general-slicer-report.md). The parenthesis above is
> also corrected there: it did **not** refuse everything it could not slice. A program with two
> `durable` folds was accepted and sliced with both folds reading one accumulator (§23.2).*

The moat.

- Effect rows, inference, effect polymorphism (§3.2).
- Placement *verification* against effects (reject `@on(client)` + `durable`; reject impure folds —
  the determinism rule that makes replay exact, §3.7) — valuable on its own.
- Placement *inference* + the cost solver, with determinism and stability guarantees (§3.4).
- `beck explain place` / `flow` / `wire` (§4.7).
- `secret[T]`, `Sendable`, capability effects; the §3.5 security property suite as executable tests.
- `.becki` published signatures and separate compilation (§3.6) — **do not slip this**; it is the
  historical failure mode (§1.6).
- Boundary versioning + `beck check --wire-compat` (§4.3).

**Exit**: on a corpus of 20+ programs, placement is inferred with no annotations for the common cases;
every §3.5 property is a passing test; a 3-module project rebuilds incrementally without recompiling
dependencies whose signatures didn't change.
**Met** ([`20`](20-phase-2-report.md) §20.3).

## Phase 3 — Make it real for developers (4–5 months) — **STARTED**

> Four of Phase 3's bullets are built, each with its own reports: `test` blocks, the general
> slicer — Phase 2's debt, delivered here rather than listed below — the language's own means of
> abstraction, and now **`Result` and error rows**, which is the first half of the
> structured-concurrency-and-errors bullet and the piece §8.5.3 trap 2 says the standard library
> may not be written before ([`27`](27-the-walls-come-down-report.md)). A fifth, incremental views, has its
> engine and its shared dataflow but not its read models, its pgwire exposure or its fusion. A
> sixth, the expressiveness suite, has started and runs two chapters. Of the fourteen below, eight
> are untouched.
>
> *That paragraph is the count as those three reports were written. The incremental-views bullet is
> now complete ([`88`](88-read-models-and-pgwire-report.md),
> [`89`](89-query-fusion-report.md)), and the reports since
> have moved several others; the bullet list below carries the current status of each and this
> paragraph carries the arithmetic of the day it was written, which is what a report does.*
>
> Alongside them, **Wave 0 is built** ([`44`](44-wave-0-report.md)): the set §8.5.4 described as
> overdue, a one-way door, or a gate on something already shippable. It is not a Phase 3 bullet and
> does not appear in the list below, because most of it is debt the phase list never carried — a
> recursion bound, an injected clock, a threat model, a disclosure policy, an identifier profile,
> and the two syntax decisions [`09`](09-risks-and-open-questions.md) §9.6 had been holding since
> the design documents were written.
>
> *Twelve was the count when [`22`](22-phase-3-report.md), [`23`](23-general-slicer-report.md) and
> [`24`](24-incremental-views-report.md) were written, and all three count against it. It is
> **fourteen** now: D18 adds the expressiveness suite, and
> [`25`](25-benchmarks-and-expressiveness.md) §25.6 found that the language's own means of
> abstraction — recursive types, user-written polymorphism, tail calls — had never been exercised
> by anything and had to be named as work rather than assumed. The reports are history and keep
> their arithmetic; this list keeps its accuracy.*
>
> **`test` blocks and inferred mocks**, with [`22`](22-phase-3-report.md) as its evidence. `beck
> test` runs a program's own tests — a log, a command and an expectation — through the same roles
> the runtime drives, with no network, no fixture and no mock written by hand; §21.3's "everything
> is stubbed by default, and the default says what it did" is built, and so is the one type-directed
> generator that stubs and `property` blocks share.
>
> **The general slicer**, with [`23`](23-general-slicer-report.md) as its evidence — the debt
> [`19`](19-phase-1-report.md) §19.9 assigned to Phase 2 and [`20`](20-phase-2-report.md) §20.5
> recorded as undelivered. The signal graph is now built as a graph rather than recognised as a
> shape: any number of `durable` folds (fused into one accumulator, because §3.7 fixes one log per
> application), any depth and any sharing above them, a `filter_map` on the fold path, and every
> tier crossing enumerated with the id §4.3 says a subscription is keyed by. It is a **precondition**
> for the incremental-views bullet below, not a down payment on it: views are still full recompute
> per event. What it did make possible immediately is `beck explain incremental` — the analysis
> §3.8 names, which says which views a plan could maintain and why the rest could not, over a
> program none of whose views are maintained (§23.8).
>
> Building it falsified the sentence both earlier reports used to justify the narrowness. The old
> splitter did not refuse what it could not slice: two `durable` folds compiled, and both were
> lowered to the same accumulator ([`23`](23-general-slicer-report.md) §23.2).
>
> **The incremental view engine**, with [`24`](24-incremental-views-report.md) as its evidence — the
> bullet the slicer unblocked. A view is compiled into a dataflow of operators and maintained from
> the change rather than recomputed: `remaining` updates by ±1 per event over a collection of any
> size, the `for` loop of a `ui:` block re-renders one row rather than all of them, and everything
> the decomposition cannot enter falls back to a full recompute of that operator, so a program the
> analysis does not understand is slow and never wrong. Recompute is now the **oracle**: every
> corpus program, every event of a generated log, maintained page against recomputed page, byte for
> byte.
>
> It is the engine and not the bullet. Per event the delta work does not grow with the collection,
> but assembling the page's children still does, so the measured end-to-end win is a 3–5× constant
> factor rather than a change of asymptote; and a maintained subscription costs about four times the
> memory it already held for its page.
>
> **The shared dataflow**, with [`26`](26-arrangement-sharing-report.md) as its evidence — §5.3's
> "a thousand connected users … must compile to *one* shared dataflow", which
> [`24`](24-incremental-views-report.md) §24.7 identified per operator and held once per subscriber.
> The operators that do not read the session now live in one dataflow the application holds,
> advanced by the first subscriber to render at a new version rather than by the sequencer, with a
> bounded history of what moved so a subscriber woken late updates by delta and one woken very late
> rebuilds. 64 subscribers over 11 versions advance it 11 times, and the counter is the test.
>
> What it is worth is a property of the program and not of the feature, so the report gives both
> ends: 256 subscribers of a public feed do **55× less work per event** and hold 4.3× less; 256
> subscribers of the todo sketch, which filters by the session immediately below the accumulator, do
> 1.3× less. Where a program reads the session is what decides its fanout cost, and `beck explain
> incremental` is where a developer can see which side of that cut each operator is on. §5.3's
> per-session memory is exported for the first time, in entries rather than bytes and split into the
> half paid once and the half paid per connection. The SQL read models, pgwire and query fusion in
> the bullet below are still untouched.
>
> *All three are built: the read models and pgwire in
> [`88`](88-read-models-and-pgwire-report.md), on the same cut this paragraph is about — a table is
> a view that does not depend on who is asking — and query fusion in
> [`89`](89-query-fusion-report.md), where that same cut turned out to be what **refuses** a
> rewrite rather than what enables one.*
>
> The nine other bullets are **untouched**, and [`26`](26-arrangement-sharing-report.md) §26.9 names
> them one at a time rather than by omission. Of the two added since (§8.4), the
> means-of-abstraction bullet is **done** — all six walls
> ([`27`](27-the-walls-come-down-report.md), [`27`](27-the-walls-come-down-report.md),
> [`27`](27-the-walls-come-down-report.md)), and the three that removing them wrote
> ([`27`](27-the-walls-come-down-report.md),
> [`27`](27-the-walls-come-down-report.md), [`27`](27-the-walls-come-down-report.md)), with traits
> across [`27`](27-the-walls-come-down-report.md), [`27`](27-the-walls-come-down-report.md) and
> [`27`](27-the-walls-come-down-report.md) — and the expressiveness suite runs two chapters of
> SICP against the book's own answers.
>
> Removing the six walls wrote three more, and all three came down too: a `list[T]` may be taken
> apart ([`27`](27-the-walls-come-down-report.md)), a `model`, a `union`, a
> `newtype` and a `type` may take a type parameter ([`27`](27-the-walls-come-down-report.md)), and
> `+` reaches a type the compiler does not know about
> ([`27`](27-the-walls-come-down-report.md)). The last refusal file said **traits**, and four reports
> answered it: declarations, impls, coherence and static dispatch in
> [`27`](27-the-walls-come-down-report.md), bounds and dictionary passing in [`27`](27-the-walls-come-down-report.md), the
> module boundary in [`27`](27-the-walls-come-down-report.md), and the operators in
> [`27`](27-the-walls-come-down-report.md) — which gives §2.1.1 its exact rationals and leaves
> `sicp/refusals/` **empty**. That is the narrow claim that every wall this project has *found* has
> been removed, not that Beck expresses SICP, and `sicp/refusals/README.md` is where the difference
> is written down. The standard-library bullet has no remaining blocker and is work rather than
> design; what the expressiveness suite needs next is more of the book, and chapter 3 is state and
> time, which is the part of SICP closest to what Beck is for.

- LLVM release backend + differential tests against Cranelift (§5.2). *The LLVM half is **built**
  over the scalar subset ([`93`](93-llvm-backend-report.md)), and the differential is against the
  **evaluator** rather than against Cranelift, because Cranelift is still not built.*
- **Incremental views**: compile subscribed/materialized views to differential-dataflow plans with
  arrangement sharing (per-session fanout, §5.3); recompute stays as the CI oracle; SQL read models
  + pgwire exposure; query fusion on symbolic plans. *The plans and the oracle are **built**
  ([`24`](24-incremental-views-report.md)); so is arrangement sharing between subscribers
  ([`26`](26-arrangement-sharing-report.md)) and its lifecycle
  ([`51`](51-arrangement-lifecycle-report.md)). **The read models and pgwire are built too**
  ([`88`](88-read-models-and-pgwire-report.md)) — and not as this line assumed: a read model is the
  arrangement projected as relations rather than a second copy written into Postgres
  ([`10`](10-decisions.md) D26), because a durable projection puts view maintenance back on the
  write path §26.2 argued it off. What decides which signals are tables is the same `per_session`
  cut the fanout uses. **Query fusion is built too** ([`89`](89-query-fusion-report.md)), with
  `beck explain query` and `beck explain cost` behind it — five local rewrites on the dataflow
  plan, refused where the operator they would absorb is read twice, is named by a signal, or is
  shared while its consumer is per session, which makes this **bullet complete**.*
- **Mode B client**: per-component WASM (view + fold + signal kernel), optimistic application with
  `seq` reconciliation, freshness-typed pending state; size budget CI gate (< 150 KB brotli per
  component bundle). — ***Built*** ([`94`](94-mode-b-report.md)): `@render(client)`, a bundle that
  is the component's slice, a `wasm32` kernel, data patches instead of DOM patches, and
  reconciliation by `seq`. Chromium runs all of it (§94.12), and a tab survives a cold start with
  the server switched off (§94.13). An interaction is measured rather than asserted (§94.14):
  13 ms on a thousand-card board, 97% of it `view`, and growing with the board rather than with the
  change. **Not** codegen ([`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md)) — which
  §94.14 finds is not what the 13 ms is — not freshness-typed, and no size-budget gate; §94.8 is
  the list. ***The last two of those are built***
  ([`102`](102-freshness-and-the-budget-report.md)): `freshness()` is §3.7's dimension as a signal
  source, so a page renders "saving…" from `Confirmed | Pending(n)` and stops when the state that
  confirms it arrives — carrying the refusal that points the other way from every rule Mode B has
  had so far, since a **server** cannot answer it (`B0518`). And the budget is a gate: `beck bundle`
  writes the artefact, CI weighs every Mode B example against 150 KB brotli, and a shape gate says
  what a threshold with eighty times its headroom cannot — that a bundle is a function of the
  component's slice rather than of the program around it. What is left of this bullet is **codegen
  alone**, which [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) defers and §8.5.5's Lane E
  schedules behind a heap.
- Client polish for both modes: router, forms, lazy routes, focus/scroll preservation, devtools
  extension showing signal graph, patch traffic and pending state. — ***Built, except lazy routes***
  ([`100`](100-client-polish-report.md)). **A route is a field of `Session`**: `view(state, session)`
  reads `session.path`, so there is no route table, no matcher and no route form — every route is a
  real URL the server renders directly, and a link is an ordinary `<a href>`. Forms are `on_submit`
  with a `$field:name` hole; the caret and the scroll survive a patch that rebuilds the page, at a
  cost proportional to the patch; and the panel shows all three of the things this line names,
  served as a page rather than shipped as an extension (§100.6 says why). What putting the route on
  the session found is the report's subject: `Session` had been one word for **who** is asking and
  **where** they are, and `B0514` was refusing every routed page in Mode B for a reason that is
  about identity — so eligibility now asks which fields the view can observe, and stopped being the
  same answer as §5.3's fanout. **Lazy routes are not built**, and §100.7 is the ordering rather than
  the excuse: a program has one component, so there is nothing to be lazy about until §5.1's
  per-component boundary exists in the language.
- **`test` blocks and inferred mocks** ([`21`](21-tests-in-beck-and-proof.md) §21.2–§21.3) —
  **BUILT** ([`22`](22-phase-3-report.md)): a test is a log, a command and an expectation, so
  cross-boundary tests need no network and no fixtures; stubs attach to *effect atoms* rather than
  to interfaces, so "any value" is the default and has no syntax. Depends on one type-directed value
  generator, which `property` blocks share. This is the first thing an outside developer will reach
  for, and Phase 2 shipped with no way for them to write a single test about their own program.
  `beck test --update` for page snapshots is the part that did not ship.
- Structured concurrency, `Result`/error rows, `match` exhaustiveness, pattern matching completion.
  *`Result`/error rows are **built** ([`27`](27-the-walls-come-down-report.md)): failure is the effect atom
  `raises(E)`, `try:` is the handler that reifies it, and row aliases arrived with them. Nothing was
  added to the effect system to make that work, which is the point — inference, the `uses` bound,
  `.becki` and `--wire-compat` all applied to failure unchanged. Structured concurrency is
  **built** ([`80`](80-a-scope-owns-its-children-report.md)): `parallel:` is a scope whose
  bindings are its children, and what it lacks is a backend that runs two of them at once
  (§80.5). `match` exhaustiveness is built for unions ([`27`](27-the-walls-come-down-report.md)
  added lists). **Pattern matching nests** ([`90`](90-nested-patterns-report.md)):
  `case Some(Circle(r))`, a literal or a constructor wherever a binder goes, through a type
  parameter and inside a list — and the exhaustiveness check was rebuilt for it, because a set of
  variant names cannot answer a pattern that names a variant and covers part of it. Unreachable
  arms come with it, as a warning. **Guards and or-patterns are built too**
  ([`91`](91-guards-and-alternatives-report.md)) — `case Circle(r) | Square(r):` and
  `case x if x < 0:`, neither of which needed a new algorithm, because an or-pattern is several
  rows of the same matrix and a guarded arm is no row — and `@` bindings with them. Pattern
  matching is **done**; what is left of this bullet is **structured concurrency's missing backend**
  ([`80`](80-a-scope-owns-its-children-report.md) §80.5), and §91.5 lists what is not there.*
- **SQLite as a durable substrate** ([`07`](07-dependencies.md) §7.8.1): a `LogStore`
  implementation beside redb and Postgres. The reason is not speed — the measurements say the
  durable substrates are within ~16% of each other — it is that SQLite is *also* the read-model
  engine, so rungs 0–2 get the same "append and project in one transaction" property production
  has, and a developer's laptop stops being merely similar to production. Measure with `beck bench
  log` and let the number pick rung 0's default. ***Built*** ([`67`](67-sqlite-report.md)), and the
  number does **not** pick: at equal durability SQLite and redb are within noise of each other, so
  rung 0's default is unchanged. The first measurement said 26× and was comparing
  `synchronous = NORMAL` against redb's fsync — a weaker promise rather than a faster engine — which
  is why durability is now a public type defaulting to fsync (§67.3,
  [`adr/0017`](adr/0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md)). The reason
  §7.8.1 gave survives the measurement and is the only reason: the transaction, not the speed.*
- Standard library v1: collections, strings, time, money/decimal, HTTP client, JSON, UUID, crypto
  primitives (delegated to `ring`/`aws-lc-rs`, not hand-rolled). **Reals first**, because §25.6
  measures that §1.1.7 of SICP — the first substantial program in the book — does not typecheck
  without them. *Reals are **built** ([`27`](27-the-walls-come-down-report.md)): §1.1.7
  runs and reproduces the doubles the book prints. Exact rationals and bignums are not, and §2.1.1
  of SICP is about the first of those rather than about reals. Collections, strings, time,
  money/decimal, HTTP, JSON, UUID and crypto are untouched. A fold over a list *can* now be written
  ([`27`](27-the-walls-come-down-report.md)) — and so can a library whose callers
  do not inherit each other's effects, which §27.1 records as the precondition nobody had listed.
  Exact rationals are now expressible as a user's type rather than a compiler's, because `+`
  resolves through the prelude trait `Num` ([`27`](27-the-walls-come-down-report.md)) — which is a
  mechanism the standard library can be built on and not a standard library. Bignums are still not
  built, and neither is coercion between numeric types.*
  ***The first half is now built** ([`46`](46-standard-library-report.md)): strings, list and map
  collections, JSON and time as thirty-one primitives, plus `compiler/lib/` — the half written in
  Beck, three libraries with their own tests. HTTP, crypto, decimal, bignums and numeric coercion
  are untouched, and §46.6 is the item-by-item list. The `Num` mechanism turned out not to be
  enough for money — §46.5 is the wall — and [`27`](27-the-walls-come-down-report.md) took it
  down: an impl may now be more effectful than its trait, so `Money` has its operator.*
- **The language's own means of abstraction, which four phases had never been pointed at**
  ([`25`](25-benchmarks-and-expressiveness.md) §25.6, measured; §25.7 orders them). Every corpus
  program is shaped like the todo sketch, which is why none of these had surfaced. ***All six are
  built.** Running a module with no merge point, recursive and forward-referencing types, and the
  `B0320` row-unification defect that refused an `if` over two function values
  ([`27`](27-the-walls-come-down-report.md)); proper tail calls, with the bounded-depth diagnostic **as well as**
  rather than instead of them, so no Beck program aborts its own process any more
  ([`27`](27-the-walls-come-down-report.md)); and reals plus user-written polymorphic definitions
  ([`27`](27-the-walls-come-down-report.md)). Everything that stood in their place is
  built too: effect polymorphism for a user's higher-order definition and a way to take a `list[T]`
  apart ([`27`](27-the-walls-come-down-report.md)), type parameters on a `model`,
  a `union`, a `newtype` and a `type` ([`27`](27-the-walls-come-down-report.md)), and traits —
  declarations and impls ([`27`](27-the-walls-come-down-report.md)), bounds
  ([`27`](27-the-walls-come-down-report.md)), the `.becki` boundary
  ([`27`](27-the-walls-come-down-report.md)) and the operators
  ([`27`](27-the-walls-come-down-report.md)). This bullet is **done**; what the suite below needs
  next is more of the book rather than more of the language.*
- **The expressiveness suite** ([`25`](25-benchmarks-and-expressiveness.md) §25.5, D18): SICP
  stage 1, and Felleisen's criterion answered for the special forms the book introduces. It needs
  macros and nothing else, so it starts now and does not wait on the bullet above. *Chapter 1 is
  complete and chapter 2 reaches §2.2's closure property ([`27`](27-the-walls-come-down-report.md)); both run as
  **libraries**, with no application wrapped around them. Chapter 1 also carries §1.2.1's *property*
  — an iterative process running a quarter of a million levels deep — now that there is one to carry
  ([`27`](27-the-walls-come-down-report.md)), and §1.1.7/§1.3.3/§1.3.4's reals asserted against the doubles
  the book prints, while chapter 2 carries §2.2.1's `map` written by the reader rather than borrowed
  from the prelude ([`27`](27-the-walls-come-down-report.md)). **Felleisen's table is
  written** ([`63`](63-felleisen-report.md)): six of the seven forms recovered and `amb` conceded,
  which is the shape §25.9's forecast predicted, with the CPS reorganisation the concession costs
  written out beside it rather than asserted.*
- **Identity**: OIDC relying-party runtime, `identity = managed()` provisioning (Keycloak/Ory),
  claims → `Session` capability mapping, dev-mode identity for rung 0, presence as a first-class
  signal ([`10`](10-decisions.md) D6). *The **seam** is built ([`48`](48-identity-report.md)):
  dev-mode identity is named rather than implied, a verifying provider exists, and both edges
  refuse before rendering — so an ownership check compares against something the caller did not
  choose. The **relying party** and the **claims mapping** are built too
  ([`95`](95-oidc-relying-party-report.md)): discovery, a cached JWKS, RS/PS/ES signatures, every
  claim check, the authorization-code flow with PKCE, and `Session.claims`. What is left of this
  bullet was presence, and it is **built** too ([`96`](96-presence-report.md)): a source in the
  signal graph performing `cap.presence`, refused to the chokepoint and to a Mode B page, and
  bounded because it is keyed by a name the client chooses. D6's
  language surface is built as written, both forms —
  `identity = external(issuer=…)` is a declaration, so §6.5's egress rule covers the issuer like any
  other peer (§95.7).*
- ~~LSP: completion, hover with *inferred placement*, go-to-def, rename, inline diagnostics.~~
  ***Built, every entry.*** Hover, go-to-def and inline diagnostics by [`65`](65-lsp-report.md);
  completion and semantic-token highlighting by [`103`](103-playground-phase-3-report.md) §103.1 —
  which also moved every one of those answers into `beck_core::editor`, so the playground's editor
  and an editor's are the same answers rather than two implementations of them; **rename**, with
  references, document highlight and inlay hints, by
  [`110`](110-the-editor-edits-report.md). Placement is shown twice over: on hover as `@on(tier)`,
  and as an inlay hint carrying the annotation the source did *not* write, which is §3.4's solved
  constraint made visible where it would be written down.
- **The playground** ([`17`](17-playground.md)) — highest-leverage adoption artefact: rung A
  (compile-time, static) and rung B (the whole app in the tab — the worker-server is the rung-0
  platform compiled to WASM, riding Mode B's kernel work; `seq` scrubber and two-client demos).
  *Both **built** ([`98`](98-playground-report.md)): `beck play` serves a page whose right-hand side
  is eleven `beck explain` answers derived in the browser, and whose lower half is the program
  running — one log, one fold, two client iframes and the scrubber. This row's forecast that rung B
  would ride Mode B's kernel work is the one thing it got wrong (§98.9): a tab server is a
  sequencer, a log and a differ, and none of those are in `beck-wasm`. What it rode was a division
  of the runtime — `beck-host`, the half of `beck-rt` that is program-shaped rather than
  machine-shaped. Rung C is Phase 4's, and §98.7's four remaining lacks — no IndexedDB, no
  content-addressed sharing, no Mode B in the tab, and a `<textarea>` where the LSP already exists —
  are **built** by [`103`](103-playground-phase-3-report.md): the log survives a reload as the
  records a durable store writes, a share link is a fragment that carries the program and names its
  digest, a `@render(client)` program runs in the client iframe in Mode B's kernel, and the editor
  highlights, completes and squiggles from the same module `beck lsp` answers from. What §103.6
  still says is not built is rung C, the playground being written in Beck, and a link short enough
  to need a registry.*
- `beck init ci`, apko image build in-process, cosign signing, SBOM. *All four are built. The
  **SBOM** ([`92`](92-sbom-report.md)): `beck sbom` emits CycloneDX 1.6 and `beck build` writes one
  beside the manifests, derived from the same object graph the image config is — so the package list
  and the apko `packages:` block cannot disagree, and a test parses the rendered YAML back to say
  so. It can exist before a release pipeline because §6.2's "no arbitrary execution" means the
  image's contents are already a list rather than something to scan for. The other three
  ([`99`](99-supply-chain-report.md)): **`beck image`** assembles an OCI image in one process —
  resolve against the Wolfi index, fetch, unpack, add the toolchain and the program, write a layout
  — with no apko, no melange and no daemon, because §92.1's argument spends a second time (a build
  that executes nothing has nothing in it a compiler cannot do). **`beck sign`/`beck verify`**
  produce and check a Sigstore signature over the manifest digest, in the shape
  `cosign verify --key` reads, and `openssl` verifies it rather than only this project's own code.
  **`beck init ci`** writes §28.3's workflow. What is left is not a piece of this bullet but the
  pipeline around it: **no registry push**, ~~**no provenance attestation**~~ — **built for the
  compiler's own release** ([`109`](109-provenance-report.md)), which is where SLSA's build track
  gets its builder identity and its transparency log; a user's `beck build` still attests nothing —
  and **no pinned package versions** (§99.7),
  which is why an image is reproducible twice over and not across weeks. **Package signatures are
  not verified** (§99.7), and that is named as the largest security gap rather than as a detail.*

**Exit**: an outside developer builds a non-trivial app from documentation alone, without asking the
team a question. Track this literally as the acceptance test.
**Not met.** Of the fourteen bullets: **nine built outright** — `test` blocks, the SQLite
substrate, the standard library, the language's own means of abstraction, the LSP, the
expressiveness suite (three chapters of SICP and the Felleisen table, with chapters 4–5 belonging
to Phase 5), incremental views, whose last part is
[`89`](89-query-fusion-report.md)'s fusion, **identity**, whose last part is
[`96`](96-presence-report.md)'s presence, and the supply-chain tooling
([`92`](92-sbom-report.md), [`99`](99-supply-chain-report.md)) — whose remainder was a release
pipeline rather than a piece of the bullet, and **that pipeline is now built**
([`104`](104-the-release-and-the-installer-report.md)), with an installer in front of it and no tag
pushed through it — and it now attests build provenance over what it publishes
([`109`](109-provenance-report.md)). What is still missing there is a signature a consumer can check
over the release *listing*: `beck sign`'s subject is an image manifest, and a release publishes
tarballs (§104.6).
**No bullet has a named remainder**: the concurrency-and-errors bullet has `Result`, error rows,
`parallel:` and pattern matching with nesting, guards and alternatives
([`90`](90-nested-patterns-report.md), [`91`](91-guards-and-alternatives-report.md)); what
`parallel:` lacks is a backend that runs two children at once, which is
[`80`](80-a-scope-owns-its-children-report.md) §80.5's item rather than this bullet's. **Three
half-built**: the codegen bullet has **both** of §5.2's code generators
([`93`](93-llvm-backend-report.md), [`97`](97-cranelift-report.md)) and a heap for the *algebraic*
half of what the language stores ([`101`](101-the-heap-report.md)) plus **text**
([`105`](105-text-on-the-heap-report.md)) and **reading a collection**
([`106`](106-lists-arrive-read-only-report.md), [`107`](107-a-map-arrives-read-only-report.md)) — a
record, a union, a newtype, a `Str`, a `list[T]` and a `Map[K, V]` compile, and a closure, every
effect, `Html` and the loop that *grows* a collection do not; Mode B has the mode, the bundle,
the data patch, the reconciliation, a browser that runs it and an offline queue, without codegen —
which waits on the half of that heap that is not built — and it is the **collection** half rather
than the text half, since a page is a tree of children
([`94`](94-mode-b-report.md) §94.8, [`105`](105-text-on-the-heap-report.md) §105.10); and the
playground has rungs A and B and not rung C, which is Phase 4's
([`98`](98-playground-report.md) §98.7). **Client polish is built too**
([`100`](100-client-polish-report.md)) except for lazy routes, which wait on the same
per-component boundary Mode B does, so **nothing on the list is untouched**. The criterion is not a
count of those, though — it is a
claim about a *person*, and the honest way to say how close it is is by the questions such a
developer would ask in order:

| Their question | The answer today |
|---|---|
| "How do I test this?" | A command, since [`22`](22-phase-3-report.md) |
| "Will this recount a million rows every time somebody clicks?" | A command *and* a number ([`24`](24-incremental-views-report.md), [`26`](26-arrangement-sharing-report.md)) |
| "Can I write my own abstractions, or only the ones the todo sketch needed?" | Ten walls down and an empty `sicp/refusals/` |
| "How do I say something failed?" | `raise` and `try:`, and the signature says so whether or not I wrote it down ([`27`](27-the-walls-come-down-report.md)) |
| "Is there a string library? A JSON parser?" | Yes, and `compiler/lib/` shows how to write the next one ([`46`](46-standard-library-report.md)) |
| "Can I trust the actor in my ownership check?" | With a verifying provider, yes ([`48`](48-identity-report.md)); against a real identity provider, yes ([`95`](95-oidc-relying-party-report.md)) — and `session.claims` says what they may do. The default still believes the client, and says so |
| "Can my DBA see the data?" | `psql` against the read models ([`88`](88-read-models-and-pgwire-report.md)) — one table per collection, derived, no annotation |
| "Where's the tutorial?" | [`86`](86-getting-started.md), published on the site since [`88`](88-read-models-and-pgwire-report.md) §88.8, and every program in it compiled and run by a test |
| "How do I get the compiler?" | One command, since [`104`](104-the-release-and-the-installer-report.md) — and it has nothing to download until a tag is pushed, so today the answer is still "build it", which §86.1 now says in that order |

**That last row has moved, and the criterion has not.** It measures a *person* — an outside
developer building a non-trivial app without asking a question — and what the guide changes is that
the answer to "from what?" is no longer "there is nothing"; §86.8 is the list of what it does not
cover. Every other row above is a prerequisite for a tutorial being worth writing, and §8.3 item 6
— "write the tutorial as you build, and treat any sentence that requires an apology as a bug report
against the design" — is the practice this phase has least honoured. The apologies it would
currently need are shorter than they were and still enumerable — and the list this paragraph carried
has been overtaken twice. ~~No OIDC~~ ([`95`](95-oidc-relying-party-report.md)), ~~no Mode B~~
([`94`](94-mode-b-report.md)), ~~no installation story~~
([`104`](104-the-release-and-the-installer-report.md)), **no released binary** — which is now one
`git tag` rather than a piece of missing work, because the pipeline that would build it exists and
has never run ([`104`](104-the-release-and-the-installer-report.md) §104.7). What is left to
apologise for is the one an outside developer meets in week two rather than minute one: a record, a
union and a newtype compile ([`101`](101-the-heap-report.md)) and **text, collections, closures and
every effect still walk**, so a program that does anything with a `Str` is running on the
tree-walker.

## Phase 4 — Production readiness (4–5 months)

- Beck operator: the deploy-rides-the-stream choreography — quiesce, drain, snapshot, `migrate`/
  `upcast`, resume (§6.4); canaries via Gateway API + Argo Rollouts; wire-compat gate; `beck
  status` with source provenance.
- Replay tooling as product: `beck replay`, `beck fork --from prod --at <time>` (privileged,
  audited, default-redacted — F4), log-backed property tests; structural shredding worked
  end-to-end per D14 (log, snapshots, read models, backups).
- Package system UX ([`16`](16-packages-and-ecosystem.md)): `beck add`/`publish`/`why` with the
  effect-diff prompt, namespaces, the Mere (index + generated docs site) + transparency log.
- `process` (sagas) and non-durable folds hardened; internal fan-out fabric (NATS) when
  multi-node demands it ([`15`](15-scale-and-distribution.md)).
- Playground rung C ([`17`](17-playground.md) §17.3): ephemeral TTL'd cloud environments with the
  compiler-as-first-sandbox effect budget — an operator workload, built alongside the operator.
- Effect-derived NetworkPolicy/RBAC/DB grants (§6.5) — the platform-team sales pitch.
- Crossplane emitter for managed Postgres/buckets/DNS; OpenTofu escape hatch; `import infra`.
- OpenTelemetry cross-tier tracing on by default; `beck tune` right-sizing.
- Multi-arch images; air-gapped install; OCI package registry via ORAS (§6.7).
- **FFI**: C ABI both directions; JS interop for the client tier; a Python bridge (§9.2) — the
  ecosystem-access question is existential, so give it real headcount.
- Security review: hygiene escapes, macro sandbox, deserialisation of untrusted wire data, generated
  SQL, the `Sendable`/`secret` proofs. Get external eyes on this.

**Exit**: one real production application running on it, with an on-call rotation and a postmortem
or two — per [`10`](10-decisions.md) D15 this is **beck.dev**, with the registry entering
production hardening.

## Phase 5 — 1.0 (3–4 months)

- Language specification written against the S-expression core (this is where that surface earns its
  keep, §2.2).
- Stability guarantees and a deprecation policy; wire-format stability commitment.
- **CRDT-valued types**: `Text` and friends (automerge/loro-backed) — concurrent edits merge within
  the value while the log still orders the updates around it ([`10`](10-decisions.md) D7).
- **FoundationDB as a durable substrate**, with §15's partitioned logs
  ([`07`](07-dependencies.md) §7.8.1): ordered keys *are* the `seq` abstraction, so the scaling
  ladder does not need a second log design. It arrives here rather than earlier because a single
  application's single writer is not the bottleneck until §15's partitioning is real, and because
  its costs — chunking above 100 KB values, a 5 s transaction bound, an operationally heavy
  cluster — buy nothing until then.
- Editor support beyond VS Code; debugger integration (DAP) with cross-tier stepping.
- Package ecosystem seeding; documentation, book, tutorials, and 5–10 non-trivial example apps.
- **The registry in production, on Beck, serving real packages** — the D15 exit criterion: the
  flagship dogfood (event-sourced by domain, saga-driven publish pipeline, counter folds, genesis
  replay as an operational tool) proving the backend and data tier in public.
- Performance: a published benchmark suite versus a hand-written React+FastAPI+Postgres+Helm baseline —
  latency, payload size, image size, build time, *and* lines of code. That last number is the one that
  travels. **Published on third-party suites, not only on ours** — TechEmpower, js-framework-benchmark,
  Are We Fast Yet — per §8.4, whose harnesses have been running since Phase 3 so the numbers arrive
  with a history rather than as a launch claim.
- **The expressiveness result** ([`25`](25-benchmarks-and-expressiveness.md) §25.5): the SICP suite
  through chapter 4, and Felleisen's criterion answered — every special form the book introduces
  either recovered as a Beck macro or recorded as requiring a global reorganisation. This is the
  premise of [`01`](01-vision-and-premise.md) §1.1 and [`10`](10-decisions.md) D9, either cashed or
  conceded, and it belongs beside the language specification because it is the same claim.

## 8.1 Milestone dependency graph

```
P0 prove ──▶ P1 skeleton ──┬─▶ P2 effects+placement ──┬─▶ P4 production ──▶ P5 1.0
                           │                          │
                           └─▶ P3 developer UX ───────┘
                                    │
                                    └─▶ playground (adoption flywheel, can ship early)
```

P2 and P3 can overlap with separate owners; P4 needs both. The playground can and should ship as soon
as P2's `explain` output exists — a page that shows source on the left and *generated SQL + generated
Kubernetes objects + inferred placement* on the right is the demo that explains this project in
15 seconds. **One predecessor was added to that after this graph was drawn**: the playground compiles
source written by strangers, and §8.5's first item is what makes that safe.

This graph is between phases. The ordering *within* the current phase — which of Phase 3's remaining
bullets is next, which have predecessors the list above does not show, and which pairs can be built
on two branches at once — is §8.5.

## 8.2 Team shape

| Role | Count | Focus |
|---|---|---|
| Language/type-system engineer | 1–2 | §2, §3 — the front end and effect system |
| Compiler backend engineer | 1 | §5.2 codegen, WASM, optimisation, size budget |
| Data-tier engineer | 1 | §3.7–3.9, §5.3 — log engine, folds, incremental views, migrations |
| Platform/Kubernetes engineer | 1 | §6 — images, operator, policy generation |
| Developer-experience engineer | 1 | errors, LSP, playground, docs — **not** a junior role; this is where adoption is won or lost |

## 8.3 Cross-cutting practices from day one

1. **Every phase ships a demo that runs** — and so must the thing that runs it. Phase 2 found that
   the Phase 1 CI workflow had been invalid YAML from the day it was written, so every gate in it was
   silently absent for a whole phase ([`20`](20-phase-2-report.md) §20.4 item 8). A workflow is an
   artefact, and [`19`](19-phase-1-report.md) §19.4 item 10 already said what that means: an artefact
   nobody has executed is a design document. Run the gates by hand once, at the start.
2. **The differential harness (§4.8) is the project's conscience.** It is the mechanised statement of
   the central promise; keep it green.
3. **Error-message snapshots in CI** from week two (§4.5).
4. **Size and latency budgets are CI gates**, not aspirations: client payload, image size, cold start,
   incremental build time, keystroke→diagnostic. Those are *our* yardsticks; §8.4 schedules the ones
   somebody else chose, on the rule that a harness is stood up a phase before its number is
   publishable.
5. **`cargo-deny` licence gate** from the first commit, given §7's open-source constraint.
6. **Write the tutorial as you build**, and treat any sentence that requires an apology ("for now you
   have to…") as a bug report against the design.
7. **Dogfood, per D15.** The playground, beck.dev, and above all the package registry are built in
   Beck — the registry's domain is a homomorphism of the semantics (immutable versions = events
   forever; transparency log = the log; yank = an event), so building it tests exactly the backend
   and data tier claims, in production, in public.

## 8.4 Benchmarks and expressiveness, by phase

[`25`](25-benchmarks-and-expressiveness.md) answers two questions — which third-party performance
suites apply, and how the expressiveness premise gets falsified — and D18 adopts both. The plan for
them is one table rather than bullets scattered through five phases, because the sequencing rule
matters more than any individual suite:

> **Stand every harness up one phase before its number is publishable.** A suite acquired after the
> thing it measures gets good has no history and therefore no regression-detecting power. Phase 0's
> measurements are worth more today than the day they were taken for exactly this reason
> ([`18`](18-phase-0-report.md)); a benchmark adopted at 1.0 to support a launch claim is worth
> nothing at all.

The consequence, stated so it cannot be quietly dropped: **the first numbers we publish will be
bad**, because §25.3 measures the tree-walking evaluator at roughly 33× CPython on `fib(30)` and
native codegen is unbuilt. Publishing them anyway is the point. *Native codegen is now built for
the scalar subset, and the first comparative number is
[`93`](93-llvm-backend-report.md) §93.5's — **against the evaluator**, not against another
language, because the benchmark suites run whole programs and a whole program still walks.*

| Phase | Stand up | Publish |
|---|---|---|
| **3** | The **expressiveness** work, which needs nothing that is not built: SICP stage 1 (chapter 1 complete) and the **Felleisen macro-expressibility table**. Macros are built and hygienic ([`19`](19-phase-1-report.md)), so this is independent of §25.7's six walls and is the cheapest item on this table. Also: the compile-speed budgets §13.7 already lists, on the rustc-perf model | Chapter 1's line-count comparison against the pinned Scheme baseline — the first honest number in either half of §24 |
| **3** (with the standard library and the LLVM backend) | **Are We Fast Yet** and **CLBG** harnesses, run against the evaluator | Nothing comparative. The interpreter-vs-Cranelift-vs-LLVM differential ([`13`](13-testing.md) §13.1) and the first honest compute number arrive together, and not before. *Both arrived in [`93`](93-llvm-backend-report.md), without the Cranelift term: the differential is `beck-cli/tests/native.rs` and the number is §93.5's. What is still not published is a comparison against **another language** — AWFY and CLBG run whole programs, and a whole program has records and lists in it, so it still walks* |
| **4** | **TechEmpower** (the five tests that map without argument; the two that assume update-in-place stated as run against a read model), **js-framework-benchmark** (three columns — Mode A at a stated RTT, Mode A at RTT 0, Mode B — never averaged), **YCSB** against the log, **Lighthouse/Core Web Vitals** as gates on the example apps. SICP stages 2–3. **The DDIA matrix** ([`15`](15-scale-and-distribution.md) §15.6) — beside the Jepsen and simulation work that discharges its rows, never before it, because a matrix written ahead of its tests is the table of intentions it exists not to be | The whole-system numbers, unflattering, with the methodology notes of §25.2 attached to each. This is [`01`](01-vision-and-premise.md) §1.5 item 3 measured by somebody else's harness rather than ours |
| **5** | **TPC-H/ClickBench** on read models once §5.3's engine exists; the incremental-view workload §25.2 records as having *no* standard, which we would be defining rather than borrowing. SICP stage 4 | The Phase 5 suite above, and the expressiveness result — including the rows §25.5 forecasts Beck will lose (§2.4–2.5 generic operations, chapter 4's evaluator), which are published or the exercise was not run honestly |

Two things this table deliberately does not do. It does not put **TPC-C** anywhere: it assumes
update-in-place OLTP, which is not Beck's data model, and entering it would be a claim we do not
make. And it does not treat the SICP suite's pass rate as a metric — §25.5's three registers
(translated / re-expressed / refused) are the result, and chapters 3.1–3.4 and 5 are expected to
land mostly in the last two.

## 8.5 What is next, in order

The phase lists above are **sets**, not sequences. Every bullet in Phase 3 is real work and none of
them says what to start on Monday, so this section supplies the order — and the parallelism that
order permits. It is the only place in `docs/` that holds a sequence: the reports end with "what is
still not" lists and the surveys ([`35`](35-standards-landscape.md) §35.5,
[`38`](38-literature-survey.md) §38.8, [`42`](42-security-assurance.md) §42.9) end with **adopt**
verdicts, and a verdict is not a schedule. This is where one acquires a position.

**The finding that motivates it.** [`14`](14-review-findings.md) F11 says deterministic simulation
cannot be retrofitted, and records the constraint: virtualize clock, network and disk from the first
line of runtime code, **Phase 1**. [`13`](13-testing.md) §13.4 restates it in bold as a hard
prerequisite. Its status is `FIXED (constraint recorded)` — and the runtime then called
`SystemTime::now()` directly anyway, for three phases
([`42`](42-security-assurance.md) §42.4). The decision was correct and written down twice. What it
never had was a **position in an order**, so nothing ever came due. A list of things to do
eventually is not a plan, and `DESIGNED` is not a schedule.

### 8.5.1 The four classes

Only two of these can be got wrong, which is what makes the ordering decidable.

| Class | Definition | Scheduling rule |
|---|---|---|
| **R — retrofit** | Cost rises with delay, sometimes discontinuously (a one-way door) | By the **date it becomes expensive**. These are the only items that can be *late* |
| **F — fan-out** | Modest cost, unblocks several others | By **how many successors it frees**. Late costs throughput, not rework |
| **G — gate** | Something already scheduled is unsafe or dishonest until this exists | **Immediately before what it gates**, never after |
| **S — free-standing** | Flat cost, no successors waiting | By **value and team shape** (§8.2). Order is genuinely free |

Most of the outstanding work is **S** — Cranelift and the rest of the codegen bullet, Mode B, the
LSP, SQL read models, query fusion — and S is where attention naturally goes, because it is where the interesting engineering
is. The items that are actually urgent are small, and several of them are prose.

The classification has just paid for itself once. Bounds were the project's textbook **F**: one
feature with six successors queued behind it ([`27`](27-the-walls-come-down-report.md) §27.1). Taking it first
carried two of those successors in behind it — the module boundary
([`27`](27-the-walls-come-down-report.md)) and the operators
([`27`](27-the-walls-come-down-report.md)) — and emptied `sicp/refusals/`. Three reports from one
feature, because it was the right feature. That is what taking the fan-out item first buys, and it
is the argument for reading the rest of this section.

### 8.5.2 The retrofit items, with the date each goes bad

| Item | Recorded in | Expensive from |
|---|---|---|
| ~~Virtualized clock~~, ~~then network~~, then disk | F11; [`13`](13-testing.md) §13.4 | **Two of three.** The clock is supplied rather than ambient ([`44`](44-wave-0-report.md) §44.3), with a test that counts the readers; the network is a seam with three implementations ([`49`](49-http-client-report.md) §49.5), and it arrived *with* the first thing that needed it rather than after. The **disk** is not, and elapsed time is not |
| ~~The two syntax decisions~~ | [`09`](09-risks-and-open-questions.md) §9.6 item 5 | **Taken** — [`10`](10-decisions.md) D21 and D22, inside the deadline |
| ~~Unicode version pinned per release + UTS #39 security profile~~ | [`35`](35-standards-landscape.md) §35.5 item 2 | **Done** ([`44`](44-wave-0-report.md) §44.5), with the bidirectional half the profile does not reach closed alongside it |
| ~~Errors as a row label, `Result` reified, lexical handlers~~ | [`38`](38-literature-survey.md) §38.4 | **Done** ([`27`](27-the-walls-come-down-report.md)), before the standard library's signatures rather than after — which is the one thing §8.5.3 trap 2 asked for |
| Trusted publishing on crates.io | [`42`](42-security-assurance.md) §42.7 | The **first** `cargo publish` — a long-lived token used once is a risk already taken |
| Diverse double-compiling / bootstrappability | [`42`](42-security-assurance.md) §42.7 | The first `beck` built by `beck` (Phase 5) |
| Deterministic libm (F9) | [`35`](35-standards-landscape.md) §35.5 item 1 | First transcendental **or** second backend |

### 8.5.3 The four traps

Where doing the right work in the wrong order costs the most.

1. **The playground before the parser's bound.** §8.1 says the playground can ship as soon as
   `explain` exists — it does. It is also a service that compiles anonymous strangers' source, with
   the compiler as its first sandbox ([`17`](17-playground.md) §17.3), and
   [`42`](42-security-assurance.md) §42.2 measures an ~8 KB file that aborts the process. Shipping in
   that order puts a denial of service on the front page, and the first outside security report
   arrives at a repository with no `SECURITY.md` to receive it. The correct order costs days.
2. ~~**The standard library before error rows.**~~ **Discharged, and it was the right call.** The
   argument was that a standard library's signatures are exactly where the error decision shows up
   — every fallible function in it — so a library written before error rows land gets rewritten
   when they do. Error rows landed first ([`27`](27-the-walls-come-down-report.md)), so the trap is closed
   and Wave 2 may start. The disagreement this bullet recorded with the prose above it is resolved
   in that prose's favour, and the note is kept rather than deleted because the *shape* of the trap
   recurs: a bullet list makes every item look independent, and the one that has a predecessor is
   never the one that says so.
3. **Phase 4 before the clock is virtualized.** Every Phase 4 bullet is a distributed-systems
   bullet whose stated test methodology is the simulator ([`13`](13-testing.md) §13.4). Building
   them against the real clock means testing them twice. The cheap move is not to build DST; it is
   to make the clock **injected** — a seam, not a simulator, and the project has done exactly this
   once before for a different resource ([`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)).
4. **The two one-way doors.** The first `cargo publish` with a long-lived token, and the first
   `beck` compiled by `beck` with no independent reproducible path. Neither is imminent; both are
   free to arrange now and impossible to arrange afterwards.

### 8.5.4 The waves

Waves, not dates — a wave is a set whose members are order-free with respect to each other and
which collectively unblock the next. Sizes are orders of magnitude, not commitments (§8.0).

**Wave 0 — days. ✅ Built** ([`44`](44-wave-0-report.md)). All seven items, kept here as the record
of what the wave contained rather than deleted, because the argument for the classification is the
list:

1. ~~Bound the front end's recursion as a count, with an ADR~~ — [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md),
   measured at 18 KiB per level and gated both ways *(G — it gated the playground)*
2. ~~Inject the clock~~ — `beck_core::clock`, with `SystemTime::now()` in exactly one place and a
   test that counts *(R — three phases overdue)*
3. ~~Threat model, `SECURITY.md`, memory-safety roadmap~~ — [`43`](43-threat-model.md) and
   `SECURITY.md` *(G)*
4. ~~A `pending_security` suite~~ — eight tests, each asserting a control is absent *(G)*
5. ~~The two syntax decisions~~ — [`10`](10-decisions.md) D21 and D22; the first split *(R)*
6. ~~Pin the Unicode version and adopt UTS #39~~ — plus Trojan Source, which that profile does not
   reach *(R)*
7. ~~Retarget [`12`](12-standards-and-conformance.md)'s four moved rows~~ *(S)*

**Wave 1 — weeks. `Result` and error rows. ✅ Built, in two halves**
([`27`](27-the-walls-come-down-report.md), [`80`](80-a-scope-owns-its-children-report.md)).
Errors as a row label with `Result` reified, and row aliases: **built**. Lexical handlers: built in
the narrow sense that `try:` is a form and therefore lexical by construction, and unbuilt in the
general sense — there is no `handle … with`, no resumption, no user-defined effect. **Structured
concurrency as scope-as-handler is built too**, a wave later and separately, exactly as the split
below predicted: the error half is what Wave 2 waited on, and the concurrency half waited on
nothing, so pairing them would have meant designing a concurrency model inside an error model's
change. `parallel:` is a scope whose bindings are its children — §38.4's shape, with `spawn` and
`await` *not* separately reachable, so a child cannot outlive its scope by construction. Its claim
is that the answer does not depend on which child ran first, and two compile errors hold it up:
no child may name another, and no child may perform an effect another child could observe. What is
**not** built is any backend that runs two children at once (§80.5).

**Wave 2 — weeks to months. The standard library. ✅ Most of it built**
([`46`](46-standard-library-report.md)). Strings, list and map collections, JSON and time are
**built** — thirty-one primitives plus `compiler/lib/`, which is the half written in Beck and the
answer to which parts of a library the *host* owns. `json_parse` and `time_parse` raise rather than
returning a `Result`, which is trap 2 cashed: a wave earlier, every one of those signatures would
now be wrong. The **HTTP client is built too** ([`49`](49-http-client-report.md)) — the item whose
effect row nobody had designed, and the reason to take it next rather than last: `net.out(host)` is
charged from the host *written at the call site*, so §6.5's egress policy stays derivable, and
[`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md) records what
that costs. **`Set` operations, sorting, grouping, deduplication, durations and date arithmetic are
built** ([`50`](50-collections-and-dates-report.md)), and added no primitives: a set is a map's
keys and the civil calendar is arithmetic, so both are files in `compiler/lib/` rather than lines
in `prelude.rs`. **Digests, encodings and identifiers are built too**
([`52`](52-crypto-and-identifiers-report.md)): nine primitives — a hash, a keyed one, a
constant-time comparison, hex, base64url and a UUID reader that normalises — plus `lib/crypto.beck`,
and no new dependency ([`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md)). That is
the item where §3.5 met the first operation that *has* to declassify a secret, because a message
authentication code exists to be given to whoever must not learn the key;
[`adr/0014`](adr/0014-a-keyed-digest-is-the-one-declassifier.md) is the decision, and it is one
function behind `cap.sign` with a test that enumerates the prelude and keeps the count at one.
**Bignums and numeric coercion** are built too
([`55`](55-bignums-report.md)) — in Beck rather than as a primitive, which is the first time
[`lib/README.md`](../compiler/lib/README.md)'s division cost the refusal of a good crate — so
**arbitrary-precision decimal** is built too ([`56`](56-decimal-report.md)) — canonical, with `/`
exact or refusing — so **§8.5.4's Wave 2 is finished** and what is left on this bullet is the
benchmark harness. And it is a standard library a program can *reach* since
[`69`](69-standard-library-imports-report.md): every file in `compiler/lib/` is carried in the
compiler and importable by name from anywhere, which nothing outside that directory could do for the
three waves it took to write. §46.6 has the item-by-item list, §49.6 the client's own, §50.6 the two newest
files' and §52.6 the crypto half's — including what a digest deliberately is not: no asymmetric
signature, no TLS, no encryption of any kind. The **Are We Fast Yet harness is complete — all fourteen
benchmarks** ([`53`](53-are-we-fast-yet-report.md), [`53`](53-are-we-fast-yet-report.md),
[`53`](53-are-we-fast-yet-report.md), [`53`](53-are-we-fast-yet-report.md), [`53`](53-are-we-fast-yet-report.md),
[`53`](53-are-we-fast-yet-report.md)), each verified against the constant
the original suite's own `verifyResult` checks, with wall-clock printed and nothing compared to
anything — §8.4's ask, half discharged. The **CLBG** harness is stood up too
([`68`](68-clbg-report.md)) — eight of the Game's ten, verified against the Game's own published
output *files*, with the oracle enforced by the gate rather than transcribed by hand — so §8.4's
ask is **discharged in full**. Its largest finding was not about either suite: **`lib/` was a
standard library nothing outside `lib/` could import**, since `import` resolved only against the
root module's own directory (§68.4). That is fixed
([`69`](69-standard-library-imports-report.md)) — the library is carried in the compiler and an
import resolves against the caller's directory first ([`10`](10-decisions.md) D23) — which took the
Game's harness from seven to eight, since `pidigits` was the benchmark it was holding. The
**Felleisen table** ([`63`](63-felleisen-report.md)) and the **compile-speed budgets**
([`64`](64-compile-speed-report.md)) are both built and sit under
[`25`](25-benchmarks-and-expressiveness.md) §25.9 rather than under this bullet.

*It found a wall too, in the same way and one wave later: **a credential could not be sent**, because
§3.5 gives a program no way to read a `secret[Str]` and a header value is a `Str` (§49.4). It had
been invisible for three phases because no program had ever tried to *spend* a secret — `corpus/03`
has held an `ApiKey` since Phase 1 and never used it. Closed by sending the secret at the edge
rather than by weakening the property, so `"Bearer " + reveal(token)` is still a compile error.*

*Writing it found a wall, which is what writing a library is for: **a trait's declared row is a
bound, so a fallible operation cannot be a trait method** — `Money` cannot have `+` because `Num`
is pure and mixing currencies has to fail (§46.5). The fix is one feature, a row variable in a
trait's method signatures, which [`27`](27-the-walls-come-down-report.md) built
for a user's higher-order definitions and nothing has built for traits. It is asserted as a
refusal, so it is a test that goes red rather than a paragraph.*

**Wave 2b — the wall Wave 2 wrote. ✅ Built** ([`27`](27-the-walls-come-down-report.md)). A
trait's declared row is a floor rather than a ceiling: an impl's row is inferred, published with
the impl so it crosses a module, and inherited by whoever calls it — so `Money` has its `+` and
every numeric or fallible type can have one. It cost one line of the checker plus the module
boundary, because bounded generics had been effect-polymorphic since
[`27`](27-the-walls-come-down-report.md) and nothing had noticed. This wave existed for a day: Wave 2 wrote it
and Wave 2b closed it, which is the shortest a wave has been and the argument for recording a wall
the moment it is found rather than at the end of the work that found it.

*Two corrections to Wave 1 came with it, both making `try:` more precise: it resolves the row
before reading it — a call to something declared later contributes a row *variable*, and a handler
that could only read atoms was wrong about exactly the forward references a program is made of —
and it catches the failure its signature names while the others travel, instead of refusing a block
that can fail two ways.*

**Wave 3 — months. ✅ Built** ([`48`](48-identity-report.md),
[`95`](95-oidc-relying-party-report.md), [`96`](96-presence-report.md)). Identity is a **seam**:
an `Actor` only a provider can mint, `DevIdentity` as the named default, and a `SignedIdentity`
that verifies a keyed-BLAKE3 credential — so an ownership check compares against something the
caller did not choose. And there is a third provider: an **OIDC relying party**, with discovery, a
cached JWKS, RS/PS/ES signatures, issuer, audience, authorized-party, expiry, not-before and nonce
checks, and the authorization-code flow with PKCE, so a browser can obtain a token rather than being
handed one. The **claims → `Session` mapping** is built with it, and §95.4 draws the line it stops
at: `validate` may read a claim, and a fold has nothing to read, because an envelope carries the
actor's name and nothing else.

D6's **language surface** is built as written: `identity = external(issuer="https://…")` is a
declaration, not a flag, so the compiler knows the issuer and §6.5's egress rule covers it like any
other peer (§95.7). That was the one thing this work found that was *worse* than unbuilt — a runtime
with a peer the compiler had never heard of — and it is closed rather than recorded.

`identity = managed()` is built too (§95.10): the object graph gains a provider, its Service, its
volume, its credentials and a **realm wired to this application's own route**, and the application's
egress to it is a `Peer` — a rule Kubernetes enforces, which the DNS-name rule an external issuer
gets is not. It is the one place in the project where a plaintext issuer is admissible, and the
argument is written out rather than left in a URL.

~~**Unbuilt**: presence — "who is connected now, as a first-class non-durable `Signal`" — which is
the last row of this bullet.~~ **Built** ([`96`](96-presence-report.md)): `presence()` is a source
in the signal graph performing `cap.presence`, which is [`14`](14-review-findings.md) F16's "gate
behind a capability" taken literally — and is what places it on the server, keeps it out of a fold
and publishes it in the row. The chokepoint may not read it (`B0515`, checked by reachability
rather than by an argument's shape) and a Mode B page may not either (`B0516`). It is **per
subscriber** rather than shared, and the reason is a clock: §5.3's shared dataflow is versioned by
the log's `seq` and this is the one input that moves when `seq` does not — §96.8's first unbuilt
item. The roster is **bounded**, because it is keyed by a name the client chooses
([`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.4 one subsystem over), and a page
that never asks who is connected is not re-rendered when somebody connects.

*Both of that row's predecessors are gone.* §48.5 said the relying party "needs an HTTP client and a
signature library, so it is an ADR rather than a line in a module". The HTTP client was built
([`49`](49-http-client-report.md)); the signature library and TLS were **one decision**, taken in
[`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md) — because rustls's cryptography provider
*is* a signature library, so [`07`](07-dependencies.md) §7.2's TLS row buys the asymmetric half for
nothing. This row's forecast that the two were one ADR was right; the reason turned out to be
better than the one it gave.

~~Then the playground rungs A and B (whose safety predecessor landed in Wave 0)~~ — **built**
([`98`](98-playground-report.md)), and the predecessor was the right one: the front end counts its
own recursion, so the page compiles a stranger's source without a stack that can be exhausted from
the outside. Then Phase 3's exit criterion. ~~What it measures is documentation an outside developer could build from, and there is
none.~~ There is now: [`86`](86-getting-started.md), gated so that every program in it compiles and
passes its own tests. That removes the *blocker* and not the criterion — which is a claim about a
person building a non-trivial app without asking the authors questions, and cannot be met by
anything this repository does to itself. §86.8 lists what the guide does not cover; the criterion
needs an outside developer, and none has read it.

**Wave 4 — free-standing, in parallel with Waves 2–3.** ~~LLVM backend and native codegen~~ —
**both code generators built** ([`93`](93-llvm-backend-report.md),
[`97`](97-cranelift-report.md)): §5.2's dual codegen exists over the scalar subset, `beck native
--backend cranelift|llvm` chooses, and §4.8's differential is **three-way** — the tree-walker, LLVM
and Cranelift on every call, with the two emitters held to accepting and refusing the same
definitions. ~~What still bounds *both* is the **heap**: no record, list, string or effect compiles~~ — the
**algebraic half is built** ([`101`](101-the-heap-report.md)): a `model`, a `union` and a `newtype`
have a layout, an arena and a place on the wire, and 283 definitions across the tree compile where
187 did. **Text is built too** ([`105`](105-text-on-the-heap-report.md)) — a `Str` is two counts and
its bytes, a literal is an offset into a pool the host writes, and **344** definitions compile where
283 did, with the corpus going 4 → 28. **Reading a collection is built too**
([`106`](106-lists-arrive-read-only-report.md), [`107`](107-a-map-arrives-read-only-report.md)) — a
`list[T]` and a `Map[K, V]` have layouts and **625** definitions compile where 344 did, with **nine
corpus folds** among them — and the operations that *grow* one are **refused** rather than shipped
with a worse asymptote than the evaluator's, which is §101.5's forecast cashed. **Closures are built too**
([`108`](108-closures-arrive-report.md)) — a `lam` is an object holding its **rank** and its captures,
an application is a switch on that word into a direct call, and every higher-order list primitive
but `list_flat_map` compiles through it — including `sort_by`, as a stable merge sort — and so does
`concat_lists`, whose refusal turned out to be false (its length is a sum over one pass). A closure is
refused at every boundary the host would read one across, so it lives inside one compiled call.
**646** definitions compile where 605 did, and the refusals that blamed a closure went 96 → 52 with 33
of the difference re-refused for a deeper reason. **A view is built too**
([`109`](109-a-view-arrives-as-a-recipe-report.md)) — and not as a tree: what a compiled `view` puts
in the arena is the **call** `html_el(tag, attrs, children)` would have been given, because a page's
leaves are *renderings* and the two functions that make them are the host's. So the host bakes the
tree with the evaluator's own `html_el`, neither emitter contains a byte of markup or JSON, and
`html_text(x)` compiles for every `x` that has a shape at all. **688** definitions compile where 650
did by that report's own count, the 42 refusals that blamed a view are **none**, and 21 of the 32
corpus programs have a `view` that compiles. It is **not faster** (§109.6) — the win is what
compiles. **A raise is built too** ([`110`](110-a-raise-arrives-report.md)) — `raise` and `try:`, and
the piece of "the effects" that needed no callback to the host, because a raise is not a call out but
a way of returning: the error cell every function already stores into and returns from *is* an
unwinder, and what this adds is a code, two words for the value and a handler the checks branch to.
**711** definitions compile where 688 did and the 38 refusals that blamed `raise` are none. What is
left of that bullet is the **rest** of the effects — the ones that need the worker to call back into
the host mid-call, which is a protocol with a second direction ([`110`](110-a-raise-arrives-report.md)
§110.9) — and growing a **map**. Growing a *list* is built
([`111`](111-a-list-grows-report.md)), and the report is mostly about why it was refused: the reason
on record was ownership, every sentence of it was true, and what forced the copy was the layout. A
list is an immutable header over a shared data block now, the accumulator is linear, and **895**
definitions compile where 711 did
([`107`](107-a-map-arrives-read-only-report.md) §107.4), which is still Phase 4's Lane E and still the prerequisite Mode B codegen is behind
([`94`](94-mode-b-report.md) §94.8) — and §105.10's correction about which row `Html` belongs to is
itself corrected by §109.9: a view follows neither the collection row nor the text one, being the
first thing in this arena whose contents are a call. Mode B and
~~client polish~~ — **built** ([`100`](100-client-polish-report.md)), and it was not a Lane B item
either: a route is a field of `Session`, so the engine, the splitter and the plan are untouched and
the work is at the edges; ~~the LSP~~ — **built** ([`65`](65-lsp-report.md)) and **finished**
([`110`](110-the-editor-edits-report.md)); ~~SQL read models, pgwire~~ — **built**
([`88`](88-read-models-and-pgwire-report.md)), and they were not a Lane B item in the shape this
list assumed: the schema derivation is a pass over the plan, the wire is `beck-rt`, and neither
touched `engine.rs` beyond one reader type; ~~query fusion~~ — **built**
([`89`](89-query-fusion-report.md)), and it is a pass over the plan that added one operator to the
engine and no scheduling to it; `test --update`; the SQLite substrate; ~~the shared dataflow's three unfinished properties
([`26`](26-arrangement-sharing-report.md) §26.9)~~ — **two of the three built**
([`51`](51-arrangement-lifecycle-report.md)): the arrangements are released when the last subscriber
goes and the change history is compacted to the oldest reader's frontier, both as one reader-set
rule rather than as the two items §26.9 listed. The third, the render lock, is still here and
§51.7 records that this change made it a *harder* item rather than an unchanged one, because
compaction is now safe partly because the lock is held the way it is; ~~structured concurrency,
which Wave 1 left behind and which has no predecessor~~ — **built**
([`80`](80-a-scope-owns-its-children-report.md)), and it acquired one on the way out: running two
children at the same time needs a `Sync` `Host`, which is a change to the execution half of
[`19`](19-phase-1-report.md) §19.9's seam and wants its own measurement; ~~more of SICP, chapter 3
being the part closest to what Beck is for~~ — **built**
([`87`](87-the-chapter-that-argues-back-report.md)), and it was the part closest to what Beck is for
in the way that mattered: §3.5.5 rewrites §3.1.1's bank account with no assignment and calls it
"fully functional … yet it embodies changing state", which is this project's premise stated by the
book, and `sicp/ch3.beck` asserts that it agrees with the fold. It produced no wall and three other
things — a declaration could take a builtin type's name (§87.4), `x.with(f = g(x.f))` was quadratic
where the constructor spelling is linear (§87.5), and §3.5.1's memoised `delay` is the one thing the
chapter cannot express, measured at ×5.2 per term (§87.7). Chapters 4 and 5 are still unattempted.
No predecessors, and they never acquire any.

**Wave 5 — the Phase 4 gates, arranged before Phase 4 rather than during it.** Supply-chain tooling
(~~SLSA v1.2 provenance~~ — **built** ([`109`](109-provenance-report.md)): the release attests
in-toto provenance over every artefact `SHA256SUMS` lists, signed by a Sigstore certificate whose
identity is the release workflow and recorded in the public transparency log, and `install.sh`
checks it on request. A *level* is not claimed and a user's `beck build` still attests nothing;
2026-element SBOMs, whose component hashes and mandatory signature are
[`92`](92-sbom-report.md) §92.5's rows and unmoved; ~~signing~~ — **the signing machinery is built**
([`99`](99-supply-chain-report.md)) and what is left of that row is a registry to push to and a
**subject the signer can take**: [`104`](104-the-release-and-the-installer-report.md)
§104.6 found that `beck sign` signs an image manifest digest and a compiler release is a tarball, so
the release *listing* carries a checksum and nothing more —
[`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md) is why that was left
where it was; trusted publishing configured *before* the first publish, which is now the whole of
what this row still owns); DST proper, on the
seam Wave 0 created; then the operator, the replay tooling and the
choreography. **Grammar-aware fuzzing is now due rather than pending**: [`42`](42-security-assurance.md)
§42.9 pinned it with the trigger "the bound lands", and the bound has landed. Kani proofs of the
solver's security invariants belong here too — that one still wants a solver that has stopped
moving.

### 8.5.5 Parallel workstreams

A wave is a dependency ordering, not a staffing plan. Most of the above can run concurrently, and
what decides *which* pairs is not the dependency graph — it is **which files two branches would both
rewrite**. The crate layout is favourable, because [`04`](04-compiler-architecture.md)'s pass
boundaries are real directories.

| Lane | Owns | Items | Collides with |
|---|---|---|---|
| **A — type system** | `beck-core/src/check/`, `ty.rs`, `core.rs`, `prelude.rs`, `iface.rs` | Error rows and handlers; `@derive`; bignums and coercion | **Itself, completely** — see below |
| **B — runtime and views** | `beck-rt/`, `beck-core/src/{engine,plan,incremental,pmap,signal}.rs` | Clock injection; the shared dataflow's release policy, history constant and render lock; SQL read models, pgwire, query fusion; Mode B's server half | Nothing in A, C, E or F |
| **C — front end and tooling** | `beck-syntax/`, `beck-cli/`, `beck-diag/` | ~~The recursion bound~~ ([`44`](44-wave-0-report.md)); ~~the two syntax decisions~~ ([`10`](10-decisions.md) D21, D22); ~~Unicode and UTS #39~~ ([`44`](44-wave-0-report.md) §44.5); ~~LSP~~ — **built** ([`65`](65-lsp-report.md)) and **finished** ([`110`](110-the-editor-edits-report.md)): references, highlight, rename and inlay hints, so §8.5.2's list of what an LSP is for has no unbuilt entry; ~~`test --update`~~ ([`66`](66-page-snapshots-report.md)); ~~fuzzing~~ ([`85`](85-what-the-generator-found-report.md)). **This lane is empty.** What a *new* Lane C item looks like is in [`110`](110-the-editor-edits-report.md) §110.7: comment-preserving printing, which `textDocument/formatting` waits on, and code actions | A, if a syntax decision changes what the checker sees |
| **D — process and supply chain** | `docs/`, `.github/`, `deny.toml`, `SECURITY.md`, `release/`, `install.sh` | Threat model, disclosure policy, memory-safety roadmap, `pending_security`, the four retargeted §12 rows, SLSA/SBOM/trusted publishing, ~~the release pipeline and the installer~~ ([`104`](104-the-release-and-the-installer-report.md)) | Nothing in code — **except that the release lands in `Cargo.toml`, a `build.rs` and `--version`**, which §104.4 is about and which this cell had assumed away |
| **E — backends** | `beck-eval/`, `beck-llvm/`, `beck-clif/`, `beck-core/src/backend.rs`, any new codegen crate | ~~LLVM backend, native codegen, the differential suite~~ — **built** ([`93`](93-llvm-backend-report.md)), ~~and Cranelift~~ — **built** ([`97`](97-cranelift-report.md)), ~~and a heap~~ — **half built** ([`101`](101-the-heap-report.md)): records, unions and newtypes, ~~and text~~ — **built** ([`105`](105-text-on-the-heap-report.md)), ~~and reading a collection~~ — **built** ([`106`](106-lists-arrive-read-only-report.md), [`107`](107-a-map-arrives-read-only-report.md)), lists and maps alike, ~~and closures~~ — **built** ([`108`](108-closures-arrive-report.md)): a rank and its captures, an application as a switch, and every list primitive that takes a function except `list_flat_map` — `sort_by` and `concat_lists` included, the second because its refusal was false, ~~and `Html`~~ — **built** ([`109`](109-a-view-arrives-as-a-recipe-report.md)): as the *call* that builds a page rather than as the page, so the rendering stays the host's, ~~and failure~~ — **built** ([`110`](110-a-raise-arrives-report.md)): `raise` and `try:`, on the error cell that was already an unwinder, ~~and growing a list~~ — **built** ([`111`](111-a-list-grows-report.md)): a header over a shared block, so an append writes a slot no reader can see. What is left is the effects that call *back* into the host, and growing a **map** | Nothing — the seam is why ([`19`](19-phase-1-report.md) §19.9), and [`93`](93-llvm-backend-report.md) is the first thing to test that claim: not one line of `beck-rt` changed |
| **F — infrastructure** | `beck-infra/` | Effect-derived NetworkPolicy/RBAC/grants; Crossplane emitter; conformance rungs | Nothing |

**Lane A is strictly serial, and that is the real staffing constraint.** It is tempting to run two
language features on two branches. Do not: they rewrite `check/mod.rs` (3,345 lines) and `ty.rs`
together, and they change what `core.rs` carries. Lane A is the critical path and absorbs one pair
of hands; everything else in this section exists to keep other hands off those files.
[`27`](27-the-walls-come-down-report.md) §27.10 records the mitigation that works — traits went into
`check/traits.rs` rather than into `check/mod.rs`, and bounds then grew that file rather than the
one everybody complains about ([`27`](27-the-walls-come-down-report.md)). Keep doing that, and a second Lane A
branch eventually becomes thinkable.

Recommended pairings, in order:

| When | Branch 1 (critical path) | Branch 2 | Why it is safe |
|---|---|---|---|
| ~~**Then**~~ | ~~Lane A: `Result` and error rows~~ | ~~Lane D, plus Lane C's half of Wave 0~~ | **Done.** Both branches landed together, and the prediction held: the error rows touched `check/`, `ty.rs`, `row.rs` and `core.rs`, and Wave 0 touched `beck-diag`, `beck-syntax`, `beck-rt` and `docs/`. The one collision was the one the table names below — `beck-diag/src/index.rs`, four new codes — and it was trivial because the numbers were far apart |
| ~~**Then**~~ | ~~Lane A: the standard library, on the error shape Wave 1 settled~~ | ~~Lane B: the shared dataflow's three loose ends~~ | **Half done.** The library's first half landed ([`46`](46-standard-library-report.md)), then the wall it wrote ([`27`](27-the-walls-come-down-report.md)), then the HTTP client ([`49`](49-http-client-report.md)) — which was Lane A *and* Lane B, because the seam is in `beck-core` and the implementation is in `beck-rt`. The prediction held anyway: nothing in `engine.rs` was touched |
| ~~**Then**~~ | ~~Lane A: the rest of Wave 2 — `Set`, dates~~ | ~~Lane B: the shared dataflow's three loose ends~~ | **Half done.** `Set` and dates landed ([`50`](50-collections-and-dates-report.md)) and were not Lane A at all: two files in `compiler/lib/`, no primitive, and the only Rust touched was one diagnostic's label. Lane B is untouched, so the pairing was never tested — the prediction it made cannot be claimed to have held |
| ~~**Then**~~ | ~~Lane A: bignums, coercion, `@derive`~~ | ~~Lane B: the shared dataflow's three loose ends~~ | **Half done, and the other half.** Lane B was taken at last, after being the recommended Branch 2 for three consecutive rewrites: two of the three loose ends are closed ([`51`](51-arrangement-lifecycle-report.md)) and the third — the render lock — is deliberately left. The prediction held exactly: `engine.rs`, `beck-rt/` and one test suite, and nothing in `check/`, `ty.rs` or `core.rs`. Lane A is untouched, so this pairing was again never actually run as a pair |
| ~~**Then**~~ | ~~Lane A: bignums, coercion, `@derive`~~ | ~~Lane B: SQL read models and pgwire~~ | **Half done, and the other half again.** Lane B was taken ([`88`](88-read-models-and-pgwire-report.md)) and the prediction held: `beck-core/src/read.rs`, `beck-rt/src/pgwire.rs`, one reader type on `engine.rs`, and nothing in `check/`, `ty.rs` or `core.rs`. Lane A is untouched, so this pairing was never run as a pair either — the fourth consecutive rewrite in which it was not |
| ~~**Then**~~ | Lane A: ~~the pattern-matching completion the error-rows bullet still names~~ — **built**, with nesting, guards and alternatives ([`90`](90-nested-patterns-report.md), [`91`](91-guards-and-alternatives-report.md)); what is left in this lane is `Ord` as a trait, which [`54`](54-ordering.md) writes out and explicitly does *not* recommend | Lane B: ~~query fusion on symbolic plans~~ — **built** ([`89`](89-query-fusion-report.md)); ~~Mode B's server half~~ — **built** ([`94`](94-mode-b-report.md)), and it is one branch in `session.rs`; what is left in this lane is the render lock ([`51`](51-arrangement-lifecycle-report.md) §51.7), which survives into the row below still unowned | `beck-rt` and `engine.rs` are untouched by anything in `check/` |
| ~~**Then**~~ | ~~Lane A, continued~~ | ~~Lane E: the LLVM backend~~ | **Half done, and the other half.** Lane E was taken ([`93`](93-llvm-backend-report.md)) and the prediction held to the letter: a new crate, one new CLI command, and one defect fixed in `beck-eval` — and nothing in `beck-rt`, `engine.rs`, `check/`, `ty.rs` or `core.rs`. Lane A is untouched, so the pairing was again not run as a pair |
| ~~**Then**~~ | ~~Lane A, continued~~ | ~~Lane D: the release pipeline and the installer~~ | **Half done, and the other half — for the sixth consecutive rewrite.** Lane D was taken ([`104`](104-the-release-and-the-installer-report.md)) and its "collides with nothing in code" held for the pipeline and failed for the *release*: a version number that means something is `compiler/Cargo.toml`, a `build.rs` and one line of `main.rs`. Lane A is untouched, so this pairing was not run as a pair |
| ~~**Then**~~ | ~~Lane A, continued~~ | ~~Lane E: a heap for the native backends~~ | **Half done, and the other half — again.** Lane E was taken ([`101`](101-the-heap-report.md)): the *algebraic* half of the heap is built, so a record, a union and a newtype compile. Lane A is untouched, which makes seven consecutive rewrites in which the recommended pair was not run as a pair — the prediction that Lane E collides with nothing keeps holding, and the one about Lane A keeps not being tested |
| ~~**Then**~~ | ~~Lane A: `Ord` as a trait~~ | ~~Lane E: the rest of the heap — text, collections, closures, the effects~~ | **Three quarters done, and the other half again.** Lane E was taken four times running ([`105`](105-text-on-the-heap-report.md), [`106`](106-lists-arrive-read-only-report.md), [`107`](107-a-map-arrives-read-only-report.md), [`108`](108-closures-arrive-report.md)) and the prediction held every time: `beck-llvm`, `beck-clif`, their two suites, and one public function moved in `beck-core` for [`108`](108-closures-arrive-report.md)'s closures. Lane A is untouched, which makes eight consecutive rewrites in which the recommended pair was not run as a pair |
| ~~**Then**~~ | ~~Lane A: `Ord` as a trait~~ | ~~Lane E: what is left of the heap~~, ~~plus Lane D as a third branch~~ | **A third of it done, and the third branch.** Lane D was taken ([`109`](109-provenance-report.md)) and the prediction in its own row held rather than the one that keeps failing: `install.sh`, one workflow, three test files, and nothing in `check/`, `ty.rs`, `core.rs`, `engine.rs` or any backend, so it could have run beside either of the other two. It did not, because neither of the other two was staffed — which is the ninth consecutive rewrite in which the recommended pair was not run as a pair, and the first in which the *third* branch is the one that moved |
| ~~**Then**~~ | ~~Lane A: `Ord` as a trait~~ | ~~Lane E: `Html`~~ | **Half done, and the other half — for the ninth time.** Lane E was taken ([`109`](109-a-view-arrives-as-a-recipe-report.md)) and the prediction held again: `beck-llvm`, `beck-clif`, their two suites, one function lifted out of `beck-eval` into `beck-core`, and **not one line of `beck-rt`**. Lane A is untouched |
| **Now** | Lane A: `Ord` as a trait, which [`54`](54-ordering.md) writes out and does *not* recommend — so realistically nothing | Lane E: **what is left of the heap** — growing a **map** and the effects that call *back* into the host ([`107`](107-a-map-arrives-read-only-report.md) §107.4, [`110`](110-a-raise-arrives-report.md) §110.9). Neither is a missing emitter and neither is the same *kind* of item: a map wants a tree in the arena, because a sorted run has to shift however the layout is separated ([`111`](111-a-list-grows-report.md) §111.7), and an effect wants a protocol with a second direction | The backend seam exists so these do not interact, and it has now been tested six times. Lane B's render lock is still open and still has no owner — it is a third branch rather than a reason to hold either of these |
| **Any time** | — | Lane F; ~~Lane C's LSP~~ — **built** ([`110`](110-the-editor-edits-report.md)), which empties Lane C; more of SICP | No predecessors, no collisions |
| ~~**Then**~~ | ~~Lane A: `Ord` as a trait~~ | ~~Lane E: failure~~ | **Half done, and the other half — for the tenth time.** Lane E was taken ([`110`](110-a-raise-arrives-report.md)) and the prediction held again: `beck-llvm`, `beck-clif`, their two suites, and nothing in `beck-rt`, `check/`, `ty.rs` or `core.rs` — the checker was not touched at all, because `raise` and `try:` were already language features and this is only their run-time half. Lane A is untouched |
| ~~**Then**~~ | ~~Lane A: `Ord` as a trait~~ | ~~Lane E: growing a list~~ | **Half done, and the other half — for the eleventh time.** Lane E was taken ([`111`](111-a-list-grows-report.md)): `beck-llvm`, `beck-clif`, their two suites and four refusal lists, and nothing in `beck-rt`, `check/`, `ty.rs` or `core.rs`. Lane A is untouched |

A third branch is viable whenever E or F is staffed. The ceiling is four, because of these:

**The heap's second half is this phase's remaining F**, and the first half is the evidence for
saying so. §8.5.1's classification says to take the fan-out item first; the algebraic half
([`101`](101-the-heap-report.md)) was taken and it moved two rows of the exit paragraph at once.
What is behind the rest of it — growing a collection, the effects that call back into the host — is
the same set it always was, four items and a half shorter: native codegen for anything a program actually manipulates, and Mode B's codegen, which
is that plus a wasm emitter ([`94`](94-mode-b-report.md) §94.8). It sits in Lane E, which collides
with nothing.

**The four shared artefacts that serialise otherwise-independent branches.** Each has a cheap
discipline that avoids the collision.

1. **`beck-diag/src/index.rs`** — a new diagnostic code needs an entry or `cargo test` fails.
   *Land each code as its own small commit, and pick non-adjacent numbers up front: the index is
   sorted, so adjacent insertions conflict and distant ones do not.*
2. **`docs/reference/`** — generated and gated against drift, so any branch touching a diagnostic, a
   prelude scheme or the CLI tree must regenerate it, and two that do produce conflicting whole-file
   diffs. *Regenerate as the last commit before review, never mid-branch; rebase-then-regenerate
   rather than merging a generated file.*
3. **[`README.md`](README.md)'s index table** — every new document appends a row, and the numbering
   is first-come. *Claim the number in a stub row early, or expect to renumber.*
4. **`Cargo.lock`** — any new dependency. *A and B should not both add dependencies in the same
   week; if they must, take the lock from `main` and re-resolve rather than merging it.*

### 8.5.6 What this corrects above, rather than editing quietly

- **Phase 3's bullets are a set presented as a list.** The standard library had a predecessor the
  list did not show — `Result`/error rows — which §8.5.3 trap 2 stated in full and
  [`27`](27-the-walls-come-down-report.md) has now removed. The correction stands for the next one: the
  concurrency half of the same bullet has *no* predecessor and should not be scheduled as though it
  shares one.
- **F11 should be read as two-thirds met, not `FIXED`.** [`14`](14-review-findings.md) recorded the
  constraint, which is all `FIXED (constraint recorded)` ever claimed; the runtime then did not
  honour it for three phases. The clock is now supplied rather than ambient
  ([`44`](44-wave-0-report.md) §44.3) and the network is a seam
  ([`49`](49-http-client-report.md) §49.5) — the second built *before* its first caller rather than
  three phases after, which is the whole of what F11 asks for. The **disk** is untouched, as is
  elapsed time. Correcting the record is this bullet, since [`14`](14-review-findings.md) is
  history like any report.
- **§8.1's "the playground can ship early" now carries a predecessor**, added by
  [`42`](42-security-assurance.md) §42.2 after that graph was drawn.
- **Three surveys' `adopt` verdicts now have a position.** They still need their D-numbers and ADRs
  before they are real, and appearing in a wave does not grant one.

This section is the most perishable thing in this document: the first wave that lands invalidates
the ordering below it. That has now happened three times — bounds were Wave 1 when this was drafted
and were built before it was committed; Wave 0 landed whole; Wave 1 landed in half, and the halves
turned out to have different successors. A sequence that is not rewritten when a wave completes is
worse than none, because it still looks authoritative.

Two things the completed waves taught that the classification did not predict, recorded because
they are what the next rewrite should assume:

- **A wave item can split.** [`10`](10-decisions.md) D21 was posed as one question and had already
  been answered as two by four phases of implementation; Wave 1's error and concurrency halves have
  different successors and should never have been one row. The classification is about *cost over
  time*, and it is silent about whether an item is one item.
- **A wave item can be in the wrong lane.** §8.5.5 filed `Set` and dates under Lane A, the type
  system, on the assumption that a standard-library item is a language item. They turned out to be
  two files of Beck and no compiler change at all
  ([`50`](50-collections-and-dates-report.md)) — which means they could have run beside a Lane A
  branch rather than behind one. The lane table is about which *files* two branches would both
  rewrite, and nothing had asked which files these would touch before assigning them.
- **A G-class item's real output is the gate, not the artefact.** Every prose item in Wave 0 —
  the threat model, the disclosure policy, the memory-safety roadmap — is worth what it is only
  because something now goes red when it stops being true. `pending_security.rs` is a smaller
  artefact than [`43`](43-threat-model.md) and is the one that keeps it honest.
