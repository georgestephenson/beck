# 15 — Scale and distribution

> **The question**, via DDIA: distributed systems are multi-faceted and hard — you can't rely on
> clocks, distributed transactions are a tar pit. Can Beck scale endlessly? If not it won't be
> taken seriously. Assume we need something like Redis.

The honest headline first: **nothing that maintains a global total order scales endlessly — and
that is Kleppmann's own thesis, not a Beck weakness.** Systems scale by *partitioning* and by
*coordinating only where an invariant demands it*. Beck's plan is exactly that shape, and its type
system turns the scaling rules from tribal knowledge into checked properties. This document walks
the DDIA problem list against the design, then gives the scale-out architecture.

## 15.1 The DDIA checklist against Beck's semantics

*Seven rows of prose, and by [`12`](12-standards-and-conformance.md) §12.1's own rule that makes
them claims rather than tests. §15.6 is the executable form — every row carrying a verdict and the
evidence that discharges it, indexed against the second edition.*

| DDIA problem | Beck's answer | Where |
|---|---|---|
| **Unreliable clocks** (skew, leap seconds, NTP) | Ordering **never** comes from wall clocks: `seq` — a logical position assigned at the sequencer — is the only order anything depends on. `at` on the envelope is data for humans and views, not a coordination primitive. Timeouts and schedules enter as *ingress events* (a timer fires → an event with a `seq`), so "time-based" logic is replayable and clock-skew-immune | [`03`](03-type-and-effect-system.md) §3.7 |
| **Distributed transactions / 2PC** | Do not exist, by construction. Within a partition: a command's events append **atomically** (contiguous seqs, all-or-nothing — F7), which covers what row-level transactions cover. Across partitions/apps: **sagas** (§15.4) — the compensation pattern, typed. There is no 2PC anywhere to stall or half-commit | §15.4 |
| **Exactly-once delivery** (impossible) | Not claimed. The achievable version — *effectively-once processing* — falls out: idempotent appends by envelope identity, deterministic folds, consumers resuming by `(subscription, seq)`. Duplicates and retries are absorbed, not prevented | [`04`](04-compiler-architecture.md) §4.3–4.4 |
| **Network partitions / CAP** | Stated per scope: **within a partition, CP** — the sequencer is the linearization point; a minority side refuses writes rather than forking history. Availability *feel* comes from Mode B optimism and offline queues (D7), which degrade gracefully instead of erroring. **Across partitions, eventual** via sagas — declared, not accidental | [`10`](10-decisions.md) D7 |
| **Replica divergence** | Replicas of a fold are deterministic replays of the same log prefix — divergence is a bug class the replay-determinism suite hunts ([`13`](13-testing.md)), not an operational condition |
| **Byzantine / adversarial input** | The trust boundary is typed: clients propose, `validate` decides, quotas on by default (F3), first-writer-wins ids (F2) | [`14`](14-review-findings.md) |
| **Process pauses (GC, VM stalls)** | Sequencer leases fence stale writers (a paused ex-leader's appends are rejected by epoch); folds are pause-tolerant by design since they're deterministic consumers | §15.3 |

## 15.2 The scaling ladder

Each rung is a deployment change, not a rewrite — that is what D2 bought by reserving envelope
fields early.

**Rung 0 — one process** (`beck run`): everything in-memory, embedded log.

**Rung 1 — one log, replicated consumers** (v1 ships this): a single sequencer (Postgres-backed,
group-committed appends); **stateless work scales horizontally now** — `validate` workers, Mode A
render/session servers, read-model maintainers, and subscription fan-out all replicate freely
because they are deterministic consumers of the log. Honest ceiling: order 10⁴–10⁵ events/s
through the sequencer, single-region write latency. This covers almost every business that will
ever evaluate Beck — say so without embarrassment, next to the rung-2 path.

**Rung 2 — partitioned logs** (the "endless" rung, designed now, built post-1.0):

```python
commands = merge_clients(partition_by=lambda c: c.customer)   # declared ordering key
```

- The app's log becomes N logs, one per partition-key range; **total order per key, no order
  across keys** — Kafka's model, Rama's model, every event-sourced system at scale.
- **The differentiator: sharding is typed.** A `durable` fold's state is keyed; the compiler checks
  that `apply_event` for a partitioned stream touches only state under the event's partition key —
  cross-key reads inside a partitioned fold are a **compile error** pointing at the exact
  expression, with the fix spelled out: lift it to a global (rung-1) store, or make it a saga.
  Sharding stops being a hope about programmer discipline (Kleppmann's cross-partition-invariant
  trap) and becomes a checked property — the same move the language already made for placement.
- Cross-key invariants ("seat 14A sold once" *per flight* is per-key and fine; "email unique
  globally" is not) either live in a small designated global partition or become sagas. The
  compiler forces the choice to be explicit; it cannot be stumbled into.
- Scaling is then linear in partitions: sequencers, fold owners, and fan-out shard by key range;
  rebalancing moves whole keys (snapshot + log tail per key).

**Rung 3 — geo-partitioning**: partitions get *homes* (`region=eu`); EU customers' partitions
sequence in the EU — write latency stays local, and data residency falls out of the same mechanism
(a GDPR bonus stacking with D4/F1). Cross-region interaction is saga-shaped and therefore honest
about latency. Active-active writes *to the same key* remain deliberately unsupported — that is
CRDT territory, bounded to CRDT-valued types (D7).

## 15.3 The internal fabric — and the Redis question

"Assume we need something like Redis." Broken into what Redis is actually used for:

| Redis job | Beck's answer |
|---|---|
| Cache (and its invalidation) | **Does not exist as a concept** — incrementally-maintained views *are* the cache, invalidated by construction ([`03`](03-type-and-effect-system.md) §3.8) |
| Hot ephemeral state (sessions, presence, cursors, rate counters) | **Four different things, and only one of them is a fold.** This row read "non-durable folds — already a language construct" for as long as it existed, and that was wrong twice over: the construct was unbuilt, and three of the four items are not folds at all. What is built now is `gestures(step, init)` ([`10`](10-decisions.md) D30) — a fold over occurrences that were *never recorded*, which is what makes it ephemeral; D1's "same semantics, no log persistence" named the wrong mechanism, because a fold over the log's own stream is a function of the log whatever it is called. It also said quota counters were "exactly this", and they are not: F3's quota is a **sharded** fixed table because a per-actor map is unbounded memory keyed by a name the client chooses, which is the denial of service it exists to prevent ([`82`](82-the-edge-report.md) §82.5) — a fold would be that map. Presence is not an instance either; it is D6's first-class non-durable `Signal`, moved by connections rather than by events, and it is a compiler-provided source rather than anything a program writes — and `awareness(f)` is that source carrying a payload, which is where *cursors* go rather than into a fold ([`104`](104-styling-and-the-component-library.md) §104.8). So of the four, `gestures` serves only interface state one client keeps to itself |
| Cross-replica pub/sub | The one real need at rung ≥1-multi-node: fold owners publish deltas to session/fan-out servers. This is the already-planned **NATS JetStream** slot ([`07`](07-dependencies.md) §7.4) — an *internal fabric the compiler wires*, never a user-visible API |
| Distributed locks | Not offered. The sequencer's per-key total order **is** the mutual exclusion; leases with epoch fencing handle sequencer failover (the Kleppmann fencing-token pattern, §15.1) |
| An actual external KV, because a team insists | `external store` — and per the licence policy it is **Valkey** (BSD-3, Linux Foundation), not Redis: Redis's RSAL/SSPL/AGPL tri-licence fails the D8-era rules |

So: Beck needs a *fabric*, not a Redis — and the fabric is internal plumbing with one approved
implementation, not a new user-facing system to administer.

## 15.4 Sagas: cross-boundary workflows as typed folds

The missing construct this document adds (real apps hit it in week two — payments):

```python
process fulfil_order(partition_by=order):
    state: FulfilState = Pending
    on OrderPlaced(o):      emit_command(ReserveStock(o.items))   # commands, not events
    on StockReserved(o):    emit_command(ChargeCard(o))
    on ChargeDeclined(o):   emit_command(ReleaseStock(o.items))   # compensation, explicit
    on timeout(30.min):     emit_command(CancelOrder(o.id))       # timers are ingress events
```

A `process` is a fold over events that **emits commands** — the process-manager/saga pattern with
Beck's guarantees attached: deterministic and replayable like any fold; resumable by `seq` across
crashes and deploys; its state visible in `beck explain`; its timeouts entering as ingress events
so even the clock is replayable. Compensation is written, not implied — the language makes the
airline-overbooking trade ([`10`](10-decisions.md) D7's referee discussion) an explicit business
decision. This is the *entire* distributed-transaction story: atomic within a key, saga across
keys, 2PC nowhere.

## 15.5 What "taken seriously" requires us to publish

Scaling claims obey the [`12`](12-standards-and-conformance.md) rule — a claim is a test or it is
marketing: the rung-1 ceiling published as a reproducible benchmark (events/s, fan-out breadth,
p99 under partition-of-a-replica chaos); the Jepsen-style suite ([`13`](13-testing.md) §13.4) run
against the *stated* consistency model per rung and published; and the rung-2 design validated by
DST with simulated partitions before it ships. Roadmap: rung 1 hardened through Phase 4; rung 2 is
the flagship post-1.0 milestone; rung 3 follows customer geography.

## 15.6 The DDIA matrix: §15.1 made executable

§15.1 is seven rows of prose. By [`12`](12-standards-and-conformance.md) §12.1's own rule that is a
scratchpad — **a claim without a test** — which is precisely the position the expressiveness
premise was in before [`10`](10-decisions.md) D18, and it is fixed the same way.

The question that prompted this was whether Beck can "implement solutions for all the problems
raised in DDIA". The honest answer is **no, and the reason matters**: the book raises problems that
are provably impossible (exactly-once delivery), problems Beck deliberately declines (active-active
writes to one key), and problems that are business trade-offs rather than technical ones (how stale
a read may be). A document claiming to solve all of them would be untrustworthy on its face. §15.1's
strongest rows are already the ones that say *"Not claimed"*.

What is achievable is to make **every row executable or explicitly conceded**, and the pattern to
copy is in this repository already: [`12`](12-standards-and-conformance.md) §12.7's OWASP ASVS
matrix, where each control is marked *unrepresentable by construction* — with the test that proves
it — or *generated*, or *the user's responsibility*. Applied here, every DDIA problem carries one of
four verdicts:

| Verdict | Meaning | Evidence required |
|---|---|---|
| **Dissolved** | The problem cannot arise, because the semantics do not admit it | The test that proves it — a `Kani` model, a compile-fail case, or a property |
| **Handled** | The problem is real and there is a mechanism | The mechanism *and* the test that exercises it under fault injection |
| **Bounded** | Handled only within a stated scope | The scope, and the test that demonstrates the boundary — including what happens *outside* it |
| **Conceded** | Not solved, and not going to be | Why, and what a user should do instead |

Three properties this matrix must have to be worth building, each learned from §12.7's version:

1. **Indexed against the second edition** — Kleppmann and **Riccomini**, whose chapter numbering
   differs from the first edition's and whose new material (nonfunctional requirements, the
   rewritten consistency-and-consensus treatment) is exactly where Beck's claims are most exposed.
   Pin the chapter numbers against the published book rather than from memory.
2. **The instruments already exist in the plan.** Nothing here needs a new kind of test:
   [`13`](13-testing.md) §13.4 has Jepsen for the consistency rows, deterministic simulation for the
   fault rows, and soak tests for the resource rows; §13.5 has TLA+ for the three protocols with
   real concurrency. The matrix's job is to say *which row each existing test discharges*, and to
   make the rows with no test visible.
3. **A row without evidence is a bug, not a blank.** The failure mode of a conformance matrix is
   that it becomes a table of intentions. Every row is either evidenced or marked *Conceded* — and
   "we intend to" is not one of the four verdicts.

The value is asymmetric and worth stating: the **Conceded** and **Bounded** rows are the ones a
platform team actually reads, and they are the ones no competitor publishes.
[`25`](25-benchmarks-and-expressiveness.md) §25.8 is where this was decided; the roadmap slot is
Phase 4, beside the Jepsen work it depends on, because a matrix written before the tests exist would
be the table of intentions it must not become.
