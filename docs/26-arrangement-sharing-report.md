# 26 — Phase 3 report, part 4: one shared dataflow

[`24`](24-incremental-views-report.md) §24.7 ended with a sentence that was a promise rather than a
finding:

> What blocks it is not the analysis, which is done, but the runtime: subscribers render at different
> times, a shared arrangement would have to be advanced under a lock the sequencer already holds, and
> a subscriber that fell behind would need either a version history or a rebuild. That is a real
> design with real choices in it and it is the next thing this work should get, not a line of
> plumbing.

This is that design, built. [`05`](05-tier-lowering.md) §5.3's "a thousand connected users … must
compile to *one* shared dataflow whose final per-session operators run per subscriber" is now what
the runtime does, and `SharedDataflow::advances` is the counter that says so: 64 subscribers over 11
versions advance the shared prefix **11 times**, not 704.

**How much that is worth is entirely a property of the program, and both ends of the range are in
here rather than in a footnote.** On `24-feed.beck` — a public feed, personalised only by its
greeting — 256 subscribers cost **4.3× less memory** and one event costs **55× less work**. On the
todo sketch, whose `mine` filters by `session.actor` immediately below the accumulator, the same
numbers are **1.4×** and **1.3×**. A report that quoted only the first would be describing a
different program.

Along the way this found a defect in the engine [`24`](24-incremental-views-report.md) shipped, in
the half of it that report was most confident about (§26.6), and closed one item from §24.10 that is
not about sharing at all: §5.3's per-session memory is now an **exported metric** rather than a
number in a report (§26.7).

466 tests, no failures, no compiler warnings, no clippy warnings — up from the 451 this merges
with, which is [`24`](24-incremental-views-report.md)'s 444 plus [`25`](25-benchmarks-and-expressiveness.md)'s
SICP suite.

## 26.1 What was asked, and what is answered

[`05`](05-tier-lowering.md) §5.3:

> a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one* shared
> dataflow whose final per-session operators (filter, project, diff) run per subscriber
>
> … per-session memory … as an exported metric

| asked for | status | where |
|---|---|---|
| The operators that do not read the session held **once** for every subscriber | done | `beck-core/src/engine.rs`, `SharedDataflow` |
| Advanced once per event rather than once per connection | done, and **counted**: 64 subscribers, 11 versions, 11 advances | §26.3 |
| A subscriber that renders at a different time from the others | done — a bounded history of what moved, per version | §26.2 |
| A subscriber that fell so far behind the history cannot serve it | done — it rebuilds, which the engine's `rebuilt` contagion already knew how to do | §26.2 |
| Recompute still the oracle, over the shared path too | done — every corpus program, every event, every lag from 1 to past the history's end | §26.4 |
| Per-session memory as an exported metric | done, in **entries** rather than bytes, and split into the half paid once and the half paid per connection | §26.7 |
| The runtime actually uses it | done — on by default, `AppConfig::share_arrangements` switches it off | §26.5 |
| §24.6's `O(n)` page assembly | **still there** — but on a program whose page is above the cut it is now paid once for the whole fanout, which is not the same as removing it | §26.5 |
| SQL read models, pgwire, query fusion | **not done** | §26.9 |

## 26.2 The three choices, and why each went the way it did

The analysis was never the hard part. `Plan::per_session` has been correct since
[`24`](24-incremental-views-report.md): a node is per-session exactly when it transitively reads the
session node, through an input or through a per-element function's capture. What was missing was
somewhere for the other nodes to live that is not one subscriber's engine, and three questions had to
be answered before there could be one.

### Who advances it

Not the sequencer. Putting view maintenance under the write lock would move it onto the *write* path
— every command paying for the views of every connected session before its ack — and it would do that
work for states nobody is looking at. `beck run` with no browser attached would maintain a dataflow
for an audience of zero.

**The first subscriber to render at a new version advances it**, under a write lock, and every
subscriber that renders at that version afterwards finds it done. That is lazy rather than eager, and
it gets three things at once: the work happens once per version, it is paid by a renderer that was
about to do it anyway, and a process with no subscribers does no view work at all.

