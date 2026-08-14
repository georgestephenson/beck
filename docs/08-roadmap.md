# 08 — Roadmap

This document is the **plan**: what is built, what is next, and in what order. It states the current
position rather than the history of how the position was reached — [`README.md`](README.md) indexes
the reports, and git holds the rest.

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

## Phase 0 — Prove the premise — **BUILT**

Hand-written in Rust, in [`phase0/`](../phase0/): the *output* the compiler would eventually
generate for the todo sketch ([`00`](00-original-idea.md)) — ingress and envelope stamping, a durable
fold over a Postgres log, server-side `view` and structural diff, the thin patch-interpreter client,
`(subscription, seq)` resumption, an image, k8s manifests, an operator stub.

**The premise holds** ([`18`](18-phase-0-report.md)): interaction p99 is ~1 ms of server work (34 ms
at a 25 ms RTT) against the ~150 ms gate, per-idle-session memory is ~5 KB against the ~50 KB
tripwire, and replay after SIGKILL is bit-identical. Neither kill/pivot gate fired, so Mode B stayed
in Phase 3 and the session representation was not redesigned. The apko image and the k3d deployment
were never executed there — the environment had no container runtime — and both were executed in
Phase 1 instead.

## Phase 1 — Walking skeleton — **BUILT**

The narrowest possible compiler taking the todo sketch from source to a running k3d deployment:
lexer, layout, parser, hygienic macros, modules, HM typechecking, `Core` IR with **manual placement
only**, signal-graph slicing, the thin client, the log engine, the k8s object graph, `beck run` and
`beck up`.

**Exit met** ([`19`](19-phase-1-report.md) §19.3, §19.5): `git clone && beck up` yields the working
todo app in a cluster, the differential and replay-determinism harnesses are green, and a killed pod
recovers by folding the log.

## Phase 2 — Effects and placement — **BUILT**

The moat: effect rows and inference, placement verification *and* inference with the cost solver,
`beck explain place`/`flow`/`wire`, `secret[T]`, `Sendable`, capability effects, `.becki` published
signatures and separate compilation, boundary versioning and `beck check --wire-compat`.

**Exit met** ([`20`](20-phase-2-report.md) §20.3): the todo sketch with **every `@on(...)` removed**
compiles, places and runs; all eight of §3.5's properties are passing tests; a three-module project
re-checks one module after a body edit and three after a signature change. Across the corpus, 44% of
everything placed is unplaced-pure.

Two items this phase was assigned and did not deliver were delivered later and are named here rather
than left implied: the **general slicer** (Phase 3, [`23`](23-incremental-views-report.md)) and
**native codegen** (Phase 3, [`93`](93-the-native-backends-report.md)).

## Phase 3 — Make it real for developers — **STARTED**

Every bullet below has been taken. **The exit criterion has not been met**, because it is a claim
about a person rather than a count of bullets — see the end of this section.

