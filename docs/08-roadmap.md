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
re-checks one module after a body edit and three after a signature change. Across the corpus, 52% of
everything placed is unplaced-pure.

Two items this phase was assigned and did not deliver were delivered later and are named here rather
than left implied: the **general slicer** (Phase 3, [`23`](23-incremental-views-report.md)) and
**native codegen** (Phase 3, [`93`](93-the-native-backends-report.md)).

## Phase 3 — Make it real for developers — **STARTED**

Every bullet below has been taken. **The exit criterion has not been met**, because it is a claim
about a person rather than a count of bullets — see the end of this section.

| Bullet | Status |
|---|---|
| **Native codegen**: LLVM and Cranelift, differential against the evaluator | **Built** ([`93`](93-the-native-backends-report.md)). `beck native --backend cranelift\|llvm`; the differential is three-way. The heap is whole — records, text, collections, closures, views, failure, generics and the four host-calling primitives — and the fifteen that are a table or somebody else's parser are **linked** rather than emitted (§93.12), so the corpus stands at **968 definitions compiled against 137 refused**. §93.15 names what is left |
| **Incremental views**: dataflow plans, arrangement sharing, SQL read models, pgwire, query fusion | **Complete** ([`23`](23-incremental-views-report.md)) |
| **Mode B client**: per-component WASM, optimistic application, freshness-typed pending state, size budget | **Built except codegen** ([`94`](94-the-client-report.md)). The mode, the bundle, the data patch, reconciliation by `seq`, a browser that runs it, an offline queue, `freshness()` and the 150 KB brotli gate. The wasm emitter exists and compiles the **scalar subset** ([`103`](103-the-wasm-emitter-report.md)); a `view` is nothing but heap, so it compiles **0 of the corpus** and the kernel still interprets |
| **Client polish**: router, forms, focus/scroll preservation, devtools | **Built except lazy routes** ([`94`](94-the-client-report.md)). A route is a field of `Session`, so there is no route table and every route is a real URL. Lazy routes wait on §5.1's per-component boundary |
| **`test` blocks and inferred mocks** | **Built** ([`22`](22-phase-3-report.md)), with page snapshots and `beck test --update` ([`22`](22-phase-3-report.md)) |
| **Structured concurrency, `Result`/error rows, pattern matching** | **Built** ([`27`](27-the-walls-come-down-report.md), [`80`](80-structured-concurrency-report.md), [`90`](90-pattern-matching-report.md), [`90`](90-pattern-matching-report.md)). `parallel:` runs its children on a thread each and stops them when one fails — including a child **blocked in the host**, which is a `Stop` predicate on the [`net`](../compiler/crates/beck-core/src/net.rs) seam rather than a change to the scope ([`80`](80-structured-concurrency-report.md) §80.14). What is left is the compiled half: a worker holds its pipe for a whole call ([`93`](93-the-native-backends-report.md) §93.15) |
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
| "Can I relate two collections?" | **Yes for a lookup, for a filtered group, and for that group's size, and you write all three as the loop you would have written anyway** ([`99`](99-the-data-tier-means-of-combination.md) §99.6) — `for x in xs:` whose body asks `map_get(ys, k(x))` compiles to a `Join` with an index; one whose body filters `ys` by an equality against `x` compiles to an `arrange_by` and a join that answers with the group; and one that only asks `list_len` of that filter is answered from a maintained count with no group built. All are maintained from both sides and `beck explain query` shows them. `sum`, `min` and `max` per group and `distinct` are still missing, and `beck explain cost` says so on the loop that pays for it |
| "Can my DBA see the data?" | `psql` against the read models ([`23`](23-incremental-views-report.md)) — one table per collection, derived, no annotation |
| "How do I make it look like anything?" | **Badly, and this is the newest row.** The stylesheet a running application serves is eight rules hard-coded in `beck-rt/src/css.rs`; `css:` appears in the tour and has no parser. `class=` takes a list now and `beck explain style` will tell you every class your pages can carry — but nothing emits a stylesheet from that set yet, so the answer to the question as asked is unchanged. Tailwind styles a Beck page with no configuration today, and its scanner cannot survive a package manager ([`104`](104-styling-and-the-component-library.md) §104.3), so the answer until §8.5.4's styling cluster lands is "bring npm" — which for a project whose product is one static binary is the wrong answer, not a smaller one |
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
- OpenTelemetry cross-tier tracing on by default; `beck tune` right-sizing. The two export gaps
  from [`101`](101-the-public-surface.md) §101.8: a push exporter (`BECK_OTLP_ENDPOINT`, currently a
  corrected-away claim) and OpenMetrics exposition — both export what is already recorded, neither
  adds a measurement to the fold's path.
- **The public surface** ([`101`](101-the-public-surface.md), D28), in §101.10's order: first the
  `@public` boundary itself — what is exposed, versioning and auth semantics, the diagnostics for
  what may not cross — **with §101.7's two prerequisites landed before any form ships**: F15's
  connection/subscription quotas and the inbound TLS posture, each closing its
  `pending_security.rs` absence and correcting [`43`](43-threat-model.md) §43.4 in the same
  change. Then `@public(rest)` (OpenAPI 3.1 + RFC 9457, closing
  [`35`](35-standards-landscape.md) §35.5's blocked item) and `@public(mcp)` (the command union as
  tools, views as resources, effect rows in the tool annotations), each gated by a foreign reader
  per §101.9, and the rest of §101.7's edge ledger — idempotency, `seq`-derived ETags, the response
  vocabulary, deprecation headers — arriving with the forms that need them.
- **The data tier's means of combination** ([`99`](99-the-data-tier-means-of-combination.md)):
  `join`, `group by` and the aggregates, which the view algebra has never had — every operator it
  implements takes one collection, so a program relating two of them escapes the algebra into a
  per-element function that captured the accumulator and is reapplied on every event. The order is
  §99.9's and its **first item is a gate that goes red today**, not an operator. This is what §8.4's
  Phase 5 TPC-H row has always been conditioned on, and it had never been assigned.
