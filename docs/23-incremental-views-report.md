# 23 — Incremental views

**Built, and the bullet is finished.** [`03`](03-type-and-effect-system.md) §3.8's incremental view
compilation and [`05`](05-tier-lowering.md) §5.3's shared dataflow, in six pieces: a general slicer
that treats the signal graph as a graph, a dataflow plan per view, an engine that maintains it from
the change, one dataflow shared between every subscriber, a lifecycle that gives it back, read models
a `psql` can query, and a fusion pass over the plan.

The numbers that matter are all properties of a *program* rather than of the feature, so they are
here rather than in a footnote:

- `remaining` updates by **±1 per event, never by recount** — ten units of delta work at ten rows and
  at five thousand (§23.8).
- Assembling the page's children is **still linear**, so the end-to-end win is a 4–37× constant
  factor rather than a change of asymptote (§23.8). It was 3–5× until the children became shared
  handles rather than owned subtrees — the copy that sentence had always described as "`n` handles"
  was in fact every node of the page, compounding with depth.
- Sharing the dataflow is worth **55× less work per event** on a public feed and **1.3×** on the todo
  sketch, at 256 subscribers. **Where a program reads the session decides what its fanout costs**
  (§23.10).
- A process that had 64 subscribers and now has none gives back **99.1%** of what the shared side
  held (§23.11).
- Across the 31-program corpus, **43 read-model tables with no annotation of any kind** (§23.12).

The chapter's transferable findings are §23.16, and none of them is about dataflow. A refusal that is
never exercised on the shape it exists to refuse is a claim rather than a check — and one such claim
was quoted approvingly by two reports while the compiler it described was silently mis-slicing a
forty-line program. A performance defect hid behind a correctness oracle for a whole report, because
a rebuild produces the right answer. And an aggregate is where a defect of that shape goes to hide:
the measurement that found it did so the moment per-subscriber numbers were printed rather than
summed.

---

## 23.1 What was asked for, and what is there

[`03`](03-type-and-effect-system.md) §3.8:

> **Subscribed views** … compile to **incremental dataflow plans** … `remaining` updates by ±1 per
> event, never by recount. … Arbitrary pure code is incrementalized where analysis allows,
> recomputed where not — **`beck explain incremental <view>` shows which, and why**.

[`05`](05-tier-lowering.md) §5.3:

> a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one* shared
> dataflow whose final per-session operators (filter, project, diff) run per subscriber — the
> differential "arrangement" sharing model — not a thousand plans.
>
> … an optimisation with an exact correctness oracle (recompute) to test against — a luxurious
> position for CI.

| Asked for | Status | Where |
|---|---|---|
| The signal graph as a graph, not a recognised shape | **built** — one vertex per signal *operation*, including those nested inside a declaration | `beck-core/src/signal.rs` |
| Any number of durable folds; any depth and sharing; a `filter_map` on the fold path; cycles as cycles | **built** — folds are **fused** into one accumulator, a signal read twice is bound once | `beck-core/src/split.rs` |
| Every tier crossing enumerated, with the id §4.3 says a subscription is keyed by | **built** — replaces one hard-coded sentence | `signal.rs::Cut` |
| A view compiled to a dataflow plan rather than to one expression | **built** | `beck-core/src/plan.rs` |
| Maintained by delta: `map_values`, `map_list`, `filter_list`, `sort_by`, `concat_lists`, flatten, `list_len`, `list_is_empty` | **built** | `beck-core/src/engine.rs` |
| "incrementalized where analysis allows, **recomputed where not**" | **built** — an operator the decomposition cannot enter is a full recompute of that operator, and only when an input moved | §23.6 |
| Recompute as the correctness oracle in CI | **built** — every corpus program, every event of a generated log, engine against recompute, byte for byte | §23.7 |
| §5.3's shared dataflow, shared *at runtime* between subscribers | **built** — 64 subscribers over 11 versions advance it **11** times | §23.9 |
| The arrangements released when nobody is reading; the change history bounded by the laggiest reader | **built** | §23.11 |
| Per-session memory as an exported metric | **built** — in entries rather than bytes, split into the half paid once and the half paid per connection | §23.14 |
| SQL read models, pgwire exposure | **built** — and not as §5.3 assumed: the arrangement projected as relations, not a second copy in Postgres | §23.12 |
| Query fusion on symbolic plans, `beck explain query`, `beck explain cost` | **built** — over the dataflow plan; the `Query` sub-language remains symbolic and unwritten | §23.13 |
| `beck explain incremental` shows which, and why | **built**, and its first line is a CI gate | §23.15 |
| The page's children maintained as a patch stream rather than reassembled | **not built**, and it is where the remaining `O(n)` is | §23.8 |
| The render lock | **not built**, deliberately | §23.19 |

## 23.2 The signal graph as a graph, and the defect the debt was hiding

[`19`](19-phase-1-report.md) §19.9 stated the debt precisely: "`Roles` and the `Inliner` encode one
topology. The splitter produces a fixed seven-field struct and inlines four combinators by name. That
is the shape `todo.beck` has, and §3.7 says the signal graph is a *graph*."

Two phases described that narrowness the same way, and **both descriptions were generous**:

> What makes this legitimate narrowness rather than a defect is that it **announces itself**: nine
> diagnostics refuse every shape the splitter does not understand, and `split` returns `None`
> whenever one fires. A test pins that — an unsliceable program is refused with a code and a message,
> never quietly mis-sliced.
> — [`19`](19-phase-1-report.md) §19.9, quoted again by [`20`](20-phase-2-report.md) §20.5

That claim was false, and here is the program that falsifies it:

```python
proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, roster, validate)
tally: Signal[Tally] = durable(fold(apply_event, Tally(joins=0, leaves=0), events))
roster: Signal[Roster] = durable(fold(apply_presence, Roster(here={}), events))
page: Signal[Html] = map2(render, tally, roster)
```

The old splitter found the durable fold with `find(|s| … Prim::Durable)` — the **first** one — and
its inliner lowered *every* reference to a durable signal to the same state parameter. So `render`
was handed a `Tally` where it expected a `Roster`, and the second fold's step function was never
called. `beck check` said `ok`. `beck explain flow` listed `roster` as "inlined into the view" —
inlined, that is, *as `tally`* — and read as normal. The whole of what a developer would learn is

```console
$ beck test 21-two-folds.beck
test "each fold sees the same log and keeps its own answer" … FAILED
  rendering the page for `test`: rendering the view
```

A message about the wrong thing, three stages from the cause. **This is exactly the outcome the
narrowness was justified by preventing, and it went unnoticed for two phases because no program in
the corpus had two folds.** A refusal that is never exercised on the shape it exists to refuse is a
claim, not a check.

Two smaller versions of the same pattern sat in the same function. Taking the first ingress was
harmless, because placement's `B0403` already refuses a second. Taking "the first client-placed
signal" as the page was not: placement can put an intermediate `Signal[Html]` on the client too, and
the splitter would slice that and never reach the real page. The general slicer takes the
client-placed **sink** — a vertex nothing reads — which is what "the page" actually means.

### What a vertex is

The graph has one vertex per signal *operation*, not per declaration.
`todos: Signal[State] = durable(fold(apply_event, empty, events))` is **two** vertices, a `Durable`
over a `Fold`, because the fold is a node in the dataflow whether or not the program gave it a name.
Only the outermost carries the declared name; the inner is labelled `todos·fold`, because a
diagnostic about a fold the program did not name still has to point at something a reader can find.

```console
$ beck explain flow examples/todo.beck
signal graph — 5 vertices, 1 cycle, 3 tier crossings

  proposals   merge_clients()           server
  todos·fold  fold(events)              data     ↺
  todos       durable(todos·fold)       data     ↺
  events      decide(proposals, todos)  server   ↺
  page        per_session(todos)        client   ← the page, per session

tier crossings — each is one subscription, resumable by (id, seq) (§4.3)
  todos → events       data → server  carries State  01b13586d89df8b7
  todos → page         data → client  carries State  1f4e63cda7c3a8e8
  events → todos·fold  server → data  carries Event  7c389d7f875d517a
```

That is the difference between a graph and a pattern: `map2(f, durable(fold(…)), summary)` needs no
new case, because there was never a case to begin with. **Three crossings, not one** — the old
report line said one, and two of the three are in the cycle. §4.3 is explicit that *every* edge
crossing tiers is a subscription; the sentence was true of the edge its author was thinking about
and false of the program.

## 23.3 Fusion of folds, and a fold that takes a slice of the log

§3.7 fixes "**one totally-ordered log per application**". A program with two `durable` folds has not
asked for two logs; it has asked for two projections of one. So the slicer compiles them into a
single accumulator — a synthetic record with a field per fold, named for the signal that declared it:

```text
$State = { tally : Tally, roster : Roster }

step(s, env) = $State{ tally  = apply_event(s.tally, env),
                       roster = apply_presence(s.roster, env) }
```