| Bullet | Status |
|---|---|
| **Native codegen**: LLVM and Cranelift, differential against the evaluator | **Built** ([`93`](93-the-native-backends-report.md)). `beck native --backend cranelift\|llvm`; the differential is three-way. The heap is whole — records, text, collections, closures, views, failure, generics and the four host-calling primitives — and the fifteen that are a table or somebody else's parser are **linked** rather than emitted (§93.12), so the corpus stands at **941 definitions compiled against 137 refused**. §93.15 names what is left |
| **Incremental views**: dataflow plans, arrangement sharing, SQL read models, pgwire, query fusion | **Complete** ([`23`](23-incremental-views-report.md)) |
| **Mode B client**: per-component WASM, optimistic application, freshness-typed pending state, size budget | **Built except codegen** ([`94`](94-the-client-report.md)). The mode, the bundle, the data patch, reconciliation by `seq`, a browser that runs it, an offline queue, `freshness()` and the 150 KB brotli gate. Codegen waits on a wasm emitter |
| **Client polish**: router, forms, focus/scroll preservation, devtools | **Built except lazy routes** ([`94`](94-the-client-report.md)). A route is a field of `Session`, so there is no route table and every route is a real URL. Lazy routes wait on §5.1's per-component boundary |
| **`test` blocks and inferred mocks** | **Built** ([`22`](22-phase-3-report.md)), with page snapshots and `beck test --update` ([`22`](22-phase-3-report.md)) |
| **Structured concurrency, `Result`/error rows, pattern matching** | **Built** ([`27`](27-the-walls-come-down-report.md), [`80`](80-structured-concurrency-report.md), [`90`](90-pattern-matching-report.md), [`90`](90-pattern-matching-report.md)). `parallel:` runs its children on a thread each and stops them when one fails. What is left is stopping a child **blocked in the host**, which is a deadline on the [`net`](../compiler/crates/beck-core/src/net.rs) seam |
| **SQLite as a durable substrate** | **Built** ([`67`](67-sqlite-report.md)). At equal durability SQLite and redb are within noise, so rung 0's default is unchanged — the reason §7.8.1 gave (the transaction, not the speed) survives the measurement and is the only reason |
| **Standard library v1** | **Built** ([`46`](46-standard-library-report.md)): strings, collections, JSON, time, HTTP, digests, encodings, identifiers, bignums, coercion and arbitrary-precision decimal, importable from anywhere. §46.16 has what a library still lacks |
| **The language's own means of abstraction** | **Done** ([`27`](27-the-walls-come-down-report.md)). Every wall this project has *found* is down and `sicp/refusals/` is empty — which is not the claim that Beck expresses SICP; `sicp/refusals/README.md` is where the difference is written down |
| **The expressiveness suite** | **Built** ([`25`](25-benchmarks-and-expressiveness.md) §25.5): SICP chapters 1–3 against the book's own answers, and Felleisen's table ([`63`](63-expressiveness-report.md)) — six forms recovered, `amb` conceded. Chapters 4–5 belong to Phase 5 |
| **Identity**: OIDC relying party, `identity = managed()`, claims → `Session`, presence | **Built** ([`48`](48-identity-report.md)) |
| **LSP** | **Built, every entry** ([`65`](65-the-editor-report.md)). Hover with inferred placement, go-to-def, inline diagnostics, completion, semantic tokens, references, rename and inlay hints. Placement is shown twice over: on hover as `@on(tier)`, and as an inlay hint carrying the annotation the source did *not* write |
| **The playground**: rungs A and B ([`17`](17-playground.md)) | **Built** ([`98`](98-playground-report.md)). Rung C is Phase 4's |
| **`beck init ci`, image build in-process, signing, SBOM** | **Built** ([`92`](92-supply-chain-and-release-report.md)), with the release pipeline and installer in front of them and build provenance over what they publish. What is left is not a piece of this bullet: no registry push, no pinned package versions, and **package signatures are not verified** — the largest security gap, named as one |

**Exit**: an outside developer builds a non-trivial app from documentation alone, without asking the
team a question. Track this literally as the acceptance test.

**Not met**, and the honest way to say how close it is is by the questions such a developer would
ask in order:

| Their question | The answer today |
|---|---|
| "How do I test this?" | A command ([`22`](22-phase-3-report.md)) |
| "Will this recount a million rows every time somebody clicks?" | A command *and* a number ([`23`](23-incremental-views-report.md)) |
| "Can I write my own abstractions, or only the ones the todo sketch needed?" | Ten walls down and an empty `sicp/refusals/` |
| "How do I say something failed?" | `raise` and `try:`, and the signature says so whether or not I wrote it down ([`27`](27-the-walls-come-down-report.md)) |
| "Is there a string library? A JSON parser?" | Yes, and `compiler/lib/` shows how to write the next one ([`46`](46-standard-library-report.md)) |
| "Can I trust the actor in my ownership check?" | Against a real identity provider, yes ([`48`](48-identity-report.md)), and `session.claims` says what they may do. The default still believes the client, and says so |
| "Can my DBA see the data?" | `psql` against the read models ([`23`](23-incremental-views-report.md)) — one table per collection, derived, no annotation |
| "Where's the tutorial?" | [`86`](86-getting-started.md), published on the site, with every program in it compiled and run by a test |
| "How do I get the compiler?" | One command ([`92`](92-supply-chain-and-release-report.md)) — and it has nothing to download until a tag is pushed, so today the answer is still "build it", which §86.1 says in that order |

