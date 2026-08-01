# 23 — Phase 3 report, part 2: the general slicer, and the analysis it unblocks

[`22`](22-phase-3-report.md) §22.6 opens its list of what Phase 3 is not with this:

> **The general slicer is still not built**, and it is now debt with two phases' names on it.
> [`19`](19-phase-1-report.md) §19.9 assigned it to Phase 2; [`20`](20-phase-2-report.md) §20.5
> named it as the one item that phase was handed and did not deliver, and §20.6 item 6 said "Phase 3
> pays for it either way". Phase 3 has not. … **It should be the next thing built.**

It is built. So is the first thing that needed it: `beck explain incremental`, which
[`03`](03-type-and-effect-system.md) §3.8 asks for by name and [`20`](20-phase-2-report.md) §20.5
recorded as unbuilt "because there is nothing to say until §3.8's incremental view compilation
exists". That was half right. There is nothing to *maintain* until the engine exists; there is
plenty to *say*, and saying it needs a plan rather than an inlined expression. §23.8.

This report says what both do, what building them found, and what they still refuse.

It is **part 2**, and the title says so for the same reason [`22`](22-phase-3-report.md)'s did:
Phase 3 is twelve bullets, two of them are now built, and ten are not. §23.9 lists the ten — and
the analysis in §23.8 is a *piece* of one of the ten, not the bullet.

## 23.1 What was asked for, and what is there

[`19`](19-phase-1-report.md) §19.9 stated the debt precisely:

> **`Roles` and the `Inliner` encode one topology — Phase 2.** The splitter produces a fixed
> seven-field struct and inlines four combinators by name. That is the shape `todo.beck` has, and
> §3.7 says the signal graph is a *graph*. … A general slicer belongs with placement inference,
> since both are about treating the graph as a graph.