Three consequences, each of them the point. **The runtime is untouched** — the log still holds one
totally-ordered stream, and snapshots, digests, `beck replay --verify` and the determinism argument
are unchanged, because the thing they are about is unchanged. **A one-fold program is bit-for-bit
what it was**: no wrapper, no synthetic type, and
`a_single_fold_program_is_untouched_by_fusion` pins that, because a generalisation that changes the
common case is a migration rather than a generalisation. And **`$State` is unwritable and
unpublished** — the name cannot be typed in the surface syntax and no `.becki` carries it — but it
*is* hashed structurally into the wire id, which is correct: adding a fold changes what a subscriber
can be sent.

**The chokepoint keeps reading the fold it named.** `decide(proposals, roster, validate)` names one
accumulator, and with a fused state the slicer has to project the right field out. Picking the wrong
one is a defect no type in the sliced `Core` would catch, because `Core` is not re-typechecked after
lowering — so `the_chokepoint_reads_the_fold_it_was_given` declares the folds in the *opposite* order
from the one the chokepoint uses, and a slicer that took "the first fold" fails it.

**A fold can take a slice of the log.** `filter_map` between the chokepoint and a fold used to be
refused; it is compiled by moving the filter into the step, which is the only place it could go if
replay is to stay a function of the log. The runtime still appends one stream and folds it once; the
filter decides which folds advance.

**A signal read by two consumers is bound once.** The old splitter inlined per use, so a program
whose page and read model both read `tally` recomputed it twice per event and nothing recorded that
the two were the same computation. That is a constant-factor improvement today and it is not why it
was built: **an arrangement can only be shared if something knows which computations are the same
one**, and under an inlining splitter that information was destroyed exactly where §5.3 needed it.

## 23.4 What the slicer refuses, and why each refusal is about meaning

The narrowness that remains is not "the slicer understands N shapes". Every refusal is a statement
about what a program would mean:

| Code | Refuses | Because |
|---|---|---|
| `B0509` | a cycle with no fold in it | a fold is where a cycle bottoms out — an accumulator is a value the slicer can take as a parameter. Without one there is no first value, and a slicer that did not check would recurse until the stack ran out |
| `B0510` | two client-placed sinks | the slicer slices both; the runtime serves one document per connection, and choosing between them is **routing** |
| `B0511` | a second `decide` | §3.5 rests on authority being one place. Two chokepoints are two answers to "may this actor do this", and the log records whichever ran |
| `B0513` | a `fold` nobody made `durable` | the log is what survives; an accumulator outside `durable` would be rebuilt from nothing on every deploy |
| `B0504` | a fold whose stream is not the chokepoint's output | the log holds what the chokepoint decided |
| `B0512` | a `decide` reading something that is not a durable fold | `decide` threads *the accumulator*, which is what makes first-writer-wins and ownership decidable (§3.7) |

`B0510` is worth reading twice: it is the first diagnostic in the compiler that names the **runtime**
as the limit rather than the compiler. Under the old splitter, "the slicer cannot" and "the runtime
cannot" were the same sentence.

Building the graph builder also found that most of its refusals cannot fire. `Signal[T]` and
`Stream[T]` are ordinary types, so unification rejects a view reading a stream with a type error
before the slicer sees it. That is the same position [`19`](19-phase-1-report.md) §19.7 took about
the durable path: an unreachable refusal is the right thing to have while the proof is missing. The
tests therefore assert *which* stage refuses only where the answer is interesting.

## 23.5 The one hard problem: a fold produces a value, a plan consumes changes

Every incremental dataflow system starts from a stream of changes. A Beck program does not have one.
`todos = durable(fold(apply_event, empty, events))` produces a **whole new accumulator** per event,
and §3.7 is emphatic that this is the semantics rather than an implementation: the accumulator is a
value, replay rebuilds it, snapshots hold it, the differential harness diffs against it.

So something has to turn a new value into a delta, and the obvious way is the trap. Walking the old
and new maps entry by entry is `O(n)` per event — the recount §3.8 exists to abolish, moved one level
down where nothing in a report would mention it. An engine built on that would print "incremental"
over a linear scan.

The way out was in the codebase and had been since Phase 1. [`19`](19-phase-1-report.md) §19.4 made
`Map[K, V]` a **persistent weight-balanced tree** so the fold would stop copying, and the property it
was built for has a second use nobody had cashed: an insert rebuilds only the path to the key and
**shares every subtree that path did not pass through, by pointer**.

`PMap::diff` is the conversion — an ordered merge of the two trees with one extra rule: **when the
heads of the two remaining sequences are the same subtree by pointer, drop both.** The soundness is
worth writing out because the whole engine rests on it: pointer-identical subtrees hold identical
entries, so the two remaining *sorted sequences* share that prefix exactly, and a merge over sorted
sequences reports nothing for a shared prefix. It holds however rebalancing moved that subtree in
either tree.

```
diffing an 8,192-entry map after one insert looked at 25 entries
```

`diffing_two_versions_that_share_structure_visits_a_handful_of_entries` fails at 64 and an `O(n)`
implementation would reach 8,192. There is also a `BTreeMap` oracle over 64 random histories, because
a diff that is fast and wrong is worse than one that is slow.

## 23.6 What a plan is, and why the fallback is the interesting part

`plan.rs` compiles a view into a DAG of operators by symbolically evaluating the sliced expression:
definitions are inlined, `let` binds a variable to an operator, and every subexpression becomes a
node. There are two kinds and the distinction is the design:

- a **delta operator** holds an *arrangement* — its output as a `BTreeMap` from an ordering key to a
  value — and updates it from the changes at its input;
- a **pointwise operator** holds a value and re-evaluates when an input changed.

Everything the decomposition cannot enter becomes one pointwise operator over the plan nodes it
reads: a `match`, an `if`, a call through a value, a primitive with no delta rule. **That fallback is
not a limitation to be apologised for** — it is the reason the engine is correct for programs it
cannot accelerate, which is what made it safe to switch on for the whole corpus at once rather than
for a recognised shape. A program the analysis does not understand is slow. It is never wrong.

And the fallback still buys something: a pointwise operator re-runs *only when an input moved*, which
a single inlined view expression cannot know. Deciding that cannot itself be a deep comparison — that
would put the `O(n)` back — so `engine::same` is conservative: scalars by value, records field by
field, and collections and rendered trees by **pointer**. It answers "unchanged" only when certain,
and "changed" costs a recompute the old runtime performed unconditionally.

### Order is a key, not a sort

An incremental view producing the right entries in a different order would be a correctness bug
rather than a cosmetic one: iteration order reaches the rendered page, the page is diffed into a
patch stream, and §4.8's replay harness compares that stream bit for bit. So order is not restored at
the end; it falls out of the arrangement's key.

| operator | key | why that key |
|---|---|---|
| `map_values(m)` | the map's key | already the order `map_values` yields |
| `map_list`, `filter_list` | the input's key, unchanged | neither operation moves an element |
| `sort_by(xs, k)` | `k(x)`, then the input's key | a *stable* sort, expressed as an order rather than performed as one |
| `concat_lists([a, b])` | the part's position, then that part's key | the parts keep their places |
| flatten | the outer key, then the position inside that element | one row's children move without disturbing another's |

That last row is what every `for` loop in a `ui:` block compiles to — the macro lowers
`for t in todos:` to `concat_lists(map_list(todos, …))` — so it is the operator that decides whether
rendering a list is incremental at all.

## 23.7 Recompute as the oracle, which is the only reason to believe any of this

