# 99 — The data tier's means of combination

> **The join, both of its indexes, and all four aggregates are built; `distinct` and difference are
> not.** §99.9 items 1, 3, 4, 5 and 6 have landed — the shape gate, `arrange_by`, the `Join` operator
> with §99.5's bilinear delta rule, the recognition that emits both for the loop a program already
> wrote, and `count`, `min`, `max` and `sum` per group, the last three as `Op::GroupBy`, which
> answers a question about a group without the group existing. `sum` is the one that owed a decision
> rather than an operator, and the decision is that **a sum is its answer and not the order it was
> added in** — exact over `Int`, raising only when the total does not fit, and absent over `Float`,
> because there the same definition would disagree with the `+` the language already has. What is
> left is `distinct` and difference, fusion for the new operators, and the read-model SQL compiling
> into the plan. §99.9 is where each of them sits. Everything below is the design; what is built says
> so where it is built.
>
> [`25`](25-benchmarks-and-expressiveness.md) asked
> whether Beck has "Scheme's full means of combination and means of abstraction" and answered it for
> the *language*, with SICP as the measurement. Nobody has ever asked it of the **data tier**, and
> the answer is worse: the view algebra has primitives and it has means of abstraction, and its means
> of combination are **unary**. Every operator the engine implements takes one collection. The only
> n-ary one, `concat_lists`, is a union of same-typed streams — the one operator that does not
> *relate* its operands.
>
> A relationship between two collections is therefore not expressed in the algebra at all. It is
> expressed by **escaping** it: the loop body reads the accumulator, the per-element function
> captures the state node, and the whole collection is reapplied on every event. That is a
> nested-loop join, recomputed in full, per event, and the compiler already prints it (§99.3).
>
> This document says what is missing, states it as an algebra rather than as a feature list, makes
> the four decisions that have to be made before any operator is written, and puts the work in
> dependency order. §99.9's first item is a gate rather than an operator, and it was written to go
> red on `27-review.beck`: **415 units of maintenance at 200 notes and 3,215 at 1,600**, which is the
> nested loop stated as a number. With the operator it is **19 at both sizes**, and the gate measures
> both settings so that number is a difference rather than an assertion.

## 99.1 The question this answers, and the one it does not

The question that started it was "should the compiler push set operations into Postgres, since
Postgres is fast at them". That question is answered elsewhere and the answer is no, for a reason
that has nothing to do with speed: **the rows are not in Postgres.** The data tier is the fold/view
engine ([`03`](03-type-and-effect-system.md) §3.7), Postgres is where the *log* lives
([`05`](05-tier-lowering.md) §5.3), and the state is a value in the engine's memory. Putting a
relation in the store so a join could run there is a durable projection, which is
[`10`](10-decisions.md) D26 — refused, with reasons that have not weakened.

What survived that exchange is a smaller and more embarrassing question. Beck cannot push a join
down **because Beck has no join**. Nor `group by`, nor an aggregate other than a count of an entire
collection, nor `distinct`. Three documents record this as a status row
([`23`](23-incremental-views-report.md) §23.19, [`23`](23-incremental-views-report.md) §23.19,
[`12`](12-standards-and-conformance.md) §12.5) and **none of them records a reason**, because there
has never been one.

## 99.2 Oversight or intent: the evidence, since the two look alike from outside

This project refuses things constantly and it always shows its working. A durable projection is
refused with three arguments and a decision number (D26). Active-active writes to one key are
refused and CRDTs are named as the boundary ([`15`](15-scale-and-distribution.md) §15.2). `aggregate`
as a keyword is refused with a paragraph about inverting the relationship between construct and
convention ([`29`](29-domain-driven-design.md) §29.3). A `per_session` view as a read-model table is
refused in four words that are nonetheless a reason — "a SQL client has no session".

Joins have **no argument anywhere in the document set**. Not a paragraph, not a sentence, not a
decision number. Three "not built" rows and silence. In a corpus of documents this argumentative,
that is the signature of something nobody looked at, not something anybody decided.

The mechanism by which it went unlooked-at is worth stating, because it is not carelessness and it
will recur:

1. **The state is one value.** `durable(fold(...))` yields a single accumulator and every collection
   is a field inside it. So a relationship is written as an id inside a record and resolved with
   `map_get` — ordinary code, in a pure function, which type-checks and runs and renders the right
   page.
2. **Nothing was ever inexpressible.** No diagnostic fires, no refusal is printed, no program fails
   to compile. The wall that [`23`](23-incremental-views-report.md) §23.2 describes — "a refusal that is
   never exercised on the shape it exists to refuse is a claim, not a check" — is worse here: there
   is no refusal at all, only an asymptotic cliff with nothing standing at the edge.
3. **The corpus never grew a relationship big enough to hurt.** Thirty-three programs when §99.3
   swept it, all small.

So: an oversight, and specifically an oversight of **cost**, not of expressiveness. Everything a
developer wants to say can be said. What cannot be said is said *slowly*, and the slowness is
invisible until the collection is large — which is the class of defect
[`AGENTS.md`](../AGENTS.md) names first: "a performance defect in the semantics survives into every
backend".

## 99.3 What a relationship costs today, measured

[`corpus/27-review.beck`](../compiler/corpus/27-review.beck) contains a join. It was not written as
one — it is a loop over `notes` whose body asks `verdicts` about each element:

```python
for e in submissions(s):
    li:
        span: (payload(e).text + " — by " + e.actor)
        span: verdict_for(s, payload(e).id)      # ← the join
```

The plan says what that costs:

```console
$ beck explain cost corpus/27-review.beck
  #14  map_values     δ touched  —  O(δ log n), the persistent map's own diff
  #15  sort_by        δ applications, at most 2δ touched — a move is a remove and an insert
  #16  flat_map       δ applications, then the entries of each changed element's list
                      n applications on every event — its function captured #0, which is
                      downstream of the state
  #17  recompute      1 recompute + n entries copied, forcing #16

  2 of 29 operators cost O(n) per event, for two reasons:
    #17  a recompute needs a `list` and an arrangement is a keyed collection —
         docs/23 §23.8's remaining constant factor.
    #16  a per-element function captured the state, so the whole collection is
         reconsidered on every event — docs/99 §99.3, and the algebra has no
         operator for what this program is doing.
```

`#0` is the state (`beck explain query` names it: `#0 state shared `queue``). **The state moves on
every event.** So `#16` reapplies the loop body to every note, every time anything happens — the
definition of a nested-loop join with no index and no incremental maintenance.

Three things about that transcript are the actual finding.

**The compiler printed the defect and did not count it.** The summary said "1 of 29 operators cost
`O(n)` per event" and named `#17`. `#16` is also `O(n)` per event, for an unrelated reason, and it
was excluded — [`plan.rs`](../compiler/crates/beck-core/src/plan.rs)'s tally collected operators
whose cost string contained `n entries copied`, and the capture line was written separately, after
the count. The headline number was wrong on the one program in the corpus where it matters most,
and wrong in the reassuring direction. **Fixed**, and the transcript above is the fixed one: the
summary is now derived from the same per-operator record the body is printed from, so the two
cannot disagree again, and `incremental.rs::the_tally_counts_every_line_the_report_prints` reads
both numbers *out of the printed text* rather than recomputing either from the plan.

**The capture line was written for a different case and silently covered this one.** Its own comment
said so: *"It is not per event — a session is constant for a subscription — but it is the one place
δ stops bounding the work."* That is true of a captured **session** and false of a captured
**state**, and the line distinguished neither — it named the node it captured, not what moves that
node. **Fixed**: every operator now carries the cadence of what it captured, computed by tracing
inputs back to a source in one pass over the plan's dependency order, and the three cases print
three different sentences —
`incremental.rs::a_capture_says_how_often_what_it_captured_moves` builds one program per cadence and
asserts they differ from each other as well as from what each should say. Sweeping `beck explain cost` for the capture line across the corpus and the examples and resolving
each `#k` in `beck explain query` gave **18 capture sites in 10 programs**, of which three moved per
event: `27-review` (`#0`), `board` (`#0`) and `32-here` (`#4 ← #3 ← #0`). The third — `32-here`'s
`#4` — is why item 2 of §99.9 was not cosmetic: it is a `recompute` node, so classifying it takes
**tracing its inputs transitively back to the accumulator**, which the printed line did not do and a
reader has no reason to do by hand.

**Re-run after the join (2026-08-17), the same sweep gives 16 sites in 8 programs and one that moves
per event:**

