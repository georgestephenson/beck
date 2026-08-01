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

> Two of the twelve bullets below are built, each with its own report, and a third is most of the
> way there.
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
> The nine other bullets are **untouched**, and [`26`](26-arrangement-sharing-report.md) §26.9 names
> them one at a time rather than by omission. So are the two added since (§8.4).

- LLVM release backend + differential tests against Cranelift (§5.2).
- **Incremental views**: compile subscribed/materialized views to differential-dataflow plans with
  arrangement sharing (per-session fanout, §5.3); recompute stays as the CI oracle; SQL read models
  + pgwire exposure; query fusion on symbolic plans. *The plans and the oracle are **built**
  ([`24`](24-incremental-views-report.md)); so is arrangement sharing between subscribers
  ([`26`](26-arrangement-sharing-report.md)). The read models, pgwire and the fusion are untouched.*
- **Mode B client**: per-component WASM (view + fold + signal kernel), optimistic application with
  `seq` reconciliation, freshness-typed pending state; size budget CI gate (< 150 KB brotli per
  component bundle).
- Client polish for both modes: router, forms, lazy routes, focus/scroll preservation, devtools
  extension showing signal graph, patch traffic and pending state.
- **`test` blocks and inferred mocks** ([`21`](21-tests-in-beck-and-proof.md) §21.2–§21.3) —
  **BUILT** ([`22`](22-phase-3-report.md)): a test is a log, a command and an expectation, so
  cross-boundary tests need no network and no fixtures; stubs attach to *effect atoms* rather than
  to interfaces, so "any value" is the default and has no syntax. Depends on one type-directed value
  generator, which `property` blocks share. This is the first thing an outside developer will reach
  for, and Phase 2 shipped with no way for them to write a single test about their own program.
  `beck test --update` for page snapshots is the part that did not ship.
- Structured concurrency, `Result`/error rows, `match` exhaustiveness, pattern matching completion.
- **SQLite as a durable substrate** ([`07`](07-dependencies.md) §7.8.1): a `LogStore`
  implementation beside redb and Postgres. The reason is not speed — the measurements say the
  durable substrates are within ~16% of each other — it is that SQLite is *also* the read-model
  engine, so rungs 0–2 get the same "append and project in one transaction" property production
  has, and a developer's laptop stops being merely similar to production. Measure with `beck bench
  log` and let the number pick rung 0's default.
- Standard library v1: collections, strings, time, money/decimal, HTTP client, JSON, UUID, crypto
  primitives (delegated to `ring`/`aws-lc-rs`, not hand-rolled). **Reals first**, because §25.6
  measures that §1.1.7 of SICP — the first substantial program in the book — does not typecheck
  without them.
- **The language's own means of abstraction, which four phases have never been pointed at**
  ([`25`](25-benchmarks-and-expressiveness.md) §25.6, measured): recursive and forward-referencing
  types; user-written polymorphic definitions; proper tail calls in the evaluator, or a bounded-depth
  diagnostic in the interim, because a Beck program can currently abort its own process with a
  recursion the user cannot bound; running a module with no merge point; and the `B0320`
  row-unification defect that refuses an `if` over two function values. §25.7 orders them. Every
  corpus program is shaped like the todo sketch, which is why none of these had surfaced.
- **The expressiveness suite** ([`25`](25-benchmarks-and-expressiveness.md) §25.5, D18): SICP
  stage 1, and Felleisen's criterion answered for the special forms the book introduces. It needs
  macros and nothing else, so it starts now and does not wait on the bullet above.
- **Identity**: OIDC relying-party runtime, `identity = managed()` provisioning (Keycloak/Ory),
  claims → `Session` capability mapping, dev-mode identity for rung 0, presence as a first-class
  signal ([`10`](10-decisions.md) D6).
- LSP: completion, hover with *inferred placement*, go-to-def, rename, inline diagnostics.
- **The playground** ([`17`](17-playground.md)) — highest-leverage adoption artefact: rung A
  (compile-time, static) and rung B (the whole app in the tab — the worker-server is the rung-0
  platform compiled to WASM, riding Mode B's kernel work; `seq` scrubber and two-client demos).
- `beck init ci`, apko image build in-process, cosign signing, SBOM.

**Exit**: an outside developer builds a non-trivial app from documentation alone, without asking the
team a question. Track this literally as the acceptance test.
**Not met**, and not close — two bullets of twelve and a third's engine
([`24`](24-incremental-views-report.md) §24.10). What did change is that the first question they
would have asked — "how do I test this?" — now has a command for an answer, and the second — "will
this recount a million rows every time somebody clicks?" — has a command *and* a number.

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
15 seconds.

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
native codegen is unbuilt. Publishing them anyway is the point.

| Phase | Stand up | Publish |
|---|---|---|
| **3** | The **expressiveness** work, which needs nothing that is not built: SICP stage 1 (chapter 1 complete) and the **Felleisen macro-expressibility table**. Macros are built and hygienic ([`19`](19-phase-1-report.md)), so this is independent of §25.7's six walls and is the cheapest item on this table. Also: the compile-speed budgets §13.7 already lists, on the rustc-perf model | Chapter 1's line-count comparison against the pinned Scheme baseline — the first honest number in either half of §24 |
| **3** (with the standard library and the LLVM backend) | **Are We Fast Yet** and **CLBG** harnesses, run against the evaluator | Nothing comparative. The interpreter-vs-Cranelift-vs-LLVM differential ([`13`](13-testing.md) §13.1) and the first honest compute number arrive together, and not before |
| **4** | **TechEmpower** (the five tests that map without argument; the two that assume update-in-place stated as run against a read model), **js-framework-benchmark** (three columns — Mode A at a stated RTT, Mode A at RTT 0, Mode B — never averaged), **YCSB** against the log, **Lighthouse/Core Web Vitals** as gates on the example apps. SICP stages 2–3. **The DDIA matrix** ([`15`](15-scale-and-distribution.md) §15.6) — beside the Jepsen and simulation work that discharges its rows, never before it, because a matrix written ahead of its tests is the table of intentions it exists not to be | The whole-system numbers, unflattering, with the methodology notes of §25.2 attached to each. This is [`01`](01-vision-and-premise.md) §1.5 item 3 measured by somebody else's harness rather than ours |
| **5** | **TPC-H/ClickBench** on read models once §5.3's engine exists; the incremental-view workload §25.2 records as having *no* standard, which we would be defining rather than borrowing. SICP stage 4 | The Phase 5 suite above, and the expressiveness result — including the rows §25.5 forecasts Beck will lose (§2.4–2.5 generic operations, chapter 4's evaluator), which are published or the exercise was not run honestly |

Two things this table deliberately does not do. It does not put **TPC-C** anywhere: it assumes
update-in-place OLTP, which is not Beck's data model, and entering it would be a claim we do not
make. And it does not treat the SICP suite's pass rate as a metric — §25.5's three registers
(translated / re-expressed / refused) are the result, and chapters 3.1–3.4 and 5 are expected to
land mostly in the last two.
