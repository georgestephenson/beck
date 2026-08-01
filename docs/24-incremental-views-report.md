# 24 — Phase 3 report, part 3: the incremental view engine

[`23`](23-general-slicer-report.md) §23.10 item 1 said the general slicer left the view engine "a
plan to compile", and §23.9 said what was true at the time: "**Every view is a full recompute per
event**, exactly as it was."

That sentence is now false, and this report is what replaced it. A view is compiled into a dataflow
of operators and maintained from the change, `remaining` updates by ±1 per event over a collection
of any size, and the recompute that used to be the only implementation is now the oracle CI checks
the engine against — [`05`](05-tier-lowering.md) §5.3's "luxurious position", occupied.

**This is the engine, not the bullet.** Phase 3's incremental-views item asks for four things —
dataflow plans *with arrangement sharing*, recompute as the CI oracle, SQL read models with pgwire
exposure, and query fusion on symbolic plans. The plans and the oracle are built. Arrangement sharing
between subscribers, the read models and the fusion are not, and §24.10 names them alongside the nine
other bullets of twelve that remain untouched.

Two numbers decide how much of this is worth having, and both are in the report rather than in a
footnote. Per event, the maintained view does **the same ten units of work over five thousand rows as
over ten** (§24.5) — which is §3.8's sentence, mechanised — but assembling the page's children is
still linear, so the end-to-end win is a **3–5× constant factor and not a change of asymptote**
(§24.6). Per subscriber, it costs **four times** the memory a subscription already held (§24.7).

## 24.1 What was asked, and what is answered

[`03`](03-type-and-effect-system.md) §3.8:

> **Subscribed views** … compile to **incremental dataflow plans** … `remaining` updates by ±1 per
> event, never by recount. … Arbitrary pure code is incrementalized where analysis allows,
> recomputed where not — **`beck explain incremental <view>` shows which, and why**.

[`05`](05-tier-lowering.md) §5.3:

> a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one* shared
> dataflow whose final per-session operators (filter, project, diff) run per subscriber
>
> … an optimisation with an exact correctness oracle (recompute) to test against — a luxurious
> position for CI.

| asked for | status | where |
|---|---|---|
| A view compiled to a dataflow plan rather than to one expression | done | `beck-core/src/plan.rs` |
| Maintained by delta: `map_values`, `map_list`, `filter_list`, `sort_by`, `concat_lists`, flatten, `list_len`, `list_is_empty` | done | `beck-core/src/engine.rs` |
| "`remaining` updates by ±1 per event, never by recount" | done, and **measured**: 10 units of delta work per event at 10 rows and at 5,000 | §24.5 |
| "incrementalized where analysis allows, **recomputed where not**" | done — an operator the decomposition cannot enter is a full recompute of that operator, and only when an input moved | §24.3 |
| `beck explain incremental` shows which, and why | done — and its first line still has to be true of *the program*, not of the feature | §24.8 |
| Recompute as the correctness oracle in CI | done — every corpus program, every event of a generated log, engine against recompute, byte for byte | §24.4 |
| The runtime actually uses it | done — one engine per subscription, driven end to end over a socket in `subscription.rs`, and switchable with `AppConfig::maintain_views` | §24.7 |
| §5.3's shared dataflow, shared *at runtime* between subscribers | **not done** — identified per operator, held once per subscriber | §24.7 |
| SQL read models, pgwire, query fusion | **not done** | §24.10 |
| The page's children maintained as a patch stream rather than reassembled | **not done**, and it is where the remaining `O(n)` is | §24.6 |

444 tests, no failures, no compiler warnings, no clippy warnings — up from
[`23`](23-general-slicer-report.md)'s 429.

## 24.2 The one hard problem: a fold produces a value, a plan consumes changes

Every incremental dataflow system starts from a stream of changes. A Beck program does not have one.
`todos = durable(fold(apply_event, empty, events))` produces a **whole new accumulator** per event,
and §3.7 is emphatic that this is the semantics rather than an implementation: the accumulator is a
value, replay rebuilds it, snapshots hold it, the differential harness diffs against it.

So something has to turn a new value into a delta, and the obvious way to do it is the trap. Walking
the old and new maps entry by entry is `O(n)` per event — which is the recount §3.8 exists to
abolish, moved one level down where nothing in the report would mention it. An engine built on that
would print "incremental" over a linear scan.