| what the captured node is | sites | programs | when it moves |
|---|---|---|---|
| a `const` | 9 | `04-kanban` (6), `05-poll` (3) | never |
| a definition used as a value, no inputs | 2 | `28-catalogue` | never |
| the `session`, or derived from it | 4 | `02-chat`, `31-tenants`, `todo`, `routed` | per route change — benign, as the comment says |
| the **state**, or derived from it | **1** | `board` (`#0`) | **every event** |

Two numbers in that comparison are worth separating, because only one of them is the operator's
doing. `27-review` and `32-here` had the shape §99.6 recognises and lost their captures to it, which
is the point. **`33-awareness` is the other number**: it had the shape too, it was never in the
eighteen, and it was not missed — it did not exist, arriving with `awareness(f)` one change after
this document was written, and nothing re-ran the sweep. That is [`08`](08-roadmap.md) §8.5.6's third
decay direction, a *quoted figure going stale because the tree grew under it*, demonstrated one
commit after the paragraph describing it.

**The sweep is now zero, and it is a gate rather than a sweep.** `arrange_by` (§99.9 item 3) took
`board`, which was the grouping the row above was waiting on, and nothing has replaced it:
**42 programs across `corpus/` and `examples/` plan, and none of them reapplies a collection on
every event — against 8 sites in 8 programs with the recognition switched off.** Every capture left
in the tree is a `const` or the `session`; not one is downstream of the accumulator.

That second number is the half that makes the first mean anything, and it is the reason this is
`incremental.rs::no_program_in_the_tree_reapplies_a_collection_per_event` rather than a fourth
reading of the same table. **A figure in a document cannot notice a new program.** This one was
read by hand three times, went stale in between twice, and the staleness was always in the
flattering direction. `Plan::reapplied_per_event` is the property, and `beck explain cost` prints
its lines from the same computation the gate counts — §99.9 item 2's lesson, applied to a second
reader rather than rediscovered.

**And the analysis used to disagree with the plan.** `beck explain incremental corpus/27-review.beck`
reported `flat_map ×1 maintained` while `beck explain cost` reported `n applications whenever #0
moves` for the same operator: maintained in the sense the analysis means — the operator has a delta
rule — and reapplied wholesale anyway, because its *function* moved rather than its input. That was
the second instance of the disagreement [`23`](23-incremental-views-report.md) §23.8 caught once for
`html_el` and said out loud. `27-review` compiles to a `Join` now and the two commands agree about
it; what the episode leaves behind is the shape rather than the case, and the gate above is what
would say so next time.

## 99.4 The algebra, stated as one

Everything the engine implements ([`plan.rs`](../compiler/crates/beck-core/src/plan.rs)'s
`OPERATORS`), against the relational basis it would take to be complete:

| operation | Beck today | note |
|---|---|---|
| projection (π) | `map_list` | ✓ |
| selection (σ) | `filter_list` | ✓ |
| union (∪) | `concat_lists` | ✓ — the one n-ary operator, and it relates nothing |
| ordering | `sort_by` | ✓ and better than the basis asks: the order **is** the arrangement's key |
| nesting / unnesting | `flatten`, `flat_map` | ✓ |
| cardinality | `list_len`, `list_is_empty` | ✓ over a whole collection, `±1` per delta |
| **join (⋈)** | `Join`, recognised | **built** — an outer equi-join against an index whose key is unique, which is what an arrangement is (§99.5 decision 2). No syntax: §99.6 |
| **grouping and aggregation (Γ)** | `ArrangeBy`, `Join`'s tally, `GroupBy` | **built** — the group as rows, its `count`, and `min`, `max` and `sum` per group, each answered without the group being assembled. Recognised from the loop that already asked (§99.6). What is missing is a *general* aggregate: a fifth one needs an operator, not a parameter |
| **difference (−)** | `Restrict`, recognised | **built** — the rows of one collection whose key another does not answer, and the intersection that is its complement. The one binary operator whose output is one of its inputs, which is what decision 2 meant by "no representational change at all". No syntax: §99.6 |
| **distinct (δ)** | `Distinct`, lowered | **built** — the values a collection holds, each once, at the input key of its first occurrence. The last row of this table, and the only operator here with no recognition at all: `list_unique` **names** it (§99.9 item 7) |

Read as SICP's three-part test, which is the standard [`01`](01-vision-and-premise.md) §1.1 sets and
[`25`](25-benchmarks-and-expressiveness.md) measures the language against:

- **Primitives** — sound. An arrangement is an ordered map from a composite key to a value, deltas
  are entry moves, and the key composes (`Key = Arc<[Value]>`; `flatten`'s key is its input's key
  followed by a position).
- **Means of abstraction** — sound, and genuinely good. A developer names a derived collection and
  the compiler shares it between subscribers ([`23`](23-incremental-views-report.md)) and projects
  it as a relation an outside SQL client can read ([`23`](23-incremental-views-report.md)),
  with no annotation.
- **Means of combination** — this was the gap, and stating it this way is what made it obviously a
  gap rather than a missing feature: an algebra whose combining forms all take one operand cannot
  express a relationship between two things, so every relationship in every program was expressed
  by leaving the algebra. Four operators have closed it — the join, the group, the difference and
  `distinct` — and three of the four were recognised from a program that had already written the
  relationship down without one. The fourth is the exception that says what the other three cost: it
  needed a **name** rather than a recognition, and until it had one the same question was a fold
  nothing could see into.

## 99.5 Four decisions to take before any operator is written

Each of these is load-bearing, each has alternatives, and each is expensive to revisit once an
operator depends on it. Three of the four should be ADRs. A fifth — **how the compiler chooses
between plans, and how that choice becomes correct rather than merely deterministic** — is §99.8,
and it is separated because it is about the solver rather than about the operators' shape.

### 1. What orders a binary operator's output — and it cannot be "nothing"

The textbook incremental algebra (DBSP, differential dataflow —
[`38`](38-literature-survey.md) §38.2, [`07`](07-dependencies.md) §7.4) works over **unordered**
Z-sets and sorts at the end. Beck cannot: [`23`](23-incremental-views-report.md) §23.13 is explicit that
"the key is what makes iteration order a consequence of the plan rather than of a sort at the end.
The order reaches the rendered page and the replay digest." A rewrite already owes three obligations
— same values, same order, same deltas — and an operator owes them too.

**Recommendation**: a join's output key is the **left key followed by the right key**, which makes
its iteration order a consequence of its inputs' orders exactly as `flatten`'s is. This is the rule
the existing keys already follow, it needs no new machinery, and it gives left-order-major
iteration, which is what a `for` loop over the left side already means. Write it down before it is
discovered, because the alternative — sort at the end — silently changes the replay digest.

**Taken, and the built operator's key is the left key alone** — which is the same rule rather than a
departure from it. The right side is an *index*, so its key is the join key and a left row matches
at most one right row; appending a component that the left key already determines would buy nothing
and would make an **unmatched** row's key shorter than a matched one's, which is an ordering that
depends on whether a lookup succeeded. Iteration is left-order-major either way, and that is the
half the page and the digest can see.

**`arrange_by` has landed and the rule did *not* come back, which is worth saying because this
paragraph predicted it would.** The forecast was that an `arrange_by`'d right side makes one left row
match several, so the output would need the right key as a second component. It does not, because
what a left row matches is the **group** rather than the rows: the expression the operator replaces
is `filter_list(…)`, which evaluates to a `list`, so one left row still produces one output row whose
right half is that list. The right key does its ordering work one level down instead — the index is
keyed by the join key *followed by the collection's own key*, which is what makes a group come back
in the order the collection held it, and that is the same rule applied to the index rather than to
the output. It comes back for real when a group is *expanded* into rows, which is `group by`
(§99.9 item 6) and not this operator.

### 2. Whether multiplicities are needed, and the answer is "for two of the four"

Difference and `distinct` are the operations that classically force signed multiplicities. Beck's
arrangements are **keyed**, not bags: one value per composite key, keys unique by construction. So
difference *by key* and union *by key* have delta rules with no representational change at all.
`distinct` on **values** does not — it needs a count per distinct value, which is a new kind of
arrangement.

**Recommendation, and taken**: build join and grouping on the existing keyed representation, and
treat `distinct`-on-values as a separate, later question. Do **not** adopt Z-sets wholesale to get two
operators; the ordered-key property in decision 1 is worth more than basis minimality.