Every row above is a prerequisite for a tutorial being worth writing rather than a substitute for
one. §8.3 item 6 — "write the tutorial as you build, and treat any sentence that requires an apology
as a bug report against the design" — is the practice this phase has least honoured, and §86.8 is
the list of what the guide does not cover. **The criterion needs an outside developer, and none has
read it.**

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
- **The managed-cloud path** ([`28`](28-releases-and-deployment.md) §28.3 owns the pipeline view;
  this is the landing order). The design is deliberately vendor-neutral Kubernetes, and the
  consensus target is AWS — half again the runner-up's share (Stack Overflow 2025 survey: 43.3%
  AWS, 26.3% Azure, 24.6% GCP) — so "deploys to a cluster" must become "deploys to EKS with
  nothing but an AWS account" in these steps, each usable without the next:
  1. **Publish the first release** — the tag through `release.yml`, which exists and has never
     run. This is a deployment feature, not just a distribution one: the workflow `beck init ci`
     emits builds the toolchain from a checkout *because there is no release to install* (the
     generated file says so in a comment), so the first tag converts every generated pipeline's
     slowest step into an install.
  2. **`beck image --push`** — `beck image` writes an OCI layout and deliberately does not upload
     it; the push to a registry (ECR, GHCR, Harbor) is the one imperative step the generated
     pipeline cannot finish without. The registry client is the same one §6.7's ORAS package
     distribution needs — build it once.
  3. **The Crossplane emitter** — the derivation that turns `durable` into a PVC turns it into a
     managed-Postgres claim (RDS, Cloud SQL) at rung 4+ of §6.6's ladder; buckets and DNS from the
     resources a program names; OpenTofu emitter as the escape hatch for estates Crossplane cannot
     reach; `import infra` for everything else. Terraform stays excluded on licence
     ([`07`](07-dependencies.md)).
  4. **Rungs 4–5 executed, not designed**: `beck deploy --to staging` run against a real EKS
     cluster, end to end, before any document claims it. An artefact nobody has executed is a
     design document.
- **A static-host `Platform`** (GitHub Pages, Cloudflare Pages, Netlify — the hosts the web
  crowd's `git push`-to-deploy expectation comes from): for a program whose effect row a CDN can
  satisfy — no `durable` server fold, no `merge_clients()` — `beck build --platform static` emits
  the directory and `beck init ci` the Pages workflow. The admission check *is* the effect row:
  the same analysis that derives a NetworkPolicy decides whether a static host is a sufficient
  computer, and a program that does not qualify is told which atom disqualified it
  (`beck explain deploy`). The playground ([`17`](17-playground.md) §17.1 rung A) is the first
  artefact through this door and already proves the shape.
- **ECS/Fargate `Platform`** — explicitly a market-scope decision rather than an engineering one:
  the Compose implementation priced a new platform at one file and one flag
  ([`07`](07-dependencies.md) §7.8), and much of "everyone uses AWS" is teams that run no
  Kubernetes at all. After the Crossplane emitter, if adoption says so.
- `beck init ci` grows the GitLab CI emitter §6.8 already names beside the GitHub Actions one.
  The third-party manifest-scanner gate landed in this project's own CI first (`compiler.yml`'s
  `ecosystem-oracles` job, [`21`](21-tests-in-beck-and-proof.md) §21.4 rung 6); the Phase 4 form
  of the same idea is `beck init ci` emitting the scanner step into a *user's* workflow, with the
  suppression list the emitter already knows it deserves.
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

P2 and P3 can overlap with separate owners; P4 needs both. The playground could ship as soon as P2's
`explain` output existed, with one predecessor this graph did not draw: it compiles source written
by strangers, so the front end's recursion bound ([`44`](44-wave-0-report.md)) came first.

This graph is between phases. The ordering *within* the current phase is §8.5.

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
bad**, because §25.3 measures the tree-walking evaluator at roughly 33× CPython on `fib(30)`. Native
codegen is now built, and the first comparative number is
[`93`](93-the-native-backends-report.md) §93.5's — **against the evaluator**, not against another
language, because the benchmark suites run whole programs and a whole program still has a fold, a
`validate` and a view in it, which are signal nodes the splitter reads rather than definitions a
body calls.