The double-check is not decoration. `advance` tests the version under a read lock, takes the write
lock, and tests again — because between those two lines another subscriber may have done exactly
this, and advancing twice would feed the second advance a delta of nothing while the history recorded
a step.

The one thing that had to be separated from the version number is *whether it has ever run*. A freshly
recovered application is at version 0 with a real accumulator behind it — an empty log is a state, not
the absence of one — so `started` is a field and `version >= version` is not the test.

### What a subscriber holds while it renders

A **read lock, for the whole of its own render.** Readers do not block readers, so a thousand
subscribers render concurrently; the only writer is the advance, which is `O(δ)`.

The alternative considered and rejected was publishing an immutable snapshot per version, which
subscribers would hold by `Arc` with no lock at all. It does not survive contact with the arithmetic:
a snapshot is immutable, so an arrangement that moved has to be *copied* to build the next one, and
that is `O(n)` per changed operator per event — the cost this engine exists to remove, reintroduced at
the point where it would be hardest to see. Copy-on-write with `Arc::make_mut` avoids the copy only
while nobody else holds the previous frame, and under a fanout somebody always does.

One consequence is worth naming because it is the only place the lock is visible in the API: the
arrangement's materialised list is a `OnceLock` rather than an `Option`. A pointwise consumer needs
its input as a `list`, that list is built on demand, and with an `Option` the cache would need the
*write* lock — serialising every subscriber behind the first. With a `OnceLock` the first subscriber
to need it builds it and the rest get the same `Arc`, through a shared reference. That turned out to
matter more than it looked (§26.5).

### What happens to a subscriber that fell behind

A subscriber is woken by a `watch`, which coalesces by design — that is the backpressure behaviour a
slow connection wants, and it means three events can land between one render and the next. So the
shared side has to answer a question a per-subscriber engine never had to: not *what changed*, but
**what changed since you last looked**.

Handing a lagging subscriber only the latest version's changes is wrong in a way that does not show up
in a page and never repairs itself. An entry inserted at version 5 and removed at version 6 is never
mentioned again; a subscriber that rendered at 4 and next renders at 7 would apply version 7's changes
to an arrangement that still holds it, and serve a row the accumulator has forgotten — for the rest of
that connection.

So the shared side keeps a **bounded history of steps**: for each of the last 64 versions, which
operators changed, which rebuilt, and what moved at each. A subscriber's window is every step since
its own version, and the changes it is handed are those steps concatenated. Beyond the history it
**rebuilds**, which is correct at any lag and needed no new machinery — a rebuild re-reads the current
arrangement whole, and `Cell::rebuilt` has been contagious downstream since
[`24`](24-incremental-views-report.md).

Two details decide whether that history is affordable:

* a **rebuilt** operator's changes are deliberately not kept. A consumer below a rebuild re-reads the
  arrangement rather than applying changes, so storing them would retain a copy of the whole
  collection per remembered version, for nothing. What is kept per step is a delta.
* the changes are **concatenated, not coalesced**. A consumer applies changes in order, so a key that
  moved twice is applied twice and lands where the second one put it. Coalescing would save one
  application per repeated key and cost a pass over the window; the window is a handful of deltas.

## 26.3 One dataflow, as a number

§5.3's claim is that a thousand connected users compile to *one* shared dataflow, and "one" is a
number, so it is a counter rather than a description. `SharedDataflow::advances` counts advances, not
renders: it stays flat as subscribers are added and moves only when the fold does.

```
64 subscribers × 11 versions → 11 advances
```

`the_shared_prefix_is_advanced_once_however_many_subscribers_render` asserts equality, not an
inequality, so an advance triggered per subscriber would fail it at the first extra one.

The memory half is asserted in the same shape:
`what_a_subscriber_holds_is_only_what_reads_the_session` checks, for every corpus program, both that a
subscriber's `arranged_shared()` is **zero** — it holds no arrangement that does not read the session
— and that the shared and per-subscriber halves *add up* to what one standalone engine held. The
second assertion is the one that would catch entries quietly going missing rather than moving.

## 26.4 The oracle, extended rather than trusted

Sharing an arrangement between subscribers is exactly the sort of optimisation that is right for one
subscriber and wrong for two, so the shared path is checked against recompute everywhere the
standalone path already was, and then in the places that only exist because of sharing.

