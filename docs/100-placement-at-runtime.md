# 100 — Placement at runtime

> **Design, not a report. Nothing here is built**, and the most aggressive levels below should not be
> built for several phases. This is the *maximalist* version written down on purpose: the question
> was how far the idea goes, and a ceiling nobody has drawn is a ceiling somebody will wander past.
>
> Placement is solved at compile time from the effect row, against a cost model
> ([`03`](03-type-and-effect-system.md) §3.4). This document asks whether the *choice* may also be
> made — and re-made — at run time, from measurements, at the granularity placement already uses,
> which is **one definition**. `place.rs` places `Key::Def(name)` and `Key::Signal(name)`; a module
> is not a placement unit and never was, so "a function moves" is the existing resolution rather
> than an escalation of it.
>
> **One line makes the whole thing safe, and everything else follows from it:** legality stays
> static, choice may become dynamic. The compiler proves *which tiers can discharge this row* and
> ships that **candidate set**; a runtime may pick within it and may never widen it. §3.5's claim —
> a secret provably cannot reach the browser — is that proof, and it is fixed before anything
> measures anything.
>
> Two properties are **requirements rather than features** here, and §100.4 and §100.5 are the
> designs for them: every level must be **switchable off**, provably rather than by promise, and
> every decision must be **auditable after the fact, in production**. §8.3 items 8–9 now state both
> as project-wide practice, because the audit in §100.5 found that neither is currently universal.

## 100.1 The line, which this project already drew somewhere else

[`38`](38-literature-survey.md) §38.4 adopts lexically scoped effect handlers, and the reason it
gives is Beck-specific:

> in a language where effects decide placement, accidental interception would mean accidental
> *re-placement*

[`80`](80-structured-concurrency-report.md) applies it again: "a nursery whose membership were
decided at run time would be the dynamic handler search POPL 2019 argues against." Both refuse the
same thing, and it is worth being precise about what:

|   | status |
|---|---|
| The **legality** of a placement decided at run time — an effect row that is not statically known, so the candidate set is not either | **refused**, twice, on grounds that have not weakened |
| The **choice among a statically proven legal set** made at run time | **open** — this document |

The distinction is the whole design. A runtime that could discover new effects could discover new
tiers, and §3.5 would be a claim about the common case rather than a proof. A runtime handed three
tiers the compiler has already blessed is choosing between three things that are each, individually,
already correct.

So: **the deployed artefact carries the candidate set, not just the chosen tier.** Dynamic placement
then cannot violate §3.5 by construction rather than by discipline.

## 100.2 What already exists, and what does not

**The static half of this architecture has twenty-five years of precedent, and
[`38`](38-literature-survey.md) §38.3 already names it** — "the placement pipeline — cost-model
min-cut with secrecy as a hard constraint — is the Coign (OSDI 1999) → J-Orchestra (ECOOP 2002) →
**Swift** (Chong et al., SOSP 2007) → Viaduct (Acay et al., PLDI 2021) lineage". That sentence is
also §100.1's rule, written down before this document existed: a hard constraint from a static
analysis, a cost model choosing inside it. Swift is worth naming twice, because it is the closest
thing to Beck's placement solver that has ever been built — a Jif program partitioned between
browser and server where information-flow labels decide what is *legal* and a cost model decides
what is *fast*. **It partitions entirely at compile time**, with no runtime re-evaluation.

What §38.3 does **not** have is the dynamic lineage, and it is real:

| lineage | what it did | how it differs |
|---|---|---|
| **MAUI** (MobiSys 2010) | **decides at run time** which methods to offload, method-level, an LP solver minimising energy under latency constraints from measured connectivity | genuinely runtime and genuinely measured — the direct ancestor of P2–P5. Optimises *energy*, and the device is untrusted in a different sense than a browser is |
| **CloneCloud** (EuroSys 2011) | static analysis *plus* dynamic profiling; migrates a **thread** mid-execution to a clone in the cloud and re-integrates it | the migration is of live thread state — the part Beck does not need (below) |
| **Wishbone** (NSDI 2009) | profiles the operators of a **dataflow graph** and solves an ILP minimising bandwidth and CPU; a different partition per platform | closest in *shape* to Beck's plan, since the unit is a dataflow operator; computed per platform rather than continuously |
| **Coign** (OSDI 1999) | scenario-based profiling of a COM binary, then a graph cut | the ancestor of the solver; profiled offline, partitioned once |
| **Emerald** (Jul, Levy, Hutchinson & Black, ACM TOCS 6(1), 1988) | fine-grained object mobility — objects move between nodes, *even during an invocation* | the strongest mobility precedent, and mobility of state is what Beck replaces with re-derivation |
| **JIT tier-up and deoptimisation** — HotSpot, V8, LuaJIT, PyPy | code moves between execution tiers from profile, continuously | one address space; "tier" means compilation level, not trust domain |
| **Multitier languages** — Links, Ur/Web, Hop, Eliom, Opa, ScalaLoci, Swift | split one program across client and server | **splitting is at compile time.** Eliom's own claim is "static slicing which separates client and server parts at compile time"; the ACM CSUR 2020 survey's dynamic category is *runtime tier checks* and *privacy-driven* splitting, not placement revised from performance |

So the honest claim is narrower and better evidenced than "unprecedented": **each half has a
literature and the conjunction appears to have no occupant.** Offloading systems re-decide at run
time but have no static legality proof — MAUI moves what fits, and safety is the programmer's.
Multitier languages have the proof and never re-decide. What neither line has is the third thing:
**deterministic replay**, which turns a cutover from a leap into a comparison.

**Why the offloading line's hard problem is not Beck's.** MAUI and CloneCloud fought *state
migration* — moving a method means moving its heap reachability, in languages with shared mutable
state, and CloneCloud needed thread-level VM capture to do it. Beck has already paid that price
three times over, for unrelated reasons:

- **Unplaced code is already everywhere.** `Tier::Any` is compiled into every tier that calls it —
  43.2% of the corpus at [`20`](20-phase-2-report.md) §20.3's measurement. Moving pure code is not
  migration; both copies exist already, and the "move" is a decision about which one to call.
- **Durable state is never migrated, it is re-derived** — a snapshot plus a log tail, which is
  already how `beck fork` works and already how [`15`](15-scale-and-distribution.md) §15.2's rung-2
  rebalancing moves whole keys.
- **Determinism makes a cutover verifiable.** Two placements of the same code must compute the same
  answer, so the runtime can shadow the alternative and diff before committing to it. That is the
  differential harness ([`04`](04-compiler-architecture.md) §4.8) used in production rather than in
  CI, and it is affordable only because the semantics are deterministic.

## 100.3 The seven levels

Each level is a superset of the one above. **Blast radius** is what a wrong decision costs, which is
what should decide how far a deployment opts in.

| | what moves | what decides | needs | reversible | blast radius |
|---|---|---|---|---|---|
| **P0** | nothing | the compiler, once | today | — | none |
| **P1** | nothing at run time — a **proposal** | measurement, offline | the decision record (§100.5) | n/a | none; a human accepts a `beck.lock` diff |
| **P2** | one choice per **process**, at start-up or on a schedule | measured conditions of that deployment | candidate set in the artefact | restart | the deployment |
| **P3** | one choice per **subscriber** | that connection's measured RTT and device | Mode B ([`94`](94-the-client-report.md)) | per connection | one session |
| **P4** | one choice per **call site** | measured cost of each copy | nothing new — the copies exist | immediate | one call |
| **P5** | **continuous** re-placement, shadow-verified | both run, answers diffed, promoted on a margin over a dwell | determinism as a production oracle | immediate | one node, bounded by the shadow |
| **P6** | placement follows **topology** | where the user is, where the partition lives | [`15`](15-scale-and-distribution.md) §15.2 rung 3 | slow | a region |
| **P7** | the **controller itself is a Beck fold** over a measurement stream | a program, replayable | §100.6, and **P5's shadow verification as a prerequisite** rather than a lower rung | — | a controller with a bug moves code, which is why P5 comes first |

**P3 is the one to build first**, and the cost model was written for it.
[`cost.rs`](../compiler/crates/beck-core/src/cost.rs) charges a crossing the *minimum* of its two
endpoints and says so in as many words: "Phase 2 only *implements* Mode A, so today the minimum is a
prediction rather than a choice". The prediction has been waiting for the choice since Phase 2.