| Phase | Stand up | Publish |
|---|---|---|
| **3** | The **expressiveness** work, which needs nothing that is not built: SICP stage 1 and the **Felleisen macro-expressibility table**. Also: the compile-speed budgets §13.7 lists, on the rustc-perf model | Chapter 1's line-count comparison against the pinned Scheme baseline. **Done** ([`63`](63-expressiveness-report.md), [`64`](64-compile-speed-report.md)) |
| **3** (with the standard library and the native backends) | **Are We Fast Yet** and **CLBG** harnesses | The three-way differential ([`13`](13-testing.md) §13.1) and the first honest compute number. **Both arrived** ([`53`](53-are-we-fast-yet-report.md), [`46`](46-standard-library-report.md) §46.13, [`93`](93-the-native-backends-report.md) §93.5). What is still not published is a comparison against **another language** on those suites; [`compiler/xlang/`](../compiler/xlang/README.md) is the one place a Beck number sits beside another language's |
| **4** | **TechEmpower** (the five tests that map without argument; the two that assume update-in-place stated as run against a read model), **js-framework-benchmark** (three columns — Mode A at a stated RTT, Mode A at RTT 0, Mode B — never averaged), **YCSB** against the log, **Lighthouse/Core Web Vitals** as gates on the example apps. SICP stages 2–3. **The DDIA matrix** ([`15`](15-scale-and-distribution.md) §15.6) — beside the Jepsen and simulation work that discharges its rows, never before it, because a matrix written ahead of its tests is the table of intentions it exists not to be | The whole-system numbers, unflattering, with the methodology notes of §25.2 attached to each. This is [`01`](01-vision-and-premise.md) §1.5 item 3 measured by somebody else's harness rather than ours |
| **5** | **TPC-H/ClickBench** on read models once §5.3's engine exists; the incremental-view workload §25.2 records as having *no* standard, which we would be defining rather than borrowing. SICP stage 4 | The Phase 5 suite above, and the expressiveness result — including the rows §25.5 forecasts Beck will lose (§2.4–2.5 generic operations, chapter 4's evaluator), which are published or the exercise was not run honestly |

Two things this table deliberately does not do. It does not put **TPC-C** anywhere: it assumes
update-in-place OLTP, which is not Beck's data model, and entering it would be a claim we do not
make. And it does not treat the SICP suite's pass rate as a metric — §25.5's three registers
(translated / re-expressed / refused) are the result, and chapters 3.1–3.4 and 5 are expected to
land mostly in the last two.

## 8.5 What is next, in order

The phase lists above are **sets**, not sequences. This section supplies the order, and the
parallelism that order permits. It is the only place in `docs/` that holds a sequence: the reports
end with "what is still not" lists and the surveys ([`35`](35-standards-landscape.md) §35.5,
[`38`](38-literature-survey.md) §38.8, [`42`](42-security-assurance.md) §42.9) end with **adopt**
verdicts, and a verdict is not a schedule.

**The finding that motivates it.** [`14`](14-review-findings.md) F11 says deterministic simulation
cannot be retrofitted, and records the constraint: virtualize clock, network and disk from the first
line of runtime code, **Phase 1**. [`13`](13-testing.md) §13.4 restates it in bold as a hard
prerequisite. It was marked `FIXED (constraint recorded)` — and the runtime then called
`SystemTime::now()` directly anyway, for three phases. The decision was correct and written down
twice. What it never had was a **position in an order**, so nothing ever came due. A list of things
to do eventually is not a plan, and `DESIGNED` is not a schedule.

### 8.5.1 The four classes

Only two of these can be got wrong, which is what makes the ordering decidable.

| Class | Definition | Scheduling rule |
|---|---|---|
| **R — retrofit** | Cost rises with delay, sometimes discontinuously (a one-way door) | By the **date it becomes expensive**. These are the only items that can be *late* |
| **F — fan-out** | Modest cost, unblocks several others | By **how many successors it frees**. Late costs throughput, not rework |
| **G — gate** | Something already scheduled is unsafe or dishonest until this exists | **Immediately before what it gates**, never after |
| **S — free-standing** | Flat cost, no successors waiting | By **value and team shape** (§8.2). Order is genuinely free |

