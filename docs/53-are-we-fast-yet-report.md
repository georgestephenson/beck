# 53 — Are We Fast Yet, in Beck

**Built.** All fourteen of Are We Fast Yet's benchmarks, ported into
[`compiler/awfy/`](../compiler/awfy/README.md) and verified against the original suite's own
constants, with a gate and a measurement. Nine micro-benchmarks first, then the five
macro-benchmarks — Richards, Json, Havlak, CD and DeltaBlue — which are the ones nothing was known
about.

> **This document replaces seven reports** — six that were one per port, and one on the flag the
> ports asked for. A port established the same thing each time (this feature has a Beck shape, and
> here is its constant); what is worth keeping is the fuel wall (§53.3) and how it was taken
> (§53.8), the three things the suite found missing in the language (§53.5), and the bug class only
> a benchmark has (§53.6). Consolidated under [`AGENTS.md`](../AGENTS.md)'s rule that a report is
> for a phase or a subsystem.

## 53.1 What was ported, and what each is verified against

Each benchmark is checked against a number the *original suite* asserts, read from its own source.
`awfy.rs` is the gate; a number invented here would defeat the point of porting somebody else's
benchmark, so none of the constants is this project's.

| | verified against |
|---|---|
| Nine micro-benchmarks (Bounce, List, Mandelbrot, NBody, Permute, Queens, Sieve, Storage, Towers) | each one's own checksum |
| **Richards** | the suite's two constants — a mutable object graph and a scheduler |
| **Json** | the suite's own count, over a hand-written parser |
| **Havlak** | **1,605 loops over 5,213 nodes** — union-find with path compression |
| **CD** (collision detection) | **42 collisions** between two aircraft over 200 frames — a red-black tree with deletion |
| **DeltaBlue** | nothing: the original asserts no answer for it, so the port asserts that it runs and is stable, which is weaker and is said so here rather than dressed up |

## 53.2 What it establishes, and what it does not

**Expressiveness.** Every one of Are We Fast Yet's common-subset features has a Beck shape, and the
five that looked least likely — a mutable object graph, a cyclic constraint network, a red-black
tree with deletion, union-find with path compression, and a hand-written parser — all have one. That
was genuinely unknown when the harness started: the five macro-benchmarks were recorded as "not
ported, not attempted and not declined … nothing is known about whether they are expressible", which
was the largest unqualified admission in the first of these reports. Each was then assessed from its
own source before being attempted, and none turned out to be blocked by the language — only by size.

**Nothing about speed.** [`25`](25-benchmarks-and-expressiveness.md) §25.9's rule is unchanged after
fourteen ports: **no comparative claim until there is a second backend**. What exists now is the
baseline that rule was waiting for, taken before the thing it measures got good — which is §25.3
item 1's whole argument, and which [`70`](70-the-evaluator-gets-fast-report.md) then made worth
having.

## 53.3 What it costs, and the wall three of them hit

The suite runs in `beck test`, so a benchmark's cost is a check and then a run. The check is
3–5 ms for every one of them; the run is the benchmark:

| benchmark | `beck check` ms | `beck test` ms |
|---|---:|---:|
| mandelbrot | 4.1 | 4.9 |
| nbody | 5.4 | 7.2 |
| bounce | 4.5 | 59.6 |
| queens | 3.8 | 80.8 |
| permute | 3.4 | 105.3 |
| towers | 4.8 | 174.3 |

**Three of the fourteen do not fit in the evaluator's 50,000,000-step fuel budget at the size their
own suite specifies**: `mandelbrot` at 500, `havlak` at its published configuration, and `deltablue`
at `extra_args: 12000`. Each is gated at a smaller size and says so in its own file.