The way out was already in the codebase and had been since Phase 1's second report.
[`19`](19-phase-1-report.md) §19.4 item 3 made `Map[K, V]` a **persistent weight-balanced tree**
([`pmap.rs`](../compiler/crates/beck-core/src/pmap.rs)) so the fold would stop copying, and the
property it was built for has a second use nobody had cashed in: an insert rebuilds only the path to
the key and **shares every subtree that path did not pass through, by pointer**. Two versions of a
1,024-entry map differ by about 40 fresh nodes and share the other 900-odd — that is the assertion
`an_insert_shares_all_but_the_path_it_rebuilt` has been making since Phase 1.

`PMap::diff` is the conversion, and it is an ordered merge of the two trees with one extra rule:
**when the heads of the two remaining sequences are the same subtree by pointer, drop both.** The
soundness is worth writing out because the whole engine rests on it — pointer-identical subtrees hold
identical entries, so the two remaining *sorted sequences* share that prefix exactly, and a merge
over sorted sequences reports nothing for a shared prefix. It holds however rebalancing moved that
subtree in either tree.

The result, asserted rather than asserted-about:

```
diffing an 8,192-entry map after one insert looked at 25 entries
```

`diffing_two_versions_that_share_structure_visits_a_handful_of_entries` fails at 64 and an `O(n)`
implementation would reach 8,192. There is also a `BTreeMap` oracle over 64 random histories, because
a diff that is fast and wrong is worse than one that is slow.

## 24.3 What a plan is, and why the fallback is the interesting part

[`plan.rs`](../compiler/crates/beck-core/src/plan.rs) compiles the view into a DAG of operators, by
symbolically evaluating the sliced expression: definitions are inlined, `let` binds a variable to an
operator, and every subexpression becomes a node. There are two kinds of node and the distinction is
the design:

* a **delta operator** holds an *arrangement* — its output as a `BTreeMap` from an ordering key to a
  value — and updates it from the changes at its input;
* a **pointwise operator** holds a value and re-evaluates when an input changed.

Everything the decomposition cannot enter becomes one pointwise operator over the plan nodes it
reads: a `match`, an `if`, a call through a value, a primitive with no delta rule. That fallback is
not a limitation to be apologised for — it is the reason the engine is **correct for programs it
cannot accelerate**, which is what made it safe to switch on for every program in the corpus at once
rather than for a recognised shape. A program the analysis does not understand is slow. It is never
wrong.

And the fallback still buys something. A pointwise operator re-runs *only when an input moved*, which
a single inlined view expression cannot know. Deciding that cannot itself be a deep comparison — that
would put the `O(n)` back — so `engine::same` is conservative: scalars compare by value, records field
by field, and collections and rendered trees by **pointer**. It answers "unchanged" only when it is
certain, and "changed" costs a recompute the old runtime performed unconditionally.

### Order is a key, not a sort

An incremental view that produced the right entries in a different order would be a correctness bug
rather than a cosmetic one: iteration order reaches the rendered page, the page is diffed into a
patch stream, and §4.8's replay harness compares that stream bit for bit. So order is not restored at
the end; it falls out of the arrangement's key.

| operator | key | why that key |
|---|---|---|
| `map_values(m)` | the map's key | already the order `map_values` yields |
| `map_list`, `filter_list` | the input's key, unchanged | neither operation moves an element |
| `sort_by(xs, k)` | `k(x)`, then the input's key | a *stable* sort, expressed as an order rather than performed as one |
| `concat_lists([a, b])` | the part's position, then that part's key | the parts keep their places |
| flatten (`concat_lists(map_list(…))`) | the outer key, then the position inside that element | one row's children move without disturbing another's |

That last row is what every `for` loop in a `ui:` block compiles to — the macro lowers
`for t in todos:` to `concat_lists(map_list(todos, …))` — so it is the operator that decides whether
rendering a list is incremental at all.

### The sketch, decomposed

`beck explain incremental examples/todo.beck`, on the view that has been a single opaque recompute
since Phase 1:

```console
  map_values     ×1    maintained  shared
  filter_list    ×2    maintained  per session
  sort_by        ×1    maintained  per session
  map_list       ×1    maintained  per session
  flatten        ×1    maintained  per session
  list_len       ×1    maintained  per session
  recompute      ×20   recomputed  12 of 20 shared
```