- **The log's own lifecycle, and the substrate that reads it** ([`10`](10-decisions.md) D3's
  obligations, [`09`](09-risks-and-open-questions.md) R6): segment archival to Parquet on object
  storage, bounded retention for the stores that opt down from `retain=forever`, and **DataFusion**
  over the archive ([`05`](05-tier-lowering.md) §5.3's analytical row,
  [`07`](07-dependencies.md) §7.4's pinned choice). Committed in five documents and scheduled in
  none until now; §8.5.4 carries the order and the reason it is a gate rather than a preference.
- **The decision record** ([`100`](100-placement-at-runtime.md) §100.5): a queryable account of what
  the compiler and runtime *chose* and why, projected as a read model so `psql` and any BI tool
  already read it. A facility rather than a feature of one caller — fusion, the plan solver
  (§99.8) and placement each make choices nothing records. Telemetry reports **quantities**; nothing
  reports a **decision**, and `beck explain` answers *why is this here* only about source. This is
  its run-time half.
- **Query fusion gets an off switch**, the one hole in a principle the rest of the runtime keeps:
  `AppConfig` turns off the incremental view engine and the shared dataflow without a recompile,
  while `Plan::compile` always fuses and `Plan::unfused` is reachable only from
  `beck explain query --unfused` and the test harness ([`100`](100-placement-at-runtime.md) §100.5).
- **Placement proposals from measurement** — [`100`](100-placement-at-runtime.md)'s level P1, the
  only level that changes no runtime behaviour: the deployment measures, and a placement change
  arrives as a `beck.lock` diff a human accepts. `beck tune` pointed at placement, riding the replay
  tooling above.
- Multi-arch images; air-gapped install; OCI package registry via ORAS (§6.7).
- **FFI and the ecosystem answer** ([`105`](105-the-ecosystem-answer.md)): C ABI both directions; JS
  interop for the client tier; a Python bridge (§9.2) — the ecosystem-access question is
  existential, so give it real headcount. What [`105`](105-the-ecosystem-answer.md) changes is
  *where the headcount goes*: measured against four ecosystems' most-downloaded packages, the
  bridge is the answer for the smallest of the four categories, and the sidecar structurally cannot
  serve the data tier at all (a bridged call carries an effect; §3.7 makes folds and views pure).
  So the bullet's own items are the **link** half — BLAS behind a primitive, a YAML reader, an S3
  signer, since `boto3` is PyPI's single most-downloaded package — plus the diagnostic that tells
  someone reaching for the bridge inside a view why it may not go there and where it may.
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
- **Placement chosen at run time, within the set the compiler proved legal** —
  [`100`](100-placement-at-runtime.md)'s levels P2 and P3: one choice per process, then one per
  subscriber, so a browser on a slow link and one on a fast link render the same component in
  different modes at the same moment. The artefact ships the **candidate set** rather than the chosen
  tier, so no runtime decision can widen what §3.5 proved; the ceiling defaults to `static` and a
  gate gives that default a byte-for-byte meaning; every move lands in the decision record above.
  Mode B exists ([`94`](94-the-client-report.md)) and [`cost.rs`](../compiler/crates/beck-core/src/cost.rs)
  has charged a crossing the *minimum* of its two ends since Phase 2 while commenting that "the
  minimum is a prediction rather than a choice" — this is the choice. Legal above the session cut
  only, for §94.2's reason. **P4–P7 are post-1.0** and §100.11 says why each waits.
- Editor support beyond VS Code; debugger integration (DAP) with cross-tier stepping.
- The public surface's second wave ([`101`](101-the-public-surface.md) §101.10): `@public(events)` —
  the publish half of the enterprise bus per §101.6 (CloudEvents identity from `(context, seq)`,
  AsyncAPI as the registry artefact, webhooks first with the declared-subscriber-host rule,
  delivery ledger as a fold), paired with [`30`](30-bounded-contexts-and-microservices.md) §30.4's
  `ingest` so a context speaks the bus in both directions — then `@public(grpc)`, and
  **`beck trace`** — replay-derived maximalist telemetry per §101.8, full-resolution OTLP spans
  emitted from a replay rather than from the serving process.
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
8. **Anything the compiler or runtime decides for you can be switched off, and the off switch is
   proved rather than promised.** A language whose selling point is that it makes choices on your
   behalf owes you the ability to stop it — otherwise a wrong choice is unworkaroundable and the
   whole proposition inverts. `AppConfig` is the standard: the incremental view engine and the
   shared dataflow can each be turned off at run time, without recompiling, and the doc comments say
   *why* the switch exists rather than merely that it does — "a *switch* rather than a fact because
   it is also a memory-for-time trade". The proof half matters as much as the switch, since a
   default nobody has run is a claim, so `off` paths belong in the gates beside the fast ones.
   **Currently strong with one hole**: query fusion cannot be turned off outside the test harness
   ([`100`](100-placement-at-runtime.md) §100.5), which is scheduled in Phase 4.
9. **Anything decided for you is auditable after the fact, in production.** `beck explain` is this
   project's best habit — `place`, `flow`, `wire`, `query`, `cost`, `incremental`, `sql`, `render`,
   `deploy`, `error` — and it has **one half**: it answers *why is this here* about source. The
   other half, *why did it do that*, has no answer anywhere. Telemetry reports quantities; no
   facility records a **decision** — what was chosen, when, from what alternatives, on what
   evidence. Every choice the system makes unbidden is one somebody will eventually have to debug,
   and the cost of retrofitting an audit trail is paid during the incident. Phase 4's decision
   record is the general answer; new work that decides should feed it rather than print its own
   format.

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
| **5** | **TPC-H/ClickBench** on read models once §5.3's engines exist — **two of them, where this row long named one**. TPC-H's 22 join-heavy queries wait on the view algebra's means of combination (**Phase 4's data-tier bullet**, [`99`](99-the-data-tier-means-of-combination.md)); **ClickBench scans a fixed dataset no fold holds in memory**, so it waits instead on the archive substrate §5.3's analytical row names — Phase 4's log-lifecycle bullet, and §8.5.4's G item. Each was assigned to a phase only after being named here, which is the point: a scheduled measurement whose prerequisite nothing builds is the "table of intentions" this row's own DDIA entry warns against; the incremental-view workload §25.2 records as having *no* standard, which we would be defining rather than borrowing. SICP stage 4 | The Phase 5 suite above, and the expressiveness result — including the rows §25.5 forecasts Beck will lose (§2.4–2.5 generic operations, chapter 4's evaluator), which are published or the exercise was not run honestly |

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

Everything Phase 3 listed has been taken. What remains follows — ordered, because this section is
the one place in `docs/` that holds an order. The list stopped being all-**S** when
[`12`](12-standards-and-conformance.md) was audited against the tree: the audit surfaced one **F**
item larger than anything else here, one **G**, and a ledger of small artefacts behind chartered
rows.

- **The data tier's means of combination** (F, and still the **largest item left** —
  [`99`](99-the-data-tier-means-of-combination.md) §99.7 lists five already-written-down items it
  closes). **The join has landed, and both of its indexes**: §99.9's gate (item 1), the `Join`
  operator with the bilinear delta rule (item 4), the recognition that emits it for a loop nobody
  wrote as a join (item 5), and **`arrange_by`** (item 3), which moved *behind* the join rather than
  in front of it because the first join's right side is a `Map` whose `map_values` arrangement was
  already keyed by the join key. `27-review.beck` gets the first with no edit, and its gate reports
  **19 units of maintenance at 200 rows and at 1,600, against 415 and 3,215 with the operator
  switched off**; `examples/board.beck` gets the second with no edit, and it is **4.5–4.9× less work
  per event** with its cards spread over the columns — but **1.1×** with all of them in one, because
  `arrange_by` removes the scan and leaves the group, which is what `group by` takes — **and its first
  aggregate has taken it**: `list_len` over the same filter is answered from a tally the join keeps,
  so `corpus/35-workload.beck` shows every person's open-issue count while copying **one** entry out
  of an arrangement at 200 issues and at 1,600. What is left, in §99.9's order: **`sum`, `min` and
  `max` per group**, each waiting on a surface rather than on a delta rule (§99.9 item 6 has the
  design for `min`/`max` and the `Int`-versus-`Float` decision `sum` needs), then `distinct` and
  difference, fusion for the new operators, and the read-model SQL compiling into the plan. This was a Phase 4 bullet from the
  day [`99`](99-the-data-tier-means-of-combination.md) was written and was never in *this* list,
  which is the same defect §8.5 opens by describing one level down — a phase is not a position. It
  lives in **Lane B** (`engine.rs`, `plan.rs`, `relate.rs`) and contends with nothing in Lane A.
  §99.8's convergence rungs interleave with it rather than following it, and **rungs 0–1 have now
  failed to come due twice**: a join inferred from a loop has the loop's order to preserve, which
  fixes which side is the left before any cost is consulted. An inferred surface postpones the
  solver, and §99.8's ladder says so now rather than predicting otherwise.
