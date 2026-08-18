# 99 — The data tier's means of combination

> **The join is built; the rest of the algebra is not.** §99.9 items 1, 4 and 5 have landed — the
> shape gate, the `Join` operator with §99.5's bilinear delta rule, and the recognition that emits it
> for the loop a program already wrote. `group by`, the aggregates, `distinct` and difference are
> still missing, and §99.9 is where each of them sits. Everything below is the design; what is built
> says so where it is built.
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
3. **The corpus never grew a relationship big enough to hurt.** Thirty-three programs, all small.
   §99.3 is the sweep.

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
commit after the paragraph describing it. So three of four are closed rather than two of three, and
`board` is the one left — which is a **grouping** rather than a lookup and waits on §99.9 item 3.

**And the analysis disagrees with the plan.** `beck explain incremental corpus/27-review.beck`
reports `flat_map ×1 maintained`. It is maintained in the sense the analysis means — the operator has
a delta rule — and it is reapplied wholesale anyway, because its *function* moved rather than its
input. This is the second instance of the disagreement [`23`](23-incremental-views-report.md) §23.8
already caught once for `html_el` and said out loud: "that table is a statement about what a plan
could do, and the engine does not do this one." Nobody has said it about this one.

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
| **grouping and aggregation (Γ)** | — | **missing**; there is no `sum`, `min`, `max`, or per-group anything |
| **difference (−)** | — | missing |
| **distinct (δ)** | — | missing |

Read as SICP's three-part test, which is the standard [`01`](01-vision-and-premise.md) §1.1 sets and
[`25`](25-benchmarks-and-expressiveness.md) measures the language against:

- **Primitives** — sound. An arrangement is an ordered map from a composite key to a value, deltas
  are entry moves, and the key composes (`Key = Arc<[Value]>`; `flatten`'s key is its input's key
  followed by a position).
- **Means of abstraction** — sound, and genuinely good. A developer names a derived collection and
  the compiler shares it between subscribers ([`23`](23-incremental-views-report.md)) and projects
  it as a relation an outside SQL client can read ([`23`](23-incremental-views-report.md)),
  with no annotation.
- **Means of combination** — **unary**. This is the gap, and stating it this way is what makes it
  obviously a gap rather than a missing feature: an algebra whose combining forms all take one
  operand cannot express a relationship between two things, so every relationship in every program
  is expressed by leaving the algebra.

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

**What it does not do is make the loop `O(δ)`, and the honest ceiling is measured rather than
argued.** The group is a `list`, because the expression replaced was one and its consumer loops over
it, so a card added to a column rebuilds *that column's* list and no other. Spread across the three
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
| [`23`](23-incremental-views-report.md) §23.19 "joins, subqueries, aggregates — **nothing**" | the operators — the join and both of its indexes are built, grouping and the aggregates are not |
| [`23`](23-incremental-views-report.md) §23.19 "joins, subqueries, `group by`, aggregates other than `count(*)`" | the read-model SQL compiling **to the plan** rather than growing a second interpreter |
| [`12`](12-standards-and-conformance.md) §12.5 `psql`'s `\d` unsupported because `pg_catalog` needs joins | the same |
| [`23`](23-incremental-views-report.md) §23.19 "`count(*)` without scanning" | grouping, which is where a maintained count *per group* lives — **the ungrouped one needed none of it** and is built: `select count(*) from t` reads the arrangement's size, which `Op::Count` has read since the engine existed. This row over-attributed: not every aggregate question is a grouping question |
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
`Work { applications, touched, materialised, recomputed }`, counted by `Engine::render` — so the
comparison is an integer difference reproducible on any machine, not a benchmark with noise in it.
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

**And it exposed the instrument.** The refused board rebuilds the whole page inside one per-element
function, and `Work` counts that as one application — so it reports the *same four numbers* at 200
cards and at 1,600 while the clock moves tenfold. That is
[`DEFECTS.md`](../DEFECTS.md)'s `work-cannot-see-inside-an-application`, it is why this item's shape
gate reads `materialised` rather than the three counters the item-1 gate reads, and it is a blindness
every `scaling.rs` gate over an opaque operator shares.

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

**6. `group by` and aggregates** — `count`, `sum`, `min`, `max` per group. `min`/`max` are the hard
ones and should be said so in advance: deleting the current minimum of a group needs either a rescan
of that group or a tree per group, which is a genuine design choice and not an implementation
detail. **It now has a predecessor's leftovers waiting for it as well as its own subject**: item 3
builds the index a group is read from and hands the group back as a `list`, so the group's own size
is paid on every event that touches it. A group that is a maintained collection rather than a rebuilt
list is what closes that, and it is the same operator either way — which is why item 6 follows item 3
rather than standing beside it.

**7. `distinct` and difference**, per decision 2 — after the above, and only with the multiplicity
question answered on its own terms.

**8. Fusion for the new operators**, and this is where [`23`](23-incremental-views-report.md) §23.19's
deferred question reopens on schedule: pushing a `filter_list` below a `join` is the first rewrite
that **competes** with another for the same node, which is exactly what §23.13 says the fixed-point
approach cannot arbitrate and what equality saturation exists to do. Expect to need the e-graph here
and nowhere earlier.

**9. The read-model SQL grows joins and `group by` by compiling into the plan**, not by growing its
own interpreter — which closes §23.19 and §12.5 together and keeps one code path.

Items 1–2 were days, items 4–5 were the phase, and item 3 followed them rather than preceding them.
Items 6–9 each stand alone and can be scheduled independently, and item 6 is the one with something
waiting on it.