Three is enough to stop calling it a finding and start calling it a missing feature. The budget
itself is right — a runaway program should stop — but a backstop nothing can raise is a ceiling, and
a benchmark suite is exactly the kind of legitimate program that lives above it. **`beck test` needs
a `--fuel`, or a program needs a way to declare its own budget.** §53.8 is that flag, and it took
this wall as its brief: all three now run at the size their suite measures at.

Everything below the wall scales the way it should: DeltaBlue at `n = 200` is 1.6 s and at
`n = 1,000` is 8.9 s, which is a constraint solver's shape, so what stops 12,000 is the budget
rather than the algorithm.

## 53.4 What a port is allowed to change

A port is held to the **answers**, not to looking alike. Three rules, the third of which the
macro-benchmarks introduced:

1. The verification constant is the original's and may not move.
2. What the port does *less* of than the original — no reflection, no class hierarchy where a union
   does — is recorded per file in [`compiler/awfy/README.md`](../compiler/awfy/README.md), which is
   where a reader goes to judge whether a comparison is fair.
3. A mutable object graph is transcribed rather than tidied. Richards is a scheduler built on
   aliasing and mutation; rewriting it into an idiomatic fold would be benchmarking a different
   program, so it keeps its shape and pays for it.

## 53.5 The three things the suite found missing, none of them about speed

This is what a common-subset suite is *for*, and it is worth more than the timing table.

**`and` and `or` did not short-circuit.** `a and b` is now `if a: b else: false` and `a or b` is
`if a: true else: b`, lowered **in the checker rather than the evaluator**, because short-circuiting
is a property of the language rather than of one backend. The effect row is deliberately unchanged —
both operands may still run, so both are still charged — and the view engine's cost was measured
rather than assumed.

**The language had no `sin`, `cos` or real-to-integer truncation.** CD needed all three; two
primitives were added.

**It has no bitwise operators**, recorded rather than fixed: nothing in the suite needed them badly
enough to justify the surface.

One language change also arrived through a port rather than a design: **a float literal may carry an
exponent** (`1e6`), because a literal whose value is not representable as an `Int` must not lex as
one ([`02`](02-syntax.md)).

## 53.6 A workload with no oracle

Havlak's discarded-run loop passed the run's own **loop count** back as its loop counter. A run finds
about 1,600 loops, so the counter became 1,602 on the first pass and the loop ran **once instead of
fifty times**.

Every verified number was still right — `benchmark(50)` returned `LoopCount(loops=1605, nodes=5213)`
and the test passed. What gave it away was that it took 4.3 s, and 51 runs of a 5,213-node graph do
not.

**The class of bug is specific to benchmarks.** `verifyResult` checks an *answer*; the discarded
runs produce no answer, by construction — that is what "discarded" means. So a benchmark's own oracle
is structurally blind to whether its main workload ran at all, and the only instrument that sees it
is the clock. Any harness with warm-up runs has this hole.

## 53.7 What is not built

- **A comparative number.** §25.9 holds it, and fourteen ports do not change that.
- **Three benchmarks are *gated* below their published size** (§53.3). They run at it with
  `--fuel` (§53.8); what the gate runs is now a decision about how long `cargo test` should take
  rather than a limit of the tool.
- **DeltaBlue verifies against nothing** (§53.1).
- **No bitwise operators** (§53.5).

## 53.8 The budget is a default, not a ceiling

**Built.** `beck test --fuel`, and with it the three benchmarks §53.3 could not run at the size
their suite measures at.

The evaluator stops a call after 50,000,000 steps (`interp.rs::DEFAULT_FUEL`). That is a
runaway-program backstop and it is right: a tree-walker with an accidental infinite loop should stop
rather than run until somebody notices. What made it wrong was that **nothing could raise it.** A
backstop that cannot be raised is a ceiling, and the difference only shows up when a program
legitimately wants to be above it. Each of §53.3's three was reported as a fact about the
*benchmark*; three of them is a fact about the **evaluator**, and that is the whole content of the
change — nothing about the budget was wrong except that it was the only one available.

