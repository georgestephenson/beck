# 51 — Phase 3 report, part 21: the arrangement lifecycle

> **What this is**: two of the three loose ends [`26`](26-arrangement-sharing-report.md) §26.9 left
> on the shared dataflow — arrangements that are never released, and a change history that is a
> constant rather than a policy — **built**. They turned out to be one missing rule rather than two
> items, and it is the rule [`38`](38-literature-survey.md) §38.2 named: a **reader set**, a
> frontier per reader, history compactable up to the minimum frontier, and the trace droppable when
> the set is empty. The third loose end, the render lock, is **not** touched and §51.7 says why.
> Measured: a process that had 64 subscribers and now has none gives back **99.1%** of what the
> shared side was holding, and a fanout that keeps up retains **one** version of change history
> rather than 64.

## 51.1 The two items, and the sentence that turned them into one

[`26`](26-arrangement-sharing-report.md) §26.9, verbatim:

> **The shared dataflow is never released.** It holds its arrangements whether or not anybody is
> subscribed. A process that had a fanout and now has none keeps the accumulator's arrangements
> warm for a reconnection that may not come. Nothing measures how much that is and nothing drops it.
>
> **The history is a constant, not a policy.** 64 versions, chosen because a subscriber further
> behind than that is not the bottleneck, and not because anything measured where the knee is. A
> deployment with slow clients and fast events would want it configurable; nothing exposes it.

They read as two pieces of work — a drop policy and a config field — and they are not. The reason is
[`38`](38-literature-survey.md) §38.2, which read the same two bullets against the literature and
came back with one answer:

> §26.9's "the shared dataflow is never released" and "the history is a constant, not a policy" are
> both answered by the reader-frontier discipline of **Shared Arrangements** (McSherry, Lattuada,
> Schwarzkopf & Roscoe, VLDB 2020): each subscriber holds a frontier; the trace is compactable up to
> the minimum subscriber frontier and droppable when the reader set is empty. […] **Adopt** — this
> is the cheapest item in this survey: the engine already has versions and subscribers; it lacks
> only the rule connecting them.

That is the whole of what this change is, and the survey's estimate held. The engine had versions
(`SharedInner::version`, a `Step` per advance) and it had subscribers (an `Engine` per subscription,
with a `seen`). Nothing anywhere related the two, so the dataflow could not answer either question —
*is anybody reading this?* and *how far behind is the furthest of them?* — and a system that cannot
answer those has no choice but to keep everything forever, which is what it did.

## 51.2 A subscriber is a counted reader

`SharedDataflow::subscriber` used to hand out an `Engine` and forget about it. It now enters the
engine in a reader set and gives the engine the means to say it has gone:

```rust
pub fn subscriber(self: &Arc<Self>) -> Engine
```

The `Arc<Self>` receiver is the load-bearing part, and it is the reason this is a change to a
signature rather than an added method. A subscription does not *end* in the runtime — nothing calls
`close()`. It ends because `beck-rt`'s `session::run` returns, by completing, by erroring or by its
socket dying, and its `Engine` is a local that goes out of scope. So the engine has to be able to
reach the dataflow from its own `Drop`, and that means holding an `Arc` of it.

There is no cycle. The dataflow's reader set holds frontiers, not engines; the engine holds the
dataflow. And the engine a `SharedInner` owns for the shared operators themselves is built by
`Engine::for_nodes` with no attachment at all, which is what keeps `detach` from being reachable
while its own write lock is held.

### The frontier is an atomic, and that is not an optimisation detail

The obvious implementation is a `BTreeMap<ReaderId, u64>` under the dataflow's existing lock, and it
is wrong for a specific reason. A frontier is written on **every render** and read only when the
dataflow **advances** — and §26.2's second choice is that subscribers render concurrently under a
read lock, precisely so a thousand of them do not queue. Publishing a frontier under the write lock
would have serialised every render behind every other render, and it would have done it in the name
of a bookkeeping update that no reader was waiting for.

So each reader holds an `Arc<AtomicU64>`, the dataflow holds the other end, and the store is
relaxed and lock-free. The compaction reads the minimum under the write lock it was already taking
to advance.

The ordering that makes this safe is worth stating because it is not obvious:

- A reader publishes its frontier **after** its render, outside the read lock. In the window between
  releasing the lock and publishing, its frontier reads *older* than it is — which retains more
  history than needed. Retaining too much is a memory cost; retaining too little is a wrong page.
  The race can only go the safe way.
