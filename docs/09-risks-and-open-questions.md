# 09 — Risks and open questions

## 9.1 Ranked risks

### R1 — Whole-program placement destroys separate compilation *(highest technical risk)*

The recurring finding in the tierless literature is that most such languages have very poor modularity
and separate compilation. Global placement inference is inherently whole-program, which fights
incremental builds, library distribution, and IDE latency.

**Mitigation**: §3.6 — placement and effects are part of a module's *published signature*. Inference is
intra-module; boundaries are declared and checked. This must be designed in Phase 2, not retrofitted;
Eliom is the existence proof that it is achievable. **Tripwire**: if a one-line body edit ever
invalidates a downstream module's typecheck, the firewall has a hole — treat as a P0 bug.

### R2 — "Magic" becomes distrust at scale (the Meteor failure)

Inferred placement is delightful in a demo and terrifying at 200 kLOC when something lands on the wrong
tier and nobody can say why.

**Mitigation**: `tier explain` shipped in v0.1 (§4.7); placement recorded in `tier.lock` with churn
reported in CI; `assert place(f) == data` in tests; determinism and stability as *specified properties*
(§3.4); and ambiguity as a compile error with a suggested annotation rather than a silent guess.

### R3 — Scope is five products

A frontend framework, a backend runtime, an ORM/query compiler, an image builder, and a Kubernetes
control plane. Any one is a startup.

**Mitigation**: the walking skeleton first (§8, Phase 1); depend rather than build wherever §7 allows;
the explicit non-goals in §1.5; and per-tier "good enough for v1" bars written down before work starts.
The largest genuinely-must-build-ourselves items are the reactive client runtime (§5.2) and the query
compiler (§3.7) — resource those two properly and buy everything else.

### R4 — Ecosystem access

A language that cannot use npm and PyPI starts from zero libraries. This has killed more good languages
than any technical flaw.

**Mitigation**: see §9.2 — treat FFI as a Phase 4 headline feature, not a footnote.

### R5 — Debuggability across tiers

A stack trace that begins in a WASM click handler, crosses a synthesised RPC, and ends inside generated
SQL is either the best debugging experience in the industry or the worst.

**Mitigation**: provenance from every `Core` node back to a source span *including macro expansion
chains* (§4.5); one trace id threaded through every synthesised boundary via OpenTelemetry (§5.2); DWARF
in the WASM plus source maps; a cross-tier DAP debugger in Phase 5; `tier explain wire`. Budget this as
a feature with an owner, not as polish.

### R6 — WASM client payload size

The single most likely reason a developer bounces off the frontend tier.

**Mitigation**: budgets as CI gates (§5.1); measure in Phase 0 *before* committing; keep the
`--client-backend=js` option genuinely working. **Kill gate**: > 500 KB brotli for hello-world in Phase 0
means JS becomes the primary client backend.

### R7 — Kubernetes coupling repels the target audience

The Python developer you want does not have a cluster and does not want one. Meanwhile the platform
engineer who *does* have one already has opinions and existing tooling.

**Mitigation**: §6.1 — `tier run` requires nothing; the `Platform` trait keeps k8s optional; emit plain
manifests for GitOps teams (§6.3); `import infra` for incremental adoption alongside an existing estate.
Never make hello-world touch a registry.

### R8 — Licence/governance change in a load-bearing dependency

Terraform's move to BUSL is the cautionary precedent, and it already constrains our choices (§7.6).

**Mitigation**: permissive-only policy with a `cargo-deny` CI gate; prefer foundation-governed projects;
the documented swap cost per dependency (§7.8).

### R9 — Correctness of the split

If the split program behaves differently from the unsplit one, the language's core promise is false.

**Mitigation**: the differential execution harness (§4.8) is the highest-value test in the project;
property-test generated programs; consider `Kani` model checking for the placement solver's invariants
(no `secret` crosses to client; no valid program rejected).

### R10 — Multi-tenancy and the data tier at scale