| Asked for | Status | Where |
|---|---|---|
| The signal graph as a graph, not a recognised shape | done — one vertex per signal *operation*, including the ones nested inside a declaration | `beck-core/src/signal.rs` |
| Any number of durable folds | done — **fused** into one accumulator, one field per fold | `split.rs::fuse` |
| Any depth and any sharing between the fold and the page | done — a signal read twice is bound once, not inlined twice | `split.rs::Slicer` |
| A `filter_map` between the chokepoint and a fold | done — the filter moves into the fused step | `split.rs::fold_field` |
| Cycles handled as cycles | done — SCCs via the existing condensation, with "every cycle contains a fold" as the rule that makes slicing terminate | `signal.rs::Graph::build` |
| Every tier crossing enumerated, with the id §4.3 says a subscription is keyed by | done — replaces one hard-coded sentence | `signal.rs::Cut` |
| `beck explain flow` printing the graph rather than four names | done | `split.rs::flow_report` |
| The runtime driving several accumulators or several pages natively | **not done** — §23.6 | — |
| `beck explain incremental <view>` (§3.8, §4.7 — not part of the slicer's brief, and the first thing that needed it) | done — the analysis; the engine is not built | `beck-core/src/incremental.rs` |

429 tests, no failures, no compiler warnings, no clippy warnings — up from
[`22`](22-phase-3-report.md)'s 396. The corpus is 26 programs, up from 23; three of them exist for
this work, and each carries its own tests written in Beck.

## 23.2 The defect the debt was hiding

Two phases described the narrowness the same way, and both descriptions were generous:

> What makes this legitimate narrowness rather than a defect is that it **announces itself**: nine
> diagnostics (B0500–B0508, plus placement's B0400–B0403) refuse every shape the splitter does not
> understand, and `split` returns `None` whenever one fires. A test pins that — an unsliceable
> program is refused with a code and a message, never quietly mis-sliced.
> — [`19`](19-phase-1-report.md) §19.9, quoted again by [`20`](20-phase-2-report.md) §20.5

That claim was false, and here is the program that falsifies it:

```python
proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, roster, validate)
tally: Signal[Tally] = durable(fold(apply_event, Tally(joins=0, leaves=0), events))
roster: Signal[Roster] = durable(fold(apply_presence, Roster(here={}), events))
page: Signal[Html] = map2(render, tally, roster)
```

The old splitter found the durable fold with `signals.iter().find(|s| … Prim::Durable)` — the
**first** one — and its `Inliner` lowered *every* reference to a durable signal to the same state
parameter. So `render` was handed a `Tally` where it expected a `Roster`, and the second fold's step
function was never called at all. Here is the compiler at commit `233d532`, on that file:

```console
$ beck check 21-two-folds.beck
ok: 4 definitions, 5 signals, wire id f66ea4ec97d9f651

$ beck explain flow 21-two-folds.beck
ingress      proposals
events       events  (validate)
state        tally  (durable fold)
page         page  (broadcast view)

inlined into the view: roster
(full recompute per event; Phase 3 makes these incremental)

one tier crossing: `page` is @on(client) over state that is @on(data), so the edge is a
single subscription carrying DOM patches.
```

`state` is `tally`, and `roster` is listed as "inlined into the view" — inlined, that is, *as
`tally`*. The report of the mis-slice is right there in the output, and it reads as normal. Add one
of the file's own tests and the whole of what a developer would learn is:

```console
$ beck test 21-two-folds.beck
test "each fold sees the same log and keeps its own answer" … FAILED
  rendering the page for `test`: rendering the view
```

A message about the wrong thing, three stages from the cause. This is exactly the outcome the
narrowness was justified by preventing, and it went unnoticed for two phases because no program in
the corpus had two folds. A refusal that is never exercised on the shape it exists to refuse is a
claim, not a check.

(Both transcripts are reproducible: `git worktree add … 233d532`, copy
[`corpus/21-two-folds.beck`](../compiler/corpus/21-two-folds.beck) in with its `test` blocks removed
— that compiler has no `state.field` in a test, which is §23.4's other half — and run the two
commands.)

Two smaller versions of the same pattern were in the same function. `find(Prim::MergeClients)` took
the first ingress — harmless, because placement's `B0403` already refuses a second. And the page was
`signals.iter().find(|s| s.tier == Tier::Client)`, the first *client-placed signal*, which is not
the same thing as the page: in the program above, placement puts an intermediate `Signal[Html]` on
the client too, and the splitter would take that as the page and never slice the real one. The
general slicer takes the client-placed **sink** — a vertex nothing reads — which is what "the page"
actually means.

[`compiler/corpus/21-two-folds.beck`](../compiler/corpus/21-two-folds.beck) is that program, with
its own tests, and `two_durable_folds_are_fused_rather_than_confused` and
`each_fused_fold_folds_its_own_events_and_the_view_reads_the_right_one` are the two halves of the
regression: one asserts the shape, the other runs it. The second is the one that matters — "it has
two fields" is a claim a compiler that filled both from one fold would also pass.

## 23.3 What a vertex is

The graph has one vertex per signal *operation*, not per declaration. `todos: Signal[State] =
durable(fold(apply_event, empty, events))` is **two** vertices, a `Durable` over a `Fold`, because
the fold is a node in the dataflow whether or not the program gave it a name. Only the outermost
carries the declared name; the inner one is labelled `todos·fold`, and `beck explain flow` prints it:

```console
$ beck explain flow examples/todo.beck
signal graph — 5 vertices, 1 cycle, 3 tier crossings

  proposals   merge_clients()           server
  todos·fold  fold(events)              data     ↺
  todos       durable(todos·fold)       data     ↺
  events      decide(proposals, todos)  server   ↺
  page        per_session(todos)        client   ← the page, per session

accumulator
  one durable fold — `todos` : State

the view recomputes, per event
  nothing between the accumulator and the page
  shared: —  (no signal is read by two consumers, so nothing is bound twice)
  (§5.3 makes these incremental; today every one is a full recompute)

tier crossings — each is one subscription, resumable by (id, seq) (§4.3)
  todos → events       data → server  carries State  01b13586d89df8b7
  todos → page         data → client  carries State  1f4e63cda7c3a8e8
  events → todos·fold  server → data  carries Event  7c389d7f875d517a
```

That is the difference between a graph and a pattern: `map2(f, durable(fold(…)), summary)` needs no
new case, because there was never a case to begin with. It is also why the anonymous vertices had to
be labelled rather than left nameless — a diagnostic about a fold the program did not name still has
to point at something a reader can find.

**Three crossings, not one.** The old report line said "one tier crossing", and the sketch has
three. Two of them are in the cycle — the chokepoint reads the accumulator and the fold reads the
chokepoint's output — and §4.3 is explicit that *every* edge crossing tiers is a subscription. The
sentence was true of the edge the author was thinking about and false of the program.

## 23.4 Fusion, and why it is the right answer rather than a workaround

§3.7 fixes "**one totally-ordered log per application**". A program with two `durable` folds has not
asked for two logs; it has asked for two projections of one. So the slicer compiles them into a
single accumulator — a synthetic record with a field per fold, named for the signal that declared it
— and synthesises the step:

```text
$State = { tally : Tally, roster : Roster }

step(s, env) = $State{ tally  = apply_event(s.tally, env),
                       roster = apply_presence(s.roster, env) }
init         = $State{ tally = Tally(joins=0, leaves=0), roster = Roster(here={}) }
```

Three consequences, each of them the point:

* **The runtime is untouched.** `Roles` is still the five things `beck-rt` drives, the log still
  holds one totally-ordered stream, and snapshots, digests, `beck replay --verify` and the
  determinism argument are unchanged — because the thing they are about is unchanged.
  `a_fused_accumulator_replays_bit_for_bit` drives a fused program through the real ingress and
  asserts that a fold from genesis, a fold to head and the live process's state all agree.
* **A one-fold program is bit-for-bit what it was.** No wrapper, no synthetic type, the program's
  own `State` is the accumulator and `roles.fold` is still `Global("apply_event")` rather than a
  synthesised lambda. `a_single_fold_program_is_untouched_by_fusion` pins that, because a
  generalisation that changes the common case is a migration, not a generalisation.
* **`$State` is unwritable and unpublished.** The name cannot be typed in the surface syntax, and it
  is registered in `program.types` but not in `own_types`, so no `.becki` carries it. It *is* hashed
  into the wire id, structurally, which is correct: adding a fold changes what a subscriber can be
  sent.

**The chokepoint keeps reading the fold it named.** `decide(proposals, roster, validate)` names one
accumulator, and `validate` takes that one's type. With a fused state the slicer has to project the
right field out, and picking the wrong one is a defect no type in the sliced `Core` would catch —
`Core` is not re-typechecked after lowering. `the_chokepoint_reads_the_fold_it_was_given` declares
the folds in the *opposite* order from the one the chokepoint uses, so a slicer that took "the first
fold" fails it.

### A fold can take a slice of the log

`filter_map` between the chokepoint and a fold used to be refused: `decide` had to be the fold's
immediate input. It is now compiled, by moving the filter into the step —

```text
let o = only_flagged(env.body) in
if is_some(o) then apply_escalation(s.escalations, env with body = o.value)
              else s.escalations
```

— which is the only place it could go if replay is to stay a function of the log. The runtime still
appends one stream and folds it once; the filter decides which folds advance.
[`compiler/corpus/23-slices.beck`](../compiler/corpus/23-slices.beck) is a ledger that folds
everything and an escalation register that folds only large refunds, and its tests assert that the
two disagree in the right direction.

It is written with `Option`'s two answers and the language's own `if`, rather than a synthesised
`Match`: a `Match` arm carries a pattern, and this needs no pattern.

## 23.5 Sharing, and what it is for

A signal read by two consumers is now bound once:

```text
the view recomputes, per event
  tally
  shared: tally  (read by more than one consumer, so computed once)
```

The old splitter inlined per use, so a program whose page and read model both read `tally`
recomputed `summarise` twice per event, and nothing in the compiler recorded that the two were the
same computation. `a_signal_read_twice_is_computed_once` asserts both halves: `roles.shared` names
it, and `summarise` is applied exactly once in the sliced `Core`.

This is a constant-factor improvement today and it is not why it was built.
[`05`](05-tier-lowering.md) §5.3:

> a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one* shared
> dataflow whose final per-session operators (filter, project, diff) run per subscriber — the
> differential "arrangement" sharing model — not a thousand plans.

An arrangement can only be shared if something knows which computations are the same one. Under an
inlining splitter that information was destroyed at the point where it was needed; under a plan it
is written down. That is the whole reason [`20`](20-phase-2-report.md) §20.6 item 6 said Phase 3
pays for the slicer either way.

### What the type system was already doing

Building the graph builder's refusals found that most of them cannot fire. `Signal[T]` and
`Stream[T]` are ordinary types, so unification already rejects `per_session(events, view)` — a view
reading a stream — with a type error, before the slicer sees it. `B0507`'s view arm and most of
`B0508` are therefore refusals that *should* be unreachable, which is the same position
[`19`](19-phase-1-report.md) §19.7 took about the durable path's refusal: "an unreachable refusal is
the right thing to have while the proof is missing." The shapes that do reach the graph builder are
the ones the types permit and the dataflow does not — a conditional between two signals, a cycle
with no fold in it, a fold nobody made durable. The tests assert which stage refuses only where the
answer is interesting, and otherwise assert only that *some* stage names itself.

## 23.6 What it still refuses, and why each refusal is about meaning

The narrowness that remains is not "the slicer understands N shapes". Every refusal below is a
statement about what a program would mean:

| Code | Refuses | Because |
|---|---|---|
| `B0509` | a cycle with no fold in it | a fold is where a cycle bottoms out — an accumulator is a value the slicer can take as a parameter. Without one there is no first value to compute, and a slicer that did not check would recurse until the stack ran out |
| `B0510` | two client-placed sinks | the slicer slices both; the runtime serves one document per connection, and choosing between them is **routing** — a Phase 3 client bullet that is not built. The diagnostic says so rather than implying the slicer cannot |
| `B0511` | a second `decide` | §3.5 rests on authority being one place. Two chokepoints are two answers to "may this actor do this", and the log records whichever ran |
| `B0513` | a `fold` nobody made `durable` | the log is what survives; an accumulator outside `durable` would be rebuilt from nothing on every deploy |
| `B0504` | a fold whose stream is not the chokepoint's output | the log holds what the chokepoint decided |
| `B0512` | a `decide` reading something that is not a durable fold | `decide` threads *the accumulator*, which is what makes first-writer-wins and ownership decidable (§3.7) |

`B0510` is the one worth reading twice, because it is the first diagnostic in the compiler that
names the **runtime** as the limit rather than the compiler. That distinction is now expressible,
and it was not before: under the old splitter "the slicer cannot" and "the runtime cannot" were the
same sentence.

## 23.7 The corrections this makes to the design documents

Applied to those documents in this commit, listed here so the diff is reviewable as a set:

| Document | Correction |
|---|---|
| [`03`](03-type-and-effect-system.md) §3.7 | Several `durable` folds are one accumulator, not several logs — the sentence "one totally-ordered log per application" now has a compilation rule attached to it |
| [`04`](04-compiler-architecture.md) §4.3 | "Every signal edge that crosses tiers becomes a subscription" is enumerated and given content-derived ids, rather than being one crossing named in prose |
| [`05`](05-tier-lowering.md) §5.3 | Arrangement sharing has its compile-time input: the plan says which computations are one computation |
| [`08`](08-roadmap.md) | The general slicer is marked built, against Phase 2's bullet where it was assigned and Phase 3's where it was paid |
| [`19`](19-phase-1-report.md) §19.9 | The claim that the narrow splitter "never quietly mis-sliced" was false; §23.2 is the program |
| [`20`](20-phase-2-report.md) §20.5 | Repeated the same claim; same correction |

Reports are history and are not rewritten. The two corrections above are recorded here rather than
edited into [`19`](19-phase-1-report.md) and [`20`](20-phase-2-report.md), which is the convention
[`18`](18-phase-0-report.md) established and [`AGENTS.md`](../AGENTS.md) states.

## 23.8 The analysis, not the engine: `beck explain incremental`

[`03`](03-type-and-effect-system.md) §3.8, in the sentence that names the command:

> Arbitrary pure code is incrementalized where analysis allows, recomputed where not — **`beck
> explain incremental <view>` shows which, and why**.

[`20`](20-phase-2-report.md) §20.6 item 3 said the input already existed — "a view whose row is
empty is a pure function of the signal — which is §3.8's precondition for compiling it to a
differential-dataflow plan" — and §20.5 said the command was not built. The missing piece was not
the row; it was the *plan*. An inlined view is one expression, and "which views are incremental" is
not a question one expression can answer. That is why this arrives with the slicer and not before.

```console
$ beck explain incremental corpus/22-shared.beck
Every view below is a **full recompute per event** today. This is the analysis §3.8 asks
for — which views a differential-dataflow plan could maintain by delta, and why the rest
could not — and the engine that would maintain them is not built (docs/23 §23.9).

  tally   incremental    (shared)
            map_len        ±1 per insert or remove
            >              pointwise
  digest  incremental
            +              pointwise
            str            pointwise
  page    incremental    (per session)
            html_el        a subtree delta — what the patch protocol already streams
            html_text      a text patch
            +              pointwise
            str            pointwise

the shape a plan would have (§5.3)
  shared arrangement: tally
  per subscriber:     page  (one plan, these operators per connected session)

3 of 3 views could be maintained by delta.
```

**The first line is the deliverable as much as the table is.** A command called `explain
incremental` that let a reader believe their view was being maintained would be the most misleading
output in the compiler, and `the_report_says_what_is_true_today_before_it_says_anything_else`
asserts that the disclaimer is literally the first line rather than a footnote.

### The rule, in the order the answers are useful

1. **The row is empty** — §3.8's precondition, from the inference Phase 2 built. A view that
   performs an effect is re-evaluated when the effect says so, and no delta rule applies. Ambient
   effects (§3.2's `log`, `metrics`) do not count, because they force nothing.
2. **Every operation has a delta rule.** `RULES` is the table, and like [`cost.rs`](../compiler/crates/beck-core/src/cost.rs)'s
   numbers it is **stated, not measured**: each entry is a rule the differential-dataflow
   literature already has, written down so it can be argued with. Nothing in the module claims an
   implementation of any of them.
3. **It is a view at all** — a vertex applying a function between the folds and a sink. The
   ingress, the chokepoint and the folds are not assessed; a fold is not maintained by delta, it is
   what *produces* the deltas.

The blocker is reported by name, and it is the *first* one in source order, because the first is
the one to fix:

```console
  chosen  recompute
            a `match` on the input picks which computation runs, and a delta can move it between arms
```

A `match` is the interesting refusal. Differential dataflow does handle branching — the scrutinee
becomes a collection and each arm a branch of the plan — and that is a real technique that is not in
this table. Calling it "recompute" is the conservative answer and the message says which rule was
missing rather than "unsupported".

### What it says about §5.3's shape

The last block is the one the slicer was built for. [`05`](05-tier-lowering.md) §5.3:

> a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one* shared
> dataflow whose final per-session operators (filter, project, diff) run per subscriber

Both halves of that sentence are now things the compiler can point at: the **shared arrangement** is
the set of vertices read by more than one consumer, and the **per-subscriber** set is the closure
below `per_session`. In `22-shared.beck` the answer is "share `tally`, run `page` per connection",
which is the plan §5.3 describes, stated by the compiler about a program rather than by a document
about a hypothetical.

### What this deliberately does not do

* **It does not maintain anything.** Repeating that here because it is the only claim worth
  guarding.
* **It does not check the table.** Every rule in `RULES` is an assertion about what a view engine
  could do, and the only test on them is that none is empty or duplicated. The oracle that would
  check them — recompute, which [`05`](05-tier-lowering.md) §5.3 calls "a luxurious position for
  CI" — needs an incremental plan to compare against, and there is not one.
* **It says nothing about cost.** "Incremental" is not "fast": a maintained plan has memory the
  recompute does not, and §5.3's per-session memory is one of the three metrics that section says
  to export. Neither number exists.
* **Every corpus view comes back `incremental`, and that is a fact about the corpus.** They are
  list, map, count and `ui:` computations, which is what the delta rules cover. The harness
  therefore asserts the three verdicts on programs written to produce them, because a corpus-only
  test would pass with an analysis that answered "incremental" to everything.

## 23.9 What Phase 3 is still not

**Two bullets of twelve are built.** Nothing below has been started, and the phase's exit criterion
— "an outside developer builds a non-trivial app from documentation alone" — is not met.

- **No incremental view engine.** This is the bullet the slicer *unblocks*, not the bullet it
  delivers. There is no differential-dataflow plan, no arrangement, no delta stream, no SQL read
  model, no pgwire, no query fusion. **Every view is a full recompute per event**, exactly as it
  was. What exists now is the analysis (§23.8) — which views *could* be maintained, and why the
  rest could not — and it says that sentence before it says anything else.
- **`beck explain query` is still unbuilt**, for the reason [`20`](20-phase-2-report.md) §20.5
  gave: the `Query` sub-language is deliberately symbolic and there is no plan to explain until the
  engine compiles one. `beck explain cost` is still unbuilt too, and still for §20.5's reason —
  `beck explain place` prints every candidate's cost, and a second view earns its place when there
  is a second cost dimension to show.
- **The runtime drives one accumulator and one page.** Fusion means a program may declare several
  folds; it does not mean the runtime holds several. Several pages is `B0510` and needs a router.
  Both limits are the runtime's and both now say so.
- **A non-durable fold has no meaning yet.** `B0513` refuses it. §3.7 describes `fold` without
  `durable` as a perfectly ordinary signal operation, and the honest position is that the runtime
  has nowhere to keep one — not that the language should not have it.
- **No LLVM backend and no native codegen**, unchanged from Phases 1, 2 and 3-part-1.
- **No Mode B, no client polish, no `test --update`, no structured concurrency, no `Result`/error
  rows, no SQLite substrate, no standard library v1, no identity beyond a dev-mode actor, no LSP, no
  playground, no supply-chain tooling.** All Phase 3 bullets, all untouched.
- **`check.rs` is 2,806 lines.** [`22`](22-phase-3-report.md) §22.6 recorded 2,644 and asked the
  next phase to open that file to *move* something out rather than add to it. This work added
  fifteen lines — the fused accumulator's test subject, delegated to `signal::durables` so the
  checker and the slicer cannot disagree about how many folds a program has — and moved nothing.
  The test-checking pass §22.6 named is still there, and moving it is still the right next edit.
  (The 2,644 → 2,791 step between the two reports is the stub-body commit, not this one.)
- **`split.rs` grew from 615 lines to 1,400, and gained `signal.rs`'s 564 and
  `incremental.rs`'s 508 beside it.** That is the honest cost: excluding tests, 396 lines became
  1,133 plus two new modules. A large share of the new volume is diagnostics and their notes, and
  `beck explain flow` — eleven lines of `println!` printing four names — became a report derived
  from the graph. What the split into modules buys is that the graph is a separate concern from the
  slice and from the analysis: `signal.rs` knows nothing about roles, `split.rs` does no
  pattern-matching on program shape, and `incremental.rs` reads the plan rather than the program.

## 23.10 What this changes for the rest of Phase 3

1. **The incremental view engine has a plan to compile.** §5.3's arrangement sharing needs to know
   which computations are shared; §3.8's `beck explain incremental <view>` needs to know which
   vertices are pure functions of a signal. Both are vertex properties of `signal::Graph`, and both
   were unrepresentable in an inlined expression.
2. **Recompute is the oracle, and it is now a *comparable* one.** [`05`](05-tier-lowering.md) §5.3
   calls full recompute "an optimisation with an exact correctness oracle (recompute) to test
   against — a luxurious position for CI". Testing an incremental plan against a recompute needs
   both to be *plans*; one of them now is.
3. **The corpus has a third dimension.** It measured placement, then behaviour, and now topology.
   Adding a program with an unusual signal graph is the cheapest way to find out whether the slicer
   is as general as this report says.
4. **A defect found by writing the program down is worth three found by reading the code.** §23.2's
   mis-slice survived two reports that quoted the refusal it was supposed to fall under. It took
   forty lines of Beck to find. Every remaining Phase 3 bullet should ship with the program that
   would embarrass it.