- Compaction runs under the **write** lock. A render holds the **read** lock for its whole duration,
  so a compaction cannot start while any render is in flight, and cannot drop a step a render has
  already decided to use.

### A reader that has not rendered pins nothing

A fresh subscriber's frontier is `UNRENDERED` (`u64::MAX`), not 0, and it falls out of the minimum
rather than having to be filtered out of it.

Treating it as 0 would be the natural reading — it has rendered nothing, so it is behind everything —
and it would have been the bug. An engine with no arrangements **rebuilds** whatever it is offered:
its `seen` is 0, no history reaches back that far on a live process, and `Upstream` resolves to
"everything changed and everything rebuilt" by construction. So a reader at frontier 0 would pin the
entire history for the one subscriber that cannot use a single step of it — and a connection that
opens and never renders would hold the ceiling's worth of change for the life of the process.
`a_subscriber_that_has_not_rendered_pins_nothing` is that case as a test.

## 51.3 Retention is a ceiling over a floor

What is kept is now bounded by two different *kinds* of thing, and the code says which is which:

```rust
fn compact(&mut self, depth: usize) {
    let floor = self.floor();
    while self.history.front().is_some_and(|s| s.to <= floor) {
        self.history.pop_front();
    }
    while self.history.len() > depth {
        self.history.pop_front();
    }
}
```

The floor is a **fact**: a step every attached reader has already rendered past is retained for
nobody, and dropping it cannot change any page. The depth is a **policy**: past it we would rather a
very late subscriber rebuild than hold change indefinitely for it. 64 is still the default depth and
it is still not a measured knee — but it is now a ceiling that almost never binds rather than the
retention itself, which is a different claim and a much weaker dependence on the number.

Both live on a `Retention` the application configures:

```rust
pub struct Retention {
    pub depth: usize,
    pub release_when_idle: bool,
}
```

`release_when_idle` is a switch and not a constant because the trade is genuinely a deployment's.
Releasing gives back almost everything (§51.5) and charges the next subscriber a cold start; a
service whose clients reconnect constantly would rather pay the memory. The default is to release,
because that is the published discipline and because a process that is idle for hours holding a
fanout's arrangements is the worse failure. `a_dataflow_told_to_stay_warm_stays_warm` and
`a_release_costs_the_next_subscriber_a_cold_start` assert both sides, so neither is folklore.

**No clock was introduced.** The obvious refinement — release after a grace period, so a reconnect
within seconds stays warm — needs elapsed time, and elapsed time is not on `beck_core::clock`'s seam
([`44`](44-wave-0-report.md) §44.3 supplies wall-clock instants and says what is deliberately not
there yet). Putting a timer inside the engine would have been the third place in the tree that reads
time ambiently, three phases after F11 said not to. It is named in §51.6 as unbuilt rather than
approximated.

## 51.4 The release is the reset that already existed

```rust
fn release(&self, inner: &mut SharedInner) {
    if !inner.started { return; }
    inner.engine.reset();
    inner.history.clear();
    inner.started = false;
    inner.version = 0;
    self.releases.fetch_add(1, Ordering::Relaxed);
}
```

This is deliberately, exactly the path an error already took. §26's engine discarded its
arrangements when an operator failed mid-advance, on the grounds that a half-updated arrangement is
the one failure mode that would be invisible — and what it left behind had to be a dataflow that
says it has *never been advanced*, rather than one that has been advanced and then hollowed out.

That is the same requirement here, and reusing the path is worth more than the four lines it saved:
the correctness of "a dataflow with nothing in it serves the next subscriber a right page" was
already under test, because the error path is already exercised. What was new was the *second*
render after a release — a dataflow that reset its arrangements but kept its version would advance
from a version it can no longer describe, and hand deltas against arrangements that are not there.
`a_page_survives_the_arrangements_being_released_underneath_it` renders twice for that reason, over
every corpus program.

## 51.5 What it gives back

`cargo test --release --test measure_incremental what_the_arrangement_lifecycle_gives_back --
--nocapture`, 200 rows, 64 subscribers:

```
What an idle process holds — 64 subscribers, then none
             program  connected KB    shared KB      idle KB given back
  examples/todo.beck          8684           57            5    90.3%
        24-feed.beck         10026          498            4    99.1%
```

