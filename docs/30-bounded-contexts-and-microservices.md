# 30 — Bounded contexts, microservices, and the outside world

> **The question**, continuing [`29`](29-domain-driven-design.md): Beck should support strategic
> DDD — bounded contexts — and with it a microservices architecture, natively. The ideal is that
> your entire distributed architecture is one Beck project; the real world makes that imperfect and
> difficult, so connecting external microservices and systems has to be easy too. Contexts within
> one project could be hosted on different architecture — ideally within one Kubernetes cluster —
> and Beck is supposed to supersede IaC, so the practical-world choices people make have to live
> inside that claim rather than beside it.
>
> **Status: designed, not built.** This document is the design item [`29`](29-domain-driven-design.md)
> §29.1 names as the real gap. It revises one settled decision explicitly (§30.7) rather than
> quietly diverging from it. The decision record is [`10`](10-decisions.md) **D20**.

## 30.1 The gap, named precisely

[`10`](10-decisions.md) D2 fixed *one totally-ordered log per application* — which is one **model**
per application. [`15`](15-scale-and-distribution.md) rung 2 partitions that log by key, which
scales one model without adding a second one: every partition folds the same `union Event` with the
same `apply_event`.

A bounded context is the other axis. Booking and routing in
[`29`](29-domain-driven-design.md) §29.4's cargo system are not two partitions of one model — they
are two *models*, with different vocabularies, different invariants, different rates of change, and
a deliberate translation between them. Partitioning divides a model's keyspace; contexts divide the
*language*. The two compose: a context may itself be partitioned at rung 2.

| Axis | Divides | Order guarantee | Cross-boundary mechanism |
|---|---|---|---|
| **Partition** (rung 2) | one model's keyspace | total per key | designated global partition, or saga |
| **Context** (this document) | the project into models | total per context (per key within a partitioned context) | published events + sagas — never shared state |

Today the compiler's splitter accepts one program with one log; several `durable` folds fuse into
one accumulator over that one log ([`23`](23-general-slicer-report.md)). Contexts are not that:
each context owns a log.

## 30.2 The construct, sketched

A `context` is a module boundary with teeth: its own commands, events, folds, views, merge
point — and its own log, sequenced independently.

```python
context booking:
    union Command: BookCargo(...) | AssignRoute(...) | ChangeDestination(...)
    union Event:   CargoBooked(...) | RouteAssigned(...) | DestinationChanged(...)
    cargo = durable(fold(apply_event, ...))
    # publishes: the subset of Event named in booking.becki

context routing:
    union Command: RequestCandidates(...)
    union Event:   CandidatesComputed(...)
    ...

# cross-context flow: subscription + saga, never a state read
process assign_on_booking in booking:
    on routing.CandidatesComputed(c): emit_command(AssignRoute(pick(c)))
```

The rules, each of which is a check rather than a convention:

1. **No cross-context state reads.** An expression in `booking` that mentions `routing`'s fold
   state is a compile error pointing at the expression — the same move, at the model boundary,
   that rung 2 makes at the key boundary ([`15`](15-scale-and-distribution.md) §15.2). The error
   names the two fixes: subscribe to the neighbour's events and fold your own copy of what you
   need, or make the interaction a saga.
2. **A context's contract is its `.becki`** ([`03`](03-type-and-effect-system.md) §3.6): the
   commands it accepts, the events it publishes, their wire shapes, and its effect row. What is
   not published is invisible — a context may rename, refactor, or drop internal events freely,
   which is the encapsulation bounded contexts exist to buy.
3. **Cross-context messages enter through the receiving context's merge point** as ingress
   events, carrying `(source_context, source_seq)` on the envelope. Consequences, in order of
   importance: each context's replay story is intact (its log contains everything it consumed);
   consistency across contexts is eventual and *visibly* so — the same honesty
   [`15`](15-scale-and-distribution.md) §15.1 states for partitions; delivery is effectively-once
   by the same idempotent-append-by-identity mechanism, resumable by `(subscription, seq)`.
4. **Within one project, tests still cross the boundary without a network.**
   `beck test` seeds several logs instead of one — `given` gains a context qualifier
   (`given booking: [...]`, spelling to be settled against [`22`](22-phase-3-report.md) §22.5's
   clause forms) — and a test asserts, in four lines, that a booking in one context becomes a
   routing request in another and a page update in a third. That is the cross-boundary property of
   [`21`](21-tests-in-beck-and-proof.md) §21.2 extended across models, and no other stack has it:
   a microservices integration test with no network, no containers, and nothing to flake.