**The difference is built and this decision was right about it, including about the part that
sounded like a slogan.** "No representational change at all" turned out to be the operator's whole
design rather than an observation about its state: `Op::Restrict` is the one binary operator whose
**output is one of its inputs**, so what a consumer reads after the rewrite is what the
`filter_list` handed it, entry for entry and key for key. That is also the reason a filter can have
this operator when it cannot have a join (§99.6): a join emits a *row*, and rewriting a filter into
one would need a projection underneath to hand the element back — an operator per element per event,
undoing what the rewrite just did. `incremental.rs::a_difference_and_the_intersection_beside_it_are_one_index_and_no_rows`
counts that rather than asserting it, by holding the recognised plan to the refused plan's number of
per-element operators.

**And `distinct` is built, on a representation this decision was right to say it would need and
wrong about where it would come from.** The recommendation was to treat `distinct`-on-values as a
later question because it needs "a count per distinct value, which is a new kind of arrangement".
It does need one, and it was **not** new by the time it was wanted: `Op::GroupBy` had been keeping a
multiset per group since `min` and `max` landed. What `Op::Distinct` keeps is that idea with the
count replaced by the *keys* — the input keys holding each value, ordered, so the smallest of them
is where the value is published. So the thing this decision deferred turned out to cost a name
rather than a representation, which §99.9 item 7 says at length.

### 3. One clock, and where that stops being true

Differential dataflow carries a timestamp lattice because its inputs move independently. Beck's do
not: every collection in a plan comes from one accumulator (`Op::State` is "the plan's one source"),
which moves atomically at one `seq`. **A join in Beck needs no timestamp lattice**, and saying so
now is what stops a port of the literature from importing machinery this design does not need.

The exception is already documented and should be named here rather than rediscovered:
`presence()` moves when `seq` does not ([`48`](48-identity-report.md) §48.13).

**That exception was expected to force a refusal, and it does not — this paragraph used to say a
join against a roster was out of scope and should be refused with a diagnostic.** It is not out of
scope, and building it is what showed why: `corpus/33-awareness.beck` looks up in the awareness
roster *and* in the accumulator, and both are joins. The second clock is a problem for **sharing**,
not for **joining**. Everything downstream of `Op::Awareness` is already per-subscriber, so a join
that reads one is per-subscriber too, and inside a single subscriber's engine the index and the left
side advance in the same tick whatever provoked the render — which is all a delta rule needs. The
second clock stays unbuilt and stays owed for the reason §48.13 gives, which is that the *roster*
cannot be held once between subscribers; nothing about it stops the roster being joined against.
`incremental.rs::a_loop_that_looks_up_twice_becomes_two_joins_and_captures_nothing` is the gate, and
the differential drives the shared dataflow as well as a standalone engine, which is where a wrong
answer about this would show.

### 4. Indexes are a second arrangement, and the sharing already exists

A join needs its right side keyed by the join key, which is not the ordering key. That is
differential's "arrangement" in the strict sense, and it is a second index over a collection that is
already arranged. The good news is that the hard part is built: `26`'s sharing already holds one
arrangement for many consumers, and an `arrange_by` operator is a node like any other, so two joins
on the same key share one index by the mechanism that already exists.

The cost is memory, per index, and [`23`](23-incremental-views-report.md) §23.14 already exports
per-subscriber memory — so the metric to hold this honest exists too.

**The first join needed no new operator for it.** The shape it recognises indexes a `Map` field of
the accumulator, and `map_values`'s arrangement is *already* keyed by the map's key, which is the
join key — so the index is an operator that existed, and hash-consing shares it with any other reader
of the same collection without anything being added for the purpose.

**`arrange_by` is now built** ([`plan.rs`](../compiler/crates/beck-core/src/plan.rs)'s
`Op::ArrangeBy`), and the paragraph above was right about the hard part and silent about a small one.
The sharing did come for free — an index is a node like any other, and two joins wanting the same one
get the same node. What it takes that was not foreseen is that sharing on a *key function* needs the
function in the hash-consing key, and `Core` is not `Eq`: `relate::fingerprint` is the structural
digest that supplies it, deliberately wrong in the direction that costs an index rather than an
answer. The memory is one entry per element of the
collection indexed, in a **shared** arrangement rather than a per-subscriber one, so §23.14's
per-subscriber metric is not where it shows.

**The safe direction was not free, and the first program with two lookups on one key found it.**
`Core` numbers variables per definition, so `lambda b: b.lot` and `lambda c: c.lot` — the same key,
written twice — reached the hash-consing key as different strings and built **two identical
arrangements**. `relate::fingerprint_fun` writes the key's own parameter canonically, which is
enough because an index key reads nothing else: the recogniser refuses the shape otherwise.
`incremental.rs::two_lookups_by_the_same_key_share_one_index` is the gate, and it is two *different
questions* about one collection rather than one expression written twice, because the second would
have been folded before the plan ever saw it.

**And the operator was already written, which is the finding worth keeping.** `arrange_by` and
`sort_by` build the same arrangement — an element keyed by `f(x)` followed by the input's key — and
the engine runs one function for the two. A sort is that arrangement *iterated*; an index is that
arrangement *probed*. They stay two operators because nothing may fuse a probe the way it fuses a
sort, and because `beck explain query` should not tell a reader their program sorts when it does
not.

## 99.6 The surface: infer the join, do not add syntax

Beck's personality is that the compiler works out what a program means and shows its working —
placement is inferred, the session cut is inferred, fusion is automatic. A join should be the same.

`for x in xs:` whose body contains `map_get(ys, k(x))` **is** an equi-join, and the decomposition in
[`plan.rs`](../compiler/crates/beck-core/src/plan.rs) has everything it needs to see one: it already
computes the per-element function's free variables and knows which of them are plan nodes.

**Built** ([`relate.rs`](../compiler/crates/beck-core/src/relate.rs)). `27-review.beck`, `32-here.beck`,
`33-awareness.beck` and `examples/board.beck` compile to joins with no edit to any of them,
`beck explain query` prints them and what orders them, and `beck explain cost` reports a per-event
capture for none of them.
**Several lookups in one body are several joins, chained** — a row that shows two related things is
an ordinary page, and refusing it would leave the capture in place and buy nothing. Each join takes
the previous one's rows on its left, so the row a body finally reads is nested and the cost of the
chain is memory rather than time (§99.5 decision 4). The condition is stated as what an
expression *reads* rather than as a shape — `m` reads only what the loop captured, `k` reads only the
element — and the recognition fires only when the rewrite removes a capture, so an index is never
built for nothing.

**`examples/board.beck` needed the second shape, and now gets it with no edit either.** Its loop is
`for c in columns():` over the constant `[0, 1, 2]`, and its body calls `cards_in(b, n)`, which is
`filter_list(map_values(b.cards), lambda c: c.column == n)`. That is a relationship — the cards
*grouped by* column — but it is not a `map_get`, so what the recogniser reads is the **predicate**:
one `filter_list(xs, lambda y: g(y) == k(x))` where `xs` reads only what the loop captured, `g` reads
only the filtered element and `k` reads only the loop's, is the same equi-join with a many-to-one
right side. It compiles to `arrange_by(cards, column)` and a join that answers with the group, and
`beck explain cost` reports **no** per-event capture for board where it reported one.

**That the index and the predicate agree is a fact about `Prim::Eq` rather than a convention**, and
it is what makes the rewrite safe: `==` in Beck is `Value`'s own total order compared for equality,
and that order is the one the arrangement is a `BTreeMap` in. So the range under `g(y)` holds exactly
the rows the predicate would have kept, in exactly the order the collection held them — which is what
`filter_list` returned, values and order both.

**What it does not do is make the loop `O(δ)` when the body wants the group's rows, and the honest
ceiling is measured rather than argued.** The group is a `list`, because the expression replaced was
one and its consumer loops over it, so a card added to a column rebuilds *that column's* list and no
other. A body that wants a *question about* the group rather than its rows pays none of that: §99.9
item 6's `count` is the first, and it is read from the same `filter_list` with a `list_len` around
it. Spread across the three
columns that is **4.5–4.9× less work per event at 200 cards and at 1,600**; with every card in the
one column the event touches it is **1.1×**, because then the group is the collection and there is
nothing left to exclude. Both rows are printed by
`cargo test --release --test measure_incremental what_a_grouped_join_is_worth -- --nocapture`.
Removing the group's own cost is `group by` (§99.9 item 6), which is why item 6 follows this one.

Where inference fires it says so, and where it declines to it says which condition failed
(`Refusal`), printed under the capture line that costs the money — which is the same sentence the
README makes about placement.

The alternative — a comprehension surface with an explicit join, per
[`02`](02-syntax.md) §2.5's `sql"..."` and [`04`](04-compiler-architecture.md) §4.2's symbolic
`Query` — should stay where it is, as the **external store** ramp. Two query surfaces in one language
is a cost with no benefit here, and the inferred version is strictly better for the programs that
already exist.