`incremental_engine.rs` now runs **both** engines for every subscriber at every event: 745 events
folded, 3,080 pages compared, byte for byte against the recompute. `shared_arrangements.rs` is
everything that is only true of sharing:

| what could be wrong | the test |
|---|---|
| A subscriber that skipped versions keeps a row the accumulator dropped | every lag from 1 to 9 and one of 200 — past the history's end, so the rebuild path — over every corpus program: **4,566 pages** |
| A subscriber joining a warm dataflow with cold operators of its own | a late subscriber against one that was there from the start |
| An empty window is mistaken for "nothing to render" | three renders at one version, at every version |
| A subscriber asking for a version the shared side has passed | asserted to be served the newer one, and the version comes back to the caller (§26.5) |
| One subscriber's rows reaching another's page | three sessions, interleaved in a different order at every version, on programs whose filter is the session |

The last of those is the failure a shared arrangement makes newly possible, and the sketch is the
program that would show it: `mine` filters by `session.actor` immediately below the accumulator, so an
arrangement being mutated per subscriber would put one actor's todos on another's page.

`differential.rs` — the harness that compares the whole split runtime against the program read
literally in one process — now drives the shared path, and the edit was two lines: ask the
*application* for an engine rather than the runtime, because `App::view_engine` returns whichever kind
the configuration asks for. So the strongest existing check on the runtime as a whole came across for
free.

## 26.5 What it costs, and on which program

Two programs, because the answer is a property of the program and quoting one number would be quoting
the more flattering one. Both carry 200 rows. `fanout_footprint` walks the accumulator, the shared
side and every subscriber with **one** exclusion set — summing per-subscriber footprints would charge
every subscriber for the page subtrees they now hold by `Arc` between them, which is precisely the
saving under measurement.

**A cold fanout — every subscriber's first render:**

```
examples/todo.beck, 200 rows: 28 of 43 operators do not read the session
 subscribers   unshared KB    shared KB        ×   unshared µs    shared µs
           1           191          191     1.0×          1051          944
           8          1533         1135     1.4×          7602         6738
          64         12271         8684     1.4×         63412        49227
         256         49084        34566     1.4×        240125       205607

24-feed.beck, 200 rows: 22 of 31 operators do not read the session
 subscribers   unshared KB    shared KB        ×   unshared µs    shared µs
           1           647          647     1.0×          2158         2070
           8          5179         1689     3.1×         17488         2758
          64         41437        10026     4.1×        138250         6071
         256        165750        38608     4.3×        620170        37748
```

**One event, over a fanout that is already connected** — the number an operator actually pays:

```
examples/todo.beck, one event over 200 rows
 subscribers   unshared µs    shared µs        ×  unshared work  shared work        ×
           1           138          131     1.1×             45          45     1.0×
           8           350          228     1.5×             66          52     1.3×
          64          3629         2154     1.7×            528         402     1.3×
         256         16016         9357     1.7×           2112        1602     1.3×

24-feed.beck, one event over 200 rows
 subscribers   unshared µs    shared µs        ×  unshared work  shared work        ×
           1           323          525     0.6×            423         423     1.0×
           8          3432          1059    3.2×           3384         465     7.3×
          64         25847          6230    4.1×          27072         801    33.8×
         256        109463         24538    4.5×         108288        1953    55.4×
```

`work` is `Engine::work().total()` — per-element applications, arrangement entries moved, entries
copied into a `list`, pointwise operators re-evaluated. A count rather than a duration, so it is the
same on any machine (§13.7); the durations beside it are printed and never thresholded.

### Why the two programs answer so differently, and what that means for a developer

The sketch's cut is immediately below the accumulator. `mine` is
`sort_by(filter_list(map_values(s.todos), λ owner == session.actor), λ text)`, and the filter reads
the session, so **everything from the filter upwards is per-session**. What is shared is the
`map_values` arrangement and the constants: real, and 28 of 43 operators, but the operators that do
the work per event are all below the cut. 1.3× less work is what sharing the source of the deltas is
worth when nothing else is shareable.