5. **A single-context program is unchanged.** `context` is optional; a program that never says it
   is exactly today's Beck, one log and all. Nothing built regresses, and nothing about the
   construct may be allowed to complicate the program that doesn't use it.

## 30.3 Contexts are the unit of deployment: microservices, derived

A context is what a microservice was always supposed to be — an independently deployable model
with an explicit contract — so the lowering is direct. The `InfraGraph` is still produced from the
program's effects before any platform is chosen ([`06`](06-kubernetes-and-packaging.md) §6.1.1:
a platform renders, it does not derive); contexts add structure to it:

- **One deployable per context** by default: its own image (apko, reproducible), its own
  sequencer/log, its own fold owners and view maintainers, its own scaling. The placement solver
  ([`03`](03-type-and-effect-system.md) §3.4) already decides tier placement from effects; the
  context boundary adds a placement constraint, not a new mechanism. A team that wants several
  contexts co-deployed in one process (rung 0/1 economics) says so at deploy, not in domain code —
  co-deployment is a rendering choice, and the semantics (separate logs, no shared state) do not
  change with it.
- **The context map becomes the network policy.** Which context consumes which other's events is
  in the effect rows, so the least-privilege `NetworkPolicy` between context deployments is
  derived the way [`06`](06-kubernetes-and-packaging.md) §6.5 already derives egress: `routing`
  can reach `booking`'s fabric subject and nothing else, because nothing else is in its row. The
  microservices "service mesh + config sprawl" problem arrives pre-solved: the mesh *is* the
  program's own declared dependencies, enforced.
- **Deploy cadence is per context, gated both ways.** `beck deploy --context booking` demands
  `booking`'s migrations ([`03`](03-type-and-effect-system.md) §3.9) and checks wire compatibility
  against every neighbour's pinned `.becki` — the consumer-contract check microservices teams
  bolt on with Pact, as a compile gate. Publishing a breaking event-shape change with a minor
  version is refused the way [`16`](16-packages-and-ecosystem.md) §16.4 refuses it for tarns.
- **The fabric carries cross-context deliveries** — the already-planned NATS JetStream slot
  ([`15`](15-scale-and-distribution.md) §15.3), compiler-wired, never user-visible. No context
  addresses another by URL; there is no service discovery to configure because there are no
  addresses in the language.

## 30.4 The outside world: three rungs of honesty

The ideal is the whole architecture as one Beck project. The design takes the real world
seriously: existing systems, other teams, other languages. The boundary construct is one
declaration with three grades of guarantee, weakest last, and the compiler knows which grade it is
looking at.

**Rung A — a context in this project.** Everything above: proofs, cross-context tests without a
network, derived policy, one `beck test` over the whole map.

**Rung B — another Beck project's context.**

```python
external context orders = beck("acme-orders", iface="orders.becki")
```

The contract is a `.becki` exchanged between projects — types, events, effects — and `--wire-compat`
gates both sides' deploys against it. Proofs stop at the process boundary (this project cannot
verify what the other one actually runs), but the contract stays *typed*, the translation stays
compiled, and drift is caught at the gate rather than at 3 a.m. Transport is the fabric where
shared, CloudEvents over the declared protocol where not.

**Rung C — a foreign system.**

```python
external context legacy_erp:
    protocol   grpc("erp.internal:443")            # or http, kafka, cloudevents
    publishes  ErpShipmentUpdated(...)             # types we declare for their traffic
    accepts    CreateShipment(...)
    translate  fn to_handling(e: ErpShipmentUpdated) -> HandlingEvent: ...
```

This is [`16`](16-packages-and-ecosystem.md) §16.8's *bridges rent from neighbours*, given the
context-shaped form: their events arrive through `ingest(source)`
([`03`](03-type-and-effect-system.md) §3.7) like any webhook, are translated by pure typed
functions — **the anti-corruption layer as code the compiler demands, the same shape as
`upcast`** — and enter this project's contexts as ordinary ingress events, replayable because they
were logged on arrival. The effect row says exactly which host the boundary talks to
(`net.out(erp.internal:443)`), so the derived egress policy covers it. What is *not* available at
rung C is stated, not implied: no wire-compat gate on their side, no replay of their internals, no
delivery guarantee beyond what their protocol offers — so the boundary carries **contract tests**
(recorded exchanges asserted in `test` blocks, stubbed at the `translate` seam per
[`21`](21-tests-in-beck-and-proof.md) §21.3) instead of proofs, and `beck explain` says which
boundaries are proved and which are rented.