**Where inference cannot see it** — a non-equi predicate, a collection derived per element, a key
that reads more than the element — the honest outcome is the one Beck uses everywhere else: compile it the slow
way and *say so*, in `beck explain cost`, with the reason. That is built and each refusal names its
own condition.

"A lookup behind a function the decomposition will not enter" was forecast here as the case that
would defeat it, and it does not: `27-review`'s lookup is inside `verdict_for`, inside a `match`, and
is found because the body is **inlined before it is searched** — α-renamed, with an argument
substituted only when copying it is free and `let`-bound otherwise, so no call is evaluated twice or
dropped. The limit moved rather than vanished: what is not entered is a *nested lambda*, because a
lookup inside one is a lookup per call of that function rather than per element.

## 99.7 What this unblocks, which is more than it looks

One build item discharges five things already written down:

| already written | closed by |
|---|---|
| [`23`](23-incremental-views-report.md) §23.19 "joins, subqueries, aggregates — **nothing**" | the operators — the join, both of its indexes and all four aggregates are built; subqueries are not |
| [`23`](23-incremental-views-report.md) §23.19 "joins, subqueries, `group by`, aggregates other than `count(*)`" | the read-model SQL compiling **to the plan** rather than growing a second interpreter |
| [`12`](12-standards-and-conformance.md) §12.5 `psql`'s `\d` unsupported because `pg_catalog` needs joins | the same |
| [`23`](23-incremental-views-report.md) §23.19 "`count(*)` without scanning" | grouping, which is where a maintained count *per group* lives — **and both halves are now built**: the ungrouped one reads the arrangement's size, which `Op::Count` has read since the engine existed, and the grouped one is a tally the join keeps (§99.9 item 6). This row over-attributed even so: not every aggregate question is a grouping question, and the ungrouped one never needed grouping to answer it |
| [`08`](08-roadmap.md) §8.4's Phase 5 **TPC-H/ClickBench** row, "once §5.3's engine exists" | the engine that row is conditioned on and no phase builds |

That last one is the reason this is a roadmap defect and not only a design gap: a Phase 5
*measurement* has been scheduled since the roadmap was written, and the thing it measures was never
assigned to a phase.

## 99.8 The second solver, and how its choices become correct

### The join is what forces a solver to exist

There are two cost models in this compiler and only one solver:

| | placement ([`cost.rs`](../compiler/crates/beck-core/src/cost.rs), [`place.rs`](../compiler/crates/beck-core/src/place.rs)) | the plan ([`plan.rs`](../compiler/crates/beck-core/src/plan.rs)`::cost_report`) |
|---|---|---|
| decides | where each definition runs | **nothing** |
| units | integer hundredths of a millisecond | symbolic — `δ`, `n`, `O(δ log n)` |
| method | exhaustive to 10 free nodes, then a deterministic sweep | none; it is a report |
| stability | `beck.lock`, churn reported | none |
| assertable | `beck check --assert-place page=client` | no |

[`23`](23-incremental-views-report.md) §23.13 is explicit that fusion needs no cost-based extraction
*because none of its rules conflict* — every one removes an operator and none adds one. A join ends
that on the first day: predicate pushdown competes with the rule beneath it, join order for three
collections is the classic exponential choice, which side to index is a choice, and maintain-versus-
compute-on-demand is a choice. §23.19 already predicted this would be the trigger for an e-graph. It
is also the trigger for a **solver**, and the solver should inherit
[`03`](03-type-and-effect-system.md) §3.4's four guardrails literally rather than by analogy —
determinism, stability (a `plan` section in `beck.lock`, churn reported), explainability (candidates
and costs in `beck explain query`, as `beck explain place` already prints them) and assertability
(`--assert-plan`).

**Rules before costs, as in placement.** The plan's equivalent of "`@on()` always wins" is the
**session cut**, and it must stay a hard condition rather than becoming a cost term.
[`23`](23-incremental-views-report.md) measured 55× across it and [`23`](23-incremental-views-report.md)
§23.13 already found that the locally-always-good rewrite is a pessimisation there. Predicate
pushdown is precisely the rewrite that wants to move a join across that line. Priced, it will
eventually be priced wrong by some guess; ruled, it cannot be.

### Symbolic costs cannot choose, and that is the real problem

The plan's cost language has no numbers in it — `δ applications`, `O(δ log n)`, `n entries copied`.
That is correct for a report and useless for a decision: nothing compares
`δ_left × |matching right|` against `δ_right × |matching left|` without magnitudes. Three quantities
would be needed and none is visible to the compiler: **rate** (maintaining is amortising — you pay
per event so that reads are free, which is only right above some reads-per-event ratio, and *no*
constant in `cost.rs` expresses how often anything happens), **selectivity**, and **cardinality**.

The third deserves a warning of its own. `ASSUMED_CARDINALITY = 16` is honest for placement, because
choosing a tier needs only the *ratio* between a `Map` and an `Int`. It is dishonest for join
ordering, which is **entirely** a function of relative cardinalities: with every collection assumed
to hold 16 elements, every join order costs the same, the solver picks by tie-break, and
`beck explain query` prints a derivation that looks principled. A constant that was fine for one
decision becomes load-bearing for another — which is the failure to catch before it ships.

### Why Beck can converge on the right answer, where the state of the art cannot

The answer is not a better estimator. **It is that Beck can move each decision to a place where its
input is exactly known, and it has three such places that no other system has all of.**

**1. Statistics are not statistics here — they are the data structure.** An arrangement's size is
`entries.len()`, `O(1)`, exact, at every moment; `list_len` is already maintained ±1 per delta
([`23`](23-incremental-views-report.md)). A relational optimiser samples with `ANALYZE` and works
from a histogram that is stale the moment it is written. Beck never estimates a base cardinality
because it can read it.

That this is the high-leverage end is established rather than assumed. Leis et al., *How Good Are
Query Optimizers, Really?* (PVLDB 9(3), 2015 — the paper that introduced the Join Order Benchmark)
found that industrial-strength estimators "routinely produce large errors", and that **cardinality
estimation is often the dominant factor behind poor plans, while cost models and enumeration
strategies matter comparatively less**; *Still Asking* (PVLDB 18, 2025) revisits the question a
decade on. So the component that dominates plan quality is exactly the one Beck can **delete rather
than improve**, because the number is sitting in a `BTreeMap`.

**2. The log makes the counterfactual exact.** Every other system knows what a plan *did* cost. Beck
can answer what an alternative plan *would have* cost, precisely, offline, with no production risk,
because [`03`](03-type-and-effect-system.md) §3.7 makes the whole history an ordered replayable
stream and the backend a deterministic function of it. Two candidate plans run over the same log
produce two exact work counts. And the unit is already there and is already not a clock —
`Work { applications, touched, materialised, recomputed, steps }`, counted by `Engine::render` — so
the comparison is an integer difference reproducible on any machine, not a benchmark with noise in
it. **The fifth field is what makes the comparison sound for this rung specifically**: the first four
stop at the boundary of a call, so two plans that differ in how much they hide inside one per-element
function used to report the same numbers, which is exactly the pair a plan search would be asked to
choose between. `steps` is what the backend executed inside those calls.
The consequence worth stating on its own: **an estimate is needed only once per plan.** After a
single replay, every intermediate cardinality in that plan is a measured fact. Systems without a
complete deterministic history re-estimate forever, because they have nothing to check against.

**3. Correctness is already gated, so search can be aggressive.** The scariest failure of any
optimiser is a rewrite that is subtly wrong. Here the differential — maintained against recomputed,
every event, byte for byte — already exists and is the project's oldest habit
([`04`](04-compiler-architecture.md) §4.8). A plan search that would be reckless elsewhere is
ordinary here.

**4. A plan change is a diff, not a surprise.** `beck.lock` makes the chosen plan a checked-in
artefact, reviewed in a pull request, with churn reported in CI. The standard operational story
elsewhere is a plan flipping silently at 3 a.m. because a statistic crossed a threshold. That is a
guardrail this project already built for a different decision and gets to reuse.

### The ladder, and each rung deletes a constant rather than tuning one

[`AGENTS.md`](../AGENTS.md) is the rule this follows: "a bad number is a design question, not a fact
to write down". So no rung calibrates a guess; each rung removes the need for one.