`mine(s, session)` is `sort_by(filter_list(map_values(s.todos), …), …)`; `remaining(todos)` is
`list_len(filter_list(todos, …))`; `render` is the `ui:` block, whose `for` loop is the `map_list`
and the flatten. §3.8's own example is the `list_len` at the bottom of that list, and it reads
`entries.len()` — `O(1)`, and it never forces its input to be assembled into a list, which is the
whole reason `list_len` is an operator rather than a pointwise call.

## 24.4 Recompute as the oracle, which is the only reason to believe any of this

[`incremental_engine.rs`](../compiler/crates/beck-cli/tests/incremental_engine.rs) is the gate. For
the sketch and for **every one of the 24 corpus programs**, a log is generated from that program's own
`Event` union (seeded by the file's name, so a failure is reproducible), folded one event at a time,
and after every event the maintained page is compared with the recomputed one — not by a digest, by
the rendered HTML, for two different subscribers. **745 events, 1,540 pages compared.**

Three more properties, because equality alone would be satisfied by an engine that recomputed
everything and called it maintenance:

* **a cold engine and a warm one agree** at a state reached by forty events, so no arrangement is
  right only because of the order events arrived in;
* **an engine rendered against an older state is still right** — walked backwards through the whole
  history and forwards again — because `beck-rt`'s resumption path renders the state as of the `seq`
  a reconnecting subscriber last saw, and an engine that assumed the state only moves forward would
  serve that subscriber a page from the future;
* **the plan actually decomposed**, asserted by name. Every other test here would still pass if the
  decomposition had silently given up and made the whole view one opaque operator, because that is
  the fallback and the fallback is correct.

There is also a **subscription harness**. `beck-rt`'s `Socket` trait has said since Phase 1 that it
exists for "the upgraded socket in the server, and an in-memory duplex in the tests", and nothing had
used the second half of that sentence: no test drove `session::run`, the loop a browser actually
talks to. It does now, because that loop is where the engine is switched on — hello, welcome, the
first frame, a command, the ack, and the patch frame carrying `1 remaining` that the maintained count
produced. Its sibling runs the same subscription with `maintain_views: false`, because a switch
nothing exercises is a switch that stops working and the failure would be silent: the page would
still render, from the other path.

And the engine moved onto the **split side of the differential harness** — §4.8's "highest-value test
in the project", §8.3's "the project's conscience". That test compares the program run as one process
against the program run through the real sequencer, the log, the diff and a client that has only ever
seen patches; its subscribers now hold engines, exactly as `session::run` does. An optimisation of the
split path belongs on the split side of the test that exists to notice the split path changing
behaviour.

### The defect the oracle found

The second property is the one that caught the real bug, and it is worth recording because it is
invisible from the maintained path alone.

An operator that **rebuilds** — because its per-element function captured something that moved, which
is exactly what happens when `lambda t: t.owner == session.actor` meets a different session — throws
its arrangement away and emits inserts. It has no previous arrangement left to derive removals from.
A downstream operator that merely applied those inserts to its own arrangement therefore kept every
entry the rebuild had dropped, and a subscriber who switched sessions saw **another subscriber's
rows** on top of their own. Every event-by-event comparison passed, because within one session
nothing ever rebuilds.

The fix is that a rebuild is contagious: `Cell::rebuilt` propagates downstream, and an operator whose
input rebuilt rebuilds too. [`23`](23-general-slicer-report.md) §23.10 item 4 — "a defect found by
writing the program down is worth three found by reading the code" — extends to oracles: this one
took one test that changed the session on a warm engine.

## 24.5 §3.8's sentence, measured

> `remaining` updates by ±1 per event, never by recount.

`cargo test --release --test measure_incremental -- --nocapture`, on the sketch, adding one todo to a
state that already holds `n`:

```
   rows       delta materialise   recomp   maintain µs recompute µs  ratio
     10          10         11        9            44          133   3.0×
    100          10        101        9           194         1014   5.2×
   1000          10       1001        9          1656         9017   5.4×
   5000          10       5001        9          9128        47293   5.2×
```

**The `delta` column is the deliverable.** Ten units of work — per-element functions applied plus
arrangement entries moved — for one event, whether the collection holds ten rows or five thousand.
`a_count_over_a_maintained_collection_does_not_visit_the_collection` is the gate on it, and it is a
**count** rather than a duration, because §13.7's rule is that a shared CI runner cannot hold a timing
threshold honestly. The timings above are printed and never thresholded.

## 24.6 What is still `O(n)`, said before anything else about the ratio

The `materialise` column grows exactly with the collection, and it is the honest half of the same
table. It is the cost of handing a **pointwise** operator a `Value::List`: `html_el("ul", attrs,
children)` takes the whole child list and builds an `Html::Element` holding all of it, so `n` handles
are copied and an `n`-child element is constructed per event even though one child changed.

So the maintained view is **not** `O(δ)` end to end. It is `O(δ)` in the *elements it computes* and
`O(n)` in the *page it assembles*, and the measured 3–5× is a constant factor on an unchanged
asymptote rather than a change of asymptote. A report that quoted "5× faster" without that sentence
would be describing a different system.

Removing the remainder is a known piece of work rather than an open question, and it is the same
piece of work in both directions: the delta at the top of the plan **is** the patch set. `beck-rt`
currently renders a whole page and structurally diffs it against the previous one
([`diff.rs`](../compiler/crates/beck-rt/src/diff.rs)) to produce the patches §5.1 streams; an engine
that emitted patches from its own output changes would skip both the assembly and the diff. That is
also precisely what Mode B's per-component kernel needs, which is why it is not done here — it is a
protocol change and a client change, not an engine change.

`RULES` in [`incremental.rs`](../compiler/crates/beck-core/src/incremental.rs) has said since
[`23`](23-general-slicer-report.md) §23.8 that `html_el` is maintained by "a subtree delta — what the
patch protocol already streams". **That table is a statement about what a plan could do, and the
engine does not do this one.** The two disagreeing in silence would be the sort of thing this project
exists not to do, so `beck explain incremental` now prints both: the analysis's verdict per view, and
the plan's operator table, in which `html_el` appears under `recompute`.

## 24.7 Per-session memory, and the trade it makes

§5.3 names per-session memory as one of three metrics to export, and [`18`](18-phase-0-report.md)
§18.3's kill gate is written in kilobytes per idle session. An engine per subscription is a
memory-for-time trade, and a trade whose cost is not measured is a claim.

```
   rows   arranged    engine KB of it shared      page KB       ×
     10         60         66.1          9.1         15.5    4.3×
    100        600        553.4         31.6        139.3    4.0×
   1000       6000       5428.7        256.6       1377.7    3.9×
   5000      30000      27108.4       1256.6       6885.5    3.9×
```

**A maintained subscription costs about four times the memory it already held for its page.** The
comparison is against the page alone because a subscription has always retained one — `drive` keeps
`last_view` so the next render can be diffed against it — so the multiplier, not the absolute, is
what a fanout estimate should be scaled by. Phase 0 measured ~5 KB per idle session against a ~50 KB
tripwire; four times that is ~20 KB, inside it. A subscription over a thousand rows is 5.4 MB, and no
gate in this project covers that case, before or after this work.

Two notes on the method, because the first attempt was wrong in a way that would have been publishable
if nobody had run it twice. It read `/proc/self/statm` around 32 live subscriptions and reported a
ratio that **swung between 2.0× and 4.9× across runs of the same binary**, because the resident set
moves with the allocator's arena rather than with the data. A counting allocator would be exact and
needs `unsafe`, which this workspace forbids. `Engine::footprint` therefore walks what is retained and
adds it up, and it does two things that decide whether the number means anything:

* it **counts shared structure once**, and
* it is given the accumulator and walks that first, so structure the engine shares with the fold is
  charged to the fold. An arrangement over `map_values(s.todos)` holds the *same* `Todo` records the
  fold holds, by `Arc`; charging a subscription for them would report a row per row where the truth
  is a handle per row.

It excludes per-allocation overhead, so it is a floor rather than a ceiling.

### The shared arrangement, which is identified and not shared

The `of it shared` column is §5.3's sentence, unpaid. 28 of the sketch's 43 plan nodes do not read the
session, so a thousand subscribers should hold them once between them; the engine holds them once
each. In the sketch that is only 9.1 KB of 66.1 — because the sketch's view really is per-session
almost all the way down, `mine` filtering by `session.actor` immediately below the accumulator — and
`24-feed.beck` was added to the corpus as the opposite case, where the sorted list is shared and only
the greeting is not.

What blocks it is not the analysis, which is done, but the runtime: subscribers render at different
times, a shared arrangement would have to be advanced under a lock the sequencer already holds, and a
subscriber that fell behind would need either a version history or a rebuild. That is a real design
with real choices in it and it is the next thing this work should get, not a line of plumbing.

Because the trade is real, it is a **switch**: `AppConfig::maintain_views`, on by default. An operator
running a fanout of a hundred thousand idle sessions over a large accumulator can decide differently
without recompiling, and `App::render` — the recompute — is still the path that serves the first
document and reconstructs a resuming subscriber's old view.

### One prepared plan, not one per subscriber

The first version prepared every operator's code per `Engine`, which cost about 90 KB per subscription
before a single row existed: `Backend::function` is where a compiling backend does its expensive work,
and even the tree-walker clones the expression into the closure. `Prepared` is now separate from
`Engine` — code once per program, arrangements once per subscriber — and
`the_runtime_drives_a_backend_it_has_never_heard_of` asserts it by counting: a hundred subscriptions
prepare nothing.

## 24.8 `beck explain incremental`, and the line it is not allowed to get wrong

[`23`](23-general-slicer-report.md) §23.8 made the report's first line a gate, on the grounds that "a
command called `explain incremental` that let a reader believe their view was being maintained would
be the most misleading output in the compiler". The line said *every view below is a full recompute
per event*. It has changed, and the obligation has not:

```console
$ beck explain incremental examples/todo.beck
Views are **maintained by delta** as far as the plan can decompose them: 7 of
this view's 27 operators update from the change itself, 20 are recomputed when
an input moves, and the page's children are still assembled in full every time
(docs/24 §24.6).
```

```console
$ beck explain incremental corpus/22-shared.beck
**Nothing in this view is maintained by delta.** The plan found no collection for a
delta to flow through, so all 20 of its operators are recomputed — each one
only when an input actually moved, which is what a plan buys even here.
```

The second one is the assertion that matters, and it is a fact about that program rather than a
caveat: `22-shared.beck`'s view computes a `map_len` and concatenates strings. There is no collection,
so there is nothing to maintain, and a report written about the feature would have told its reader
otherwise. Both first lines are CI gates, as the old one was.

The report now has two halves because there are two questions. The **analysis** — which this module
has always done — asks whether a *view* is built only from operations with delta rules; the **plan**
says what the engine actually compiled the view into. They can disagree in both directions: a view
with a `match` in it is `recompute` by the analysis while its collections are still maintained around
the `match`, and the page is `incremental` by the analysis while `html_el` is pointwise in the plan
(§24.6). Printing one of them would have been printing the wrong one for half the programs.

## 24.9 The corrections this makes to the design documents

| Document | Correction |
|---|---|
| [`23`](23-general-slicer-report.md) §23.9 | "Every view is a full recompute per event, exactly as it was" — no longer true. §23.8's report and its first line changed with it |
| [`05`](05-tier-lowering.md) §5.3 | The shared dataflow is a property of **operators, not of signals**. A computation inside a `per_session` view can be session-independent and therefore shareable — `24-feed.beck` is that program, and `beck explain incremental` puts each operator on one side of the cut. §5.3's own example reads as though the boundary were a vertex of the signal graph; it is a boundary in the plan, and the two do not coincide |
| [`03`](03-type-and-effect-system.md) §3.8 | "`remaining` updates by ±1 per event, never by recount" is built and measured. The *view* is not thereby `O(δ)`: assembling the page's children is still linear, and §3.8's promise is about the count rather than about the render (§24.6) |
| [`03`](03-type-and-effect-system.md) §3.8 | "Arbitrary pure code is incrementalized where analysis allows, recomputed where not" — the unit of "where" is an **operator**, not a view. A view is not incremental-or-not; parts of it are |
| [`23`](23-general-slicer-report.md) §23.8's `RULES` | The table's `html_el`/`html_text`/`html_attr` rows are what a plan *could* do and are not implemented; the engine treats them as pointwise. The table is still worth having — it is where the next piece of work is written down — but the report no longer lets it stand alone |
| [`08`](08-roadmap.md) | Phase 3's incremental-views bullet: the engine and the CI oracle are built, arrangement sharing at runtime, SQL read models, pgwire and query fusion are not, and the bullet is marked accordingly rather than as done |

## 24.10 What Phase 3 is still not

**Two bullets of twelve are built, and a third has its engine.** The phase's exit criterion — "an
outside developer builds a non-trivial app from documentation alone" — is not met.

- **The incremental-views bullet is not finished.** Its engine is. What the roadmap also asks for and
  this does not deliver: **arrangement sharing between subscribers** (§24.7 — identified per operator,
  held once per subscriber, and the runtime design for sharing it is not written); **SQL read models
  and pgwire exposure** (nothing); **query fusion on symbolic plans** (nothing — and `beck explain
  query` is still unbuilt for [`20`](20-phase-2-report.md) §20.5's reason, that the `Query`
  sub-language is deliberately symbolic and there is no plan to explain until the engine compiles
  one). `beck explain cost` is still unbuilt too.
- **The page is still assembled and diffed, not streamed as deltas.** §24.6. This is the remaining
  `O(n)` and it is where the next factor is.
- **A stub for the arrangement's memory is not a plan.** §5.3 asks for per-session memory as an
  exported metric and `Engine::footprint` computes it; nothing exports it. It is not on the dashboard
  and there is no gate on it.
- **No LLVM backend and no native codegen**, unchanged from Phases 1, 2, 3-part-1 and 3-part-2. The
  engine is one more thing a second backend would have to be differentially tested through, and
  `Prepared` is the seam it would arrive at.
- **No Mode B, no client polish, no `test --update`, no structured concurrency, no `Result`/error
  rows, no SQLite substrate, no standard library v1, no identity beyond a dev-mode actor, no LSP, no
  playground, no supply-chain tooling.** Nine Phase 3 bullets, all untouched.
- **The engine is only as general as the decomposition.** 60 operators are maintained across the
  corpus's 24 programs and 403 are recomputed. Some of that is correct — most corpus views compute a
  scalar from a map and have no collection to maintain — and some of it is the fallback firing on a
  `match` or an `if` that differential dataflow does handle by branching the plan. `beck explain
  incremental` names which, per program, which is the only reason the ratio is a fact rather than an
  impression.
- **Interception, closures and the plan.** [`22`](22-phase-3-report.md) §22.6 recorded that a function
  stored in a record and called through a field cannot be stubbed. The same limit is now also a
  performance limit: `plan.rs` cannot see through such a call either, so it becomes one opaque
  operator. The fix is still naming closures at their binding site.
- **`check.rs` is 2,806 lines, unchanged.** This work did not open it, which is the first time in
  three phases that number has not grown — but §22.6's request was to *move* the test-checking pass
  out, and that is still not done. The cost is elsewhere and it is not small: `plan.rs` (931 lines)
  and `engine.rs` (1,108) are new, `pmap.rs` went from 589 to 928, and `incremental.rs` from 509 to
  620. Roughly 1,900 new lines of compiler and runtime for a 5× constant factor and an oracle — the
  oracle being the part that will still be worth it when the constant factor is superseded.

## 24.11 What this changes for the rest of Phase 3

1. **Recompute is now a *load-bearing* oracle rather than an available one.** Every future
   optimisation of the view path — patches from deltas, a compiling backend, Mode B's client kernel —
   is checkable by the harness that exists, over the corpus that exists, because the comparison is
   between two implementations of one function rather than between a system and a description of it.
2. **The plan is where the client work attaches.** Mode B needs a per-component kernel; a component is
   a subtree of the plan, `per_session` is already the cut, and the operators below it are the ones
   whose deltas a client would apply. That was unrepresentable in an inlined expression and it is a
   field access now.
3. **The memory question is the fanout question, and it is now numeric.** §5.3's thousand connected
   users have a per-subscriber cost of 4× the page, of which the shared fraction is a measured number
   per program. Whether arrangement sharing is worth its complexity is a question with a table behind
   it rather than an argument.
4. **A conservative equality is a language-level decision hiding in an engine.** `engine::same`
   compares collections by pointer because a deep comparison would cost what the engine saves. That
   works because `Map[K, V]` is persistent and shares structure — a property [`19`](19-phase-1-report.md)
   §19.4 chose for the fold's asymptotics — so a data-structure decision made two phases ago is what
   makes change propagation cheap two phases later. The same will be true of `list`, which is *not*
   persistent, and every list-shaped input to a pointwise operator pays for it.