The migration path is the point of the ladder: a foreign system strangled into Beck moves C → B →
A with the *same declared boundary*, tightening the guarantee at each step without the consumers
changing shape.

## 30.5 Heterogeneous hosting, and what "supersedes IaC" obliges

Contexts within one project may need different substrates: the routing context wants big
CPU-optimised nodes, the booking context is ordinary, an inference-adjacent context wants a GPU
pool, one context must stay in the EU. Ideally all of it in one Kubernetes cluster. The design
rule that keeps this compatible with [`06`](06-kubernetes-and-packaging.md):

**Hosting choices are constraints on derivation, never patches on output.** The moment a team
hand-edits generated manifests, drift returns and the IaC-supersession claim dies. So the
practical-world choices live in the deploy target's declaration — data the solver consumes, like
the cost model it already consumes ([`03`](03-type-and-effect-system.md) §3.4) — and `beck build`
must be able to express them, because refusing reasonable constraints is what forces teams to
patch output:

```python
target production:
    platform  kubernetes(cluster="main")
    context routing:  nodes(arch=arm64, pool="compute"), replicas(min=2)
    context booking:  region("eu-west")                  # residency, stacking with D4/F1
    context legacy_erp: unmanaged                        # rung C: exists, is not deployed by us
```

- Constraints render as scheduling directives (node selectors, affinities, topology spread) on the
  context's derived objects — Kubernetes vocabulary stays compiler output
  ([`06`](06-kubernetes-and-packaging.md) §6.1's corollary).
- **One cluster is the recommended shape** and the tested one: one control plane, one fabric, the
  derived NetworkPolicies meaningful. Multi-cluster is a later rung, taken deliberately — it
  aligns with [`15`](15-scale-and-distribution.md) rung 3's geo-homes, where a context's home
  region and a partition's home region are the same mechanism.
- A platform that cannot honour a constraint says so in `explain.txt` the way Compose already
  reports what it cannot express ([`06`](06-kubernetes-and-packaging.md) §6.1.1 item 2):
  `unsupported` is visible, never inferable from an absence.
- The escape hatch for estates the derivation cannot reach stays what [`07`](07-dependencies.md)
  §7.6 chose: Crossplane claims, and the OpenTofu emitter — emitting *from the same InfraGraph*,
  so even the escape hatch is derived.

## 30.6 The ideal and the practical: one ladder, guarantees stated per rung

The claim this table protects: **you choose your rung for organisational reasons, and the compiler
tells you exactly what the choice costs.** Nothing here is lost silently.

| Rung | Shape | Kept | Forfeited |
|---|---|---|---|
| 0 | One context, one program (today's Beck) | everything: one log, whole-program proofs, replay, cross-tier tests | model separation |
| 1 | Several contexts, one project, co-deployed (`beck run`, one process) | context isolation checked, cross-context tests with no network, whole-map replay from the logs | nothing else — this is the recommended dev rung for any rung-2/3 production shape |
| 2 | Several contexts, one project, one cluster, per-context deployables | all of rung 1's checks, derived per-context policy and scaling, per-context deploy cadence with two-sided wire gates | cross-context consistency is eventual (it already was semantically at rung 1 — now it is also observable) |
| 3 | Contexts across Beck projects (rung B boundaries) | typed contracts, two-sided wire-compat gates, compiled translations | whole-map tests and proofs; each project proves its own side only |
| 4 | Foreign systems attached (rung C boundaries) | typed ingress, demanded ACL, derived egress policy, logged-on-arrival replay of *our* side, contract tests | proofs and gates on their side; their delivery semantics are theirs |

The unusual property is rung 1: a full microservices architecture — several models, several logs,
eventual consistency between them — developed and tested as one process with deterministic tests,
then deployed at rung 2 *without changing the program*. The dev→prod parity ladder of
[`06`](06-kubernetes-and-packaging.md) §6.6 gains a dimension, and the microservices tax
(integration environments, contract-test infrastructure, mesh configuration) is either derived or
dissolved.

## 30.7 What this revises, explicitly

Per the repository rule that a change contradicting a decision says so:

- **D2 is revised by D20** ([`10`](10-decisions.md)): "one totally-ordered log per application"
  becomes **one totally-ordered log per context; an application is one or more contexts**. A
  program that declares no context has exactly one, so every existing program, the corpus, and
  everything built keeps its meaning and its ceiling. D2's envelope reservations (per-entity keys,
  logical timestamps) are untouched and rung 2 composes per context.
- **[`15`](15-scale-and-distribution.md) gains an axis, not a rung.** §15.2's ladder is
  per-context; §15.4's sagas become the *only* cross-context write mechanism, which strengthens
  their position from "the missing construct" to load-bearing boundary semantics.
- **[`16`](16-packages-and-ecosystem.md) forces and contexts are orthogonal**: a force contributes
  declarations *into* a context (a payments force lands its commands, fold and saga inside the
  context that adds it); whether a force may declare a whole context of its own is an open
  question (§30.8), deliberately unanswered until a real force wants it.
- **[`23`](23-general-slicer-report.md)'s slicer is unaffected in meaning, extended in scope**:
  fusion of several folds into one accumulator happens per log, therefore now per context; the
  signal graph gains cut points at context boundaries, which is what §30.2's rule 1 checks.

## 30.8 Open questions, stated rather than glossed

1. **Surface form.** Is `context` a block, a directory convention, or a `beck.toml` stanza?
   Directory-as-context reads well and keeps files small; the block form keeps a two-context
   example on one screen. To be settled with [`02`](02-syntax.md)'s block-passing rules.
2. **Sessions across contexts.** One identity subsystem mints `Session`
   ([`03`](03-type-and-effect-system.md) §3.7); does a session's capability row span contexts, or
   does each context see its own projection of it? The security argument (§3.5's least privilege)
   favours projection; the ergonomics favour span. Unresolved.
3. **Shared kernel.** Two contexts sharing a small set of types (money, ids) is DDD's shared
   kernel; Beck's version is presumably an ordinary tarn both import. Does anything need to stop
   a *fold* type from being shared? Rule 1 already prevents shared state; shared shapes seem
   harmless. Needs an adversarial pass.
4. **Ordering across contexts.** `(source_context, source_seq)` gives per-source order at the
   consumer. Is that enough for every saga the cargo benchmark needs, or does some choreography
   need a cross-context barrier — and if so, is the answer "no, restructure" (the rung-2 posture)
   or a designated construct?
5. **`beck test` syntax for multi-log `given`**, and whether `expect` may name another context's
   events directly or only its own state. Settled by building
   [`29`](29-domain-driven-design.md) §29.4's two-context cargo program, which is this design's
   acceptance test.
6. **Fabric topology at rung 2** — one JetStream domain per cluster or per context — is an
   engineering decision for an ADR when it is taken, not a design question.

## 30.9 Where this lands in the plan

Design now (this document); the forcing function and acceptance test is the two-context
cargo-shipping program ([`29`](29-domain-driven-design.md) §29.4), whose refusal files are written
first and assert the walls until the construct lands. The build order that keeps every intermediate
state shippable: the `context` boundary check and multi-log runtime under `beck run` (rung 1 —
semantics and tests, no deployment changes); then `process` ([`15`](15-scale-and-distribution.md)
§15.4), which rung 1 makes urgent because it is the only cross-context write path; then per-context
lowering in the `InfraGraph` and the two-sided wire gates (rung 2); then rungs B and C boundaries,
C first — it is rented rather than proved, so it is smaller, and it is what the strangler migration
path needs on day one. Scheduling against the existing roadmap is [`08`](08-roadmap.md)'s to
absorb; nothing here is claimed for v1 by this document.

## 30.10 What this document does not claim

- **Nothing in it is built.** No `context` parses today; the corpus has no two-log program; the
  ladder of §30.6 is a design with one rung (0) in existence.
- **It does not claim microservices are the recommended architecture.** It claims that when an
  organisation chooses them — for team boundaries, deploy cadence, or the real world's existing
  systems — Beck makes the choice checked, derived, and honest about its costs, instead of a
  YAML-and-Pact estate. Rung 0 remains the right answer for most programs, and the table says so
  by making rung 1's "forfeited" cell read *nothing else*.
- **It does not claim the outside world becomes safe.** Rung C is rented, §30.4 says which
  guarantees are absent, and `beck explain` is obliged to keep saying it.
