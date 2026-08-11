# 89 — Phase 3 report, part 57: query fusion, and the rewrite that would have been wrong

**Built.** Query fusion on the view's dataflow plan, with `beck explain query` and
`beck explain cost` behind it — which is the **last part of Phase 3's incremental-views bullet**,
and the item [`88`](88-read-models-and-pgwire-report.md) §88.10 named as all that was left of it.

The pass is five local rewrites over [`plan.rs`](../compiler/crates/beck-core/src/plan.rs)'s
operator graph, each stated with the property that makes it sound. Fourteen of the thirty-one
programs measured — the sketch and the corpus's thirty single-file programs — lose an operator;
seventeen arrangements go in all; the sketch's
subscriber holds **17% fewer entries at every collection size**, which is the number to read,
because the wall clock moves by 2–6% and that is not distinguishable from noise on this machine.

The more interesting half is §89.3. A rewrite that is an improvement everywhere else is a
**pessimisation** when it crosses §5.3's session cut: fusing a shared operator into a per-session
one moves work the process does once per event into work it does once per event *per subscriber*,
which on [`26`](26-arrangement-sharing-report.md)'s public feed is a factor of 55 spent rather than
saved — and the page is identical either way, so no differential can see it. That is the condition
this pass exists to have, and it is the reason a fusion pass in this language is not the textbook
one.

## 89.1 What §5.3 asked for, and which plan it is about

[`05`](05-tier-lowering.md) §5.3:

> Query fusion still matters (a `for` over a view of a view should become one plan, not N+1
> lookups); it is a plan-rewrite on symbolic `Query` nodes, kept symbolic in `Core` precisely for
> this ([`04`](04-compiler-architecture.md) §4.2).

Two things in that sentence are about different objects, and the difference decides what was built.

**The `Query` sub-language does not exist yet.** §4.2 lists it as a part of `Core` kept symbolic;
[`20`](20-phase-2-report.md) §20.5 and [`23`](23-general-slicer-report.md) §23.9 both said
`beck explain query` was waiting on "an engine to compile one", and no engine compiles a `Query`
today because no program writes one. A relational query language with joins and aggregates is what
[`88`](88-read-models-and-pgwire-report.md) §88.6 lists as unbuilt, and it is unbuilt still.

**The plan does exist.** Since [`24`](24-incremental-views-report.md) a view is compiled into a
dataflow of operators, and since [`26`](26-arrangement-sharing-report.md) each operator is on one
side or the other of the session cut. `for t in todos:` is decomposed into a `map_list` whose
arrangement holds one rendered list per row and a `flatten` that takes those lists apart again —
which is *exactly* the "`for` over a view of a view" §5.3 names, one construct at a time, because
the decomposition walks the source rather than the shape. Nothing reads the arrangement in between.

So the pass is over the plan, and this report claims fusion of the dataflow rather than of a query
language. [`plan.rs`](../compiler/crates/beck-core/src/plan.rs)'s own module documentation has said
since [`24`](24-incremental-views-report.md) that it "is not a *query* plan … this compiles the
signal graph, which is a different thing that happens to share the word"; that sentence still
stands, and `beck explain query` is named for the command §4.7 asks for rather than for the
sub-language it presumed.

One consequence, stated rather than left to be found: §4.7 writes both commands as
`beck explain query <fn>` and `beck explain cost <fn>`, and both take a **file**. A plan is a
property of the program's page, and a program has one page ([`23`](23-general-slicer-report.md)
§23.9's `B0510`); `<fn>` presumed one plan per query.

## 89.2 The five rules, and the property that makes each sound

An arrangement is a `BTreeMap` from an ordering key to a value, and the key is what makes iteration
order a consequence of the plan rather than of a sort at the end. The order reaches the rendered
page and the replay digest, so a rewrite has three obligations rather than one: **the same values,
in the same order, and the same deltas** — a fused operator has to move exactly the entries the pair
moved, or a subscriber woken late updates by a delta that does not describe what happened.

| rule | what it does | why it is sound |
|---|---|---|
| `map_list` over `map_list` | one `map_list` applying the composition | neither moves an element, so both arrangements are keyed by the input's key and so is the composition |
| `filter_list` over `filter_list` | one `filter_list` applying the conjunction | same key; and the conjunction **short-circuits**, so the outer predicate is applied to exactly the elements the inner one kept, which is what the pair did |
| `flatten` over `map_list` | one `flat_map` | the map's key is the input's, and the flatten's is the map's followed by a position — so one operator keyed by the input's key and a position produces the same order |
| `list_len` / `list_is_empty` over `map_list` or `sort_by` | the count reads the producer's input | both produce one entry per entry, so *how many* is a question about the input, and the arrangement between them is never read at all |
| `concat_lists` of one list | the list | a union of one delta stream is that delta stream; every entry gained the same `[0]` key prefix, and a prefix every entry shares does not order anything |