For the sketch and for **every corpus program**, a log is generated from that program's own `Event`
union (seeded by the file's name, so a failure is reproducible), folded one event at a time, and
after every event the maintained page is compared with the recomputed one — not by a digest, by the
rendered HTML, for two different subscribers.

Three more properties, because equality alone would be satisfied by an engine that recomputed
everything and called it maintenance:

- **a cold engine and a warm one agree** at a state reached by forty events, so no arrangement is
  right only because of the order events arrived in;
- **an engine rendered against an older state is still right** — walked backwards through the whole
  history and forwards again — because `beck-rt`'s resumption path renders the state as of the `seq`
  a reconnecting subscriber last saw, and an engine that assumed the state only moves forward would
  serve that subscriber a page from the future;
- **the plan actually decomposed**, asserted by name. Every other test here would still pass if the
  decomposition had silently given up and made the whole view one opaque operator, because that is
  the fallback and the fallback is correct.

There is also a **subscription harness**. `beck-rt`'s `Socket` trait had said since Phase 1 that it
exists for "the upgraded socket in the server, and an in-memory duplex in the tests", and nothing had
used the second half of that sentence: no test drove `session::run`, the loop a browser actually
talks to. It does now, because that loop is where the engine is switched on. Its sibling runs the
same subscription with maintenance **off**, because a switch nothing exercises is a switch that stops
working and the failure would be silent — the page would still render, from the other path.

And the engine moved onto the **split side of the differential harness**, §8.3's "the project's
conscience". An optimisation of the view path belongs on the split side of the test that exists to
notice the split path changing behaviour.

## 23.8 §3.8's sentence, measured — and what is still `O(n)`

`cargo test --release --test measure_incremental -- --nocapture`, on the sketch, adding one todo to a
state that already holds `n`:

```
   rows       delta materialise   recomp   maintain µs recompute µs  ratio
     10           9         11        9            15           66   4.4×
    100           9        101        9            28          532  19.0×
   1000           9       1001        9           125         4585  36.7×
   5000           9       5001        9           697        24370  35.0×
```

**The `delta` column is the deliverable.** Ten units of work — per-element functions applied plus
arrangement entries moved — for one event, whether the collection holds ten rows or five thousand.
`a_count_over_a_maintained_collection_does_not_visit_the_collection` is the gate, and it is a
**count** rather than a duration, because [`13`](13-testing.md) §13.7's rule is that a shared CI
runner cannot hold a timing threshold honestly. The timings are printed and never thresholded.

**The `materialise` column grows exactly with the collection, and it is the honest half of the same
table.** It is the cost of handing a *pointwise* operator a `Value::List`:
`html_el("ul", attrs, children)` takes the whole child list and builds an element holding all of it,
so `n` handles are copied and an `n`-child element is constructed per event even though one child
changed.

**"`n` handles" was not true when this sentence was first written, and the difference was the
larger half of the cost.** `Html::Element` held `children: Vec<Html>` — owned subtrees — so `child`
deep-copied every child it was given, and because each enclosing element copied what its own
children had just copied, one event rebuilt *every node of the page* rather than `n` of them. The
cost therefore compounded with nesting depth, which is invisible in a column counting entries. It is
`Vec<Arc<Html>>` now and the sentence above is true as written: an untouched subtree costs a
refcount. The table is the same measurement before and after —

| rows | maintain µs, owned | maintain µs, shared |
|---|---|---|
| 10 | 34 | **15** |
| 100 | 182 | **28** |
| 1,000 | 2,336 | **125** |
| 5,000 | 14,827 | **697** |

— and the recompute path halves as well (50,845 µs → 24,370 µs at 5,000 rows), because
`beck_core::html::element` is the one function the evaluator *and* both native backends build a page
with. `incremental_engine.rs::one_event_allocates_a_handful_of_html_nodes_whatever_the_page_holds`
is the gate, and it is counted by pointer identity rather than by a clock (§13.7): **9 new nodes on
a 200-row page and 9 on a 1,600-row page, against 211 and 1,611 with the copy put back**.

**And the render is only half of what the server does per event**, which this section did not say
until the assembly stopped dominating it. `Feed::Dom` maintains the page *and* structurally diffs it
against the one the client already holds, and both are on the interaction path:

```
   rows     render µs      diff µs     total µs  diff %
     10            16            1           17      6%
    100            33            6           39     15%
   1000           148           55          203     27%
   5000          1448          929         2377     39%
```

`measure_incremental.rs::what_one_event_costs_the_runtime_end_to_end`. Before the children were
shared the diff was the *smaller* half at every size; it is now **39% of the per-event cost at
5,000 rows**, because only one of the two got cheaper — and it was 54% of it in the window between
the children being shared and the trim landing. A table showing the render alone was describing
half the process.

**The differ trims the ends the two pages physically share** — a run of children that are the same
allocation in both needs no ops and no examination — which is worth 2–3× (203 µs → 71 µs at a
thousand rows, 1,961 µs → 1,036 µs at five thousand) and is available only because a child is now a
handle. `diff::tests::a_shared_page_and_a_copied_one_produce_the_same_ops` is the gate, and it is a
**differential rather than an assertion**: every scenario runs twice, once shared and once through
`rehash`, which rebuilds node by node and shares nothing, and the two op streams must be equal. The
differ's other tests cannot do that job — they build every node fresh, so nothing in them is ever
shared and the trim would never run.

**Every number above is an *edit*, and the edit is the case the trim flatters.** One changed row
leaves a window of one, so what the table measures is mostly the two `keyed` passes over the full
lists. **A reorder is the opposite case**: sorting a table by another column shares no prefix and no
suffix, so the whole list is the window — and there reconciliation was **quadratic in the rows that
move**.

```
   rows     scan µs     rank µs
    500         506         196
   1000        2786         396
   2000        5981        1216
   4000       25476        2105
```

`beck-cli/tests/scaling.rs::reordering_a_keyed_list_costs_the_same_per_row_however_long_it_gets`.
Per row, the scan cost 1.01 µs at 500 rows and 6.37 µs at 4,000 — each doubling roughly quadrupling
it — where the replacement holds 0.39–0.56 µs/row out to 8,000. The cause is that the client applies
a patch against the children it already holds, so every index the differ emits must be the one that
child occupies *at that point in the stream*, and reading that off the list is a scan; one scan per
child is `O(w²)`. Because a `Move` lifts a child out and re-inserts it at the front, the children
nobody has claimed yet keep their relative order, so the distance a child sits ahead of where it
belongs is exactly the number of unclaimed children before it — a **rank query**, `O(log w)` against
a Fenwick tree, making the pass `O(w log w)`.

**The op stream is unchanged, and that had to be proved rather than assumed.** Reconciliation's
output is a contract with a client that has already applied everything before it, so a faster route
to the same *page* is not good enough. Round-tripping cannot see the difference — many distinct
streams land on the same tree — so the scan is kept as the oracle in
`diff::tests::the_rank_structure_and_the_scan_it_replaced_emit_the_same_ops`, and 300 generated
cases assert the two streams equal. Making the new code emit one redundant no-op `Move` leaves
`round_trips_over_a_long_random_walk` green and turns that test red, which is the difference between
the two gates stated as a fact.

**This corrects the scope of a claim this section previously made.** It said the residue could not
be removed by a better differ. That was measured on edits only, and a quadratic was hiding in the
case the measurement did not cover — which is the argument for two sizes made against the report
that made it.

**The residue was `keyed`, and it was most of the cost.** Deciding whether a list reconciles by key
at all means hashing every child's key into a set, on both lists, before any trim or rank query can
help — and on a page where one row changed that was **62% of the diff at 1,000 rows, 87% at 5,000
and 89% at 8,000**. This section twice said the remainder was not a differ problem. Both times that
was reasoning rather than measurement, and both times measuring it disagreed; the second time is
recorded here rather than quietly fixed, because the pattern is the finding.

**It is asked once now instead of twice.** Whether a list is keyed is a question about the *whole*
list — a key repeated anywhere makes the reconciliation ambiguous — so unlike the reconciliation it
cannot be narrowed to the window. But the children at the shared ends are the same allocations in
both lists and so carry the same keys, so hashing them once answers for both, and only the windows
are hashed twice. Measured A/B in one process, alternating between the two predicates so that
nothing about the machine separates them:

```
   rows    two passes    one pass
   1000       53.7 µs     26.3 µs   2.04×
   5000      343.3 µs    174.2 µs   1.97×
   8000      593.2 µs    306.0 µs   1.94×
```

**The predicate is unchanged, and that is what makes this safe**: it computes the same answer, so no
op moves, and the two differentials above hold it. A microbenchmark of the predicate alone reads
only 1.3–1.4×, which is the smaller and more flattering number for the *old* code — calling the two
implementations back to back leaves the second pass reading keys the first has just pulled into
cache, which is a luxury the full diff does not have.

**Narrowing the question to the window would have been worth more and was refused.** It would leave
a window of one to hash instead of a list of 8,000 — but a page whose shared prefix repeats a key
would then reconcile by key while the same page assembled without sharing would not, and "the same
two pages produce the same ops however they were built" is the property the trim is only sound
under. `diff::tests::a_repeated_key_in_the_part_two_pages_share_forces_the_positional_path` is the
gate, and it was written because nothing else in the file could fail for it: a predicate that
skipped the shared ends passed all fifteen other tests.

**What is left after that is the walk itself** — the trim compares `n` pointers, which is cheap per
element and still `O(n)`. Given two pages and nothing else, finding what moved means looking at what
is there: whatever the engine knew about its own changes, the differ has to rediscover. That is
§8.5.4's open item, stated from the other end — and this time the claim is that the *asymptote* is
the differ's floor, not that the constant in front of it cannot move.

So the maintained view is **not** `O(δ)` end to end. It is `O(δ)` in the *elements it computes* and
`O(n)` in the *page it assembles*, and the measured 4–37× is a constant factor on an unchanged
asymptote rather than a change of asymptote. **A report that quoted "37× faster" without that
sentence would be describing a different system** — the assembly is `n` refcounts now instead of `n`
subtree copies, which is a much smaller `n` term and still an `n` term.

Removing the remainder is a known piece of work rather than an open question, and it is the same
piece of work in both directions: **the delta at the top of the plan *is* the patch set.** `beck-rt`
renders a whole page and structurally diffs it against the previous one to produce the patches §5.1
streams; an engine emitting patches from its own output changes would skip both the assembly and the
diff. That is also precisely what Mode B's per-component kernel needs, which is why it is not done
here — it is a protocol change and a client change, not an engine change.

The analysis table has said since the slicer landed that `html_el` is maintained by "a subtree delta
— what the patch protocol already streams". **That table is a statement about what a plan could do,
and the engine does not do this one.** The two disagreeing in silence would be the sort of thing this
project exists not to do, so `beck explain incremental` prints both: the analysis's verdict per view,
and the plan's operator table, in which `html_el` appears under `recompute`.

**There was a second instance of that disagreement, and printing both did not catch it.** A
per-element function that captured a plan node is a *different function* when that node moves, so its
whole collection is reapplied (§23.13's rebuild rule) — and when what it captured is the
**accumulator**, that is every event. `beck explain incremental corpus/27-review.beck` reported its
`flat_map` as `maintained`, because the operator does have a delta rule; `beck explain cost` reported
`n applications whenever #0 moves` for the same operator, because its *function* moved rather than
its input. Both were printed, both were true, and nothing put them side by side.

The shape was a join written as a loop with a lookup inside it, and there is now an operator to
compile it to ([`99`](99-the-data-tier-means-of-combination.md) §99.6): `27-review` compiles to a
`Join`, its `flat_map` captures nothing, and the two commands agree about it. §99.3 has the
transcripts and the sweep that found three such sites in the tree among fifteen benign ones the same
line reported identically — **all four are now closed**, the last of them (`board`) being a grouping
rather than a lookup, which took the `arrange_by` §99.9 item 3 schedules. Four rather than three
because a fourth site arrived with `awareness(f)` after that sweep was run and nothing re-ran it.
That last clause is why the sweep is now a gate:
`incremental.rs::no_program_in_the_tree_reapplies_a_collection_per_event` plans all 42 programs in
`corpus/` and `examples/` and holds the count at zero, against 8 sites with the recognition switched
off. What the episode leaves behind is the lesson rather than the case: two commands that are each
true about one operator, and no reader who would run both.

**So this section's `materialise` column is now the whole of what is left**, and that is a change in
its standing rather than in its number. When it was written it was one of two per-event linear costs
and the smaller-sounding one; with the captures closed it is the only one, in every program in the
tree. It also had no position in [`08`](08-roadmap.md) §8.5's order until that was measured —
"a known piece of work rather than an open question" is precisely the description §8.5's preamble
gives of an item that never comes due — and it has one now, as an **F** item, because the fix is the
work Mode B's per-component kernel needs anyway.

## 23.9 One shared dataflow: the three choices

`Plan::per_session` is correct by construction — a node is per-session exactly when it transitively
reads the session node, through an input or through a per-element function's capture. What was
missing was somewhere for the other nodes to live that is not one subscriber's engine, and three
questions had to be answered before there could be one.

**Who advances it. Not the sequencer.** Putting view maintenance under the write lock would move it
onto the *write* path — every command paying for the views of every connected session before its ack
— and would do that work for states nobody is looking at. `beck run` with no browser attached would
maintain a dataflow for an audience of zero. So **the first subscriber to render at a new version
advances it**, under a write lock, and everyone rendering at that version afterwards finds it done.
Lazy rather than eager, and it gets three things at once: the work happens once per version, it is
paid by a renderer that was about to do it anyway, and a process with no subscribers does no view
work at all. The double-check is not decoration — `advance` tests the version under a read lock,
takes the write lock, and tests again, because between those two lines another subscriber may have
done exactly this.

**What a subscriber holds while it renders: a read lock, for the whole of its own render.** Readers
do not block readers, so a thousand render concurrently; the only writer is the advance, which is
`O(δ)`. The alternative — publishing an immutable snapshot per version, held by `Arc` with no lock —
does not survive contact with the arithmetic: a snapshot is immutable, so an arrangement that moved
has to be *copied* to build the next one, which is `O(n)` per changed operator per event, the cost
this engine exists to remove reintroduced where it would be hardest to see.

**What happens to a subscriber that fell behind.** A subscriber is woken by a `watch`, which
coalesces by design, so three events can land between one render and the next. The shared side
therefore has to answer a question a per-subscriber engine never had: not *what changed*, but **what
changed since you last looked**. Handing a laggard only the latest version's changes is wrong in a
way that does not show in a page and never repairs itself — an entry inserted at version 5 and
removed at 6 is never mentioned again, so a subscriber that rendered at 4 and next renders at 7 would
serve a row the accumulator has forgotten, for the rest of that connection. So the shared side keeps
a **bounded history of steps**: for each remembered version, which operators changed, which rebuilt,
and what moved at each. Beyond the history it **rebuilds**, which is correct at any lag and needed no
new machinery.

Two details decide whether that history is affordable. A **rebuilt** operator's changes are
deliberately not kept — a consumer below a rebuild re-reads the arrangement whole, so storing them
would retain a copy of the entire collection per remembered version, for nothing. And the changes are
**concatenated, not coalesced**: a consumer applies them in order, so a key that moved twice lands
where the second one put it, and coalescing would cost a pass over a window that is a handful of
deltas.

§5.3's claim is that a thousand connected users compile to *one* shared dataflow, and "one" is a
number, so it is a counter rather than a description:

```
64 subscribers × 11 versions → 11 advances
```

`the_shared_prefix_is_advanced_once_however_many_subscribers_render` asserts **equality**, so an
advance triggered per subscriber fails at the first extra one. The memory half is asserted in the
same shape: for every corpus program, a subscriber's shared arrangement count is **zero**, and the
shared and per-subscriber halves *add up* to what one standalone engine held — the second assertion
being the one that would catch entries quietly going missing rather than moving.

## 23.10 Where you read the session decides what your fanout costs

Two programs, because the answer is a property of the program and quoting one number would be quoting
the more flattering one. Both carry 200 rows. `work` is per-element applications, arrangement entries
moved, entries copied into a list and pointwise operators re-evaluated — a count rather than a
duration, so it is the same on any machine.

**A cold fanout — every subscriber's first render:**

```
examples/todo.beck, 200 rows: 28 of 43 operators do not read the session
 subscribers   unshared KB    shared KB        ×
           8          1533         1135     1.4×
         256         49084        34566     1.4×

24-feed.beck, 200 rows: 22 of 31 operators do not read the session
 subscribers   unshared KB    shared KB        ×
           8          5179         1689     3.1×
         256        165750        38608     4.3×
```

**One event, over a fanout that is already connected** — the number an operator actually pays:

```
examples/todo.beck   subscribers   unshared work  shared work        ×
                               8              66           52     1.3×
                             256            2112         1602     1.3×

24-feed.beck         subscribers   unshared work  shared work        ×
                               8            3384          465     7.3×
                             256          108288         1953    55.4×
```

**Why the two answer so differently.** The sketch's cut is immediately below the accumulator: `mine`
is `sort_by(filter_list(map_values(s.todos), λ owner == session.actor), λ text)`, and the filter reads
the session, so *everything from the filter upwards is per-session*. What is shared is the
`map_values` arrangement and the constants — real, and 28 of 43 operators, but the operators that do
the work per event are all below the cut. `24-feed.beck` puts the cut at the top: `visible(s)` sorts
a public feed and reads no session, and the `ui:` loop over it captures nothing session-dependent
either, so the sorted list, the `li` for every post, the `ul` that assembles them and the page's whole
`O(n)` half are all *above* the cut. Only the greeting is per-session.

**This is the finding a developer should take from this chapter, and it is not "sharing is worth
4×".** It is that where you read the session decides what your fanout costs; that the boundary is a
property of **operators rather than of signals**; and that `beck explain incremental` prints which
side every operator is on, so moving the cut is a decision rather than an accident.

### §23.8's `O(n)`, paid once instead of removed

Sharing does not remove the linear page assembly. What it decides is *how many times* it is paid. On
`24-feed.beck` the assembly is above the cut:

```
  50 posts: the shared side materialised 100 entries once; 8 subscribers materialised 24 between them
 400 posts: the shared side materialised 800 entries once; 8 subscribers materialised 24 between them
```

Eight subscribers pay a constant 3 entries each at 50 posts and the same 3 at 400. The `O(n)` did not
become `O(δ)`; it stopped being multiplied by the fanout. On the sketch, where the assembly is
per-session, it is multiplied by the fanout exactly as before.

## 23.11 The arrangement lifecycle: a reader set, a frontier, a ceiling over a floor

Two loose ends read as two pieces of work — a drop policy and a config field — and were one.
[`38`](38-literature-survey.md) §38.2 read both against the literature and came back with one answer:
the **reader-frontier discipline** of *Shared Arrangements* (McSherry, Lattuada, Schwarzkopf &
Roscoe, VLDB 2020). Each subscriber holds a frontier; the trace is compactable up to the minimum
subscriber frontier and droppable when the reader set is empty. The survey called it the cheapest
item in it — the engine already had versions and subscribers and lacked only the rule connecting them
— and the estimate held. **A system that cannot answer *is anybody reading this?* and *how far behind
is the furthest of them?* has no choice but to keep everything forever**, which is what it did.

**A subscriber is a counted reader**, and the signature is `fn subscriber(self: &Arc<Self>) -> Engine`
rather than an added method. A subscription does not *end* in the runtime — nothing calls `close()`.
It ends because `session::run` returns, by completing, by erroring or by its socket dying, and its
engine is a local that goes out of scope. So the engine has to reach the dataflow from its own
`Drop`, and that means holding an `Arc` of it. There is no cycle: the reader set holds frontiers, not
engines.

**The frontier is an atomic, and that is not an optimisation detail.** The obvious implementation is
a map under the dataflow's existing lock, and it is wrong: a frontier is written on *every render*
and read only when the dataflow *advances*, and §23.9's second choice is that subscribers render
concurrently under a read lock precisely so a thousand of them do not queue. Publishing under the
write lock would have serialised every render behind every other render, in the name of a bookkeeping
update no reader was waiting for. The ordering that makes the lock-free version safe is worth stating
because it is not obvious: a reader publishes its frontier **after** its render, outside the read
lock, so in that window its frontier reads *older* than it is — which retains more history than
needed. **Retaining too much is a memory cost; retaining too little is a wrong page. The race can only
go the safe way.** Compaction runs under the write lock, and a render holds the read lock for its
whole duration, so a compaction cannot start while a render is in flight.

**A reader that has not rendered pins nothing.** A fresh subscriber's frontier is `u64::MAX`, not 0,
and falls out of the minimum rather than having to be filtered out of it. Treating it as 0 would be
the natural reading — it has rendered nothing, so it is behind everything — and would have been the
bug: an engine with no arrangements *rebuilds* whatever it is offered, so a reader at frontier 0
would pin the entire history for the one subscriber that cannot use a single step of it.

**Retention is a ceiling over a floor**, and the code says which is which. The floor is a **fact**: a
step every attached reader has rendered past is retained for nobody, and dropping it cannot change
any page. The depth is a **policy**: past it we would rather a very late subscriber rebuild. 64 is
still the default depth and still not a measured knee — but it is now a ceiling that almost never
binds rather than the retention itself, which is a much weaker dependence on the number.

`release_when_idle` is a switch rather than a constant because the trade is genuinely a deployment's:
releasing gives back almost everything and charges the next subscriber a cold start, so a service
whose clients reconnect constantly would rather pay the memory. The default is to release, because a
process idle for hours holding a fanout's arrangements is the worse failure, and both sides are
asserted so neither is folklore.

**No clock was introduced.** The obvious refinement — release after a grace period — needs elapsed
time, which is not on `beck_core::clock`'s seam. Putting a timer inside the engine would have been
the third place in the tree that reads time ambiently, three phases after F11 said not to.

The release is deliberately, exactly the reset an error already took: the engine discards its
arrangements when an operator fails mid-advance, and what it leaves behind has to be a dataflow that
says it has *never been advanced* rather than one advanced and then hollowed out. Reusing that path
is worth more than the four lines it saved, because the correctness of "a dataflow with nothing in it
serves the next subscriber a right page" was already under test. What was new was the *second* render
after a release — a dataflow that reset its arrangements but kept its version would advance from a
version it can no longer describe.

```
What an idle process holds — 64 subscribers, then none
             program  connected KB    shared KB      idle KB given back
  examples/todo.beck          8684           57            5    90.3%
        24-feed.beck         10026          498            4    99.1%

How much change history a fanout pins — 24-feed.beck, one laggard
 laggard's lag    retained  the ceiling      saved
             0           1           64      64.0×
             4           4           64      16.0×
            70          64           64       1.0×
```

**Read the percentage against the right denominator.** It is of the *shared* column, not of
`connected`: the per-subscriber arrangements go when the subscribers go, and always did. What is new
is the shared column dropping. The residue is the plan's constants and empty cells — a per-operator
cost rather than a per-row one. The honest statement is that this fixes a leak of **bounded** size:
the arrangements were always proportional to the accumulator, never to uptime.

The history table's first rows are the common case and the one the constant was most wrong about: a
fanout whose subscribers all render at every version was costing 64 versions of retained change and
costs one. The last row is the ceiling still doing its job — a subscriber 70 versions behind is
served by rebuilding, which is the policy 64 was always meant to express and is now the only thing it
expresses.

**A number this table does not contain**: bytes per retained version. A step is a delta rather than a
collection, so 64 versions of a quiet program is small and 64 versions of a program that churns its
whole collection every event is not, and one program cannot say which.

## 23.12 The read model is the arrangement

§5.3 says a read model is "generated tables in the same Postgres". **It is not, and it should not
be.** [`10`](10-decisions.md) D26 is the decision and
[`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md) the wire.

The row has two claims in it, and building it separated them. The middle column is an
*implementation*; the right column — "`psql`, BI tools, DBeaver see materialized views as ordinary
tables, the single cheapest trust-builder for adopting teams" — is the *value*, and **nothing in that
value depends on the rows being in Postgres.** It depends on them being reachable by a Postgres
client, and on their being correct, which is the part a second copy makes harder.

Why the durable projection is the wrong half, in the order the reasons bite:

- **It puts view maintenance on the write path.** That is §23.9's argument with a weaker case: a
  subscriber at least exists while it is being paid for, and a read model's reader is a BI tool that
  connects twice a day.
- **It is a second code path over the same events.** The engine's correctness argument is that
  recompute is the oracle. A projection written beside the dataflow is covered by none of it, and
  "the read model and the page disagree" is the class of bug that shows up in a support ticket rather
  than in CI.
- **It doubles the storage** of every maintained collection, to hold a copy of what is already in
  memory, in the order it is already in.

So the tables *are* the arrangements. Nothing is written on the append path, no projection exists to
lag behind, and there is no second code path that can drift from the page.

| Table | Rows | Read from |
|---|---|---|
| a collection-valued field of the accumulator | its elements | the state value |
| the accumulator's remaining scalar fields | exactly one | the state value |
| a declared signal that does not read the session | its elements, or exactly one | the maintained node |

**The third row needed no new analysis.** `Plan::per_session` has been a field on a plan node since
the engine landed, and §5.3's fanout argument is about exactly the operators for which it is false. A
SQL client has no session — nothing about `psql` says who is asking — so the signals it can be shown
are the ones whose value does not depend on that. **What decides which signals are tables is the same
cut the fanout uses**: a table is a view that does not depend on who is asking. `page` is excluded by
its type rather than by its name; `Html` is not a relation.

Across the 31-program corpus, with no annotation in any of them: **31 of 31 programs have at least one
table, 43 tables in all**, 39 read from the accumulator and 4 from the maintained dataflow.

`beck explain sql` prints the schema as `create table` statements — DDL nothing executes, because
there is nothing to create. The four SQL types are `bigint`, `double precision`, `boolean` and
`text`, and the choice of four is a wire decision rather than a taste one: every one of their OIDs is
in every driver's built-in table, so nothing ever has to ask a catalogue that does not exist what it
just received. Anything else is `text` holding the JSON the browser already gets.

**`Option[T]` is where SQL's null comes from, and it is the only place.** Beck has no null; a column
is nullable exactly when its field is an `Option`, `None` is `NULL`, and a non-nullable column can
never hold one. That fell out of the type mapping rather than being designed.

Two things the first working version got wrong, both found by reading its output over the corpus
rather than by a test. **A `Signal[State]` was a table** — every corpus program declares its fold as a
signal, so the derivation produced a one-row table whose only column held the entire map as JSON,
beside the real table for the same data. A fold's own name is not a table; its collections and its
scalars are. And **`corpus/17-derived.beck` has a field called `distinct`** — an ordinary Beck
identifier and a SQL reserved word. Beck's namespace and SQL's are not the same namespace and never
will be, so the generated DDL quotes any name that needs it, and the reader finds out in a generated
page rather than at `CREATE TABLE` time in production.

**A query is a reader, and it holds a snapshot.** A query advances the dataflow itself — it is a
renderer that produces rows instead of a page — and the consequence is the strongest thing here: **a
`select` issued after an ack sees that ack's event**, with no subscriber connected, no projection
written and no lag waited out. There is nothing to be stale. A pgwire connection is a member of the
reader set (§23.11), because a client holding a connection is going to ask again; its frontier stays
at `UNRENDERED`, deliberately, since a reader that never applies a delta cannot use the change
history. And a query runs under the accumulator's read lock, so two tables in one query cannot
disagree about which events have happened. **The cost is stated rather than hidden: a scan of a large
table delays the next commit by the length of the scan.** The alternative — clone the accumulator and
let the arrangements move underneath — is cheaper for the writer and gives a query that sees two
versions at once, which for a read model whose entire argument is that it cannot disagree with the
page is the wrong trade.

What it costs per event, counted rather than timed. **Nothing per event**: a connected client that has
asked nothing leaves the write path exactly as it was, 0 advances after 200 and after 1,600 committed
events. And the delta per query does not grow with the collection — 2 entries touched, 1 function
applied, 8 operators recomputed, at 200 rows and at 1,600. The gate is a shape rather than a rate.

## 23.13 Query fusion, and the rewrite that would have been wrong

§5.3 asks for fusion "on symbolic `Query` nodes". Two things in that sentence are about different
objects. **The `Query` sub-language does not exist yet** — no engine compiles one because no program
writes one, and a relational query language with joins and aggregates is unbuilt. **The plan does
exist**, and `for t in todos:` is decomposed into a `map_list` whose arrangement holds one rendered
list per row and a `flatten` that takes those lists apart again — which is *exactly* the "`for` over
a view of a view" §5.3 names, arrived at because the decomposition walks the source rather than the
shape. Nothing reads the arrangement in between.

An arrangement is a `BTreeMap` from an ordering key to a value, and the order reaches the rendered
page and the replay digest, so a rewrite has three obligations rather than one: **the same values, in
the same order, and the same deltas** — a fused operator has to move exactly the entries the pair
moved, or a subscriber woken late updates by a delta that does not describe what happened.

| rule | what it does | why it is sound |
|---|---|---|
| `map_list` over `map_list` | one `map_list` applying the composition | neither moves an element, so both arrangements are keyed by the input's key and so is the composition |
| `filter_list` over `filter_list` | one `filter_list` applying the conjunction | same key; and the conjunction **short-circuits**, so the outer predicate is applied to exactly the elements the inner one kept |
| `flatten` over `map_list` | one `flat_map` | the map's key is the input's and the flatten's is the map's followed by a position, so one operator keyed by the input's key and a position produces the same order |
| `list_len` / `list_is_empty` over `map_list` or `sort_by` | the count reads the producer's input | both produce one entry per entry, so *how many* is a question about the input, and the arrangement between them is never read |
| `concat_lists` of one list | the list | a union of one delta stream is that delta stream; a key prefix every entry shares does not order anything |

The conjunction is written as an `If` rather than as a strict primitive, because `and` **is** an `If`
in `Core`: the strict spelling would apply the outer predicate to elements the inner one rejected,
which the pair never did — invisible in the values, and visible the first time an outer predicate can
fail. `flat_map`'s rebuild rule is `map_list`'s rather than `flatten`'s, which is not a detail: a
per-element function that captured a plan node is a *different function* when that node moves, so the
whole collection has to be reapplied.

A producer is fused into its consumer only when three things hold, and **the second is why a fusion
pass in this language is not the textbook one**:

1. **Nothing else reads it.** An arrangement two operators read is the shared prefix, and fusing it
   into one of them computes it again for the other.
2. **It does not cross the session cut.** Operators that do not read the session are held once for
   the whole fanout and advanced once per event; operators below the cut run per subscriber. A local
   rewrite cannot see that — fusing a shared `map_list` into a per-session one produces a smaller
   plan whose *shared half has disappeared*, so work the process did once per event it now does once
   per event per connection. On the public feed of §23.10, that is the 55× spent rather than saved.
   **The page is byte-for-byte identical either way**, so this is not a correctness condition and no
   differential harness can see it — which is why it is asserted on a program built to make it bite,
   and why deleting the condition turns that test red.
3. **No name points at it.** A declared signal is projected as a read-model table (§23.12), so an
   operator a developer *named* is observable to a SQL client even when the page never reads it, and
   fusing it away would silently remove a table. The corpus's read model is unchanged by this pass:
   the same 43 tables.

**There is no fourth condition and no cost model, and that is worth saying plainly.**
[`38`](38-literature-survey.md) §38.2 says to adopt the shape — small local rewrites, each sound
against the change semantics, extracted by a cost model — pointing at egg and egglog. The shape is
adopted and the machinery is not: equality saturation earns its keep when rewrites *conflict*, so
that phase order would otherwise decide the answer. None of these conflict — every rule removes an
operator and none adds one — so there is nothing for an extraction pass to choose between.

**What it is worth is memory rather than time.** Fourteen of thirty-one programs lose an operator;
seventeen arrangements go in all. On the sketch, whose `for t in mine:` is the shape the rewrite is
for, a subscriber holds **17% fewer entries at every collection size** — `n + 1` fewer, gated as a
difference at two sizes rather than printed. Per event the rewrite saves exactly one arrangement
insert, and the 2–6% that shows on the wall clock is inside what this measurement can distinguish;
saying otherwise would be [`70`](70-the-evaluator-gets-fast-report.md) §70.1's mistake in the other
direction. The two plans are measured **alternating** rather than one after the other, because §70.7
found that a fixed A-then-B order biases a wall-clock comparison by as much as the effects this
project reports.

Seventeen programs lose nothing, and fifteen of them for one reason: their view holds no collection
at all. `beck explain query` says that rather than "nothing matched", because the two are different
facts about a program.

## 23.14 Per-session memory, exported

§5.3 names per-session memory as one of three metrics to export, and [`18`](18-phase-0-report.md)
§18.3's kill gate is written in kilobytes per idle session. An engine per subscription is a
memory-for-time trade, and a trade whose cost is not measured is a claim.

**A maintained subscription costs about four times the memory it already held for its page.** The
comparison is against the page alone because a subscription has always retained one, so the
multiplier rather than the absolute is what a fanout estimate should be scaled by. Phase 0 measured
~5 KB per idle session against a ~50 KB tripwire; four times that is ~20 KB, inside it. A subscription
over a thousand rows is 5.4 MB, and **no gate in this project covers that case, before or after this
work.**

Two notes on the method, because the first attempt was wrong in a way that would have been
publishable if nobody had run it twice. It read `/proc/self/statm` around 32 live subscriptions and
reported a ratio that **swung between 2.0× and 4.9× across runs of the same binary**, because the
resident set moves with the allocator's arena rather than with the data. A counting allocator would
be exact and needs `unsafe`, which this workspace forbids. `Engine::footprint` therefore walks what
is retained and adds it up, and it does two things that decide whether the number means anything: it
**counts shared structure once**, and it is given the accumulator and walks that first, so structure
the engine shares with the fold is charged to the fold. It excludes per-allocation overhead, so it is
a floor rather than a ceiling.

Four gauges are on the dashboard and in the OTLP export:

- `beck.views.shared_arranged` — entries the one shared dataflow holds;
- `beck.views.session_arranged` — entries the connected subscriptions hold **between them**;
- `shared_retained` — versions being kept, which reads as a **lag** signal rather than a memory one:
  it sits at 1 on a healthy fanout and rises when renders stop keeping up with events;
- `shared_releases` — a process whose releases track its connection count is one whose clients are
  flapping, which is the case for turning `release_when_idle` off.

Putting the first two side by side is the whole operational question, because the first is paid once
and the second is multiplied by the fanout: **a program whose second number dwarfs its first has its
cut in the wrong place**, and that is now visible on a running process rather than in this document.

Three decisions in it. **Entries, not bytes** — a byte figure needs the walk above, which is right
for a report and far too expensive to sample on every render. **Maintained as a difference**, so the
gauge follows a subscription that grew without re-summing every connection. And **released by a
guard**: a subscription ends by returning, by erroring, or by its socket dying, and a gauge that only
releases its share on the happy path drifts upward until it describes connections that closed hours
ago. The same reasoning gives `shared_arranged` a guard declared *before* the engine, so Rust's
reverse drop order runs it *after* — otherwise a process whose last subscription ended would report
the fanout's entries for as long as it stayed idle, which is the one moment an operator most wants
the number to say zero. **A metric that is wrong exactly when it matters is worse than an absent
one.**

The test that asserts the per-session gauge returns to zero has a **test binary to itself**, and that
is the point rather than an accident: telemetry is one value per process, so a test asserting a gauge
returned to zero cannot share a binary with a test that has a subscription open.

There is still **no gate** on any of these numbers. Making a number visible is the cheap half;
deciding what value of it should fail a build is the half this project keeps deferring.

## 23.15 `beck explain incremental`, and the line it is not allowed to get wrong

When the analysis shipped ahead of the engine, its report's first line was made a gate on the grounds
that "a command called `explain incremental` that let a reader believe their view was being
maintained would be the most misleading output in the compiler". The line said *every view below is a
full recompute per event*. It has changed twice since, and the obligation has not:

```console
$ beck explain incremental examples/todo.beck
Views are **maintained by delta** as far as the plan can decompose them: 7 of
this view's 27 operators update from the change itself, 20 are recomputed when
an input moves, and the page's children are still assembled in full every time.
```

```console
$ beck explain incremental corpus/22-shared.beck
**Nothing in this view is maintained by delta.** The plan found no collection for a
delta to flow through, so all 20 of its operators are recomputed — each one
only when an input actually moved, which is what a plan buys even here.
```

The second is the assertion that matters, and it is a fact about that program rather than a caveat:
that view computes a `map_len` and concatenates strings, so there is no collection and nothing to
maintain — **and a report written about the feature would have told its reader otherwise.** Both
first lines are CI gates.

The report has two halves because there are two questions. The **analysis** asks whether a *view* is
built only from operations with delta rules; the **plan** says what the engine actually compiled the
view into. They disagree in both directions: a view with a `match` in it is `recompute` by the
analysis while its collections are still maintained around the `match`, and the page is `incremental`
by the analysis while `html_el` is pointwise in the plan. Printing one of them would have been
printing the wrong one for half the programs. It also prints which side of the session cut each
operator is on, which is what makes §23.10's finding actionable.

The analysis's rule, in the order the answers are useful: **the row is empty** (§3.8's precondition —
a view that performs an effect is re-evaluated when the effect says so); **every operation has a
delta rule**, from a table that is *stated, not measured*, each entry a rule the differential-dataflow
literature already has, written down so it can be argued with; and **it is a view at all** — a fold
is not maintained by delta, it is what *produces* the deltas. The blocker is reported by name and it
is the *first* one in source order, because the first is the one to fix.

A `match` is the interesting refusal. Differential dataflow does handle branching — the scrutinee
becomes a collection and each arm a branch of the plan — and that is a real technique not in the
table. Calling it "recompute" is the conservative answer, and the message says which rule was missing
rather than "unsupported".

## 23.16 What building it found

Seven defects, and the interesting thing about them is how each was found.

**The mis-slice two reports had certified** (§23.2). It took forty lines of Beck. *A defect found by
writing the program down is worth three found by reading the code*, and every remaining bullet should
ship with the program that would embarrass it.

**A rebuild is contagious, and it took an oracle to see it.** An operator that *rebuilds* — because
its per-element function captured something that moved, which is exactly what happens when
`lambda t: t.owner == session.actor` meets a different session — throws its arrangement away and
emits inserts. It has no previous arrangement left to derive removals from. A downstream operator
that merely applied those inserts therefore kept every entry the rebuild had dropped, and **a
subscriber who switched sessions saw another subscriber's rows on top of their own**. Every
event-by-event comparison passed, because within one session nothing ever rebuilds. The fix is that
`rebuilt` propagates downstream; the finding is that it took one test that changed the session on a
warm engine.

**Then the same flag, stale, hid a performance defect behind that correctness oracle for a whole
report.** `map_list`, `filter_list` and `sort_by` had an early return for the case where nothing
arrived and nothing forced a rebuild. It cleared `changed`, cleared `changes`, and **did not clear
`rebuilt`** — so the flag silently came to mean "has ever rebuilt", which is true of every operator
after its cold start, and **every operator below a collection that had stopped changing rebuilt on
every event for the life of the subscription**. It is a performance defect and not a correctness one
— a rebuild produces the right answer — which is why nothing caught it. On the sketch with eight
subscribers over 200 todos, one event: **1,137 applications before, 66 after.**

Two things about how it was found, because neither was a review of the code. The single-subscriber
measurement *could not have shown it*: the sort does change on the event being measured, so the flag
is never read in the stale state. It appeared when the fanout was measured, and only because the
per-subscriber numbers were **printed rather than summed** — subscriber 0 did 4 applications and
subscribers 1–7 did 51 each, and there is no correct engine in which the subscriber whose page *did*
change is the cheap one. **An aggregate is where a defect of this shape goes to hide.**

**The version a page reflects was a latent bug, and the shared dataflow made it visible.** The state
was read under a lock and the head was not, so a frame could be labelled with a version newer than
the page it carried whenever an event landed in between. A frame's `seq` is what a resuming client
asks for the difference from: the server would diff `view(seq)` against the current view and send
patches the client applies to a page that was never `view(seq)`. **A wrong DOM, one reconnect later,
from a race with no symptom at the time.** Both halves are fixed together — the version is read under
the same read lock as the state, and the render returns the version it actually served, which may be
*newer* than the one asked for. Serving the newer page is deliberate: unwinding an arrangement to an
older version would need a history of values rather than of changes. What makes it safe is that the
number comes back to the caller.

**A plan contained operators nothing could reach, and they came from dictionary passing.** The
decomposition builds an operator for every argument of a call it inlines, before it knows whether the
body reads them — and a bounded definition's arguments include one **dictionary per method of each
bound**. A trait declaring two methods, a definition calling one, two call sites: two operators for a
method the body never mentions. They were harmless — a pointwise operator nothing reads is never
evaluated — and they were **counted**, in every operator total any report ever quoted. The fix is a
reachability pass at the end of decomposition, and the roots are the page, the two sources, and
**every named signal**, because a name is a read-model table whether or not the page reads it.

**A rule that could not fire was written, found and deleted.** `flat_map over map_list` looked
necessary — a `for` over a view of a view is a chain of maps under a flatten — and never fires,
because the pass scans in dependency order and restarts after each rewrite, so a chain of maps is
always collapsed from the bottom before the flatten above it is reached. The published rule list is
now held to the programs that reach it: **each rule must be exercised by a program somebody can
open**, asserted as set equality rather than as a checklist. A rule with no program is a rule the
differential harness says nothing about, sitting in the module looking like coverage.

**An operator the engine implements was reached by no program in the tree — and the fusion pass is
what took the last one away.** `flatten` had exactly one shape in the whole repository: the
`map_list` under it that every `ui:` loop compiles to. Fusing that pair means `flatten` survives only
when its collection of lists came from somewhere else, and no program had one — so the arm the engine
runs it with stopped being exercised, silently, in the same commit that made the rewrite. The
operator list is held to the programs the same way the rule list is. **That gate then found a hole
the work did not make**: `list_is_empty` has never reached a plan at all, because the corpus writes it
twice and both are inside an `if`, which is one opaque operator — so the engine's emptiness arm had
never once been compared with recompute.

**A refusal outlives the operator it was recorded against.** The first version of the fusion pass
dropped a refusal whose consumer was later fused into something else, which is exactly the case a
developer most needs to see: the shared `map_list` is refused, the per-session map above it is
absorbed by the loop, and the only line explaining why the shared half stayed shared vanishes from
the report. Refusals are carried forward to whichever operator absorbed the one they were recorded
against, and dropped only when the pair they name actually fuses.

## 23.17 The corrections this makes to the design documents

| Document | Correction |
|---|---|
| [`03`](03-type-and-effect-system.md) §3.7 | Several `durable` folds are one accumulator, not several logs — "one totally-ordered log per application" now has a compilation rule attached |
| [`03`](03-type-and-effect-system.md) §3.8 | "`remaining` updates by ±1 per event, never by recount" is built and measured. The *view* is not thereby `O(δ)`: assembling the page's children is still linear. And the unit of "incrementalized where analysis allows" is an **operator**, not a view — a view is not incremental-or-not, parts of it are |
| [`04`](04-compiler-architecture.md) §4.3 | Every signal edge crossing tiers is enumerated with a content-derived id, rather than one crossing named in prose. And a patch frame's `seq` is the version the *page* reflects, which is not always the log head when the frame is written (§23.16) |
| [`04`](04-compiler-architecture.md) §4.7 | `beck explain query` and `beck explain cost` are built, and both take a **file** rather than a function: a plan is a property of the program's page, and a program has one page |
| [`05`](05-tier-lowering.md) §5.3 | The shared dataflow is a property of **operators, not of signals** — a computation inside a `per_session` view can be session-independent and therefore shareable. It is advanced **lazily by the first subscriber to render at a new version**, not under a lock the sequencer holds. "Read models: generated tables in the same Postgres" is superseded by [`10`](10-decisions.md) D26; the right-hand column is met. "Query fusion is not built" — it is, on the dataflow plan; the `Query` sub-language the same paragraph names is still symbolic and unwritten |
| [`07`](07-dependencies.md) §7.4 | The pgwire row's "no alternatives" meant no alternative *protocol*. There are alternative implementations and one was refused |
| [`12`](12-standards-and-conformance.md) §12.5 | "verified against `psql`, JDBC and BI drivers in CI" — one Rust driver, and `psql`'s backslash commands do not work |
| [`19`](19-phase-1-report.md) §19.9, [`20`](20-phase-2-report.md) §20.5 | The claim that the narrow splitter "never quietly mis-sliced" was false; §23.2 is the program |
| [`38`](38-literature-survey.md) §38.2 | Two **adopt** verdicts cashed. The reader-frontier estimate held exactly. The fusion trigger fired and the answer is half of what it forecast: the shape is adopted, the e-graph is not, because these rewrites do not conflict |
| [`43`](43-threat-model.md) §43.4 | A new absence: no authentication on the read-model port, with the loopback bound as the compensating control |
| [`67`](67-sqlite-report.md) §67.1 | "this substrate is what read models would be built on" — it is not, and D26 is why. The transaction property is still available and still unused |
| `AGENTS.md`'s verification list | Its rustdoc step never ran: `RUSTDOCFLAGS` was assigned an unquoted value beginning with a flag, so the shell ran the next word as the command. Quoted, and gated |

## 23.18 The gates

- **`incremental_engine.rs`** — every corpus program, every event of a generated log, maintained page
  against recomputed page, byte for byte, for two subscribers: **745 events, 3,080 pages**. Plus the
  cold-versus-warm, render-at-an-older-state and did-it-actually-decompose properties of §23.7.
- **`shared_arrangements.rs`** — everything that is only true of sharing. Every lag from 1 to 9 and
  one of 200 (past the history's end, so the rebuild path) over every corpus program: **5,496 pages**.
  A late subscriber against one there from the start; three renders at one version; a subscriber
  asking for a version the shared side has passed; and three sessions interleaved differently at
  every version on programs whose filter *is* the session — which is the failure a shared arrangement
  makes newly possible.
- **Eight lifecycle tests**, and the first two are over the whole corpus, because a dropped
  arrangement that was still needed is not a crash but a page quietly one row wrong. Three
  subscribers arriving and leaving at different points, the middle one departing halfway to compact
  the history the survivors are using; a released dataflow rendering **twice**, because one that
  reset its arrangements and kept its version would hand out deltas against arrangements that are
  gone; compaction past a reader that still needs it, and a laggard leaving without releasing what
  was kept *for it*; §23.11's frontier-zero case; and both sides of the release policy, so neither is
  folklore.
- **`fusion.rs`** — the fused plan against the unfused plan over the same generated log, warm, two
  subscribers, every event: **more than 2,000 pages**, over every program in `corpus/` plus the
  todo sketch, the board and the fixtures written for the rules and operators the corpus does not
  reach, so a failure names the
  rewrite rather than the engine. Plus set equality on the rule list and the operator list, and both
  refusal conditions asserted on programs built to make each bite — they are *pessimisations* when
  dropped rather than errors, so nothing else would see them. The differential runs **recompute as a
  third plan**, because a fixture checked only against the unfused plan would agree with it about a
  shared mistake.
- **`scaling.rs`** — the read model costs nothing per event and a delta per query, at 200 rows and at
  1,600, with a 3× bound rather than a rate.
- **`view_metrics.rs`** — the gauges, through a running process with real sockets rather than through
  the engine's own API.
- **`differential.rs`**, the project's conscience, drives the shared path: the edit was two lines,
  because the *application* decides which engine a subscriber gets.

[`82`](82-the-edge-report.md) §82.10 asks what would have to be true for a
gate to go red, and to check rather than assume. Four mutations were applied to the shipped fusion
pass and reverted:

| mutation | what went red |
|---|---|
| drop the "nothing else reads it" check | `an_arrangement_two_operators_read_is_not_fused` |
| drop the session-cut check | `fusion_does_not_move_shared_work_per_subscriber` |
| let the count rule fire over a `filter_list` — cardinality-changing, and therefore unsound | the differential, on `examples/todo.beck` at event 12 |
| give `flat_map` `flatten`'s rebuild rule, ignoring that its function has captures | the differential, on `corpus/27-review.beck` at event 6 |

The last two are the ones worth having done: both produce a plausible plan and a wrong page only on a
program whose loop reads the session or whose collection shrinks, and **neither would have been found
by reading the rule**.

One thing is **written for a reason and gated by nothing**, and it should be said rather than left to
be assumed: the conjunction's short-circuit. No view in the tree has a predicate that can fail, so
the strict spelling would render identical pages on every program there is, and no mutation of it
goes red. It is written the short-circuiting way because that is what the pair of operators did — and
the day a program has a fallible predicate, the difference is between a page and an engine that reset
itself.

## 23.19 What is not built

| | Status |
|---|---|
| **The page streamed as deltas** | **Not built**, and it is the remaining `O(n)` (§23.8). The delta at the top of the plan *is* the patch set; emitting patches from it would skip both the assembly and the structural diff. It is a protocol change and a client change rather than an engine change, and it is the same work Mode B's per-component kernel needs |
| **The render lock** | **Unchanged, and deliberately.** Subscribers render under a read lock, so the advance waits for the slowest render in flight. That is right for readers and it is not a design that has been run at a fanout where it would not be. The lifecycle work made it *more* load-bearing rather than less — compaction is safe partly because a render holds the read lock for its whole duration — so replacing it is now a change to two things at once, and this says so before somebody does it |
| A grace period before releasing | **Not built.** It needs elapsed time, which is not on the clock seam. The switch is the whole policy today: release immediately, or never |
| A measured knee for the history depth | **Still not measured.** 64 is a guess; it is now a ceiling that binds only past the laggiest reader, so the guess costs much less |
| Bytes per retained version | **Not measured**, and §23.11 says why one program cannot say it |
| Partial state and upqueries | **Not built.** [`38`](38-literature-survey.md) §38.2's **borrow** verdict on Noria — evicted arrangements refilled on demand — is the finer-grained version of the lifecycle. What is built is all-or-nothing per dataflow |
| Per-operator lifecycle | **Not built.** An operator no *connected* subscriber reads is still maintained if any operator does |
| A gate on any of the exported numbers | **Not built.** A subscription over a thousand rows is 5.4 MB and nothing covers that case |
| Joins, subqueries, `group by`, aggregates other than `count(*)`, `distinct` | **Not built in this surface**, and the reason is below the SQL rather than in it. The plan now **has a join** ([`99`](99-the-data-tier-means-of-combination.md) §99.6, `Op::Join`), inferred from a loop that looks something up or filters another collection by an equality rather than written as one, over either a unique index or an `arrange_by`; what the read-model SQL still lacks is the compilation *into* it, which is §99.9 item 9. The plan now has all four aggregates too — `count` from a tally the join keeps, `min` and `max` from `Op::GroupBy`'s multiset per group, and `sum` from a running total the same operator keeps instead of one (§99.9 item 6) — so what is missing from the plan itself is `distinct` and a `group by` that answers with the groups rather than with a question about each; for those there is still nothing to compile into. The view algebra's combining forms were all unary until the join — every other operator takes one collection, and `concat_lists`, the one that takes several, unions same-typed streams rather than relating them. [`99`](99-the-data-tier-means-of-combination.md) is the design that closes it, and §99.2 argues the absence is an oversight rather than a decision. When the plan has them, this surface grows by compiling *into* it rather than by growing a second interpreter |
| `count(*)` without scanning | **Built, ungrouped.** `select count(*) from t`, with nothing narrowing it, is answered from the collection's own size: a maintained arrangement is a `BTreeMap` and a `Map` in the accumulator knows its length, and neither was being asked — the query cloned every value and built a `Cell` per column of every one to answer with an integer. `read::Rows::count` is the seam and it defaults to "not without a scan", so a reader that does not implement it is exactly as correct and exactly as slow as it was. Gated by `read_models.rs::a_bare_count_is_answered_without_building_a_row`, whose instrument is a reader that knows the size and **refuses to produce a row** — a query that scanned cannot be answered by it at all. `a_count_that_narrows_anything_still_scans` says where the fast path stops. The *grouped* form is not a SQL question at all but [`99`](99-the-data-tier-means-of-combination.md) §99.4's aggregation half, which the **plan** now has for `count`, `min` and `max` and this surface still cannot reach — §99.9 item 9 is the compilation into it |
| `pg_catalog`, and therefore `psql`'s `\d` | **Not built.** `select * from beck_columns` is the substitute, and it is a table this project invented rather than what any tool expects. The correct long-run answer is a small read-only emulation of `pg_class` and friends — which needs joins, so the two are the same item |
| TLS and authentication on the read-model port | **Not built**, and the port is loopback-only and off by default because of it |
| Writes through pgwire | **Refused by name**, at any privilege. The log is the only way state changes |
| A durable projection | **Not built, and now deliberately so** ([`10`](10-decisions.md) D26). What would reopen it: a read model that has to survive the process, or be reachable by a tool that cannot reach this port |
| An e-graph and cost-based extraction | **Not built, and not needed by these rules.** What would need one is a rewrite that can *add* an operator or that competes with another for the same node — pushing a `filter_list` below a `sort_by` is the first such: sound, sometimes a large win and sometimes a loss, which is exactly the choice equality saturation exists to make |
| `filter_map` fusion; `list_len` over a `filter_list` | **Not built.** The first is one new operator and no program in the tree has the shape; the second is not a fusion at all, since a filter changes cardinality and the count would have to be maintained as a threshold over the predicate |
| Fusing across the session cut when the fanout is one | **Not built and not planned.** Right for a single subscriber, wrong for the second one, and the plan is compiled once per program |
| **Nothing in the language says "publish these read models"** | The flag is a runtime decision, so [`06`](06-kubernetes-and-packaging.md) §6.5's argument — that what a program exposes is derived from what the program says — does not apply to this port. There is nothing to derive it from. An effect atom or a signal annotation is what would give it one, and that is a language decision |
| A BI tool | **Nothing has been tried.** [`12`](12-standards-and-conformance.md) §12.5 claims verification against `psql`, JDBC and BI drivers in CI, and this delivers one Rust driver |
| Interception through a closure | **Unchanged.** A function stored in a record and called through a field cannot be stubbed, and `plan.rs` cannot see through such a call either, so it becomes one opaque operator. The fix is still naming closures at their binding site |

**The engine is only as general as its decomposition**, and the ratio is a fact rather than an
impression because `beck explain incremental` names which operators are which, per program. Some of
what is recomputed is correct — most corpus views compute a scalar from a map and have no collection
to maintain — and some is the fallback firing on a `match` or an `if` that differential dataflow does
handle by branching the plan.

**And a conservative equality is a language-level decision hiding in an engine.** `engine::same`
compares collections by pointer because a deep comparison would cost what the engine saves. That
works because `Map[K, V]` is persistent and shares structure — a data-structure decision made two
phases earlier for the fold's asymptotics is what makes change propagation cheap now. The same will
be true of `list`, which is *not* persistent, and every list-shaped input to a pointwise operator
pays for it.