One flag, and one builder behind it. `Evaluator::with_fuel` sets the per-call step budget,
`beck_eval::backend_with_fuel` is the backend built with one, and `beck test --fuel N` is how a
person says it. The default is unchanged and is named rather than inlined —
`beck_eval::DEFAULT_FUEL` — so the flag's default *is* the backstop rather than a second number that
could drift from it.

**Per call, not per process.** `constant` and `function` each build a fresh `Interp`, so the budget
bounds one evaluation. That was already true; it is now written down, because "how much fuel does a
test get" has an answer only if you know what a call is. `beck run` does **not** take the flag: a
fold runs per event and each event is its own call, so a long-running application never accumulates
against the budget — and a fold that cannot finish one event is the case the backstop is for.

All three benchmarks now run at the size their suite measures at. Release build, and the budget
needed is stated rather than rounded up silently:

| | at the suite's size | `--fuel` |
|---|---|---|
| `deltablue`, `n = 12,000` | **1 m 51 s** | 2,000,000,000 |
| `havlak`, 50 discarded runs | **1 m 32 s** | 4,000,000,000 |
| `mandelbrot`, size 500 | **43.4 s** | 4,000,000,000 |

Those are not published numbers and are not compared to anything —
[`25`](25-benchmarks-and-expressiveness.md) §25.9's rule survives this unaltered. They are here
because "it fits now" is a claim that needs a number behind it, and because the *ratio* is the
useful part: the default budget is roughly an eightieth of what DeltaBlue's published configuration
needs, so no smaller default would have helped.

**The gates are unchanged.** `awfy/`'s files still run their reduced configurations, because a suite
that took three minutes per benchmark is a suite nobody runs. What changed is that the reduction is
now a *choice about the gate* rather than a limit of the tool, and §53.3's two tests — the answers do
not depend on the parameter, and each discarded run does the whole job — are what make that choice
honest.

**`mandelbrot` reaches the suite's *default* size, and that is worth more than the other two.**
`deltablue` and `havlak` now run at the configuration the suite **measures** at; `mandelbrot` at
size 500 produces **191**, which is `Mandelbrot.java`'s own `verifyResult` at the size Are We Fast
Yet's published results are about. §53.3 had to record that as a caveat — the port verified at size
1, which the suite publishes a value for but does not measure at — and it took 43.4 s and a flag
rather than any change to the port.

`awfy.rs::the_fuel_budget_is_a_default_rather_than_a_ceiling` asserts **both** directions on the
same program: a loop long enough to exhaust the default is stopped without the flag, and completes
with it. Both, for [`20`](20-phase-2-report.md)'s reason — a flag that raises a ceiling has to be
shown raising it *and* shown that the ceiling is still there without it, or it is indistinguishable
from having removed the ceiling. That test uses a synthetic loop rather than a benchmark on purpose:
it takes about a hundred seconds, where the smallest benchmark that would demonstrate the same thing
takes ninety.

What the flag is not:

| | Status |
|---|---|
| A program-declared budget | **not built.** `--fuel` is a person's decision at the command line; a `@budget` on a definition, or a manifest, would be the program's. Nothing needs one yet |
| `--fuel` on `beck run` | **not built**, and the reason is that a fold is a call per event |
| A budget on the *depth* ceiling | **not built**, and deliberately: `Interp::with_max_depth` only lowers, because the number that is safe is a property of the host stack rather than of the program ([`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)). Fuel is not like that — it bounds work, not memory |
| A default that is not 50,000,000 | **unchanged.** The ratio above is why: nothing between the old default and what a benchmark needs would have helped anybody |
| Reporting how much fuel a run used | **not built.** A program that ran out is told so; a program that nearly did is not |

One caveat this section cannot close, and it is not a fuel bug: the budget counts **nodes**, not
work. [`46`](46-standard-library-report.md) §46.14 is where a primitive that copies ten thousand
values turned out to be one step.