The classification has paid for itself twice, both times on **F** items. Trait bounds had six
successors queued behind them and taking them first carried two in behind
([`27`](27-the-walls-come-down-report.md) §27.1). The native heap had the whole of Lane E behind it,
and its algebraic half moved two rows of Phase 3's exit table at once
([`93`](93-the-native-backends-report.md) §93.9).

### 8.5.2 The retrofit items still open

| Item | Recorded in | Expensive from |
|---|---|---|
| Virtualized **disk**, and **elapsed time** | F11; [`13`](13-testing.md) §13.4 | Now. The clock is supplied rather than ambient ([`44`](44-wave-0-report.md) §44.3) and the network is a seam with three implementations ([`46`](46-standard-library-report.md) §46.11); these two are what is left, and every Phase 4 bullet is a distributed-systems bullet whose stated test methodology is the simulator |
| Trusted publishing on crates.io | [`42`](42-security-assurance.md) §42.7 | The **first** `cargo publish` — a long-lived token used once is a risk already taken |
| Diverse double-compiling / bootstrappability | [`42`](42-security-assurance.md) §42.7 | The first `beck` built by `beck` (Phase 5) |
| Deterministic libm (F9) | [`35`](35-standards-landscape.md) §35.5 item 1 | Passed: there is a transcendental and a second backend. Owed rather than pending |

### 8.5.3 The traps still live

Where doing the right work in the wrong order costs the most.

1. **Phase 4 before the clock is virtualized.** Every Phase 4 bullet is a distributed-systems
   bullet whose stated test methodology is the simulator ([`13`](13-testing.md) §13.4). Building
   them against the real clock means testing them twice. The cheap move is not to build DST; it is
   to make each resource **injected** — a seam, not a simulator. The clock and the network are done
   this way; the disk is not.
2. **The two one-way doors.** The first `cargo publish` with a long-lived token, and the first
   `beck` compiled by `beck` with no independent reproducible path. Neither is imminent; both are
   free to arrange now and impossible to arrange afterwards.

A third trap is discharged and worth keeping for its *shape*: the standard library was scheduled
before error rows, and a library's signatures are exactly where the error decision shows up. Error
rows landed first, so nothing was rewritten. A bullet list makes every item look independent, and
the one that has a predecessor is never the one that says so.

### 8.5.4 What is left, by class

Everything Phase 3 listed has been taken. What remains is a short list, and none of it is **R** or
**G**:

- **Mode B's codegen** (S, and the item with a user in front of it): a wasm emitter plus what
  [`94`](94-the-client-report.md) §94.8 names.
- **The three named backend items** (S): the signal vocabulary compiled rather than read by the
  splitter, a **bounded** definition — a dictionary is a function value — and a worker that can
  answer two calls at once. [`93`](93-the-native-backends-report.md) §93.15 is the list, and the
  largest refusal class behind it is a **function value at a boundary**, which is moved by compiling
  definitions called only by *other compiled definitions*.
- **Cancelling a child blocked in the host** (S): a deadline on the
  [`net`](../compiler/crates/beck-core/src/net.rs) seam rather than a change to the scope
  ([`80`](80-structured-concurrency-report.md)).
- **The render lock** (S): the third of the shared dataflow's loose ends, deliberately left, and
  [`23`](23-incremental-views-report.md) §23.19 records that closing the other two made it *harder*
  rather than unchanged.
- **`Ord` as a trait** (S): [`54`](54-ordering.md) writes it out and explicitly does not recommend
  it.
- **Lazy routes** (S): waits on §5.1's per-component boundary existing in the language.
- **Chapters 4 and 5 of SICP** (S): no predecessors, and they never acquire any.
- **Trusted publishing** (R, above): an account setting rather than a branch.
- **Grammar-aware fuzzing and Kani proofs of the solver's invariants** (S, due rather than
  pending): [`42`](42-security-assurance.md) §42.9 pinned the first with the trigger "the bound
  lands", and the bound has landed. The second still wants a solver that has stopped moving.

Behind those, the Phase 4 gates arranged before Phase 4 rather than during it: **DST proper** on the
seams §8.5.2 names, then the operator, the replay tooling and the choreography.

### 8.5.5 Parallel workstreams

A wave is a dependency ordering, not a staffing plan. What decides which pairs can run concurrently
is not the dependency graph — it is **which files two branches would both rewrite**. The crate
layout is favourable, because [`04`](04-compiler-architecture.md)'s pass boundaries are real
directories.