`24-feed.beck` puts the cut at the top. `visible(s)` sorts a public feed and reads no session, and the
`ui:` loop over it captures nothing session-dependent either — so the sorted list, the `li` for every
post, the `ul` that assembles them, and the page's whole `O(n)` half are all *above* the cut. Only the
greeting and the `main` that wraps everything are per-session. 256 subscribers over that program do
55× less work per event than they did.

This is the finding a developer should take from this report, and it is not "sharing is worth 4×". It
is that **where you read the session decides what your fanout costs**, the boundary is a property of
operators rather than of signals ([`24`](24-incremental-views-report.md) §24.9 said so and this
measures it), and `beck explain incremental` prints which side every operator is on:

```
  in the plan:        28 of 43 operators do not read the session, and the
                      runtime holds those once for every subscriber — one shared
                      dataflow, advanced per event rather than per connection
                      (docs/25). The other 15 run per subscriber.
```

A program that filters by the session as early as the sketch does can often filter later instead, and
the report is what makes that a decision rather than an accident.

### §24.6's `O(n)`, paid once instead of removed

[`24`](24-incremental-views-report.md) §24.6 named the remaining linear cost precisely: handing a
pointwise operator a `Value::List` copies `n` handles, and `html_el` builds an `n`-child element per
event even when one child changed. **That is still true and this work does not remove it.**

What it does is decide *how many times* it is paid. On `24-feed.beck` the assembly is above the cut,
so:

```
  50 posts: the shared side materialised 100 entries once; 8 subscribers materialised 24 between them
 400 posts: the shared side materialised 800 entries once; 8 subscribers materialised 24 between them
```

Eight subscribers pay a constant 3 entries each at 50 posts and the same 3 at 400. The `O(n)` did not
become `O(δ)`; it stopped being multiplied by the fanout. On the sketch, where the assembly is
per-session, it is multiplied by the fanout exactly as before. Removing it — patches emitted from the
plan's own output deltas — is still the next factor, still a protocol change, and still §24.6's
sentence rather than this one's.

### The version a page reflects, which was a latent bug

`App::maintain` now returns the page **and the version it reflects**, and `session::drive` labels the
patch frame with that instead of with `app.head()` sampled afterwards.

This was wrong before and the shared dataflow is what made it visible. The state was read under a lock
and the head was not, so a frame could be labelled with a version newer than the page it carried
whenever an event landed in between. A frame's `seq` is what a resuming client asks for the difference
from (§4.3): the server would diff `view(seq)` against the current view and send patches the client
applies to a page that was never `view(seq)`. A wrong DOM, one reconnect later, from a race with no
symptom at the time.

Both halves are fixed together: the version is read under the same read lock as the state — the
sequencer publishes both under its write lock — and `SharedDataflow::render` returns the version it
actually served, which may be *newer* than the one asked for when another subscriber advanced the
shared side in between. Serving the newer page is the deliberate choice: unwinding an arrangement to
an older version would need a history of values rather than of changes, and the subscriber is about to
be woken for the newer version anyway. What makes it safe is that the number comes back to the caller.

## 26.6 The defect this found in the engine `24` shipped

`map_list`, `filter_list` and `sort_by` have an early return for the case where nothing arrived and
nothing forced a rebuild. It cleared `changed` and cleared `changes` and **did not clear `rebuilt`**.

`rebuilt` means "this operator threw its arrangement away and rebuilt it *this tick*", and a rebuild is
deliberately contagious downstream — that is the fix
[`24`](24-incremental-views-report.md) made for a subscriber that saw another subscriber's rows. With
the flag left set, it silently came to mean "has ever rebuilt", which is true of every operator after
its cold start. So **every operator below a collection that had stopped changing rebuilt on every
event, for the life of the subscription.** `concat` and `flatten` always cleared it; these three never
did.

It is a performance defect and not a correctness one — a rebuild produces the right answer, which is
why nothing caught it. The cost, on the sketch with eight subscribers over 200 todos, one event:

```
              before   after
applications    1137      66     per event, across the fanout
```

Seven of the eight subscribers were re-applying their predicate and their sort key to all 25 of their
todos, on every event, because a `sort_by` that had nothing to do three events ago still said it had
rebuilt.