`connected` is the whole fanout — the shared side plus every subscriber, walked with one exclusion
set so structure held by `Arc` between subscribers is counted once (§26.5's `fanout_footprint`).
`shared` is the part of that held once for everybody. `idle` is what is left when the last
subscription ends.

**Read the percentage against the right denominator.** It is of the *shared* column, not of
`connected`: the per-subscriber arrangements go when the subscribers go, and always did. What is new
is the shared column dropping, and on `24-feed.beck` — the program written for the case where the
shared prefix is most of the plan — that is 498 KB down to 4. The residue is the plan's constants
and the empty cells, which is a per-operator cost rather than a per-row one.

`examples/todo.beck` gives back less in absolute terms and for the reason §26.5 gave about the same
two programs: the sketch filters by `session.actor` immediately below the accumulator, so there was
never much above the cut to release. 57 KB is what a program like that was holding for nobody; 498
KB is what a program like the feed was. Neither is a large number on a laptop, and both are per
*process*, so the honest statement is that this fixes a leak of a bounded size rather than an
unbounded one — the arrangements were always proportional to the accumulator, never to uptime.

The history half:

```
How much change history a fanout pins — 24-feed.beck, one laggard
 laggard's lag    retained  the ceiling      saved
             0           1           64      64.0×
             1           1           64      64.0×
             4           4           64      16.0×
            16          16           64       4.0×
            70          64           64       1.0×
```

Retained is the laggard's own lag, exactly, until the ceiling takes over. The last row is the
ceiling still doing its job: a subscriber 70 versions behind is served by rebuilding rather than by
retaining 70 versions for it, which is the policy 64 was always meant to express and now is the only
thing it expresses.

The first two rows are the common case and the one the constant was most wrong about: a fanout whose
subscribers all render at every version was costing 64 versions of retained change and costs one.
`a_fanout_that_keeps_up_keeps_one_version_of_history` pins that from the harness at a fanout of 32.

**A number this table does not contain**: bytes per retained version. A `Step` is a delta rather
than a collection — a rebuilt operator's changes are deliberately not kept (§26) — so 64 versions of
a quiet program is small and 64 versions of a program that churns its whole collection every event
is not, and one program cannot say which. What the table measures is *versions*, which is the unit
the ceiling is in.

## 51.6 The correctness claim, and where it is made

Everything above is memory. A dropped arrangement that was still needed is not a crash — it is a
subscriber whose page is quietly one row wrong, which is the failure mode §26.4 built the recompute
oracle for. So the tests assert the page is unaffected **before** they assert anything is dropped,
and the first two of the eight new ones are over the whole corpus:

| Test | What it would catch |
|---|---|
| `what_is_retained_never_changes_a_page` | Three subscribers arriving and leaving at different points, against recompute at every version, over every corpus program. The middle one departs halfway, compacting the history the survivors are using, and is replaced by one attaching to a dataflow that has moved on |
| `a_page_survives_the_arrangements_being_released_underneath_it` | A released dataflow that is not indistinguishable from one never started — including the second render, where a kept version would hand out deltas against arrangements that are gone |
| `the_history_is_bounded_by_the_laggiest_subscriber_and_not_by_the_ceiling` | Compacting past a reader that still needs it; and the converse — a laggard leaving without releasing what was being kept *for it* |
| `a_fanout_that_keeps_up_keeps_one_version_of_history` | The constant coming back |
| `a_subscriber_that_has_not_rendered_pins_nothing` | §51.2's frontier-0 bug |
| `the_arrangements_go_when_the_last_subscriber_does` | Releasing while somebody is still reading (asserted at 7-of-8 gone), and not releasing when the last one goes |
| `a_dataflow_told_to_stay_warm_stays_warm` / `a_release_costs_the_next_subscriber_a_cold_start` | The policy doing nothing, and the cost of the default going unstated |

The nine pre-existing tests in `shared_arrangements.rs` are unchanged and green, which matters more
than the eight new ones: `a_subscriber_that_skipped_versions_still_gets_the_recomputed_page` compares 5,496
pages against recompute at six lags including one past the history's end, and it is the test
that would break first if compaction were wrong. `incremental_engine.rs` — every corpus program,
every event, every subscriber at every version — is likewise untouched and green.

### The gauge that would have gone stale, and the drop order that fixes it

`shared_arranged` is sampled on a render. With nothing else changed, a process whose last
subscription ended would have reported the fanout's entries for as long as it stayed idle — the one
moment an operator most wants the number to say zero, and a metric that is wrong exactly when it
matters is worse than an absent one.

The fix is a guard declared *before* the engine, so Rust's reverse drop order runs it *after*:

```rust
let _shared = SharedGauge(app.clone());
let mut engine = app.view_engine()?;
```

Two new numbers join it, both exported to the dashboard JSON and to OTLP. `shared_retained` is the
versions being kept, which reads as a **lag** signal rather than a memory one — it sits at 1 on a
healthy fanout and rises when renders stop keeping up with events. `shared_releases` is the count,
and a process whose releases track its connection count is one whose clients are flapping, which is
the case for turning `release_when_idle` off. `view_metrics.rs` asserts all of it through a running
process with real sockets rather than through the engine's own API.

`Counter::sync` is new and is the one piece of API here worth flagging as a smell: the shared
dataflow counts its own releases, and the telemetry counter adopts that total rather than counting
the same events twice. Its doc comment says `incr` and `sync` must not be mixed on one counter,
because nothing enforces it.

## 51.7 What is **not** built

| | Status |
|---|---|
| **The lock is still a lock** | **Unchanged**, and deliberately. §26.9's third loose end: subscribers render under a read lock, so the advance waits for the slowest render in flight. That is right for readers and it is not a design that has been run at a fanout where it would not be. This change makes it *more* load-bearing rather than less — compaction is safe because a render holds the read lock for its whole duration (§51.2) — so replacing the lock is now a change to two things at once, and the report should say so before somebody does it |
| A grace period before releasing | **Not built.** It needs elapsed time, which is not on the clock seam (§51.3). The switch is the whole policy today: release immediately, or never |
| A measured knee for `depth` | **Still not measured.** 64 is still a guess; it is now a ceiling that binds only past the laggiest reader, so the guess costs much less. §51.5's table is where a deployment would look to pick its own |
| Bytes per retained version | **Not measured**, and §51.5 says why one program cannot say it |
| Partial state and upqueries | **Not built.** [`38`](38-literature-survey.md) §38.2's **borrow** verdict on Noria — evicted arrangements refilled on demand — is the finer-grained version of this whole change. What is built is all-or-nothing per dataflow: the reader set is empty or it is not |
| Per-operator lifecycle | **Not built.** An operator no *connected* subscriber reads is still maintained if any operator does; the unit of release is the dataflow |
| SQL read models, pgwire, query fusion, `beck explain cost` | **Nothing**, unchanged from §26.9. Still the largest part of the incremental-views bullet |
| The page streamed as deltas | **Nothing**, unchanged from [`24`](24-incremental-views-report.md) §24.6 and §26.9 |

## 51.8 What this corrects

- **[`26`](26-arrangement-sharing-report.md) §26.9 loses two of its eight bullets.** "The shared
  dataflow is never released" and "the history is a constant, not a policy" are done. The other six
  — the unfinished views bullet, the unstreamed page, the lock, no LLVM backend, the nine untouched
  Phase 3 bullets, and `check.rs` — stand exactly as written. §26.9 is history and is not edited;
  this section is the correction.
- **[`38`](38-literature-survey.md) §38.2's first **adopt** verdict is cashed**, and its estimate
  was right: the engine did have versions and subscribers and did lack only the rule. It is the second of that survey's verdicts to become
  work — [`27`](27-the-walls-come-down-report.md) cashed §38.4's before it — and it is worth recording that
  the survey's value here was not a technique, since a reader set is not a hard idea, but the
  observation that **two bullets were one bullet**. §38.9 says an adopt verdict "awaits its named
  piece of work"; this one has had it.
- **[`08`](08-roadmap.md) §8.5.5's "Now" pairing was half right.** It named Lane B as "the shared
  dataflow's three loose ends, then SQL read models", and predicted no collision with Lane A's
  `check/` work. Two of the three are done and nothing in `check/`, `ty.rs` or `core.rs` was
  touched, so the prediction held — but the row has now been the recommended Branch 2 for three
  consecutive rewrites without being taken, which is a fact about staffing rather than about
  sequencing and the next rewrite should stop treating it as newly available.
- **[`05`](05-tier-lowering.md) §5.3's per-session memory has a second half.** §26.7 exported what a
  fanout costs; what a process holds *between* fanouts was not a number anybody could read. It is
  two now, and one of them (`shared_retained`) turned out to be a better lag signal than a memory
  one — which is not what it was added for.
