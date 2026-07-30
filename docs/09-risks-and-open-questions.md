# 09 — Risks and open questions

## 9.1 Ranked risks

### R1 — Whole-program placement destroys separate compilation *(highest technical risk)*

The recurring finding in the tierless literature: poor modularity and separate compilation. Global
inference fights incremental builds, library distribution, and IDE latency.

**Mitigation**: placement, effects and event types in published module signatures
([`03`](03-type-and-effect-system.md) §3.6); inference intra-module only. Designed in Phase 2, not
retrofitted. **Tripwire**: a body edit that invalidates a downstream module's typecheck is a P0 bug.

### R2 — "Magic" becomes distrust at scale (the Meteor failure)

**Mitigation**: `tier explain` (place / flow / incremental / wire / deploy) shipped in v0.1;
placement persisted in `tier.lock` with churn reported in CI; `assert place(...)` in tests;
ambiguity is a compile error with a suggested annotation, never a silent guess; determinism and
stability are specified solver properties ([`03`](03-type-and-effect-system.md) §3.4).

### R3 — Scope is five products

A UI runtime, a server runtime, a log/IVM engine, an image builder, and a Kubernetes control plane.

**Mitigation**: the walking skeleton first ([`08`](08-roadmap.md)); buy everything
[`07`](07-dependencies.md) allows; non-goals in [`01`](01-vision-and-premise.md) §1.5. The two
genuinely-must-build items are the **patch/signal runtime** and the **log + incremental-view
engine's integration** (the engines exist; the integration is ours). Resource those two; everything
else is assembly. The thin-client decision ([`05`](05-tier-lowering.md) §5.1) deliberately shrinks
v0.1: no GC-in-WASM, no bundle-size crisis, no hydration matrix.

### R4 — Ecosystem access

No npm/PyPI reach = research artefact. **Mitigation**: §9.2; FFI is a Phase 4 headline, not a
footnote.

### R5 — Per-session fanout and connection state (the LiveView cost, squared)

The original's own admission: real auth means per-session signals, turning one broadcast into
per-client fanout. Add Mode A's per-session view state and a deploy's thundering reconnect, and
this is where the runtime lives or dies operationally.

**Mitigation**: shared-prefix dataflow plans with per-session final operators
([`05`](05-tier-lowering.md) §5.3); subscriptions resumable by `(id, seq)` so reconnects replay
gaps, cross-replica, instead of re-rendering the world; per-session memory, subscription counts and
shared-prefix hit rate exported as metrics from day one; Phase 0 measures memory/connection and
patch p99 **before** the architecture ossifies ([`08`](08-roadmap.md)). **Tripwire**: if per-idle-
session server memory exceeds ~50 KB in Phase 1, stop and redesign the session representation.

### R6 — Event-sourcing estrangement: ops, erasure, and log growth

Three familiar objections to log-centric systems, all deserved: DBAs can't see "the database";
GDPR/right-to-erasure collides with an immutable log; "the log grows forever."

