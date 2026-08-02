# 29 — Domain-driven design: the mapping, and the tests it earns

> **The question**: would there be a benefit to designing Beck to be highly compatible and
> conformant with domain-driven design, so that one can most easily build DDD systems in Beck —
> supporting DDD concepts natively, supporting Gherkin and behaviour-driven development natively in
> `test` blocks, perhaps as a layer of packages the way .NET layers over C#?
>
> The answer has an unusual shape: **for tactical DDD, the compatibility already exists, because
> Beck's semantics are what the tactical patterns were approximating all along** — the work is to
> claim the equivalence and prove it with a benchmark, not to add a layer. For BDD, Beck already
> has Given/When/Then load-bearing ([`21`](21-tests-in-beck-and-proof.md) §21.2), and the honest
> move is to *render* prose from tests rather than parse prose into steps. For strategic DDD —
> bounded contexts, context maps, the outside world — there is a real gap, and it gets its own
> design document: [`30`](30-bounded-contexts-and-microservices.md).
>
> Decided in [`10`](10-decisions.md) **D19** (this document) and **D20** ([`30`](30-bounded-contexts-and-microservices.md)).

## 29.1 The mapping: tactical DDD onto Beck, pattern by pattern

DDD's tactical patterns (Evans 2003, part II) are techniques for keeping a domain model honest
inside a mutable-object, ORM-mediated, request-scoped world. Beck removes that world. The result is
that most of the patterns land in one of two columns: **native** — the pattern is a language
construct with stronger guarantees than the pattern book asks for — or **dissolved** — the pattern
exists to compensate for a problem Beck's semantics do not admit, so there is nothing to support.

| DDD pattern | Beck | Status | Where |
|---|---|---|---|
| **Domain event** | `union Event` — not a pattern applied to the model but the substrate the model is made of; the log is the book of record ([`10`](10-decisions.md) D1, D3) | **native, built** | [`03`](03-type-and-effect-system.md) §3.7 |
| **Command** | `union Command` — clients propose, one chokepoint decides | **native, built** | [`03`](03-type-and-effect-system.md) §3.7 |
| **Aggregate** (decide/evolve, consistency boundary) | `validate` + `apply_event` over a `durable` fold; atomicity per command is F7's contiguous-seq append; at rung 2 the boundary is *checked* — cross-key reads in a partitioned fold are a compile error | **native, built** (partition checking designed, [`15`](15-scale-and-distribution.md) §15.2) | [`03`](03-type-and-effect-system.md) §3.7, [`15`](15-scale-and-distribution.md) |
| **Invariant enforcement** | fold purity, exhaustive event matching, and the `validate` chokepoint are compiler-checked, not convention | **native, built** | [`03`](03-type-and-effect-system.md) §3.5, [`10`](10-decisions.md) D9 |
| **Read model / projection (CQRS)** | views maintained incrementally by the compiler from the change | **native, built** | [`24`](24-incremental-views-report.md), [`26`](26-arrangement-sharing-report.md) |
| **Process manager / saga** | `process` — a fold that emits commands, compensation written explicitly, timeouts as ingress events | **native, designed — not built** | [`15`](15-scale-and-distribution.md) §15.4 |
| **Specification** | a pure predicate — a function; nothing to name | **dissolved** | — |
| **Repository** | does not exist. State is a fold over the log; there is nothing to load, save, or mock. The pattern compensates for a persistence seam Beck does not have | **dissolved** | [`03`](03-type-and-effect-system.md) §3.7 |
| **Factory** (as test fixture) | does not exist. `given [events]` drives the *real* `apply_event`, so a test cannot construct a state the program could not reach — "a fixture can build an impossible object; a log cannot" | **dissolved** | [`21`](21-tests-in-beck-and-proof.md) §21.2 |
| **Domain-event dispatch infrastructure** (in-memory bus, outbox pattern, dual-write mitigation) | does not exist. The dual-write problem the outbox pattern patches cannot arise: the event *is* the write | **dissolved** | [`15`](15-scale-and-distribution.md) §15.1 |
| **Entity vs value object** | identity is what a fold keys state by; value semantics are the default for every other type. The distinction survives; the ceremony (identity fields, equals-by-id boilerplate) does not | **native, built** | [`03`](03-type-and-effect-system.md) §3.1 |
| **Ubiquitous language** | the program's own type names flow into tests, views, migrations, and the *infrastructure*: `service`, `expose`, `store` are domain concepts and `Deployment` is compiler output, "in the same way `mov` is compiler output" | **native, built** | [`06`](06-kubernetes-and-packaging.md) §6.1 |
| **Anti-corruption layer** | typed `upcast`/`migrate` at a boundary — demanded by the compiler at deploy, not remembered by a reviewer | **native, built at the version boundary**; the *system* boundary form is [`30`](30-bounded-contexts-and-microservices.md) §30.4 | [`03`](03-type-and-effect-system.md) §3.9 |
| **Bounded context / context map** | the real gap: [`10`](10-decisions.md) D2's "one totally-ordered log per application" is one model per application | **designed in [`30`](30-bounded-contexts-and-microservices.md); not built** | — |