The conjunction is written as an `If` rather than as `Prim::And`, for the reason
[`53`](53-are-we-fast-yet-report.md) found and fixed: `and` **is** an `If` in `Core`. Building the
strict primitive here would have applied the outer predicate to elements the inner one rejected,
which the pair of operators never did — a difference invisible in the values and visible the first
time an outer predicate can fail.

The two compositions do not substitute anything. A [`Fun`](../compiler/crates/beck-core/src/plan.rs)
is a `Lam` over the operators it captured followed by the element, and both halves are closed, so
the composition binds fresh parameters and *applies* both lambdas. Nothing in the pass has to reason
about variable capture, which is the sort of thing that is worth arranging rather than getting
right.

`flat_map` is the one new operator, and it is `map_list`'s application inside `flatten`'s loop.
Its rebuild rule is `map_list`'s rather than `flatten`'s, which is not a detail: a per-element
function that captured a plan node is a *different function* when that node moves, so the whole
collection has to be reapplied. Keeping `flatten`'s rule would have been correct for a loop whose
body ignores the session and wrong for one that reads it — see §89.7, where that exact mistake is
made on purpose.

## 89.3 The two conditions that refuse a rewrite, and the second is why this pass is not the textbook one

A producer is fused into its consumer only when three things hold. The first and third are
bookkeeping; the second is the finding.

**1. Nothing else reads it.** An arrangement two operators read is
[`26`](26-arrangement-sharing-report.md)'s shared prefix, and fusing it into one of them computes it
again for the other. `corpus/24-feed.beck` sorts a public feed once and reads the result three times
— the loop, the count and the emptiness check — so the `list_len` over that `sort_by` matches the
count rule and is refused, and `beck explain query` says so:

```console
what matched a rule and did not fuse
  #20  a count over a cardinality-preserving operator kept #4
       #4 is read by 3 operators, and fusing it into one of them would compute it 3 times (docs/26)
```

**2. It does not cross the session cut.** This is the one a fusion pass written from the literature
would not have. [`26`](26-arrangement-sharing-report.md) established that operators which do not
read the session are held **once for the whole fanout** and advanced once per event, and that
operators below the cut run per subscriber. A local rewrite cannot see that: fusing a shared
`map_list` into a per-session one produces a smaller plan whose *shared half has disappeared*, so
work that a process did once per event it now does once per event per connection. On the public feed
[`26`](26-arrangement-sharing-report.md) measured, that is the 55× that report reports, spent.

The page is byte-for-byte identical either way. So this is not a correctness condition and no
differential harness can see it — which is why `fusion.rs` asserts it on a program built to make it
bite, and why deleting the condition turns that test red (§89.7).

**3. No name points at it.** A declared signal is projected as a read-model table
([`88`](88-read-models-and-pgwire-report.md)), so an operator a developer *named* is observable to a
SQL client even when the page never reads it. Fusing it away would silently remove a table. The
corpus's read model is unchanged by this work: **43 tables**, the same 43.

There is no fourth condition and no cost model, and that is worth saying plainly.
[`38`](38-literature-survey.md) §38.2 says to "**adopt** the shape when fusion is built: small local
rewrites, each sound against the change semantics, extracted by the cost model
[`20`](20-phase-2-report.md) already has", pointing at egg and egglog. The shape is adopted and the
machinery is not: equality saturation earns its keep when rewrites *conflict*, so that the phase
order would otherwise decide the answer. None of these conflict — every rule removes an operator and
none adds one — so there is nothing for an extraction pass to choose between, and the fixed point is
reached by applying rules until none matches. §89.6 says what would need an e-graph.

## 89.4 What it is worth, and it is memory rather than time

`cargo test --release --test measure_incremental -- --nocapture what_query_fusion_is_worth`.

Across the sketch and the corpus's thirty single-file programs — operators and arrangements, before
the pass and after:

| | before | after |
|---|---|---|
| programs with an operator removed | — | **14 of 31** (the sketch and the corpus's thirty single-file programs) |
| arrangements removed, in all | — | **17** |
| `examples/todo.beck` | 43 operators, 6 arrangements | 42, **5** |
| `corpus/24-feed.beck` | 31 operators, 5 arrangements | 30, **4** |
| `corpus/04-kanban.beck` | 69 operators, 13 arrangements | 66, **10** |
| `corpus/28-catalogue.beck` | 49 operators, 12 arrangements | 47, **10** |

Seventeen of the thirty-one lose nothing, and fifteen of them for one reason: their view holds no collection
at all, so there is no pair of collection operators for a rule to match. `beck explain query` says
that rather than "nothing matched", because the two are different facts about a program. The other
two are the interesting ones. `corpus/16-money.beck` counts a `map_values` directly, and the count
rule's producers are `map_list` and `sort_by`. `corpus/05-poll.beck` writes `list_len(filter_list(…))`
three times over one shared arrangement — and a filter changes cardinality, so that is not a fusion
this pass declines to make but one that would be **wrong** (§89.6).

What one of those arrangements costs, on the sketch, whose `for t in mine:` is the shape the rewrite
is for. The two plans are measured **alternating** rather than one after the other, because
[`70`](70-the-evaluator-gets-fast-report.md) §70.7 found that a fixed A-then-B order biases a
wall-clock comparison by as much as the effects this project reports:

| rows | entries held, unfused | entries held, fused | held | work/event | µs/event, unfused | µs/event, fused |
|---|---|---|---|---|---|---|
| 10 | 66 | 55 | 83% | 30 → 29 | 32 | 32 |
| 100 | 606 | 505 | 83% | 120 → 119 | 177 | 161 |
| 1,000 | 6,006 | 5,005 | 83% | 1,020 → 1,019 | 2,001 | 1,869 |
| 5,000 | 30,006 | 25,005 | 83% | 5,020 → 5,019 | 13,416 | 12,577 |

**The memory column is the claim.** One arrangement of the collection's size is not built, so a
subscriber holds `n + 1` fewer entries — at every size, which is what makes it a shape rather than a
constant, and it is gated as `held_before - held_after == n + 1` at two sizes rather than printed.
[`24`](24-incremental-views-report.md) §24.6 measured a maintained subscription at about four times
the memory its page already held; this gives back a sixth of what the sketch holds, per connection.

**The time column is not a claim**, and the microseconds move between runs by more than the
difference between the two columns. Per event the rewrite saves exactly one arrangement insert —
`work/event` falls by 1 at every size, which is the honest arithmetic — and the 2–6% on the clock is
inside what this measurement can distinguish. Saying otherwise would be
[`70`](70-the-evaluator-gets-fast-report.md) §70.1's mistake in the other direction.

## 89.5 What building it found, and it is three kinds of unreachable

**A plan contained operators nothing could reach, and they came from dictionary passing.**
`corpus/28-catalogue.beck` built two operators that no other operator, no name and no root referred
to. The decomposition builds an operator for every argument of a call it inlines and for every
`let`'s value, before it knows whether the body reads them — and a bounded definition's arguments
include one **dictionary per method of each bound** ([`39`](39-bounds-report.md)). `Priced` declares
`pence` and `describe`; `priced_total[T: Priced]` calls only `pence`; so each of its two call sites
contributed an operator for a `describe` the body never mentions. They were harmless — a `Pointwise`
nothing reads is never evaluated — and they were *counted*, in `beck explain incremental`'s operator
totals and in every report that quoted one.

The fix is in the decomposition rather than in this pass: `Plan::unfused` ends by dropping whatever
the roots cannot reach, so no plan in the compiler has an unreachable operator and no report counts
one. Deciding it earlier — not building the argument until the body asks for it — would mean a scope
of thunks rather than of operators, which changes the order operators are created in and therefore
what the hash-consing shares; a pass afterwards costs one traversal and changes nothing else. The
roots are the page, the two sources, and **every named signal**, because a name is a read-model
table whether or not the page reads it.

**A rule that could not fire was written, found and deleted.** `flat_map over map_list` is the same
composition as `flatten over map_list`, one operator further down, and it looked necessary: a `for`
over a view of a view is a chain of maps under a flatten. It never fires. The pass scans nodes in
dependency order and restarts after each rewrite, so a chain of maps is always collapsed from the
bottom by `map_list over map_list` before the flatten above it is reached — and the refusal
conditions for the pair are the same either way, so there is no program where one is refused and the
other allowed. It is deleted, and `beck_core::fuse::RULES` now lists every rule by name so that
`fusion.rs` can assert **each one is exercised by a program somebody can open**. A rule with no
program is a rule the differential harness says nothing about, sitting in the module looking like
coverage.

**An operator the engine implements was reached by no program in the tree — and this pass is what
took the last one away.** `flatten` had exactly one shape in the whole repository: the `map_list`
under it that every `ui:` loop compiles to. Fusing that pair means `flatten` now survives only when
its collection of lists came from somewhere else, and no program in the corpus, the sketch or the
examples has one — so the arm the engine runs it with stopped being exercised, silently, in the same
commit that made the rewrite. `fusion.rs` gains a program that reaches it and
`beck_core::plan::OPERATORS` is held to the set the programs compile to, the same discipline as
`RULES` one level down.

That gate then found a hole **this work did not make**: `list_is_empty` has never reached a plan at
all. The corpus writes it twice and both are inside an `if`, which is one opaque operator — so the
engine's emptiness arm had never once been compared with recompute. It has a program now, and the
differential in this file runs **recompute as a third plan** rather than only comparing the two
plans with each other, because a fixture checked only against the unfused plan would agree with it
about a shared mistake.

**A refusal outlives the operator it was recorded against.** The first version dropped a refusal
whose consumer was later fused into something else, which is exactly the case a developer most needs
to see: on the program in §89.7 the shared `map_list` is refused, then the per-session map above it
is absorbed by the loop, and the refusal — the only line explaining why the shared half stayed
shared — vanished from the report. Refusals are now carried forward to whichever operator absorbed
the one they were recorded against, and dropped only when the pair they name actually fuses.

## 89.6 What is not built

| | Status |
|---|---|
| Fusion of the dataflow plan, on by default for every program | **built** — five rules, `Plan::compile` fuses and `Plan::unfused` is what the gate compares against |
| `beck explain query`, `beck explain cost` | **built**, per file rather than per function (§89.1) |
| A gate on the conjunction's short-circuit | **not built, and not buildable from the tree** — no view has a predicate that can fail, so both spellings render the same page (§89.7) |
| An e-graph, and cost-based extraction | **not built, and not needed by these rules** (§89.3). What would need one is a rewrite that can *add* an operator or that competes with another for the same node — pushing a `filter_list` below a `sort_by` is the first such: it is sound, it is sometimes a large win and sometimes a loss, and it is exactly the choice equality saturation exists to make |
| `filter_map` — `filter_list` over `map_list`, and the reverse | **not built.** Both are one new operator each and neither shape appears in the tree; the rule that would use them is written the day a program has one |
| `list_len` over a `filter_list` | **not built**, and it is not a fusion: a filter changes cardinality, so the count would have to be maintained as a threshold over the predicate rather than read off an arrangement |
| `count(*)` on the read-model port without scanning | **not built**, unchanged from [`88`](88-read-models-and-pgwire-report.md) §88.6. The plan's `list_len` is ±1 per delta and the SQL count still scans the rows it projected |
| Joins, subqueries, aggregates — the `Query` sub-language of §4.2 | **nothing**, unchanged (§89.1) |
| Fusing across the session cut when the fanout is one | **not built and not planned.** It would be right for a single subscriber and wrong for the second one, and the plan is compiled once per program |
| The render lock | **still here**, unchanged from [`51`](51-arrangement-lifecycle-report.md) §51.7 |

## 89.7 The gate, and what makes it go red

[`fusion.rs`](../compiler/crates/beck-cli/tests/fusion.rs). `incremental_engine.rs` already compares
the maintained view with the recomputed one, and `Plan::compile` fuses, so that harness covers this
pass — which is exactly why this one exists separately. Three things it cannot say:

1. **which plan was wrong.** The new differential runs the **fused** plan and the **unfused** plan
   over the same generated log, warm, two subscribers, every event, and compares the rendered pages
   byte for byte — more than 2,000 pages across 34 programs, the 31 above plus the three written
   for the rules the corpus does not reach — so a failure names the rewrite rather than the engine;
2. **whether a rule ever fired.** Every name in `fuse::RULES` must be exercised by a program in the
   corpus or by one written for it, and the assertion is set equality rather than a checklist, so a
   rule added without a program fails here;
3. **whether a refusal still refuses.** Both conditions in §89.3 are *pessimisations* when dropped,
   never errors, so they are asserted on programs built to make each one bite.

It also holds two published sets to the programs that reach them — `fuse::RULES` and
`plan::OPERATORS` — so a rule or an operator with no program fails here rather than sitting in a
match arm looking like coverage (§89.5).

[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5 asks what would have to be true for a
gate to go red, and to check it rather than assume it. Four mutations, each applied to the shipped
code and reverted:

| mutation | what went red |
|---|---|
| drop the `consumers > 1` check | `an_arrangement_two_operators_read_is_not_fused` |
| drop the session-cut check | `fusion_does_not_move_shared_work_per_subscriber` |
| let the count rule fire over a `filter_list` — cardinality-changing, and therefore unsound | the differential, on `examples/todo.beck` at event 12 |
| give `flat_map` `flatten`'s rebuild rule, ignoring that its function has captures (§89.2) | the differential, on `corpus/27-review.beck` at event 6 |

The last two are the ones worth having done: both are mistakes that produce a plausible plan and a
wrong page only on a program whose loop reads the session or whose collection shrinks, and neither
would have been found by reading the rule.

One thing here is **written for a reason and gated by nothing**, and it should be said rather than
left to be assumed: the conjunction's short-circuit (§89.2). No view in the tree has a predicate
that can fail, so the strict spelling would render identical pages on every program there is, and
no mutation of it goes red. It is written the short-circuiting way because that is what the pair of
operators did — and the day a program has a fallible predicate, the difference is between a page
and an engine that reset itself.

## 89.8 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`08`](08-roadmap.md) | The incremental-views bullet said "**Query fusion is the only part of this bullet still untouched**, and `beck explain query` and `beck explain cost` behind it". All three are built, and the bullet is done |
| [`04`](04-compiler-architecture.md) §4.7 | "`query` and `cost` are not [built]" — both are, and both take a file rather than a function (§89.1) |
| [`05`](05-tier-lowering.md) §5.3 | "Query fusion is not [built]" — it is, on the dataflow plan; the `Query` sub-language the same paragraph names is still symbolic and still unwritten |
| [`03`](03-type-and-effect-system.md) §3.10 | The incremental-views row's remaining item was fusion |
| [`20`](20-phase-2-report.md) §20.5 | "whether a separate `cost` view earns its place is a question for when there is a second cost dimension to show" — there is one: placement costs are about *where* a definition runs, once; `beck explain cost` is about what the program does *per event*, for as long as it runs, and no placement decision can see it |
| [`23`](23-general-slicer-report.md) §23.9 | Same two commands, same reason, now answered |
| [`26`](26-arrangement-sharing-report.md) §26.9 | Its last untouched item on this bullet. Its quoted plan sizes have each moved by one operator — `examples/todo.beck` is "28 of 42" and `24-feed.beck` "21 of 30" — because the fused plan is what `beck explain incremental` now prints |
| [`38`](38-literature-survey.md) §38.2 | Its trigger ("adopt the shape when fusion is built") has fired, and the answer is half of what it forecast: the shape is adopted, the e-graph is not, because these rewrites do not conflict (§89.3). Laddad et al. and egglog stay a **watch** against the first rewrite that competes with another |
| [`88`](88-read-models-and-pgwire-report.md) §88.6, §88.10 | "Query fusion on symbolic plans, `beck explain query`, `beck explain cost` — **nothing**" |
| `AGENTS.md` | The harness list gains `fusion` |

## 89.9 What Phase 3 is still not

**The incremental-views bullet is finished.** The plans, the recompute oracle, arrangement sharing,
the arrangement lifecycle, the read models with their pgwire exposure, and now the fusion — every
part of the line [`08`](08-roadmap.md) has carried since the phase began.

Beyond it, and unchanged: **no LLVM backend and no native codegen**; **no Mode B and no client
polish**; **no playground**; **no supply-chain tooling**; the OIDC relying party, `managed()`
provisioning, the claims mapping and presence ([`48`](48-identity-report.md) §48.5); the rest of
pattern matching ([`45`](45-error-rows-report.md)); the page is still assembled and diffed rather
than streamed as deltas ([`24`](24-incremental-views-report.md) §24.6) — and this report makes that
one *more* visible rather than less, because `beck explain cost` now prints the operator that does
it, by number, on every program that has one. `parallel:` still has no backend that runs two
children at once ([`80`](80-a-scope-owns-its-children-report.md) §80.5).

The exit criterion is a claim about a person, and no outside developer has read the guide
[`88`](88-read-models-and-pgwire-report.md) published.