- **A columnar value, and Arrow** (F, after the aggregates): the second representation
  [`105`](105-the-ecosystem-answer.md) §105.10 argues for — `Value::List(Arc<Vec<Value>>)` is 16
  boxed bytes an element, which is right for a keyed arrangement and wrong for a million doubles.
  One change discharges four commitments: [`07`](07-dependencies.md) §7.4's DataFusion choice,
  §8.5.4's own Parquet-archival G item below (Parquet is Arrow written down), the numeric-interop
  gap, and the aggregates' own representation. It is listed **after** the aggregates because an
  aggregate is what makes a column worth having, and **before** the G item because that item cannot
  start without it.
- **What the macro interpreter unblocked** (F — the interpreter itself is **built**,
  [`102`](102-the-macro-interpreter-report.md), with its G-class sandbox gate beside it). A macro
  body is ordinary Beck now, so the successors this item existed to free are the item:
  - **`derive` and `.as_model()`** (Lane A, and the largest): a `typed macro` receives the AST
    *with inferred types attached* ([`02`](02-syntax.md) §2.4), which the untyped interpreter runs
    before. This is the piece that needs the checker's answers to reach a macro body, and it is
    what retires the compiler-provided `ui:` special case standing in for a user-written macro
    (D22).
  - **§2.5's typed literal macros** (`sql"…"`, `html"…"`, `regex"…"`) — the DSL escape hatch, and
    the mechanism the security suite already points at for SQL and HTML. Sugar over a macro call
    (§2.3's table) plus a parse at compile time, so this one is free of Lane A.
  - **`inject`/`unsafe_macro`**, the deliberate-capture escape, and **nested quoting's
    `(quote depth node)`**.
  - **A `Node` a *running* program can hold**: a `quote` that survives expansion is still `B0332`,
    so code-as-data is compile-time only. [`12`](12-standards-and-conformance.md) §12.10 records
    which half of D9 that leaves open.
  - **Salsa-memoised expansion**, §2.4's "macro-heavy code must not destroy IDE latency" — which
    now has something to memoise.
- **Mode B's codegen: the heap on a wasm target** (S, and the item with a user in front of it).
  The emitter is written and the scalar subset compiles, in a real engine, against the
  tree-walker ([`103`](103-the-wasm-emitter-report.md)) — and it compiles **0 of the corpus's 195
  definitions**, because an application is records, lists and a page. What is left is therefore one
  thing and it is the big one: a value representation in linear memory, string and collection
  primitives, closures through an indirect call table, and §5.1's unanswered choice between the GC
  proposal and a refcounting discipline. Behind it, in order: the four host effects as imports,
  bundle format 2 with the type table
  ([`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) anticipated both), and the kernel
  loading a compiled component instead of interpreting one. The WebAssembly spec-suite obligation
  [`12`](12-standards-and-conformance.md) §12.3 pins to core 3.0 lands with it.
- **The three named backend items** (S): the signal vocabulary compiled rather than read by the
  splitter, a **bounded** definition — a dictionary is a function value — and a worker that can
  answer two calls at once. [`93`](93-the-native-backends-report.md) §93.15 is the list, and the
  largest refusal class behind it is a **function value at a boundary**, which is moved by compiling
  definitions called only by *other compiled definitions*.
- **A fast path in front of the exact sine** (S, and the successor of the item that was first
  here): `sin` and `cos` are computed rather than asked for and are correctly rounded
  ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)), which closed
  the retrofit item and the defect under it. What that costs is **~640 ns against a platform
  libm's 11 ns**, and the missing piece is not a faster exact path — that is what an exact path costs —
  but Ziv's technique in front of it: a double-double first pass answers unless its own error
  bound leaves the rounding in doubt, and the exact path arbitrates the rest, which is under one
  call in 2^45. It **changes no answer**, because the exact path stays the definition of what the
  answer is, and that is why it is S rather than R: nothing depends on it and no claim is false
  without it. The measured impact today is 0.1% of `awfy/cd.beck`, the only program in the tree
  that calls either function — below that benchmark's own variance. Lane E, or wherever
  `beck-prim` is held to sit.
- **A worker that can answer two calls at once** (S, and it is what is left of cancelling a child
  blocked in the host): the tree-walker stops a child inside an outbound call now
  ([`80`](80-structured-concurrency-report.md) §80.14), and `beck-llvm` passes `Stop::never()`
  because a worker holds its pipe for a whole call — so two children that both reach compiled code
  serialise before cancellation is even the question. Named already in
  [`93`](93-the-native-backends-report.md) §93.15's list above; recorded here because it is now
  the only thing between `parallel:` and the native backends.
- **The render lock** (S): the third of the shared dataflow's loose ends, deliberately left, and
  [`23`](23-incremental-views-report.md) §23.19 records that closing the other two made it *harder*
  rather than unchanged.
- **`Ord` as a trait** (S): [`54`](54-ordering.md) writes it out and explicitly does not recommend
  it.
- **Lazy routes** (S): waits on §5.1's per-component boundary existing in the language.
- **The presence roster's second clock** (S, and it has a successor, which is why it is here rather
  than in a report's tail): [`48`](48-identity-report.md) §48.13's first row — a `(seq, roster)` pair
  or a render epoch — is what would let the shared dataflow hold the roster once instead of per
  subscriber, and §48.13 says "one file would change". It was in no phase. Its successor is
  [`99`](99-the-data-tier-means-of-combination.md) decision 3: presence moves when `seq` does not, so
  a join against presence is the one case the algebra above must refuse, and it should refuse it with
  a diagnostic rather than a surprise until this exists. **`awareness(f)` now sits on the same
  clock and doubles what this row is worth**: a second roster, per subscriber for the same reason,
  moving more often than the first because it follows navigations rather than connections.
- **Awareness over a client-local value** (M): `awareness(f)` is built for `f : Session -> T`
  ([`10`](10-decisions.md) D6, `corpus/33-awareness.beck`), which is *who is looking at what*. A
  cursor or a selection needs a value the client holds and can publish, and there is none —
  `beck-patch.js` listens for five events and `mousemove` is not among them. That prerequisite is
  the same one the client-local fold for interface state has
  ([`104`](104-styling-and-the-component-library.md) §104.8), so the two are **one piece of work**:
  a client-local stream, a client-placed non-durable fold over it, and `awareness` accepting a
  signal as well as a function.
- **Comment-preserving printing** (S, due rather than nice). **Done.** Ordinary `#` comments were
  dropped by the lexer, so `beck fmt` deleted them — a formatter that eats comments is one nobody
  runs twice, which is why `textDocument/formatting` was withheld rather than missing. They are now
  collected by position in the pass that already collected documentation, in the three positions a
  comment holds, and the LSP offers formatting in the same change so the fix has a caller. It had
  been in §8.5.5's Lane C cell since that table was written, and **a lane is a collision map rather
  than an order**, which is how it stayed unpositioned for as long as it did.
- **Chapters 4 and 5 of SICP** (S): no predecessors, and they never acquire any.
- **Trusted publishing** (R, above): an account setting rather than a branch.
- **Grammar-aware fuzzing and Kani proofs of the solver's invariants** (S, due rather than
  pending): [`42`](42-security-assurance.md) §42.9 pinned the first with the trigger "the bound
  lands", and the bound has landed. The second still wants a solver that has stopped moving.
- **The standards ledger** (S — small artefacts, each a day rather than a phase, from
  [`12`](12-standards-and-conformance.md)'s audit; each closes a chartered row, and the row names
  it back). Free now: a JSONTestSuite-class vector run for the JSON library; an Autobahn-class
  vector run for the WebSocket channel; a test that a TLS-1.2-only peer is refused; **the
  vulnerability matrix**, whose CWE half is **done** ([`43`](43-threat-model.md) §43.8, gated by
  `docs.rs::every_test_the_vulnerability_matrix_names_exists`) and whose ISO/IEC 24772-1 half is
  **blocked on the standard's text** rather than on time — paywalled, not in this tree, and recorded
  as blocked in the matrix itself ([`35`](35-standards-landscape.md) §35.2); a
  Prometheus exposition endpoint beside the JSON dashboard; a Scorecard workflow and REUSE
  per-file metadata; the CLI exit-status table; a semantic-conventions check over the attributes
  telemetry actually emits. Blocked, and recorded as blocked rather than listed as free:
  OpenAPI 3.1 + JSON Schema 2020-12 + RFC 9457 (on the `@public(rest)` emitter, Phase 4); the OCI
  distribution conformance suite (on a registry push — the managed-cloud path's item 2); the
  OpenID Foundation suite (pre-1.0 trigger); the two-independent-runner reproducible release
  build and SBOM signing (on the first tag); the SIGTERM-begins-drain contract (on the
  choreography defining what drain is).
- **The public surface's first form** (G then F — [`101`](101-the-public-surface.md) §101.10's
  order): no `@public` annotation exists in the compiler and no OpenAPI, MCP, gRPC or AsyncAPI
  artefact has ever been emitted ([`101`](101-the-public-surface.md) §101.11). It is a Phase 4
  bullet and was not in this list. **G
  first**: §101.7's two prerequisites — F15's connection/subscription quotas and the inbound TLS
  posture — each close a `pending_security.rs` absence and correct [`43`](43-threat-model.md) §43.4
  in the same change, and shipping a public form over an unquotaed socket is the dishonest ordering
  G-class exists to prevent. Then the `@public` boundary itself, then `@public(rest)`.
- **Styling and the component library** ([`104`](104-styling-and-the-component-library.md), D29) —
  eight items, in this order, and the first two are defects rather than features
  ([`DEFECTS.md`](../DEFECTS.md)). They are grouped here because they share a lane and a document,
  not because they are one item; the first four are each smaller than the paragraph describing
  them.
  1. **The SVG namespace** (S, and it was **first** because it was the cheapest thing on this list
     that unblocks a whole component class). **Done.** `beck-patch.js` built patched-in subtrees
     with `document.createElement`, so a chart painted once and did not render after the patch that
     changed it; it now takes the namespace from the tag where the tag opens one and from the
     destination otherwise. Gated by `browser.rs::a_patched_in_chart_is_still_a_chart`, which
     asserts the **laid-out width** of a patched `rect` in two positions — a fix that reads only the
     tag fails the second ([`104`](104-styling-and-the-component-library.md) §104.9). Lane B.
  2. **A vocabulary for `ui:`** (G). **Done.** `beck_macro::vocabulary` is the table — five events,
     the HTML and SVG attribute names, and the elements §12.4's checks read — with `B0217` refusing
     an event the client does not listen for, `B0218` an attribute HTML does not have, and
     `data_…`/`aria_…` admitted by prefix, because the escape hatch for an attribute that is
     genuinely yours is HTML's own. It was **G** because §12.4's three accessibility checks are
     scheduled over the same typed tree and a tree that accepted `on_keydown` and `cls=` in silence
     could not honestly carry an accessibility claim. **Those three have since landed** —
     `B0219`/`B0220`/`B0221`, reading a `NAMING` table beside `ELEMENTS` rather than bringing one —
     and what they found on their first run says the ordering was right: every example in this tree
     with a text input had labelled it with a placeholder and nothing else. The events are held to the client's own listener table by a test that reads
     `beck-patch.js`, in both directions
     ([`104`](104-styling-and-the-component-library.md) §104.8). A **table** rather than expander
     code, because typed macros above retire the compiler-provided `ui:` special case (D22) and a
     user-written `ui:` has to be held to the same names. Lane C, with a `beck-macro` half.
  3. **`class=` takes a list, and `Class` is a type** (F): the prerequisite for everything else in
     the styling half, and what makes the editor's existing completion, hover and rename answer for
     utilities without an extension ([`65`](65-the-editor-report.md)). **The list and the analysis
     are done; the type is not.** A list where HTML defines a space-separated value is joined in the
     `ui:` lowering, so every backend agrees by construction and the seam learned nothing, and
     `beck_core::style` enumerates every class that can reach a `class=` — through a call and through
     both arms of an `if`, which is how every dynamic class in this tree is already written.
     `beck explain style` prints the set and, beside it, every site where a class is *built* rather
     than named, with which of three reasons it was. A `Class` **type** has nothing to be checked
     against until item 4's table exists, so it moved behind it rather than in front: a type whose
     checking is empty is a scaffold. Lanes A and C.
  4. **The utility table and the sheet emitter** (S): exact extraction over the typed tree,
     `beck build` writing the stylesheet, `styles = none` turning all of it off, and the
     differential gate that holds the accepted table against Tailwind's own compiler
     ([`104`](104-styling-and-the-component-library.md) §104.4). This is what retires
     `beck-rt/src/css.rs`. **Both paths gated**, per §8.3 — the switched-off program is compiled and
     run beside the switched-on one, because a default nobody has run is a claim. Lane B, with the
     table generated into Lane A's tables.
  5. **The theme as a Beck value**, and a styled `beck new` (S): tokens as a record generating both
     the theme block and the accepted names, so renaming a brand colour is a rename. Lane B.
  6. **Where interface state lives** (F, and a **decision before an implementation**): a modal's
     open flag, a combobox's highlighted option and a table's sort column have nowhere to live but
     the durable log, and §104.8's three candidate answers — a client-placed non-durable fold, the
     URL, or the document itself — are not equivalent. This wants a D-number of its own before code,
     and it is what the last two items wait on. `DEFECTS.md::non-durable-fold` is the separable
     defect underneath it: D1's construct is decided and silently unbuilt. Lane A.
  7. **Focus as a function of state** (S): an attribute the view writes and the client reconciles,
     never a `focus()` effect — the page stays a pure function of state, which is the property the
     design is for. Every APG pattern worth having moves focus, so this and item 6 are what the
     combobox, the menu and a custom date picker are actually blocked on. Lanes A and B.
  8. **The kit** (S, and **last**): table, chart, dialog, accordion, tabs, each shipping the
     WAI-ARIA Authoring Practices keyboard table as its own test. Whether it is a `lib/` directory
     or a tarn is a decision to take *after* items 3–5 land rather than in advance
     ([`104`](104-styling-and-the-component-library.md) §104.10).
- **TLA+ specifications, model-checked in CI** (G): the deploy choreography and the
  subscription-resume protocol, written and checked **immediately before the operator is built**,
  never after — [`12`](12-standards-and-conformance.md) §12.9 stated this as present for as long
  as the document existed, and no `.tla` file has ever existed. G-class means it now sits directly
  in front of the Phase 4 operator work.
- **The log's own lifecycle, and the substrate that reads it** (G, and **last** because what it
  gates is): segment archival to Parquet on object storage, bounded retention for stores that opt
  down from `retain=forever`, and DataFusion over the archive. Five documents commit to this — D3's
  obligations ([`10`](10-decisions.md)), R6's mitigation ([`09`](09-risks-and-open-questions.md)),
  [`05`](05-tier-lowering.md) §5.3's analytical row, [`07`](07-dependencies.md) §7.4's pinned
  dependency choice, and [`12`](12-standards-and-conformance.md)'s chartered Arrow/Parquet row — and
  **not one of them gave it a position in an order**, which is the failure this section opens by
  describing. Nothing is built: there is no `datafusion`, `parquet` or `arrow` dependency in the
  workspace, `durable` takes one argument so neither `retain=` nor `snapshot=` parses
  ([`03`](03-type-and-effect-system.md) §3.7), and the `compact` the engine does have is the
  arrangement trace's rather than the log's ([`23`](23-incremental-views-report.md) §23.11). **G
  rather than S** because §8.4 already schedules ClickBench against read models for Phase 5, and
  ClickBench scans a fixed dataset that no `durable` fold holds in memory: without the archive that
  row is unrunnable rather than merely unflattering. **Its predecessor is now named**: the columnar
  value above, because Parquet is Arrow written down and there is nothing to archive *from* until a
  Beck value can be a column. The **retention** half is separable and S — a
  language surface and a store policy, with no benchmark waiting on it — and it is the half D3 makes
  optional, since `retain=forever` is the default and tiering is what keeps that affordable.

Behind those, the Phase 4 gates arranged before Phase 4 rather than during it: **DST proper** on the
seams §8.5.2 names, then the TLA+ gate above, the operator, the replay tooling and the choreography.

### 8.5.5 Parallel workstreams

A wave is a dependency ordering, not a staffing plan. What decides which pairs can run concurrently
is not the dependency graph — it is **which files two branches would both rewrite**. The crate
layout is favourable, because [`04`](04-compiler-architecture.md)'s pass boundaries are real
directories.

| Lane | Owns | What is left in it | Collides with |
|---|---|---|---|
| **A — type system** | `beck-core/src/check/`, `ty.rs`, `core.rs`, `prelude.rs`, `iface.rs` | **Typed macros and `derive`** (§8.5.4's first item — a macro body that receives inferred types is a checker change, whatever crate the body runs in); `Ord` as a trait, which [`54`](54-ordering.md) does not recommend; the styling cluster's items 3, 6 and 7 (§8.5.4) — `Class` as a type, where interface state lives, and focus as an attribute | **Itself, completely** — see below |
| **B — runtime and views** | `beck-rt/`, `beck-core/src/{engine,plan,incremental,pmap,signal}.rs` | The render lock, unowned; the styling cluster's items 1, 4 and 5 (§8.5.4) — the SVG namespace in `client/beck-patch.js`, the sheet emitter that retires `css.rs`, and the theme | Nothing in A, C, E or F |
| **C — front end and tooling** | `beck-syntax/`, `beck-cli/`, `beck-diag/` | Code actions ([`65`](65-the-editor-report.md) §65.8); the standards ledger's front-end vectors (§8.5.4). Comment-preserving printing and the `ui:` vocabulary were this lane's and are done | A, if a syntax decision changes what the checker sees |
| **D — process and supply chain** | `docs/`, `.github/`, `deny.toml`, `SECURITY.md`, `release/`, `install.sh` | Trusted publishing; a registry to push to; a subject `beck sign` can take over a release *listing* ([`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md)) | Nothing in code — **except that a release lands in `Cargo.toml`, a `build.rs` and `--version`** |
| **E — backends** | `beck-eval/`, `beck-llvm/`, `beck-clif/`, `beck-wasmgen/`, `beck-core/src/backend.rs`, any new codegen crate | Mode B's codegen; the three items of [`93`](93-the-native-backends-report.md) §93.15, of which the two-calls-at-once worker is now what blocks cancelling a compiled child | Nothing — the seam is why ([`19`](19-phase-1-report.md) §19.9), and sixteen consecutive Lane E changes have held that prediction, several without touching one line of `beck-rt` |
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
- **A wave item can be in the wrong lane**, and it has now happened twice. `Set` and dates were
  filed under Lane A on the assumption that a standard-library item is a language item; they
  turned out to be two files of Beck and no compiler change at all. **The macro interpreter** was
  filed there too, on the stronger-sounding reasoning that it "changes what reaches the checker" —
  and it touched `beck-macro/`, one table in `beck-diag/`, and two new test suites, with **not one
  line of `check/` or `ty.rs`**. What reaches the checker is a `Node` either way; the lane rule is
  about *files*, and a plausible argument about consequences is not one. Ask which files an item
  touches before assigning it a lane.

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

### 8.5.6 Keeping this list true, as a procedure rather than a habit

§8.5 opens with the finding that a decision written down twice still never came due, because it
never had a position in an order. The corollary is that **this list decays**, in two directions at
once, and both are invisible from inside a document:

| Direction | What it looks like | Why it survives |
|---|---|---|
| **A document is behind the code** | A row says "absent" about something that was built | Whoever built it corrected the report they were writing and not the four documents that mentioned it. `pending_security.rs` catches this for security controls **and nothing catches it anywhere else** |
| **The code is behind the documents, and nothing is scheduled** | Several documents commit to a thing, all of them truthfully saying it is unbuilt, and no phase lists it | Every individual document is *correct*. The defect only exists between them, so no reader of any one document can see it |

The second is the one §8.5.4's log-lifecycle item was found by — "five documents commit to this and
not one gave it a position in an order" — and it has now been found four more times, so it is a
pattern rather than an incident.

**The sweep, run 2026-08-16.** From `docs/`, collect every claim of absence:

```console
$ grep -rniE "nothing is built|not built|does not exist|has never existed|unbuilt|still absent" docs/*.md
```

then for each candidate do two things a grep cannot: **confirm it against the tree** (the claim may
be stale), and **look for it in §8.5.4** (a phase bullet is not a position, and neither is a cell in
§8.5.5's lane table). What that produced:

| Found | Direction | Outcome |
|---|---|---|
| Macro expansion fuel (F17) | Document behind code | [`42`](42-security-assurance.md) §42.4 said "absent — nothing in `beck-macro` bounds expansion". `MAX_EXPANSION` is 100,000 nodes, `B0214` refuses, `macro_bomb.rs` gates it both ways, and `pending_security.rs` had already deleted its own test saying so. **Corrected in place** |
| Deterministic transcendentals (F9) | Unscheduled | §8.5.2 said "owed rather than pending" for three phases. Placed as §8.5.4's first item and recorded as a **defect** rather than an absence, because the replay-determinism claim it falsified is one this project ships — and **built** in the change that placed it ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)). It is the sweep's shortest path from *nobody scheduled this* to *it is done* |
| The data tier's algebra | Unscheduled in *this* list | A Phase 4 bullet, never in the order. Now placed, in Lane B, and the largest item left now that the macro interpreter has landed. **The join half is built**, and the sweep's third case of carrying an item from *nobody scheduled this* to *it is done*. The instrument it was found with — the cost report's own undercount — was itself a defect, fixed, and its register entry left with it |
| Charting | Unscheduled **and undesigned** | Not in `docs/` at all until [`105`](105-the-ecosystem-answer.md), which ranked it, and [`104`](104-styling-and-the-component-library.md), which found the same defect from the UI side and owns the fix. Now the styling cluster's item 1 and `DEFECTS.md::svg-namespace`. **Two independent sweeps reached one line of JavaScript from opposite directions**, which is the strongest evidence in this section that the procedure works |
| A columnar value / Arrow | Unscheduled | Committed in five documents as a *dependency choice*; nothing ever built the value it would apply to. Now placed, as the log-lifecycle G item's named predecessor |
| The presence second clock | Unscheduled | [`48`](48-identity-report.md) §48.13's first row, in no phase, with a successor in [`99`](99-the-data-tier-means-of-combination.md) decision 3. Now placed |
| Comment-preserving printing | In a lane, not in the order | §8.5.5's Lane C cell since that table existed. Placed, recorded as a defect, and **built** — the second item the sweep carried from *nobody scheduled this* to *it is done* |
| The `@public` surface | Unscheduled in this list | A Phase 4 bullet with its own internal order (§101.10) and no position here. Now placed, G-first |
| `cls=` "compiles and reaches the browser" | Document behind code | [`11`](11-language-tour.md) §11.6 and [`README.md`](README.md)'s index both said `ui:` checks no attribute names, for as long as the vocabulary that refuses `cls=` had existed. **Corrected in place, and this one now has a gate**: `docs.rs::a_document_showing_a_refused_spelling_names_the_diagnostic_that_refuses_it` reads `beck_macro::vocabulary`'s own alias tables, so a new alias is covered the day it is added. It goes red on both documents as they stood |
| [`01`](01-vision-and-premise.md)'s canonical example | Document behind code, and *not* to be fixed | The vision's flagship program has not compiled since the surface settled — method calls, `f"…"`, `cls=`, `css:`, an unlabelled `input` — and said nothing about it. It is a faithful translation of the original sketch and rewriting it would break the claim the section makes, so what it gained is the sentence saying so and a pointer to `examples/todo.beck`, which is the same program in the language and is gated. **Not every stale document is a document to update** |

**A third direction, found by running the sweep on numbers instead of on absences**, and a fourth
under it: a document can show **code the compiler now refuses**, which is the same decay with a
different instrument and the one class of it that turned out to be mechanically checkable (the row
above). Both were found the same way — by asking, after a change that refuses something, where else
in the tree that spelling appears. A document can
also be behind the code by *quoting a figure that has since moved*, which neither table above
describes because every sentence involved stays grammatically true. Re-running the measurement suites
on 2026-08-17 found three at once — the corpus's placement share (43%, quoted as 44%, actually 52%),
the native backends' compiled-definition count, and how many corpus programs compile their `view` —
and all three had drifted for the same reason and in the same direction: the corpus grew and nothing
re-read the number. This is the more insidious direction, because a stale *absence* is contradicted
by the tree and a stale *number* is contradicted by nothing until somebody re-runs the command.

**The gate this wants**, and it is now one gate for both. Everything above is a procedure a person
has to remember, which is the category this project converts to tests. The shape is `docs.rs`'s:
every `docs/` sentence claiming something is unbuilt names either a §8.5.4 item or an explicit
`unscheduled:` marker, and the marker is what a reviewer argues with; every `docs/` sentence quoting
a figure a measurement suite prints names the suite, and the suite's own output is what the gate
compares against. That is not built, and saying so here rather than in a table of intentions is the
point — it is **S**, small, and it is the only item in this section that would stop the section being
needed.

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

### 8.6.2 The same rule, applied to libraries

§8.6 has always been scoped to the survey's cloud and infrastructure section, and the scoping was
never argued — it is simply where the question came up. The rule itself says nothing about
infrastructure: *a technology ≥1% of developers report using earns an explicit verdict rather than
silence*. Applied to the same survey's **Other frameworks and libraries** section
([`105`](105-the-ecosystem-answer.md) §102.4), it convicts this document of exactly the silence it
was written to prevent.

**Every entry in that section, in its order.** All 39 clear 1%, so the rule admits no shortlist —
picking the interesting ones is how the silence happens.

| Reality (usage, [SO 2024](https://survey.stackoverflow.co/2024/technology)) | Verdict |
|---|---|
| .NET (25.2%), .NET Framework (16.4%) | decline as a *framework* — this is the application framework Beck replaces rather than interoperates with. Reaching its **libraries** is the C ABI FFI bullet; being consumed *by* a .NET estate is `@public(rest)`/`@public(grpc)` |
| **NumPy (21.2%)** — most-used library in the survey | **adopt**, split in two: the *notation* (broadcasting, slicing) is a language feature and queues behind the macro interpreter; the *kernels* are linked and never reimplemented ([`105`](105-the-ecosystem-answer.md) §102.8). Nothing is built |
| **pandas (20.7%)** | **adopt** — not a library here at all but [`99`](99-the-data-tier-means-of-combination.md)'s missing algebra, §8.5.4's Lane B item |
| Spring Framework (11.1%) | decline — DI, ORM and web framework, all three dissolved ([`105`](105-the-ecosystem-answer.md) §102.6) |
| RabbitMQ (10.9%), Apache Kafka (9.4%) | **adopt** as *ingress and egress* — [`101`](101-the-public-surface.md) §101.6's `@public(events)` and [`30`](30-bounded-contexts-and-microservices.md) §30.4's `ingest`, Phase 5. A broker subscription is a merge point, which is a thing this language already has a word for |
| scikit-learn (10.6%), Torch/PyTorch (10.6%), TensorFlow (10.1%), Keras (4.3%), Hugging Face (4.5%), JAX (1.0%), mlflow (1.2%) | **supported through the bridge**, merge points only — the capability half, and the concession [`01`](01-vision-and-premise.md) §1.7 actually made |
| Flutter (9.4%), React Native (8.4%), SwiftUI (4.3%), .NET MAUI (3.1%), Xamarin (2.9%) | decline — native mobile UI. The offline-capable web client is the answer this project has ([`94`](94-the-client-report.md); `beck-sw.js` is the service worker), and it is a different product |
| OpenCV (8.6%) | decline — computer vision is a capability, bridgeable if anyone asks, and no territory of ours |
| Qt (7.3%), GTK (2.6%), MFC (1.0%) | decline — native desktop UI |
| OpenGL (6.8%), DirectX (1.9%), CUDA (5.8%), OpenCL (1.7%) | decline as *targets*; CUDA and OpenCL are reachable through the bridge as capabilities. GPU rendering is not a tier this language has |
| **Electron (6.5%), Ionic (2.5%), Tauri (2.4%), Capacitor (1.8%), Cordova (2.2%)** — **15.4% together** | **watch, and the shape is closer than it looks.** These wrap a web UI as an installable app; Beck emits a web client *and* a statically linked native binary, which is Tauri's architecture with the parts already built. Nothing is claimed and nothing is scheduled — the trigger is somebody executing it, per the rule that an artefact nobody has run is a design document. Recorded because it was invisible |
| Apache Spark (4.4%), Hadoop (2.3%) | decline as a *substrate* — the analytical half is DataFusion over Parquet (§8.5.4's G item), the single-node answer [`07`](07-dependencies.md) §7.4 sized for |
| **Ruff (3.0%)** | **dissolved** — the formatter and the linter are the compiler (`beck fmt`, the LSP, [`65`](65-the-editor-report.md)). Worth a second look rather than a tick: Ruff's adoption is developers switching formatter for *speed alone*, which is the clearest external evidence that [`64`](64-compile-speed-report.md)'s budgets are a product feature and not hygiene |
| **Tidyverse (1.7%)** | **adopt** — the same verdict as pandas, and the reason it is listed separately: R's `dplyr` is a **fourth** independent convergence on the same dozen verbs (with pandas, polars and LINQ), which is what makes [`99`](99-the-data-tier-means-of-combination.md)'s gap a missing algebra rather than one library's taste |
| Roslyn (1.7%) | **dissolved into tooling** — compiler-as-a-service is the LSP plus `beck explain` ([`65`](65-the-editor-report.md)), and the half that is genuinely missing is compile-time evaluation, which is §8.5.4's macro interpreter |
| Quarkus (1.3%), Ktor (1.2%) | decline — JVM/Kotlin service frameworks, replaced rather than interoperated with |
| **Charting** — matplotlib, Chart.js, D3. *Not in the survey's list*, and universal in practice | **adopt** — an `svg:` vocabulary, §8.5.4, blocked on one `createElement` call ([`105`](105-the-ecosystem-answer.md) §102.9). Listed here because the rule's failure mode is silence, and a category the survey happens not to itemise is the easiest silence of all |
| **LLM clients** — litellm, openai, langchain. *Post-dates the survey entirely*; litellm is PyPI #46, above `pip` | **supported through the bridge**, and the fit is better than "supported" suggests: an LLM call is nondeterministic, so §3.7 forbids it in a fold or a view and forces it to arrive at a merge point as a command whose response becomes an **event**. Prompt, response and model version land in the log, so the session replays exactly without re-calling the model — the discipline these applications need and usually have to remember. No new work beyond the sidecar ([`105`](105-the-ecosystem-answer.md) §102.4) |
| **Polars (rising), `uv`/Ruff (rising)** — post-date the survey | Polars is a *fifth* convergence on the dataframe verbs and strengthens the pandas row rather than competing with it. `uv` at 2.2× Poetry's downloads and Ruff at #132 are external evidence for [`64`](64-compile-speed-report.md)'s premise that tooling speed is a product feature; no work is owed, and the row exists so the evidence is not lost |

**Four rows had no verdict anywhere in `docs/` before
[`105`](105-the-ecosystem-answer.md)** — NumPy, pandas, charting and the Electron/Tauri adjacency —
which is the failure this rule exists to catch, occurring in the document that states the rule.
§8.5.4 now carries the first three; the fourth is deliberately unscheduled and says so.

The largest declined block is worth stating rather than leaving as a pattern in the table: native
desktop and mobile UI, plus GPU rendering and computer vision, is roughly **half the section by
cumulative percentage**, and all of it is declined on the same [`01`](01-vision-and-premise.md) §1.7
scope. That is an honest statement of how much of the developer population this language is not
for, and it belongs in the open rather than in the gaps between rows.