The dissolved rows are the claim, not a gap in it. A DDD practitioner's day is substantially spent
maintaining repositories, factories, outboxes and dispatch plumbing — the patterns that exist
because the language underneath does not cooperate. "Beck supports DDD" undersells it; the accurate
sentence is **Beck is what the tactical patterns compile down to when the language cooperates**,
and the dissolved column is the evidence.

## 29.2 The vocabulary decision: claim the equivalence, refuse the jargon

Should `aggregate`, `entity`, `value object`, `repository` become Beck keywords? **No** — for the
same reason [`10`](10-decisions.md) D9 refused to be a framework: *a framework can suggest; only a
compiler can refuse.* Beck's version of "aggregate" is refusable — a fold that reads across its
partition key fails to compile. DDD's version is a convention in a book. Renaming the refusable
thing after the convention would invert the relationship: it would tell practitioners the construct
is their familiar pattern, when the point is that it is the *checked* form of it, with different
edges (no lazy-loaded object graphs, no repository seam to mock, consistency per key rather than
per object cluster).

The .NET analogy from the prompt resolves the same way, and [`16`](16-packages-and-ecosystem.md)
already draws the line: whatever needs refusal or proof belongs in the language; whatever is
vocabulary or convenience belongs in tarns. .NET's layers add capability C# lacks. A Beck DDD tarn
could only add *dialect* — an `aggregate` macro expanding to `validate`/`apply_event`, an
Evans-flavoured naming kit — because the capability is already the substrate. Such a tarn is
legitimate ecosystem material (the macro system is the railtie, §16.3) and nothing for the core to
ship. The core ships the semantics; this document ships the translation.

**The deliverable this section commits to** is a practitioner-facing page — "Beck for DDD
practitioners", derived from §29.1 — on the docs site when there is one ([`16`](16-packages-and-ecosystem.md)
§16.4, the Mere). CQRS/event-sourcing practitioners are the audience most likely to recognise what
Beck is on first contact, because they have been hand-building its semantics out of patterns; the
mapping table is the shortest path to that recognition.

## 29.3 BDD: Given/When/Then is already load-bearing — render prose, never parse it

Look again at the built `test` construct ([`21`](21-tests-in-beck-and-proof.md) §21.2,
[`22`](22-phase-3-report.md) §22.2):

```python
test "an empty todo is rejected":
    given []
    when Add(id="1", text="   ")
    expect Err(Empty)
```

That *is* Gherkin's Given/When/Then — except each clause is a value in a type the program already
declares, driven through the real `apply_event` and the real `validate`, with nothing for a flake
to come from.

Gherkin's actual cost was never the prose; it is the **step-definition glue layer** — regex
bindings between English sentences and code, maintained by hand, drifting from both sides at once.
Cucumber's own literature spends most of its maintenance advice on this layer. Beck's clauses bind
by type, so the layer does not exist. Parsing `.feature` files natively would *reintroduce* it:
every sentence would need a binding to a `Command` or an `Event`, and that binding is the glue
layer under another name.

So the position, recorded as [`10`](10-decisions.md) D19:

1. **The test is the spec; prose is derived, not parsed.** `beck test --explain` renders `test`
   blocks as Given/When/Then English — for a stakeholder review, a living-documentation page, an
   audit. The direction of derivation is the whole point: when the prose and the code disagree,
   the code wins and the prose regenerates, instead of the regex layer failing somewhere in
   between. This is the same instrument family as `beck explain place` and `beck explain
   incremental` — the compiler explaining what is true, rather than a document asserting it.
2. **`.feature` parsing is refused**, and this section is the recorded reason. A team migrating
   from Cucumber ports scenarios by transcription — mechanical, one-way, and an improvement each
   time — not by pointing Beck at their feature files.
3. **A grouping construct is accepted as sugar** — a `feature`/`scenario` wrapper (surface
   spelling to be settled with [`02`](02-syntax.md)) that organises `test` blocks for
   stakeholder-facing rendering and gives `--explain` its section headings. No new semantics;
   `property` blocks already cover the scenario-outline/table-of-examples shape with generated
   rather than enumerated inputs, which is strictly stronger.

And the headline worth saying loudly: the scenario BDD tooling is worst at — *Ana does X and Bo
sees the consequence* — is three lines in Beck, with no network and no simulation of one, because
the boundary is a placement of one graph:

```python
test "one client's command reaches another client's page":
    given []
    when session("ana") sends Add(id="1", text="milk")
    expect page(session("bo")) contains "milk"
```

A Cucumber suite asserting the same property needs a running API, a browser pair, and a tolerance
for flakes. This is [`21`](21-tests-in-beck-and-proof.md) §21.2's cross-boundary claim wearing
BDD's clothes, and it belongs in the practitioner page of §29.2.

## 29.4 The benchmark: Evans's cargo-shipping system, on [`25`](25-benchmarks-and-expressiveness.md)'s rules

§29.1 is a table of prose, and by [`12`](12-standards-and-conformance.md) §12.1's rule that makes
it claims rather than tests. The fix is the one [`10`](10-decisions.md) D18 established for
expressiveness: somebody else's workload, their stated answers as the oracle, refusals checked in
as tests that assert the walls are still standing.

For DDD the workload chooses itself. **The cargo-shipping system** — Evans's own running example,
maintained publicly as the DDDSample application — is to DDD what SICP is to the means of
combination: the canonical text's own exercise, with published reference behaviour. And its domain
is almost provocatively Beck-shaped: `HandlingEvent` is *literally an event log*; `Delivery` is
*defined in the book as a derivation from handling history* — that is a fold, stated in prose in
2003; the tracking page is a read model; `RouteSpecification.isSatisfiedBy` is a pure predicate.
The sample exists to demonstrate patterns Beck either natively provides or dissolves, so writing it
measures exactly the distance between the pattern book and the semantics.

**The shape**, following §25.5's protocol:

- `cargo.beck` — booking (commands `BookCargo`, `AssignRoute`, `ChangeDestination`), handling
  (`RegisterHandlingEvent` → `HandlingEventRegistered`, the fold's substance), delivery status as
  a derived fold (`in port`, `onboard`, `misdirected`, `arrived`), the tracking view, and the
  misdirection detection that the sample drives from handling history.
- **The oracle is the sample's own scenarios**: the published booking-and-tracking walkthrough and
  the reference implementation's acceptance cases, transcribed as `test` blocks — which is also
  §29.3's migration-by-transcription claim, exercised once in public.
- **The counting protocol is §25.5's**, unchanged: same counting algorithm on both sides, more
  than one unit, the reference implementation's LOC beside ours. The number that travels is the
  dissolved column made quantitative — how much of the reference implementation is repositories,
  factories, and dispatch plumbing that the Beck version simply does not contain.
- **Refusals are checked in**, `sicp/refusals/`-style: one file per wall between Beck-as-built and
  the full sample, each with a test that asserts the wall stands, so a wall coming down is a test
  that starts failing.

**The walls, forecast honestly** — this is the part that makes the benchmark a forcing function
rather than a demo:

1. **Sagas.** Misdirection and re-routing choreography wants `process`
   ([`15`](15-scale-and-distribution.md) §15.4), which is designed and not built.
2. **A second context.** The sample is *itself* a two-context system — booking/tracking on one
   side, the routing team's model on the other, with the book's original anti-corruption layer
   between them. It cannot be written faithfully in a language whose applications are one model
   with one log. This is the concrete program that forces [`30`](30-bounded-contexts-and-microservices.md),
   and the reason that document exists as design rather than as an aspiration.
3. **An external system.** The routing context is, in the sample's fiction, *another team's
   system* — so the faithful Beck version exercises §30.4's external-context boundary, not just
   two co-located contexts.

**Status: specified here; not built.** Nothing in this section has run. The staging follows
[`25`](25-benchmarks-and-expressiveness.md) §25.7's pattern: the single-context subset of
`cargo.beck` (booking, handling, delivery fold, tracking view — no routing context, no saga) is
writable against Beck-as-built and belongs in [`corpus/`](../compiler/corpus/) now, where it has to
place itself like the other programs; the full two-context form with its refusal files lands with
[`30`](30-bounded-contexts-and-microservices.md)'s construct and is its acceptance test.

## 29.5 What this document does not claim

- **Not** that DDD's strategic discipline comes free. Context boundaries are a modelling judgement;
  a compiler can check the boundary you drew, not draw it for you.
  [`30`](30-bounded-contexts-and-microservices.md) makes the checkable part checkable and says so
  about the rest.
- **Not** that the dissolved rows mean DDD practitioners have nothing to learn or unlearn. No
  repository means no lazy-loaded object graph; consistency is per key, not per object cluster;
  optimistic UI changes what "the current state" means at the edge. The practitioner page owes
  those differences as much as it owes the mapping.
- **Not** that any number exists yet. §29.4 is a specification with a forecast; the corpus subset
  is the first claim that will be measured, and until it is checked in and green, "Beck expresses
  the cargo sample" is marketing and is not to be said.