| Lane | Owns | What is left in it | Collides with |
|---|---|---|---|
| **A — type system** | `beck-core/src/check/`, `ty.rs`, `core.rs`, `prelude.rs`, `iface.rs` | `Ord` as a trait, which [`54`](54-ordering.md) does not recommend — so realistically nothing | **Itself, completely** — see below |
| **B — runtime and views** | `beck-rt/`, `beck-core/src/{engine,plan,incremental,pmap,signal}.rs` | The render lock, unowned | Nothing in A, C, E or F |
| **C — front end and tooling** | `beck-syntax/`, `beck-cli/`, `beck-diag/` | **Empty.** What a new item looks like: comment-preserving printing, which `textDocument/formatting` waits on, and code actions ([`65`](65-the-editor-report.md) §65.8) | A, if a syntax decision changes what the checker sees |
| **D — process and supply chain** | `docs/`, `.github/`, `deny.toml`, `SECURITY.md`, `release/`, `install.sh` | Trusted publishing; a registry to push to; a subject `beck sign` can take over a release *listing* ([`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md)) | Nothing in code — **except that a release lands in `Cargo.toml`, a `build.rs` and `--version`** |
| **E — backends** | `beck-eval/`, `beck-llvm/`, `beck-clif/`, `beck-core/src/backend.rs`, any new codegen crate | Mode B's codegen; the three items of [`93`](93-the-native-backends-report.md) §93.15; cancelling a child blocked in the host | Nothing — the seam is why ([`19`](19-phase-1-report.md) §19.9), and sixteen consecutive Lane E changes have held that prediction, several without touching one line of `beck-rt` |
| **F — infrastructure** | `beck-infra/` | Effect-derived NetworkPolicy/RBAC/grants; Crossplane emitter; conformance rungs | Nothing |

**Lane A is strictly serial, and that is the real staffing constraint.** It is tempting to run two
language features on two branches. Do not: they rewrite `check/mod.rs` and `ty.rs` together, and
they change what `core.rs` carries. Lane A is the critical path and absorbs one pair of hands;
everything else in this section exists to keep other hands off those files. The mitigation that
works is recorded in [`27`](27-the-walls-come-down-report.md) §27.10 — traits went into
`check/traits.rs` rather than into `check/mod.rs`, and bounds then grew that file rather than the
one everybody complains about. Keep doing that, and a second Lane A branch eventually becomes
thinkable.

A third and fourth branch are viable whenever E and F are staffed. The ceiling is four, because of
the shared artefacts below.

**Two things the completed work taught that the classification did not predict**, recorded because
they are what the next ordering should assume:

- **A wave item can split.** [`10`](10-decisions.md) D21 was posed as one question and had already
  been answered as two by four phases of implementation; errors and structured concurrency were one
  row with different successors. The classification is about *cost over time*, and it is silent
  about whether an item is one item.
- **A wave item can be in the wrong lane.** `Set` and dates were filed under Lane A on the
  assumption that a standard-library item is a language item. They turned out to be two files of
  Beck and no compiler change at all, so they could have run beside a Lane A branch rather than
  behind one. Ask which files an item touches before assigning it a lane.

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

## 8.6 The ≥1% rule: which deployment realities earn support

A technology that ≥1% of developers report using in a major annual survey is a **reality**, not a
preference, and earns an explicit verdict here rather than silence — unless it is a passing fad
(the survey two years running is the test), incompatible with Beck's semantics, or a competitor
whose problem Beck dissolves. The verdicts reuse [`35`](35-standards-landscape.md)'s vocabulary —
**adopt** (named roadmap work), **supported** (works today, or falls out of vendor-neutral
emission with no specific work), **watch** (a dated pin with a named trigger), **decline** (with
the reason stated).

The list below is the Stack Overflow 2025 developer survey's cloud/infrastructure section
(percentage of all respondents; a dated claim, re-read it when the survey is), filtered to what
touches deployment. The pattern worth stating once: **most rows cost nothing because the emission
is vendor-neutral** — a standard OCI image, standard Kubernetes objects, OTLP telemetry — and the
rule's real work is naming the few places where neutrality is not enough.

| Reality (usage) | Verdict |
|---|---|
| Docker (71.1%) | supported — load-bearing: the image is an OCI layout, the Compose `Platform` is rung 2 |
| AWS (43.3%) | **adopt** — Phase 4's managed-cloud path: EKS first, the Crossplane emitter for RDS/buckets/DNS, ECS/Fargate as the named market-scope decision |
| Kubernetes (28.5%) | supported — the backend ([`06`](06-kubernetes-and-packaging.md)). The 71.1 − 28.5 gap is the ECS/Fargate argument in one subtraction: most teams that containerise never run a cluster |
| Azure (26.3%), Google Cloud (24.6%) | supported through neutrality — AKS/GKE take today's manifests unchanged, and the Crossplane emitter must stay provider-neutral (a managed-Postgres claim, not an RDS claim) so these two come for free. No provider-specific work until demand names some |
| Cloudflare (20.1%) | **adopt** for the static half — Pages is a static-host `Platform` target (Phase 4). *Watch* Workers for the server tier: its compute model may suit a stateless slice, and the trigger is the static rung existing |
| Terraform (17.8%) | supported as a *target*, excluded as a *dependency* — the licence verdict ([`07`](07-dependencies.md)) is about what Beck links, not what estates speak: the OpenTofu-compatible emitter is how a Terraform shop consumes Beck's infra without Beck depending on BUSL code |
| Firebase (13.1%) | decline as a platform — the event log, derived sync and generated auth are the parts of Firebase Beck exists to dissolve. Interop stands where it already does: `identity = external(issuer=…)` accepts any OIDC issuer |
| Prometheus (11.8%), Datadog (8.9%), Splunk (4.5%), New Relic (3.8%) | supported through OTLP — D17's telemetry speaks the one vendor-neutral wire format all four ingest; no per-vendor work, ever |
| Ansible (11.7%) | decline — VM configuration management is a layer Beck's artefacts do not have; GitOps over emitted manifests is the replacement, not a port |
| Podman (11.1%) | supported nearly for free — the image is daemonless OCI already; the cheap follow-through is rung 2 accepting a Podman socket where it finds one |
| DigitalOcean (10.7%) | supported through neutrality (DOKS + any managed Postgres); watch for a Crossplane provider gap |
| Vercel (10.6%), Netlify (5.9%) | **adopt** for the static half, decline for the server tier — §8.6.1 |
| Supabase (5.4%) | decline as a platform (same dissolution as Firebase); noted that its hosted Postgres is wire-compatible with the substrate, which costs nothing and is not work |
| Heroku (5.4%), Railway (1.5%) | watch — both run OCI containers, so the image likely deploys today; the trigger for saying so in docs is somebody executing it, per the rule that an artefact nobody has run is a design document |
| IBM Cloud (1.2%) | supported through neutrality; no named work |

### 8.6.1 Meet the bar, don't rent the platform

The strategic question under the Vercel row deserves its answer stated once: **how much of Beck is
"deploys fast to Vercel", and how much is "you don't need Vercel"?**

The premise decides it. Vercel's value is a developer-experience bar — `git push`, site is live,
previews per branch — and a compute model: stateless functions plus a CDN. Beck's semantics
*contradict* the compute model (a `durable` fold and `merge_clients()` need a long-lived server
that owns state) and *must meet* the DX bar with its own ladder: rung 0 boots in under a second,
`/_beck` is the dashboard no AppHost had to declare ([`10`](10-decisions.md) D17), `beck init ci`
plus GitOps is `git push`-to-deploy on infrastructure the team already owns. So the split is:

- **the static half is a supported target** — where the effect row says a CDN is a sufficient
  computer, emit for Pages/Netlify/Vercel and be the best citizen there is;
- **the server tier is not for rent** — its home is the generated, self-owned stack, and making
  that stack match a PaaS for time-to-first-deploy is roadmap work (the first release, the
  registry push, the Crossplane emitter — Phase 4), not a reason to bend the semantics to a
  function runtime.

Preview environments per branch — the one Vercel feature the ladder does not yet name — are
`beck up` pointed at a namespace with a TTL, which is the playground's rung C machinery
([`17`](17-playground.md) §17.3) wearing a different hat. Named here so the gap is a row on a
list rather than a surprise.