Two things about how it was found are worth recording, because neither was a review of the code.
[`24`](24-incremental-views-report.md) §24.5's table is a single subscriber, where the sort *does*
change on the event being measured, so the flag is never read in the stale state — the measurement
that would have shown this could not have. It appeared only when the fanout was measured, and only
because the per-subscriber numbers were printed rather than summed: subscriber 0 did 4 applications
and subscribers 1–7 did 51 each, and there is no correct engine in which the subscriber whose page
*did* change is the cheap one.

`shared_arrangements.rs`'s lag tests are what now hold the flag to account from the other side: an
operator that reported a rebuild it did not perform would send its consumers down the whole-collection
path, and one that failed to report a rebuild it did perform would leave them holding withdrawn
entries — the first is slow, the second is a wrong page, and 4,566 compared pages is where the second
one would surface.

## 26.7 Per-session memory, exported

[`24`](24-incremental-views-report.md) §24.10: "A stub for the arrangement's memory is not a plan.
§5.3 asks for per-session memory as an exported metric and `Engine::footprint` computes it; nothing
exports it. It is not on the dashboard and there is no gate on it."

Two gauges now are:

* `beck.views.shared_arranged` — arrangement entries the one shared dataflow holds;
* `beck.views.session_arranged` — arrangement entries the connected subscriptions hold **between
  them**.

Both are on the dashboard and in the OTLP export. Putting them side by side is the whole operational
question, because the first is paid once and the second is multiplied by the fanout: a program whose
second number dwarfs its first has its cut in the wrong place, and that is now visible on a running
process rather than in this document.

Three decisions in it:

* **Entries, not bytes.** A byte figure needs `Engine::footprint`, which walks the accumulator so
  that structure shared with the fold is charged to the fold — right for a report, far too expensive
  to sample on every render. Entries are `O(operators)` to read and they are the number that scales.
* **Maintained as a difference.** `Gauge::adjust(was, now)` replaces one contributor's share, so the
  gauge follows a subscription that grew or shrank without re-summing every connection.
* **Released by a guard.** A subscription ends by returning, by erroring, or by its socket dying, and
  a gauge that only releases its share on the happy path drifts upward until it is describing
  connections that closed hours ago. `Arranged` is a `Drop` impl for the same reason `SessionGuard`
  is, and `what_the_views_cost_is_exported_while_the_process_is_running` asserts the per-session gauge
  returns to zero when the subscriptions end — and that the shared one does not move, because those
  entries were never per session.

That test has a **test binary to itself**, and that is the point rather than an accident: `telemetry`
is one value per process, so a test asserting a gauge returned to zero cannot share a binary with a
test that has a subscription open.

There is still **no gate** on either number. A subscription over a thousand rows was 5.4 MB in
[`24`](24-incremental-views-report.md) §24.7 and no gate covered that case; it is now observable and
still ungated.

## 26.8 The corrections this makes to the design documents

| Document | Correction |
|---|---|
| [`24`](24-incremental-views-report.md) §24.7 | "identified per operator, held once per subscriber, and the runtime design for sharing it is not written" — no longer true. The design is §26.2 and the counter is §26.3 |
| [`24`](24-incremental-views-report.md) §24.7's table | The `of it shared` column measured what a standalone engine holds redundantly. A subscriber attached to a shared dataflow holds **none** of it; the fanout tables in §26.5 replace it, and the per-subscriber table in `measure_incremental.rs` now says which engine it is describing |
| [`24`](24-incremental-views-report.md) §24.5's table | It is a **single subscriber**, and the sort it measures moves on the event it measures — so the stale-`rebuilt` defect of §26.6 could not appear in it. The numbers in it are unchanged and were never wrong; what they could not show is what the same program costs a fanout |
| [`24`](24-incremental-views-report.md) §24.10 | "§5.3 asks for per-session memory as an exported metric … nothing exports it" — two gauges do (§26.7). No gate, still |
| [`05`](05-tier-lowering.md) §5.3 | The shared dataflow is advanced **lazily by the first subscriber to render at a new version**, not "under a lock the sequencer already holds". Putting it on the write path would charge every command for the views of every session, and would maintain a dataflow nobody is watching |
| [`04`](04-compiler-architecture.md) §4.3 | A patch frame's `seq` is the version the *page* reflects, which is not always the log head at the moment the frame is written. The runtime read the head separately from the state and could label a frame with a version its page did not show (§26.5) |
| [`08`](08-roadmap.md) | Phase 3's incremental-views bullet: arrangement sharing is built; SQL read models, pgwire and query fusion are not |