**The convergence rungs interleave rather than follow.** §99.8's ladder is not a second project to
start afterwards, and treating it as one is how a guessed constant becomes permanent:

| with | do |
|---|---|
| item 4 (`join`) | **rungs 0 and 1 did not come due, and saying why is the point.** Both are about a solver *choosing* between plans, and the join that landed has nothing to choose: its right side is an index and its left is the loop the program wrote, so there is one plan. No cardinality, assumed or measured, reaches any decision, because no decision is taken. This row expected the rungs to come due with `arrange_by` (item 3), on the grounds that it is the first time a join has two sides that could be swapped; the row below is what happened instead |
| item 5 (recognition) | **rung 2** — the two programs that already contain a join, replayed under both plans, are the first real measurement, and `Work` is the unit |
| item 3 (`arrange_by`) | **rungs 0 and 1 did not come due either, and this row was wrong about why they would.** It expected `arrange_by` to give a join two sides that could be swapped. It does not, and the reason is the surface rather than the operator: the join is *inferred from a loop*, and the loop fixes which side is the left, because the left side's order is the output's order (decision 1). Nor is building the index a choice — it is ruled by "the rewrite must remove a capture" rather than priced, and the measurement says the index is never worse (§99.9 item 3's 1.1× ceiling). Rung 0 is satisfied in the strict sense it asks for: **no plan decision rests on a named constant**, checked rather than assumed. What would make a choice exist is a join whose sides are both indexed and neither of which is the output's order — which arrives with `group by` and with an explicit surface, not here |
| items 6–8 | **rung 4** — search, once replay is the oracle rather than the model |
| Phase 4's `beck tune` | **rung 3** — the rate, fed back from a deployment, because it exists nowhere else |

Rung 1 belongs *inside* the first item that makes a choice, rather than after it, and that is the
sequencing decision worth arguing over: shipping a join that reads `ASSUMED_CARDINALITY` would make
the constant load-bearing for a decision it was never honest for (§99.8), and a constant that has
shipped is a constant that gets tuned instead of removed. Both joins that have landed keep that rule
by making no choice at all — which is the cheapest way to honour it and was not the way this table
expected it to be honoured, twice now. **The pattern the second time makes visible: an inferred
surface postpones the solver.** A plan choice exists only where two plans could produce the same
answer, and a rewrite of a loop the programmer wrote has the loop's order to preserve, which fixes
most of the plan before any cost is consulted. `ASSUMED_CARDINALITY` is still in `cost.rs` and still
reaches no plan decision.

## 99.10 What this document does not claim

- **What is built is the join, its two indexes, and nothing else in the algebra.** `group by`, the
  aggregates, `distinct` and difference are unbuilt. The measurements in §99.3 are of the compiler
  *before* the operator, and every command is quoted in full so they can be re-run — with
  `--no-join`, which is how that transcript is reproduced today.
- **It does not reopen D26.** Nothing here puts a relation in the store or writes anything on the
  append path. Every operator proposed is a read-side maintained arrangement whose oracle is the
  recomputed answer.
- **It does not price the operators.** §99.8 says what the cost model must grow to hold them. What
  is now measured is one program's *maintenance* at two sizes (§99.9 item 1), which says the join
  does not cost the collection; it does not say what a join costs against a recompute, or what the
  index costs in memory, and [`23`](23-incremental-views-report.md) §23.14's per-subscriber metric is
  where the second of those would be answered.
- **It does not settle the surface** beyond the two shapes. §99.6's inference is built, reaches a
  lookup behind a call, and reads a `filter_list`'s predicate as well as a `map_get`; it does not
  reach a site inside a nested lambda, a key that reads anything but the element, a predicate that is
  not an equality, or a predicate whose probe reads a *capture* rather than the loop's element. Each
  refuses with its own reason rather than silently compiling slowly, which is the part that was
  assumed and is now tested.
- **`arrange_by` removes the scan and not the group.** One left row's answer is the whole group as a
  `list`, so an event that touches a group rebuilds it. §99.6 measures both ends of what that is
  worth and item 6 is what closes the rest.
- **It does not fix the instrument the refused path is measured with.** `Work` counts one application
  for a per-element function whatever that function does, so the switched-off board reports the same
  four numbers at every size; the clock is what says otherwise, and
  [`DEFECTS.md`](../DEFECTS.md)'s `work-cannot-see-inside-an-application` is the entry.
- **`min`/`max` over a group and `distinct` over values are named as hard**, not designed. Item 6 and
  item 7 each need their own decision before they are written.

## 99.11 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`05`](05-tier-lowering.md) §5.3 | The incremental-views paragraph describes a joined read model updating "by delta, not by re-join". There is no join to update, and no operator relates two collections; the paragraph now says so and points here |
| [`23`](23-incremental-views-report.md) §23.19 | "Joins, subqueries, aggregates — **nothing**, unchanged" is no longer true of joins: an equi-join against a unique index and a many-to-one one against an `arrange_by` are both built and both inferred. Subqueries and aggregates are unchanged and §99.9 holds their order |
| [`23`](23-incremental-views-report.md) §23.19 | Same, for the read-model half — and its `count(*)` row is grouping's, not the SQL's |
| [`08`](08-roadmap.md) §8.4 | The Phase 5 TPC-H row is conditioned on "§5.3's engine" that no phase builds. Phase 4 now carries the bullet |
| [`23`](23-incremental-views-report.md) §23.8 | Its "the analysis says a plan could, the engine does not" caveat has a second instance — a captured per-element function — and it was undocumented |