It carries one constraint that is not obvious and is not negotiable.
[`94`](94-the-client-report.md) refuses *inferring* the render mode — "a wrong inference ships a state
to a browser" — and the refusal is about views that **read the session**, because Mode B hands the
browser the state a per-session view was filtering. So dynamic A/B is legal exactly **above the
session cut**. That is the fourth unrelated question answered by that one boundary — arrangement
sharing ([`23`](23-incremental-views-report.md)), read-model tables
([`23`](23-incremental-views-report.md)), the fusion refusal
([`23`](23-incremental-views-report.md) §23.13), and now this. A boundary that keeps being the answer to
questions it was not drawn for is load-bearing, and should be treated as a primitive of the design
rather than as a property of the fanout optimisation.

## 100.4 Configurable: a ceiling, not a set of flags

**The switch is one ordered ceiling**, not seven independent toggles:

```toml
[placement]
dynamism = "static"   # static | propose | process | session | callsite | continuous | topology
```

Ordered because the levels are cumulative and independent flags would let a deployment enable P5's
promotion without P2's candidate machinery — a configuration that type-checks and means nothing.
One ceiling has one meaning, prints in one line, and diffs in a pull request.

Four rules, and the second is the one that makes this a guarantee rather than an intention:

1. **`static` is the default**, and it is what every existing deployment gets without editing
   anything.