Row-level invalidation, connection pooling under thousands of subscriptions, and per-tenant data
isolation are where "live queries" projects historically hit a wall (Meteor's `oplog` tailing, again).

**Mitigation**: table-level invalidation in v1 with an explicit, documented scaling ceiling; design the
subscription registry for sharding from the start; make tenancy a *type* (`Tenant[T]` with the tenant
key threaded through queries and policy) rather than a convention.

## 9.2 The ecosystem question, in detail

Three levels, in order of cost and value:

1. **C ABI FFI both directions** (Phase 3–4). Table stakes; unlocks native libraries.
2. **JS interop on the client tier** (Phase 3). Necessary in practice — charting, maps, editors. Design
   it as *typed* bindings generated from TypeScript declaration files, so the boundary keeps its types.
3. **Python bridge** (Phase 4). The strategically important one, given the audience. Options:
   - *In-process embedding* of CPython in the server tier — fastest to build, but drags the GIL and the
     whole Python runtime into our image, and undermines the reproducibility story.
   - *Out-of-process, typed sidecar* — a generated Python stub package that a Tier service calls over
     the internal wire format; a `python_service` declaration renders it as its own container in the
     same pod, with generated types on both sides. **Recommended**: it keeps our images clean, matches
     how ML workloads actually deploy, and makes the boundary explicit and typed.
   - *Compile a Python subset to Tier* — do not attempt. Python's semantics are the problem, not the
     syntax, and a subset that silently diverges is worse than no support.

Say clearly in the docs: Tier is Python-*flavoured*, not Python-compatible. Over-promising here is the
fastest way to lose credibility with exactly the audience you want.

## 9.3 What could make this fail commercially even if the engineering succeeds

- **The demo is amazing and the second week is miserable.** The "one file, whole app" wow is easy; the
  hard part is week two — adding a background job, an external API, an existing database, a
  non-trivial UI. Write those four scenarios as first-class tutorials in Phase 3 and treat friction in
  them as P1 bugs.
- **No incremental adoption path.** Nobody rewrites a working system. `import infra`, the pgwire
  endpoint, GitOps manifest emission, and the Python sidecar are all *adoption* features masquerading as
  technical ones. Protect them in planning.
- **The wrong first audience.** Solo developers and small teams get the productivity win but don't feel
  the pain that motivates the tierless design. Platform teams feel it acutely but need the security and
  policy story (§3.5, §6.5). Recommendation: **lead with the security/least-privilege story to platform
  teams and the productivity story to product teams**, and build the playground so both can see it in
  60 seconds.
- **Two idiomatic dialects form** (Lisp faction vs Python faction). Mitigate with `tier fmt` normalising
  to the Python surface in all committed code, and S-expressions positioned explicitly as the *spec and
  macro-debugging* notation (§2.2).

## 9.4 Decisions I made on your behalf

Flagged so you can overrule them cheaply:

| Decision | Chosen | Alternative | Cost to change later |
|---|---|---|---|
| Host language | Rust | OCaml, Zig | Total rewrite — decide now |
| Client target | WASM primary, JS optional | JS primary | Moderate (Phase 0 gate exists) |
| Placement | Inferred, verifiable, overridable | Always explicit annotations | Low — explicit is Phase 1 anyway |
| Data tier | Postgres + DataFusion | DuckDB, custom storage | Low (behind `store` abstraction) |
| Static typing | Mandatory, no gradual/dynamic mode | Gradual typing like mypy | High — affects everything |
| GC | Yes, tracing per tier | Ownership/borrowing in the surface | High |
| Async | No `async` colouring in the surface | Explicit async/await | Moderate |
| Infra state | Kubernetes API + Crossplane | Own state engine, OpenTofu | Low (emitters are pluggable) |
| Images | apko | BuildKit | Low |
| Package registry | OCI/ORAS | Bespoke registry | Low |

## 9.5 What I need from you

1. **The original transcript.** The premise in [`README.md`](README.md) and
   [`01-vision-and-premise.md`](01-vision-and-premise.md) is a reconstruction from the repo name and
   your three questions. If your Lisp sketch encoded a *different* central mechanism — staged
   metaprogramming, a specific data model, an actor/process model, content-addressed code — then §1–§3
   need revising, and it is much cheaper to do that now than after Phase 1.
2. **Is the security/least-privilege framing (§3.5, §6.5) interesting to you, or a distraction?** I've
   promoted it to a headline feature because it is the most defensible claim in the design, but it does
   pull the roadmap toward platform teams and away from solo developers.
3. **Ambition setting for the data tier.** "Compiles to SQL against Postgres" (Phase 3, safe) versus
   "Tier *is* the database, with our own storage engine" (multi-year, and a different company). I've
   assumed the former throughout.
4. **Team and horizon.** §8's estimates assume 3–6 engineers. If this is a solo or two-person project,
   I'd cut scope hard and differently: Phase 1 + Phase 2 only, single-process and Docker platforms,
   Postgres, **no Kubernetes tier until there are users** — a "typed tierless framework" rather than a
   platform. Tell me which and I'll rewrite §8 to match.

## 9.6 Open technical questions

1. **Placement of *data*, not just code.** Should the solver move *values* (cache this list on the
   client, materialise this view in the DB)? Very powerful, and a large increase in solver complexity.
   Proposal: v1 places code only; data placement is explicit (`cache`, `materialised`), inferred later.
2. **Effect granularity.** `db.read(orders)` (table-level) vs `db.read(orders.total)` (column-level)?
   Column-level gives finer policy and better byte estimates; it also risks signature churn on every
   refactor. Proposal: table-level in signatures, column-level internally for the cost model.
3. **Migration safety in a rolling deploy.** Expand/migrate/contract must be *derived*, and some
   schema changes are genuinely unsafe to automate. Proposal: the planner classifies each step
   safe/unsafe and refuses unsafe ones without an explicit annotation and a two-phase deploy.
4. **Subscription semantics under partition.** What does `live` promise when the client is offline?
   Proposal: explicit, typed staleness (`Signal[T]` carries a freshness state the UI must handle) rather
   than pretending consistency.
5. **Where the effect system stops.** Does `log` count? `time`? Over-granular effect rows make every
   signature noisy. Proposal: an `ambient` set (log, time, metrics, rand) that is implicitly available
   and elided from signatures, with a strict mode for those who want purity.
6. **The two syntax decisions from §2.9** — effect clauses vs decorators; `ui:` macro vs JSX-like
   literal. Cheap to decide now, expensive after Phase 3.
7. **Naming.** `tier` is a good internal name and a hard one to search for. Worth deciding before public
   artefacts exist.