## 26.9 What Phase 3 is still not

**Two bullets of twelve are built, and a third is now most of the way.** The phase's exit criterion —
"an outside developer builds a non-trivial app from documentation alone" — is not met.

- **The incremental-views bullet is still not finished.** Built: the dataflow plans, recompute as the
  CI oracle, and now arrangement sharing. Not built: **SQL read models and pgwire exposure**
  (nothing); **query fusion on symbolic plans** (nothing — and `beck explain query` is still unbuilt
  for [`20`](20-phase-2-report.md) §20.5's reason, that the `Query` sub-language is deliberately
  symbolic and there is no plan to explain until the engine compiles one). `beck explain cost` is
  still unbuilt too.
- **The page is still assembled and diffed, not streamed as deltas.** [`24`](24-incremental-views-report.md)
  §24.6, unchanged. On a program whose page is above the session cut it is now paid once for the whole
  fanout instead of once per subscriber (§26.5), which is a different fact from removing it, and on a
  program like the sketch it is paid per subscriber exactly as before.
- **The shared dataflow is never released.** It holds its arrangements whether or not anybody is
  subscribed. A process that had a fanout and now has none keeps the accumulator's arrangements warm
  for a reconnection that may not come. Nothing measures how much that is and nothing drops it.
- **The history is a constant, not a policy.** 64 versions, chosen because a subscriber further behind
  than that is not the bottleneck, and not because anything measured where the knee is. A deployment
  with slow clients and fast events would want it configurable; nothing exposes it.
- **The lock is a lock.** Subscribers render under a read lock, which is right for readers and means
  the advance waits for the slowest render in flight. That is fine at the fanouts measured here and it
  is not a design that has been run at a fanout where it would not be.
- **No LLVM backend and no native codegen**, unchanged from Phases 1, 2, 3-part-1, 3-part-2 and
  3-part-3.
- **No Mode B, no client polish, no `test --update`, no structured concurrency, no `Result`/error
  rows, no SQLite substrate, no standard library v1, no identity beyond a dev-mode actor, no LSP, no
  playground, no supply-chain tooling.** Nine Phase 3 bullets, all untouched.
- **`check.rs` is 2,806 lines, unchanged**, for the second report running. §22.6's request to move the
  test-checking pass out of it is still not done. This work added
  about 550 net lines to `engine.rs` and about 180 across the runtime, against roughly 1,900 for
  [`24`](24-incremental-views-report.md) — the expensive part had already been paid by whoever made
  `per_session` a field on a plan node.

## 26.10 What this changes for the rest of Phase 3

1. **The cut is a thing a developer can move, and now has a reason to.** §26.5's two programs differ by
   55× in per-event fanout cost and the difference is where they read the session. That is a
   *language-level* performance property, visible in `beck explain incremental`, decided by ordinary
   program structure rather than by a directive — which is the shape §3.8 always claimed for
   incrementality and the first time it has a number attached.
2. **Mode B's cut already exists, and it is the same one.** [`24`](24-incremental-views-report.md)
   §24.11 item 2 said a component is a subtree of the plan and `per_session` is the cut. It is now
   also a *runtime* boundary with a lock, a version and a change history on it — which is most of what
   a client kernel needs from the server side, because a client that applies deltas is a subscriber
   that fell behind and catches up.
3. **A performance bug hid behind a correctness oracle for a whole report.** §26.6 was invisible to
   every test in the suite, because a rebuild is right. The lesson is not "test performance" but the
   narrower one: the single-subscriber measurement could not have shown it, and the fanout measurement
   showed it immediately — as soon as the per-subscriber numbers were printed rather than summed. An
   aggregate is where a defect of this shape goes to hide.
4. **Two of the three metrics §5.3 names are now exported and none is gated.** Making a number visible
   is the cheap half; deciding what value of it should fail a build is the half this project keeps
   deferring, and it has now been deferred by four reports.