| rung | the decision rests on | deletes | needs |
|---|---|---|---|
| **0** | a named constant, and every decision resting on one says so in `beck explain query` | — | nothing; this is today plus a label |
| **1** | the arrangement's **own size**, read at prepare time | `ASSUMED_CARDINALITY` for every base collection | `entries.len()`, already `O(1)`. The plan compiles to a *policy*, and the sizes are read where they are known |
| **2** | **replay**: candidate plans run over a real log, `Work` counted, the winner written to `beck.lock` | the fan-out and selectivity guesses | `beck replay` / `beck fork --from prod --at <time>` — [`08`](08-roadmap.md)'s Phase 4 replay bullet, which is this rung with nothing added |
| **3** | **production counters fed back**, and validated by replay *before* adoption | the rate guess — reads per event, which is a property of the deployment and can never be known at compile time | the metrics [`23`](23-incremental-views-report.md) §23.14 already exports |
| **4** | **search**, since the differential already refuses a wrong plan | conservatism; this is where the e-graph §23.19 defers belongs | rungs 1–3 as the oracle |

Rung 2 is the one to notice: it is two bullets already on the roadmap pointed at each other. Replay
tooling is scheduled for Phase 4 as an *operational* feature — time-travel debugging, `beck fork`.
Used as a **plan oracle** it costs almost nothing more and is the single largest step from "a model
that guesses" to "a measurement that knows".

Rung 3 is where the honest limit sits, and it is not a limit of the estimator. Reads-per-event is a
fact about how an application is used. No compile-time analysis can produce it, no better model
approximates it, and the only correct response is to make the decision where the number exists —
which is why this rung is feedback rather than inference.

### What this does not fix

- **Replay describes the past.** A workload that changes shape still mis-plans until the loop runs
  again; the mitigation is that churn is reported rather than absorbed, so the change is visible.
- **The first deployment has no log.** Cold start is rungs 0–1 only. It is bounded rather than
  solved: collections are also small then.
- **Exact base cardinalities do not make join-order search polynomial.** They remove the error, not
  the combinatorics.
- **Intermediate cardinalities of a plan never run are still estimated** — once, and exactly once,
  per the paragraph above.

## 99.9 The order of work

**1. The gate, first, before any operator. Done.**
`scaling.rs::maintaining_a_view_whose_loop_looks_something_up_costs_the_same_at_any_size`: the
maintenance one event costs `27-review.beck`, at two collection sizes, asserted not to grow with the
collection. It was written to go red and it did — **415 units at 200 notes against 3,215 at 1,600**,
7.7× over 8× the rows, which is the nested loop with nothing left to interpret. This is
[`82`](82-the-edge-report.md) §82.10's discipline — write the gate that fails on the shape of the gap
— and it was first because every item below needed it as the oracle. Note what the suite did *not*
cover before it: `a_view_over_a_large_state_is_still_one_pass` measures the **cold recompute** path
and asserts nothing about maintenance, and [`23`](23-incremental-views-report.md)'s "same work at 200
rows and at 1,600" was measured on programs whose loops capture nothing.

**It now measures both settings, and that is the part worth copying.** `Relate::Refuse` is the off
switch [`08`](08-roadmap.md) §8.3 item 8 requires of a choice the compiler makes unbidden, and
running the refused path inside the gate does two jobs with one measurement: it proves the switch
works, and it leaves the gate carrying its own evidence that it can fail. A gate whose green run
reports 19 against 19 says nothing about what the operator is worth; one that reports 19 against 19
*and* 415 against 3,215 says both.

**2. Make the report count what it prints.** **Done.** The tally is derived from the same
per-operator record the body is printed from, so the count and the lines cannot disagree; and every
capture now names the *cadence* of what it captured — never, per subscription, or per event —
computed by tracing inputs back to a source, which is what tells a captured `const` from a captured
`session` from a captured **state** without a reader following edges by hand. `27-review` reports
**2 of 29** where it reported 1. §99.3 has the transcript and the two gates.

**3. `arrange_by`. Done**, and behind item 4 rather than in front of it for the reason recorded when
it moved: the right side of the first join is a `Map` field of the accumulator, so `map_values`'s
arrangement already answered it and building `arrange_by` first would have put an operator with a
delta rule and **no program** into the engine. Its program is `examples/board.beck`, which groups
cards by column, and the recognition reads the *predicate* of a `filter_list` the way item 5 reads a
`map_get` — same condition on what each expression may read, one more condition because a predicate
has to be an equality (§99.6). Three things it cost or taught are worth naming:

- **The operator was already written.** `arrange_by`'s arrangement is `sort_by`'s: an element keyed
  by `f(x)` followed by the input's key. A sort is that iterated and an index is that probed, and the
  engine runs one function for the two (§99.5 decision 4).
- **The output key of decision 1 did not come back**, and the forecast that it would was wrong for an
  instructive reason: what a left row matches is the group rather than the rows, so one left row is
  still one output row. §99.5 decision 1 has the correction.
- **It removes the scan and not the group**, which is measured rather than argued: 4.5–4.9× per event
  with the cards spread over the columns, 1.1× with all of them in the one column the event touches.
  §99.6 has the command.