**Mitigation**: read models are ordinary Postgres tables, browsable via pgwire — the outside world
sees tables, not theory ([`05`](05-tier-lowering.md) §5.3); erasure via **crypto-shredding**
(per-subject envelope encryption; deleting the key erases the subject across log, snapshots and
backups) — a worked design in Phase 4, not a FAQ answer. Per [`10`](10-decisions.md) D3 the default
is `retain=forever` (the ledger is the truth), so log growth is managed by *tiering*, not
truncation — old segments archive to Parquet on object storage, doubling as the analytical corpus —
and permanent upcaster chains are kept honest by the genesis-replay CI gate; stores that want
bounded liability opt down to `retain=<window>` (snapshot-and-compact as "this world's garbage
collection"). Position the substrate as boring on purpose: "your data is in Postgres; Tier is how
it got there."

### R7 — The merge ceiling

One totally-ordered log per app is a single-sequencer throughput and single-region latency
ceiling. This is SICP's merge wound resurfacing at scale, and the original concedes the adjacent
version of it: concurrent edits to the same value need CRDTs/OT — "no type system absolves you."

**Mitigation**: v1 states its ceiling honestly (a single well-implemented sequencer over Postgres
comfortably clears small-to-mid SaaS: order 10⁴ events/s); the language reserves the seams —
per-entity ordering keys on `ingress`, logical timestamps in envelopes — so sharding is an
implementation upgrade, not a semantics break; collaborative-text is out of scope for core v1,
offered later as a CRDT-valued model type (`Text` as an automerge-backed value) rather than as
folklore. Decision needed: §9.5 Q2.

### R8 — Licence/governance shift in a load-bearing dependency

Terraform→BUSL is the precedent; Materialize (BUSL) and Redpanda (BSL) already shaped our data-tier
choices. **Mitigation**: permissive-only policy, `cargo-deny` gate from first commit, foundation-
governed preferences, and the per-dependency swap costs in [`07`](07-dependencies.md) §7.8.

### R9 — Correctness of the split and of incrementalization

Two ways to silently betray the premise: split behaviour ≠ single-process behaviour; incremental
view ≠ recompute.

**Mitigation**: the differential harness (split vs unsplit) and the incremental-vs-recompute oracle
are the two highest-value test suites in the project ([`04`](04-compiler-architecture.md) §4.8);
replay determinism (same log twice ⇒ identical states and patches) as a third, nearly-free
invariant; `Kani` model checking on solver invariants (no `secret` crosses; no valid program
rejected).

### R10 — Kubernetes coupling repels the target audience

**Mitigation**: `tier run` needs no container, registry, or cluster
([`06`](06-kubernetes-and-packaging.md) §6.1); `Platform` trait; plain-manifest emission for GitOps
teams; `import infra` for adoption beside an existing estate. Hello-world never touches a registry.

### R11 — Debuggability across tiers

**Mitigation**: provenance from every `Core` node to source spans including macro chains; one
OpenTelemetry trace across click → command → event → fold → patch; `seq`-scrubbing time-travel
debugger (determinism makes it cheap); DWARF/source maps for Mode B; cross-tier DAP in Phase 5.
Replay (`tier fork --from prod`) turns "cannot reproduce" into a command.

## 9.2 The ecosystem question, in detail

1. **C ABI FFI both directions** (Phase 3–4). Table stakes.
2. **JS interop on the client** (Phase 3). Typed bindings generated from TypeScript declaration
   files; a Mode-B component can wrap a JS widget (charts, maps, editors) behind a typed boundary.
3. **Python bridge** (Phase 4) — the strategically important one. Recommended shape: an
   out-of-process **typed sidecar** — a `python_service` declaration generates a Python stub
   package speaking the internal wire format, rendered as its own container in the pod. Keeps
   images clean and reproducible, matches how ML workloads deploy, keeps the boundary typed.
   In-process CPython embedding drags the GIL and runtime into our images; compiling a Python
   subset is a tar pit — neither recommended.

Say plainly: Tier is Python-*shaped*, not Python-compatible. Over-promising here burns exactly the
audience it courts.

## 9.3 Commercial failure modes (even if the engineering succeeds)

- **The demo is amazing and week two is miserable.** The four "week two" walls — a background job,
  an external API, an existing database, a non-trivial UI — get first-class tutorials in Phase 3;
  friction there is a P1 bug.
- **No incremental adoption path.** `import infra`, `external store`, pgwire read models, GitOps
  manifest emission, the Python sidecar: adoption features wearing technical clothes. Protect them.
- **The wrong first audience.** Product teams feel the productivity win; platform teams feel the
  pain that justifies it and buy the security story
  ([`03`](03-type-and-effect-system.md) §3.5, [`06`](06-kubernetes-and-packaging.md) §6.5). The
  playground demo — source left; inferred placement, generated SQL/dataflow, generated Kubernetes
  objects right — serves both in 60 seconds.
- **Two dialects form** (Lisp vs Python factions). `tier fmt` normalises committed code to the
  Python surface; S-expressions are the spec/macro-debugging notation, positioned as such.

## 9.4 Decisions taken (overrule cheaply, now rather than later)

| Decision | Chosen | Alternative | Cost to change later |
|---|---|---|---|
| Semantic core | Event-sourced: commands → validate → events → durable folds → views (per the original) | Relational-first with SQL pushdown | High — it *is* the language now |
| Host language | Rust | OCaml, Zig | Total rewrite — decide now |
| Client default | Thin patch interpreter (Mode A); local WASM (Mode B) opt-in per component | WASM-first everywhere | Low-moderate — modes share one source |
| Log substrate v1 | Postgres (+ object storage snapshots; redb embedded for dev) | Purpose-built log store | Low — behind the log-engine interface |
| Incremental views | Differential-dataflow lineage, recompute as oracle | Recompute always / hand-rolled IVM | Moderate — plans are symbolic either way |
| Migration doctrine | Events-forever default (genesis replay is the invariant); per-store bounded-retention opt-in | Snapshots-authoritative default | Decided — [`10`](10-decisions.md) D3 |
| Identity | Bundled OSS IdP (Keycloak/Ory) or external OIDC issuer; never own auth | Hand-rolled sessions | Decided — [`10`](10-decisions.md) D6 |
| Offline | Offline-tolerant v1 (Mode B + queued commands); CRDT-valued types v1.x; peer-to-peer local-first out of scope | Local-first core | Decided — [`10`](10-decisions.md) D7 |
| Typing | Static, mandatory public signatures | Gradual | High |
| Async | No colouring; compiler inserts awaits | Explicit async/await | Moderate |
| Infra state | Kubernetes API + Crossplane | Own engine / OpenTofu-first | Low (emitters pluggable) |
| Images / packages | apko / OCI+ORAS | BuildKit / bespoke registry | Low |

## 9.5 Questions for George — answered

All substantive questions are answered; the answers and their reasoning are recorded in
[`10-decisions.md`](10-decisions.md) (D1–D8, all decided). Remaining open, both non-blocking:

1. **Security/least-privilege as marketing headline vs productivity** — defaults to "both,
   audience-dependent" (§9.3) until directed otherwise.
2. **Name**: decided — **Beck** ([`10`](10-decisions.md) D10): Cumbrian for a fast upland stream
   (becks merge into rivers — the merge point), with the second English sense of a summons ("beck
   and call" — a `Command`). Handle `becklang`. Rename executed as one deliberate commit on
   George's word; trademark/domain checks at go-public.

## 9.6 Open technical questions (tracked, not blocking)

1. Effect granularity: value-level (`durable(todos)`) in signatures vs field-level internally for
   cost/policy — where exactly to draw it.
2. Ambient effects (`log`, `time`, `metrics`): elided outside folds — is `log` allowed *inside*
   folds (deterministic replay would re-log) or ingress-only?
3. Clock signals: what tick granularities are declarable, and how do time-varying views interact
   with incremental plans (differential handles it via time as input — surface how?).
4. Data placement (cache this slice client-side, materialise that view) — v1 is explicit
   (`materialized`, Mode B); when does the solver take it over?
5. The two syntax decisions from [`02`](02-syntax.md) §2.9: effect clauses vs decorators; `ui:`
   macro vs JSX-likes. Cheap now, expensive after Phase 3.
6. Multi-app composition: two Tier apps sharing events — federation of logs, or one app with two
   deployments? (Touches org boundaries, so it will be asked early.)
7. Backpressure surfacing: v1 hides it in the runtime contract; when a client can't keep up with
   its patch stream, what does the *language* say (drop-to-latest for signals is sound — is it
   ever wrong)?