2. **`static` must be provably identical to today**, not approximately: a gate runs the corpus with
   the ceiling at `static` and asserts the same placement, the same rendered pages and the same
   replay digest as the locked solution. An off switch nobody has proved is off is a claim
   ([`23`](23-incremental-views-report.md) §23.2's rule, in a new place).
3. **`@on(...)` remains absolute.** The language already has the per-definition opt-out and it
   already always wins; a dynamic level may not move an annotated node, at any ceiling. So a
   developer who wants one function pinned in an otherwise dynamic program has the mechanism today.
4. **No level may widen the candidate set** (§100.1). This is not configuration — there is no
   setting that permits it.

This mirrors what `AppConfig` already does for the view engine, where the doc comments state the
principle better than a rule would: `maintain_views` is "a *switch* rather than a fact because it is
also a memory-for-time trade", and an operator "should be able to decide that differently without
recompiling".

## 100.5 Auditable: the decision record, and the gap the audit found

**What "auditable" has to mean here**: after the fact, in production, without a debugger, a person
can find out *what moved, when, from where to where, what measurement caused it, what the
alternative was estimated to cost, and what dwell or hysteresis applied*. Anything less makes a
dynamic system undebuggable at exactly the moment somebody needs to debug it.

Three design choices:

- **The decision record is a read model.** [`23`](23-incremental-views-report.md) already
  projects maintained state as relations any Postgres client can read, so the audit surface needs no
  new protocol, no new tool and no new page — `psql` and whatever BI tool the team owns already
  work. This is reuse rather than a new facility.
- **`beck explain place --at <time>`.** The `beck explain` family is this project's audit culture and
  it answers about *source*. The same question asked about a *running or past system* is the missing
  half (below).
- **Replayable, not merely recorded.** Given the measurement stream, the same decisions must
  re-derive exactly. "Here is what I did, and you can re-derive it" is a strictly stronger claim than
  "here is what I did", and §100.6 is what makes it available.

### The audit, and it is the reason §8.3 grew two items

Asked how universal these two principles currently are, the answer is: one is strong with a hole,
and the other has a whole half missing.

| | state |
|---|---|
| **Configurable** | **strong.** `AppConfig` carries `maintain_views`, `share_arrangements`, `retention`, `quota`, `presence`, `snapshot_every`, `max_batch`, `dedup_capacity` — the incremental view engine, the largest optimisation in the system, can be switched off at run time without recompiling, and the doc comments say why |
| | **with one hole: query fusion has no off switch.** [`plan.rs`](../compiler/crates/beck-core/src/plan.rs)`::compile` always fuses; `Plan::unfused` is reached only from `beck explain query --unfused` and the test harness, never from `beck-rt`. The engine that *runs* the plan is switchable and the rewrite that *produces* it is not — an inconsistency rather than a decision, and the one a wrong fusion would be diagnosed by |
| **Auditable, compile time** | **exceptional.** `beck explain` answers `place`, `flow`, `wire`, `query`, `cost`, `incremental`, `sql`, `render`, `deploy` and `error` |
| **Auditable, run time** | **quantities only.** [`telemetry.rs`](../compiler/crates/beck-rt/src/telemetry.rs) has counters, gauges, histograms, records and OTLP; [`23`](23-incremental-views-report.md) §23.14 exports subscription and arrangement metrics. **Nothing anywhere records a *decision*.** Metrics say what happened; they do not say what the system chose or why |

That last row is the general gap, and it is what dynamic placement would fall into. It is also the
reason to build the decision record as a **facility** rather than as a feature of placement: fusion
already makes choices nothing records, the plan solver ([`99`](99-the-data-tier-means-of-combination.md)
§99.8) will make more, and each one built separately is a different format nobody can join.

**The universal, stated once:** `beck explain` has a compile-time half and needs a run-time half.
*Why is this here* is answered; *why did it do that* is not.

## 100.6 P7 — the controller is a fold, which is what makes both principles free

The maximalist end is not "more aggressive heuristics". It is that the placement controller is
**written in Beck**: measurements are a stream, and the placement decision is a fold over it.

```python
placement: Signal[Placement] = durable(fold(decide, initial, measurements))
```

This is the level worth reading the rest of the document for, so the argument is made rather than
asserted. **Every property the two principles ask for stops being a feature and becomes a
consequence**, and each line below is a thing that would otherwise have to be built, tested and kept
in sync by hand:

| the principle asks for | a bolted-on controller | a controller that is a fold |
|---|---|---|
| an audit trail | a log format, a writer, a retention policy, and a reviewer checking it still matches the code | **the log is the audit** — there is no second artefact and nothing to drift |
| reproducing a past decision | best effort, from whatever was logged | replay the measurement stream and **re-derive it exactly** |
| two engineers agreeing about an incident | they compare screenshots of dashboards | both replay the same log and get the same answer |
| testing the policy | a mock harness in Rust, testing the controller against invented inputs | `test` and `property` blocks in Beck ([`21`](21-tests-in-beck-and-proof.md)) — *"no measurement sequence may move this node to the client"* is an assertion in the program |
| turning it off | a flag threaded through the controller's call sites | the fold does not run |
| changing the policy | a compiler release | an ordinary program change, with the wire and migration discipline the language already has |

Two further consequences that are not on that list because nothing else offers them at all.

**The controller can be replayed against a counterfactual.** §99.8's ladder says a plan choice can be
settled by running candidates over a real log and counting `Work`. A controller that is a fold is
subject to exactly the same treatment: *what would a different placement policy have done to last
month's traffic?* is a replay, not a simulation — so a policy change is evidence-backed before it
ships, which is the standard §99.8 sets for plans and which nothing in this space currently meets.

**It is the project's own claim, turned on itself.** [`10`](10-decisions.md) D15 makes dogfooding a
decision, and the registry is the flagship. A placement controller written as a Beck fold is a
stronger version: the *runtime's own control plane* expressed in the semantics the language sells.
If the fold-over-a-log model is not good enough to decide where code runs, that is worth knowing
before telling anybody else to build a business on it.

**One correction this design needs, and it is easy to get wrong.** The measurement stream must have
its **own log, disjoint from the application's**. [`03`](03-type-and-effect-system.md) §3.7 makes the
application's log the only description of its history, and a placement change is an *operational*
fact — like which pod served a request. Putting it in the application's log would make a replay of
the business history depend on the RTT of a browser in 2027, which is precisely the nondeterminism
§1.1's third move exists to confine to declared merge points. Two logs, two replays, one mechanism:
the business history is a function of its log alone, and the operational history is a function of
its own.

**What P7 is not.** It is not a claim that the controller is easy, and three things about it are
genuinely open: the policy language a `decide` function needs is unspecified; a fold over a
measurement stream is continuous work whose cost is unbudgeted (§100.10); and a controller with a
bug is a controller that moves code, so the shadow-verification of P5 is a **prerequisite** for P7
rather than a lower rung it supersedes. The order in §100.9 reflects that.

## 100.7 What it costs: least privilege, which is the non-obvious price

[`06`](06-kubernetes-and-packaging.md) §6.5 derives NetworkPolicy, RBAC and Postgres grants **from
placement** — "the part no existing tool can do", and the feature the platform-team pitch leads with.

If a node's tier can change at run time, its derived policy must be the **union over its candidate
set**. Dynamism is therefore paid for in exactly the currency §6.5 sells. Three consequences:

- **Opt in per node, not only per deployment.** A definition that may move is a definition whose
  effects must be permitted on every tier it may move to.
- **`beck explain deploy` must show the widened policy**, beside what a static placement would have
  emitted, so the trade is visible in the diff rather than discovered in an audit.
- **The default ceiling should be low**, and the cost is the argument for it — not caution in the
  abstract.

## 100.8 What must not change

**Placement stays semantically invisible.** §3.7 makes the backend a deterministic function of the
log and §1.1's third move admits nondeterminism only at declared merge points. So:

- a placement change is **not an event**, and does not appear in the application's log (§100.6);
- it does not touch the **replay digest**, and the gate is that a replay under any two legal
  placements produces identical digests;
- it does not change **what** anything computes — only where. This is already true, because
  `Tier::Any` code is compiled into every tier and must agree; dynamic placement makes it a property
  to be *tested*, not merely believed.

Two operational properties that a controller needs and a static solver never did: **hysteresis and a
minimum dwell**, because any feedback controller can thrash; and **`Method`-style honesty** — the
placement solver already reports `Exhaustive` versus `Sweep` rather than implying optimality, and a
runtime controller should report what it moved, why, and on what evidence, in the same spirit.

## 100.9 Where this goes on the roadmap

| level | phase | with |
|---|---|---|
| **P1** — measured proposal | **4** | `beck tune` right-sizing and the replay tooling, both already in Phase 4. This level changes no runtime behaviour |
| **the decision record** (§100.5) | **4** | built as a facility, not for placement — fusion and the plan solver are already making unrecorded choices |
| **the fusion off switch** (§100.5) | **4** | closing the configurability hole the audit found; small |
| **P2, P3** — per process, per subscriber | **5** | Mode B exists; the cost model already predicts the choice |
| **P4–P6** — per call site, continuous, topology | **post-1.0** | P6 needs [`15`](15-scale-and-distribution.md) §15.2's rung 3, which is post-1.0 already |
| **P7** — the controller as a fold | **post-1.0**, and it is the end state rather than an increment | needs its own log (§100.6) and the package/dogfood discipline of D15 |

## 100.10 What this does not claim

- **Nothing is built**, and P4–P7 should not be built soon. The value of writing them down now is the
  ceiling and the two invariants, not the schedule.
- **The survey in §100.2 is checked but not exhaustive.** Each system named there was read back to
  its paper; "the conjunction appears to have no occupant" is a claim about an absence, which is the
  kind no search establishes. It is stated so that it can be falsified by one citation.
- **No number here is measured.** Whether P3 is worth anything on a real connection is unknown; the
  cost model *predicts* it, which is what §99.8's ladder exists to replace with evidence.
- **The controller's own cost is unbudgeted.** A fold over a measurement stream is work, and P7 pays
  it continuously.
- **It does not reopen the §3.5 proof, or ask to.** §100.1's line is the reason this is a design and
  not a hazard.

## 100.11 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`08`](08-roadmap.md) §8.3 | Cross-cutting practices did not include configurability or auditability, and the audit in §100.5 found one hole and one missing half. Items 8–9 now state both |
| [`38`](38-literature-survey.md) §38.4 | Its re-placement argument is now load-bearing for a second reason, and the distinction it implies — legality static, choice possibly dynamic — is drawn explicitly here |
| [`05`](05-tier-lowering.md) §5.1 | The Mode A/B table's "inferred from those requirements" was corrected by [`94`](94-the-client-report.md) to "declared, never inferred". P3 is the conditions under which inference becomes legal again — above the session cut, measured rather than guessed, and reversible |