**And it exposed the instrument, which has since been fixed.** The refused board rebuilds the whole
page inside one per-element function, and `Work` counted that as one application — so it reported the
*same four numbers* at 200 cards and at 1,600 while the clock moved tenfold, and this item's gate had
to be written against a variant program because the off switch was invisible. `Work` now carries
`steps`, what the **backend** executed inside those calls, taken through the seam
([`backend.rs`](../compiler/crates/beck-core/src/backend.rs)'s `Steps`), and the gate says the thing
it always meant: **98 steps at 200 cards and at 1,600 with the operator on, against 12,830 and
101,030 with it off**. The blindness was general — every `scaling.rs` gate over an opaque operator
shared it — which is why fixing it was worth a change of its own rather than a workaround here.

**4. `join`. Done.** An outer equi-join against an index, with §99.5's bilinear delta rule: a left
row that moved is looked up once, and a right row that moved reaches exactly the left rows waiting on
its key, through a reverse index the operator keeps. Neither side costs the collection. Unmatched
left rows survive as rows, because the expression it replaced was a `map_get` and its callers `match`
on an `Option`. `corpus/34-assignments.beck` is the program that is *about* a relationship, and its
subject is the half `27-review` cannot reach: **many** issues waiting on one person, so renaming that
person is one entry moving on the right and several rows moving on the left. The differential harness
(maintained vs recomputed, every event, byte for byte) is the correctness argument, unchanged, and it
is what holds "several rows moved" — `contains` asks about a string somebody thought to name.

**5. Recognise the loop-plus-lookup shape. Done** (§99.6): `27-review.beck`, `32-here.beck` and
`33-awareness.beck` get the operator with no edit, and the feature has no syntax. Two things it cost
are worth naming in advance for item 6. The recogniser had to **inline** to find a lookup written
behind a call, so the plan now carries a small β-reducer, and the honest bound on it is a nested
lambda rather than a call depth. And a body with *several* lookups had to become several joins rather
than a refusal: the first version refused it, which left `33-awareness` paying the whole cost, and a
refusal that keeps a program at `O(n)` per event is not a conservative choice — it is the defect with
a sentence attached.

**6. `group by` and aggregates** — `count`, `sum`, `min`, `max` per group. **All four are done**,
and they arrived in three pieces because what separated them was never effort. It was whether the
language had a spelling for the question.

`count` is done because it does: `list_len` over the same `filter_list` item 5 recognises is a
question about the group that the group does not have to exist to answer. The join keeps a tally per
key beside its reverse index and moves it by ±1 as the index moves, so the answer is `O(1)` and **no
group is built** — which is exactly the leftover item 3 handed over. `corpus/35-workload.beck` is the
program: every person, and how many issues name them, with the set of people coming from the data
rather than written out. Its page copies **one** entry out of an arrangement at 200 issues and at
1,600, against 202 and 1,602 for the same page whose count is written so the recogniser reads it as
a group (§99.9's gate, and the same one is 2.8× then 4.9× on a clock).

**Where the tally lives is the finding, and it is not where it looks like it should go.** The
obvious home is the index: `arrange_by` knows its own keys. It cannot go there, because an operator
reads its inputs' *values and changes* and never their private state — an index in the shared
dataflow is not the reading engine's cell at all. So the count is the **join's**, rebuilt from the
change stream, where `+1` for an entry that arrived and `-1` for one that left is the same arithmetic
whatever operator produced the change. That constraint is [`23`](23-incremental-views-report.md)'s
sharing showing up as a design rule rather than as a cost.

**`min` and `max` are done, and they cost the same — which this section predicted they would
not.** `list_min` and `list_max` became primitives first, because the language had no minimum at all
and `lib/collections.beck` spelled one as `list_get(sorted(xs), 0)`, which is a sort and a copy of
the whole list to answer a question about one element of it. With a spelling in hand the operator
follows the count's own rule rather than this section's earlier sketch, and that is the whole
finding:

- The sketch was to append the aggregate's key to `arrange_by` — index by `(g(y), y)` — and take the
  first entry of each range. Under it `min` is `O(log n)` and **`max` is not symmetric**: a
  `BTreeMap` prefix range can be entered from its start and not from its end, so the last entry under
  a key needs an upper bound, and there is no successor of an arbitrary `Value` and no maximum one
  to bound with. Beck has no descending order to key by either
  ([`46`](46-standard-library-report.md) §46.16, [`54`](54-ordering.md)). So the sketch left `max` as
  a walk of the group or a maintained extreme with an `O(g)` repair, and this section called that the
  genuine design decision item 6 still owed.
- **The decision dissolves under the count's rule.** An aggregate is not the index's, it is the
  reading operator's, rebuilt from the change stream — and a tree an operator builds *itself* is
  keyed by the projection alone, per group, and is therefore bounded at both ends by construction.
  What cannot be entered from its end is a prefix range of **somebody else's** arrangement. So the
  asymmetry belonged to the design and not to the problem, and `max` costs what `min` costs.

What is built is `Op::GroupBy`: one operator, keyed by the group, holding per group a **multiset** of
what its rows projected to, of which `min` and `max` are the two ends. It is the one shape in this
document whose right side is not an index over the collection — it holds one entry per *group* — so
the join above it is a `Matching::Unique`, the same point lookup a `map_get` gets, answering `Some`
for a group with rows and `None` for one without. That is `list_min`'s own answer for a list and for
an empty one, so the operator's contract needed nothing added to the surface.

`corpus/36-auction.beck` is the program: the lowest and the highest bid on every lot, written
`list_min(map_list(filter_list(…), …))` and its mirror, with no `group by` and no `min by` in the
file. Two things it is held to, and the second is the one worth having:

- **The cost.** `scaling.rs::asking_a_group_for_one_end_does_not_build_it` measures a new low
  arriving on a pile of 200 bids and of 1,600 — the worst case, because the answer moves and the
  page is reassembled: **72 backend steps at both sizes, against 4,097 and 32,097 with the operator
  switched off**, and one entry copied out of an arrangement either way.
- **The silence.** A bid *between* the standing ends moves the group and moves neither answer, so the
  operator publishes no change and nothing below it runs — not the join, not the loop, not the page.
  That is a property of the output rather than of the cost, and it is what separates an aggregate
  from a `filter_list` a consumer measures: the second reports every event that touched the group.
  `incremental_engine.rs::a_bid_between_the_ends_does_not_re_render_the_page` is the gate.

The **multiset** rather than a set is the detail that a differential over a generated log does not
find: two bids of the same amount are two bids, and a tree holding values rather than counts of them
drops the answer when half of a tie is withdrawn. So the log in
`incremental_engine.rs::a_maintained_extreme_per_group_survives_the_events_that_take_it_down` is
written rather than generated, and it withdraws the standing minimum, the standing maximum, half of
a tie, and the last bid on a lot. Deleting the multiplicity leaves the corpus-wide differential
**green** and turns that one red, which was measured rather than assumed.

**`sum` is done, and it needed a spelling rather than an operator.** It had none — there was no `sum`
primitive, and `corpus/28-catalogue.beck` writes one as a recursion — and the two edges this section
named before it landed were both real:

- A **float** sum cannot be maintained by adding what arrived and subtracting what left, because
  floating-point addition is not associative and the maintained answer is held to the recomputed one
  byte for byte.
- An **integer** sum can be, arithmetically — but Beck's `+` is `checked_add` and *raises* on
  overflow, so a running total that is maintained passes through different intermediate values from
  one recomputed from zero, and the two can disagree about whether the program failed. Exact is not
  the same as total.

**Both are edges of the fold rather than of the sum, and naming that is the decision: a sum is its
answer, not the order it was added in.** `list_sum(xs)` is the *exact* sum of `xs` and raises only
when **that** does not fit an `Int`. It is therefore a function of the multiset its rows project to
and of nothing else — which is the property every other aggregate here already had, and the reason
`min` and `max` never raised this question. A running total and a recompute are then the same
number, and they fail on the same lists.

**That makes `list_sum` a conservative extension of `+` rather than a rival to it**, which is the
test a second spelling for an old operation has to pass. Where `x1 + x2 + …` has an answer this is
the same answer; where the fold raises on the way to a total that fits — `[Int_MAX, Int_MAX,
-Int_MAX]` — this one has it. Strictly more lists have an answer and no list has a different one, so
nothing that held before holds differently. `interp.rs::a_sum_is_its_answer_and_not_the_order_it_was_added_in`
is that sentence as three assertions, at the two functions themselves.

**`Float` gets no sum at all, and that is the decision rather than the omission.** The same
definition over `Float` would not extend the fold, it would *disagree* with it: an order-independent
float sum is a different number in the last bits, on ordinary inputs, not merely defined on more of
them. A program holding one of each would have two answers to one question, and the difference would
land on whoever wrote the program rather than on the engine. So a float total stays the recursion a
program writes, `beck explain cost` prices it as the recompute it is, and
[`46`](46-standard-library-report.md) §46.16 carries it as an absence somebody chose. The asymmetry
between the two numeric types is the point: over `Int` the new function differs from the fold only
where the fold has **no answer at all**.

**What the extremes did not do is answer it, and this section was right about that.** `Op::GroupBy`
holds a multiset, so a sum could be derived from one — but in `O(distinct)` per probe rather than
`O(1)`. So `Agg::Sum` keeps no multiset: a running total and a row count, moved by `±n` and `±1` and
read in `O(1)`. One operator, two shapes of state, each holding what its aggregate needs and no
more — the count that rides along is not bookkeeping, it is what separates a group whose total is
zero from a key the arrangement must not hold.

**And the empty group is where the aggregates part company downstream.** `list_min` of no rows is
`None`; `list_sum` of no rows is `0`. So the join above a total reads a missing entry as a *value*
rather than as an absence — `Matching::Total`, the one place the four aggregates differ after the
group — and an account nobody has posted to renders a balance rather than a dash.
`incremental.rs::a_total_is_a_group_by_probed_as_a_value_rather_than_an_option` is the gate, and it
holds the other half too: no `arrange_by` in the plan, because an implementation that indexed the
rows and added up each range would answer correctly and cost the group.

`corpus/37-ledger.beck` is the program: every account and its balance, amounts signed because a
ledger's are, written `list_sum(map_list(filter_list(…), …))` with no `group by` and no `sum by` in
the file. Three things it is held to:

- **The cost.** `scaling.rs::totalling_a_group_does_not_build_it` measures a posting landing on a
  pile of 200 and of 1,600: **47 backend steps at both sizes, against 2,060 and 16,060 with
  the operator switched off**, and one entry copied out of an arrangement either way. Unlike the
  extremes' gate there is no worst case to choose, and that is worth stating rather than skipping:
  every posting moves its account's total, so an ordinary event *is* the reassembling case. A `sum`
  is the aggregate that never takes the "the group moved and the answer did not" discount.
- **The agreement, including about failure.** The differential holds the maintained page to the
  recomputed one, and `incremental_engine.rs::a_total_outside_int_fails_where_it_is_asked_for_and_nowhere_else`
  holds the two to the same *failures*. This is why `Op::GroupBy` **publishes** a total no `Int`
  holds instead of raising one: the operator maintains every group and the recompute only ever sums
  the groups the loop reaches, so raising at maintenance time would fail renders that never asked.
  The raise belongs at the probe, where the program wrote the question.
- **The bookkeeping.** `incremental_engine.rs::a_maintained_total_survives_the_events_that_take_it_back_down`
  writes the log rather than generating it, for the extremes' reason one paragraph up: a posting
  voided out of the middle, a *credit* voided (a subtraction of a negative), one of two identical
  amounts, and the last posting on an account, so the group empties to `0` and is rebuilt from
  nothing.

**7. `distinct` and difference**, per decision 2. **The difference is done, and `distinct` is not**
— and what separates them turned out to be the same thing that separated item 6's four aggregates
from each other: whether the language had a spelling for the question.

**The difference had one already, and that is why it went first.** `map_contains` is a primitive, so
`filter_list(xs, lambda x: not map_contains(m, k(x)))` is the difference by key written out, and its
mirror without the `not` is the intersection. Nothing had to be added to the surface at all — the
first operator in this document for which that is true. `Op::Restrict` is one operator with two
directions, `relate::restriction` is the recognition, and §99.6's two conditions are unchanged: the
collection may read only what the function captured, the probe key only the element.

**Where it is looked for is the difference from every shape above, and the reason is what comes out
of it.** A join is recognised at a *site inside a body*, because a loop does other things besides
look up. A restriction is the whole of what the operator computes, so the predicate is not
rewritten — it is **deleted**. That is also why a `filter_list` may have this operator when item 5's
rule says it may not have a join: a join emits a row, and a filter's consumers read the element. So
this is the one binary operator whose **output is one of its inputs**, which is decision 2's "no
representational change at all" turning out to be the design rather than a remark about the state.

Three things it is held to:

- **The cost, measured on the side that costs something.**
  `scaling.rs::stocking_one_item_does_not_reconsider_every_order` measures a delivery landing on a
  pile of 200 orders and of 1,600: **134 backend steps at both sizes, against 10,064 and 80,064**
  with the operator switched off — 8× the steps for 8× the orders, which is the whole-collection
  reconsideration stated as a measurement. **Choosing the other event would have measured nothing**,
  and saying so is the point: an *order* arriving moves the left side, which the refused
  `filter_list` already handles per delta because its capture did not move. Everything this operator
  removes is on the right, where a delivery changes the predicate itself.
- **The right-hand delta, which no test over one collection can see.**
  `corpus/38-backorders.beck` is the program — the orders for something in stock and the orders for
  something not — and `incremental_engine.rs::a_maintained_difference_survives_the_events_that_move_it_from_the_right`
  writes the log rather than generating it, for item 6's reason: an item stocked so several orders
  move at once, an item delisted so rows come back, an order **amended while it was waiting** so
  what comes back is what it is now, an order cancelled while it was ready so a row that left the
  left side is not resurrected by a change on the right, and an item stocked that nobody ordered.
- **The silence**, which is the aggregates' property arrived at from the other operator. A key with
  no rows waiting on it stops at the operator: stocking something nobody ordered moves neither list
  and nothing below either of them runs — 2 recomputes and 1 touched entry against 16 and 9 for a
  delivery two orders were waiting on.
  `incremental_engine.rs::stocking_something_nobody_ordered_re_renders_nothing` is the gate.

**And it holds no copy of what it filters**, which is the one piece of state the shape needs and
does not keep. A row this operator dropped is not in its arrangement, so when the index entry that
dropped it leaves, the value has to come from somewhere — and the somewhere is the left input, which
is already holding it as an arrangement or as the shadow the engine keeps of a plain list. The
operator's own state is a probe key per left row and the reverse index, and never a row. A cached
value would be a *stale* value for exactly one event — the one where the row was edited while it was
off the page — which is why that event is written into the log above rather than left to a
generator.

**`distinct` is done, and what it needed was a name.** This section said it was waiting on "the
multiplicity question, on its own terms", and §99.10 called it hard. Neither was where the work
was. The representation was already built — `Op::GroupBy`'s multiset is a count per distinct value —
and what actually stood in the way was that **no expression in the language named the question**:
`lib/collections.beck`'s `unique` is a fold and `elements(set_of(xs))` is a fold over a fold, and a
fold is one opaque operator [`crate::plan`] rebuilds in full on every event.

**So the decision is which of the two the primitive takes, and it is a decision because both are
maintainable.** `unique(xs)` drops later duplicates and keeps the order the list had;
`elements(set_of(xs))` is duplicate-free and *sorted*. Neither is ruled out by decision 1: the
sorted form would key its output by the value and need only counts, and the order-preserving form
keys its output by the **smallest input key** holding each value, which is one ordered set of keys
per distinct value and `O(log n)` per delta. So nothing about the engine forced the answer.

**What forced it is the test a second spelling of an old operation has to pass, which is
`list_sum`'s rule applied to an order instead of to a total.** `list_unique` is `unique`'s answer
and that library function's body is now a call to it, so **no third answer entered the language**;
a primitive with the sorted answer would have been a fourth name for a question the library already
answered two ways, and the difference between the two would land on whoever wrote the program.
`interp.rs::a_unique_list_keeps_the_order_it_was_given_and_not_the_values_own` is that sentence as
assertions, at the function it is about.

`Op::Distinct` publishes one entry per value at the smallest input key holding it, so the output is
a **sub-order of the input's** — the same relationship `filter_list`'s output has to its input, which
is why nothing downstream had to learn anything and why there is no fourth `Matching`: the operator's
output is a collection somebody loops over, not an index anybody probes. `corpus/39-topics.beck` is
the program: the topics its notes are filed under, shown as a chip row above the notes.

Three things it is held to:

- **The cost, against the fold that spells the same thing.** There is no off switch here and that is
  itself the finding — `list_unique` *names* the operator, so nothing is being decided on the
  program's behalf and [`08`](08-roadmap.md) §8.3 item 8 has nothing to ask for. The control is
  therefore the same program with the dedup written as the fold it used to be:
  `scaling.rs::the_values_in_use_are_maintained_and_a_fold_over_them_is_not` measures a note
  arriving on a board of 200 and of 1,600 — **62 backend steps at both sizes, against 2,280 and
  17,680** for the fold. The event measured is the worst case, a note that *moves* a topic's
  published occurrence, because one that changed no answer would be flat for a reason that has
  nothing to do with the operator.
- **The bookkeeping, and one defect it found.** A value can be published at a key another value is
  arriving at — one row changing what it contributes is exactly that — so **every departure has to
  be applied before any arrival**, or settling the arriving value first has the departing one's
  removal take the new entry straight back out. The corpus-wide differential found it on a
  generated log; `incremental_engine.rs::a_maintained_set_of_values_survives_the_events_that_move_where_each_one_sits`
  is the written log that holds it, and the events before the interesting one exist to put the two
  values in the order that makes the wrong settle order wrong.
- **The plan shape**, because two of the operator's properties are decisions that no rendered page
  would show: its output is an arrangement, so `list_len` over it is `Op::Count` — §3.8's "±1 per
  event, never by recount" — rather than a recompute that would build the list to measure it; and
  refusing the recogniser does not remove it, which is what says it is a lowering.
  `incremental.rs::a_distinct_is_an_arrangement_and_the_count_above_it_is_not_a_recompute` holds
  both.

**8. Fusion for the new operators**, and this is where [`23`](23-incremental-views-report.md) §23.19's
deferred question reopens: pushing a `filter_list` below a `join` is the first rewrite that
**competes** with another for the same node, which is exactly what §23.13 says the fixed-point
approach cannot arbitrate and what equality saturation exists to do. Expect to need the e-graph here
and nowhere earlier.

**It has no program, and that is a scheduling fact rather than a detail.** Swept across all 45
programs in `corpus/` and `examples/` with the operators in place: the only fusible operator sitting
anywhere above a relational one is `list_len`, three times, and a count over an arrangement is
already `O(1)` — there is nothing to fuse. **No program in the tree has a `filter_list` above a
join**, because the recogniser *consumes* the filter into the operator rather than leaving one above
it, which is the surface working as §99.6 intends. So the rewrite this item is named after has
nothing to be about, and building the e-graph for it now is item 3's lesson arrived at from the
other side: an operator with a delta rule and no program is a hole in the differential, and a
rewriting machinery with no rewrite to arbitrate is worse — it is a decision procedure whose
decisions nobody can check. What would give it a subject is a program that filters a joined row on
something the *left* side decides, which is an ordinary page (`the issues whose assignee is on the
roster, sorted`) and simply is not written here yet.

**9. The read-model SQL grows joins and `group by` by compiling into the plan**, not by growing its
own interpreter — which closes §23.19 and §12.5 together and keeps one code path.

Items 1–2 were days, items 4–5 were the phase, and item 3 followed them rather than preceding them.
Item 6 arrived in three pieces for the reason it names — the count with item 3, the extremes once the
language had a spelling for them, and the total once that spelling had been *decided* rather than
merely added. Item 7 split the same way and for the same reason, one step further on: the
difference needed **no** spelling, because `map_contains` was already a primitive, and it was
therefore the shortest operator in this document to land; `distinct` had two and the work was
choosing, which is a decision of the kind item 6's `sum` took and not an implementation. Items 8–9
each stand alone and can be scheduled independently.

**The convergence rungs interleave rather than follow.** §99.8's ladder is not a second project to
start afterwards, and treating it as one is how a guessed constant becomes permanent:

| with | do |
|---|---|
| item 4 (`join`) | **rungs 0 and 1 did not come due, and saying why is the point.** Both are about a solver *choosing* between plans, and the join that landed has nothing to choose: its right side is an index and its left is the loop the program wrote, so there is one plan. No cardinality, assumed or measured, reaches any decision, because no decision is taken. This row expected the rungs to come due with `arrange_by` (item 3), on the grounds that it is the first time a join has two sides that could be swapped; the row below is what happened instead |
| item 5 (recognition) | **rung 2** — the two programs that already contain a join, replayed under both plans, are the first real measurement, and `Work` is the unit |
| item 3 (`arrange_by`) | **rungs 0 and 1 did not come due either, and this row was wrong about why they would.** It expected `arrange_by` to give a join two sides that could be swapped. It does not, and the reason is the surface rather than the operator: the join is *inferred from a loop*, and the loop fixes which side is the left, because the left side's order is the output's order (decision 1). Nor is building the index a choice — it is ruled by "the rewrite must remove a capture" rather than priced, and the measurement says the index is never worse (§99.9 item 3's 1.1× ceiling). Rung 0 is satisfied in the strict sense it asks for: **no plan decision rests on a named constant**, checked rather than assumed. What would make a choice exist is a join whose sides are both indexed and neither of which is the output's order — which arrives with `group by` and with an explicit surface, not here |
| item 6, aggregates (`count`, `min`, `max`) | **rungs 0 and 1 did not come due a third time**, and the row above had already said why they would not: the aggregate's right side is one entry per group, its left is the loop, and there is nothing to swap. What it adds is that the pattern survives an operator with *no index at all* on one side — so "an inferred surface postpones the solver" is about the surface and not about what the operators happen to be |
| item 6, the total (`sum`) | **rungs 0 and 1 did not come due a fourth time.** The row above forecast that they would arrive "with `group by` and with an explicit surface"; `group by` is now complete and the surface is still inferred, so there is still nothing to swap. What this one adds is that the pattern survives an aggregate whose right side is not even a *reading* of the group — a running total is a number the operator keeps — so the postponement is the loop's order and not anything about the group |
| item 7, the difference | **rungs 0 and 1 did not come due a fifth time**, and this one has the least to choose of any operator here: there is no right side to swap, because the right side is a *question* rather than a collection — one probe returning a bool. A `filter_list` also has an order to preserve, so even the left is fixed. The row above forecast that a choice would arrive "with `group by` and with an explicit surface"; `group by` is complete, the difference is complete, and the surface is still inferred |
| item 7, `distinct` | **rungs 0 and 1 did not come due a sixth time**, and this one could not have: `list_unique` names the operator, so there is not even a shape being read — one expression, one plan, no alternative to price |
| item 8 | **rung 4** — search, once replay is the oracle rather than the model |
| Phase 4's `beck tune` | **rung 3** — the rate, fed back from a deployment, because it exists nowhere else |

Rung 1 belongs *inside* the first item that makes a choice, rather than after it, and that is the
sequencing decision worth arguing over: shipping a join that reads `ASSUMED_CARDINALITY` would make
the constant load-bearing for a decision it was never honest for (§99.8), and a constant that has
shipped is a constant that gets tuned instead of removed. Every operator that has landed keeps that
rule by making no choice at all — which is the cheapest way to honour it and was not the way this
table expected it to be honoured, six times now. **The pattern the second time made visible and the
three after it confirmed: an inferred surface postpones the solver.** A plan choice exists only where two plans could produce the same
answer, and a rewrite of an expression the programmer wrote has that expression's order to preserve,
which fixes most of the plan before any cost is consulted. `ASSUMED_CARDINALITY` is still in
`cost.rs` and still reaches no plan decision.

## 99.10 What this document does not claim

- **What is built is the join, its two indexes, all four aggregates, the difference and
  `distinct`** — which is every row of §99.4. There is still no `group by` and no `except` a program
  can *write*: every operator here but one is emitted for an expression somebody already wrote, and
  `Op::GroupBy` and `Op::Restrict` are the names of operators rather than of constructs. The
  exception is `distinct`, and it is an exception in the other direction — `list_unique` is a
  *function*, not a construct, and the operator is its lowering. The measurements in
  §99.3 are of the compiler *before* the operator, and every command is quoted in full so they can
  be re-run — with `--no-join`, which is how that transcript is reproduced today.
- **It does not reopen D26.** Nothing here puts a relation in the store or writes anything on the
  append path. Every operator proposed is a read-side maintained arrangement whose oracle is the
  recomputed answer.
- **It does not price the operators.** §99.8 says what the cost model must grow to hold them. What
  is now measured is one program's *maintenance* at two sizes (§99.9 item 1), which says the join
  does not cost the collection; it does not say what a join costs against a recompute, or what the
  index costs in memory, and [`23`](23-incremental-views-report.md) §23.14's per-subscriber metric is
  where the second of those would be answered.
- **It does not settle the surface** beyond the shapes named. §99.6's inference is built, reaches a
  lookup behind a call, and reads a `filter_list`'s predicate as well as a `map_get`; it does not
  reach a site inside a nested lambda, a key that reads anything but the element, a predicate that is
  not an equality, or a predicate whose probe reads a *capture* rather than the loop's element. Each
  refuses with its own reason rather than silently compiling slowly, which is the part that was
  assumed and is now tested.
- **The difference is recognised from a predicate that is a membership test and nothing else.**
  `filter_list(xs, lambda x: p(x) and map_contains(m, k(x)))` is refused, with the same sentence
  anything else gets, and it is refused rather than split because splitting a conjunction into two
  operators is a rewrite [`crate::fuse`] owns and not a shape a recogniser reads. That refusal keeps
  such a program at `O(n)` per event, which §99.9 item 5 is explicit is not a conservative choice —
  it is the item's remainder, named here rather than left for a reader to discover from a slow page.
  Nor does the operator reach a membership test against a collection that is not a `Map`: an index
  over a `list` is an `arrange_by` and a presence question of it is its tally, both of which exist,
  and neither is wired to this shape because no program in the tree writes it.
- **`arrange_by` removes the scan and not the group.** One left row's answer is the whole group as a
  `list`, so an event that touches a group rebuilds it. §99.6 measures both ends of what that is
  worth, and item 6 closes it only for the questions that have a spelling — which is now four of
  them, and not the fifth somebody writes next.
- **The four aggregates are the four, and there is no fifth.** `count`, `min`, `max` and `sum` per
  group are built and each is a *variant* rather than a parameter, so an average, a product or a
  `string_agg` is an operator somebody writes and not a function somebody passes. That is decision 4
  read from the other side — a general fold per group would have to state its own inverse before it
  could be maintained, and none of these four had to, because each carries the inverse in the
  operator.
- **What the instrument can and cannot compare.** `Work::steps` is what the *backend* executed, so it
  sees inside a per-element function and does not see the engine's own bookkeeping — the `BTreeMap`
  work and the value copies a group probe does are the clock's business, not its. Two runs of one
  backend are comparable in it; two backends are not, and it is a count rather than a duration for
  [`13`](13-testing.md) §13.7's reason.
- **`distinct` is over values and not over a *key*.** `list_unique` compares whole values by
  `Value`'s own order, which is what `==` compares, so a collection of records is deduplicated by
  every field. "The distinct customers", where two orders from one customer are one customer, is
  `list_unique(map_list(xs, f))` — a projection the program writes over an operator that is already
  there — and there is no `distinct on (key)` that keeps a whole row per key. That is a surface
  somebody would have to design, not a delta rule anybody is missing.
- **Nor is it a `distinct` per *group*.** `unique` inside a loop body, over a filtered collection,
  would be a fifth aggregate on `Op::GroupBy` — the multiset it already holds has the answer — and
  it is unbuilt for the reason the row above gives about a fifth aggregate generally: it is an
  operator somebody writes, not a function anybody passes.

## 99.11 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`05`](05-tier-lowering.md) §5.3 | The incremental-views paragraph described a joined read model updating "by delta, not by re-join" when there was no join to update and no operator related two collections. It now says that, in the past tense, and lists what has since been built |
| [`23`](23-incremental-views-report.md) §23.19 | "Joins, subqueries, aggregates — **nothing**, unchanged" is now true of **subqueries alone**: an equi-join over either index, all four aggregates, a semi-join and anti-join by key, and `distinct` are built. §99.9 holds what is left, which is no longer an operator — it is the read-model SQL compiling into the plan (item 9) |
| [`23`](23-incremental-views-report.md) §23.19 | Same, for the read-model half — and its `count(*)` row is grouping's, not the SQL's |
| [`08`](08-roadmap.md) §8.4 | The Phase 5 TPC-H row is conditioned on "§5.3's engine" that no phase builds. Phase 4 now carries the bullet |
| [`23`](23-incremental-views-report.md) §23.8 | Its "the analysis says a plan could, the engine does not" caveat has a second instance — a captured per-element function — and it was undocumented |
