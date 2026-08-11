# `awfy/` — Are We Fast Yet, in Beck

[`docs/25`](../../docs/25-benchmarks-and-expressiveness.md) §25.2 picks **Are We Fast Yet** (Marr,
Daloze, Mössenböck, DLS '16) as "the methodologically strongest choice for Beck's core", because it
is written against a *common subset* of language features so that a comparison is of
implementations rather than of standard libraries. §25.9 schedules the harness for Phase 3 and
schedules the number for later. This directory is the harness.

[`docs/53`](../../docs/53-are-we-fast-yet-report.md) is the report for the nine micro-benchmarks:
what it establishes, what it refuses to claim, and the three findings porting it produced.
[`docs/53`](../../docs/53-are-we-fast-yet-report.md) is the report for `richards`, the first macro-benchmark
— a mutable object graph in a language with no mutation — and it assesses the remaining four.

## What is here

**All fourteen** — Are We Fast Yet's nine micro-benchmarks and all five of its macro-benchmarks —
each a Beck **library** whose `test` block is the original's `verifyResult` with the original's own
constant in it. DeltaBlue is the exception and says so below: the suite publishes no constant for
it.

| File | What it measures | The suite's own number |
|---|---|---|
| [`bounce.beck`](bounce.beck) | Object allocation and field update, 100 balls × 50 steps | 1,331 wall hits |
| [`list.beck`](list.beck) | Calls and linked-node allocation — Takeuchi over three chains | a tail 10 links long |
| [`mandelbrot.beck`](mandelbrot.beck) | Float loops and bit packing | checksum 128 at size 1 |
| [`nbody.beck`](nbody.beck) | Float arithmetic and `sqrt` | energy `-0.16907495402506745` after one advance |
| [`permute.beck`](permute.beck) | Recursion and array swaps | the procedure entered 8,660 times |
| [`queens.beck`](queens.beck) | Backtracking search | eight queens placed, ten times |
| [`sieve.beck`](sieve.beck) | Array writes in a loop | 669 primes at or below 5,000 |
| [`storage.beck`](storage.beck) | Allocation — a four-way tree seven deep | 5,461 nodes |
| [`towers.beck`](towers.beck) | Stack discipline and recursion | 8,191 moves |
| [`richards.beck`](richards.beck) | **Macro.** An OS task scheduler — a mutable object graph | 23,246 packets queued, 9,297 holds |
| [`json.beck`](json.beck) | **Macro.** A JSON parser over 25,820 characters of RAP protocol traffic | an object whose `operations` array has 156 entries |
| [`havlak.beck`](havlak.beck) | **Macro.** Loop recognition — DFS, edge classification and union-find over a control-flow graph | 1,605 loops over 5,213 nodes |
| [`cd.beck`](cd.beck) | **Macro.** Aircraft collision detection over a red-black tree of voxels | 42 collisions between two aircraft |
| [`deltablue.beck`](deltablue.beck) | **Macro.** An incremental constraint solver over a cyclic object graph | **nothing** — the assertions inside its own planner ([`53`](../../docs/53-are-we-fast-yet-report.md)) |

`beck-cli/tests/awfy.rs` gates the directory: a file added here is run by being here, and all fourteen
names are enumerated in one place — micro and macro apart, because that is the suite's own division
— so dropping one is a red test rather than a shorter table.
`measure_awfy.rs` prints wall-clock and gates on nothing.

## Provenance

These are ports. The originals are the Java implementations in
[are-we-fast-yet](https://github.com/smarr/are-we-fast-yet), which are themselves derived from the
SOM class library, and — for `mandelbrot` and `nbody` — from the Computer Language Benchmarks Game.
Both are **MIT-licensed**, and the licence requires the notice to travel with the code: each file
names the suite it is a port of, and `awfy.rs` fails if one stops doing so.

Every verification constant in this directory was read from the original's `verifyResult`, not
remembered. Nothing here invents a number.

## What the port changes

Are We Fast Yet's common subset assumes mutable arrays, mutable object fields and bitwise
operators. Beck has none of the three. Rather than let each file improvise, the whole directory
follows four rules:

1. **A mutable array is a `Map[Int, T]`.** An absent key reads as the Java's zero-initialised
   element (`unwrap_or`), a write is `map_insert`, and the algorithm is otherwise untouched.
   `sieve`, `permute`, `queens` and `nbody` are the four that need one.
2. **A mutable object is a record, rebuilt.** Where the original mutates a field in a loop, the port
   threads the record — and where it mutates *two* things at once, the port carries both in one
   record, because that is what a language without mutation makes you write down.
3. **A bitwise operator is arithmetic.** `& 65535` is `% 65536` on a value the recurrence keeps
   non-negative; `<<` is repeated doubling and `^` is written bit by bit
   ([`mandelbrot.beck`](mandelbrot.beck)). The replacements are tested against what the operators
   mean, not only through the benchmark that uses them.
4. **A number is the suite's.** Where a size had to change, the number changes with it and the file
   says which of the suite's published sizes it is at. No file invents a verification value.

`richards` and `json` need a fifth rule that none of the nine did, and [`53`](../../docs/53-are-we-fast-yet-report.md)
§53.4 is why: **a closure over mutable state is a function of the state**. Java's `ProcessFunction`
returns the next task and changes the scheduler on the way out; here every one of them takes the
scheduler and returns both.

Two differences are worth naming separately because they are not mechanical:

- **`queens` does not undo.** The original sets its arrays on the way down and unsets them on the
  way back up; the port passes the board the caller still holds, so backtracking needs no undo. The
  search visits the same nodes in the same order and does four fewer assignments per rejected
  placement.
- **`towers` keeps its exceptions.** The original throws if a disk lands on a smaller one, and a
  port that dropped the check would pass while moving nonsense. `raise` is
  [`docs/27`](../../docs/27-the-walls-come-down-report.md)'s row label, so the benchmark publishes
  `raises(TowersError)` and its test faces it with `try:`.

## What is not here

**Nothing of Are We Fast Yet is unported.** The reports are
[`53`](../../docs/53-are-we-fast-yet-report.md) for the nine micro-benchmarks and
[`53`](../../docs/53-are-we-fast-yet-report.md), [`53`](../../docs/53-are-we-fast-yet-report.md),
[`53`](../../docs/53-are-we-fast-yet-report.md), [`53`](../../docs/53-are-we-fast-yet-report.md) and
[`53`](../../docs/53-are-we-fast-yet-report.md) for the five macro-benchmarks.

**Three of them run a smaller configuration than the suite measures at, and each says so.**
`mandelbrot` and `nbody` verify at sizes the suite publishes values for but does not measure at
([`53`](../../docs/53-are-we-fast-yet-report.md) §53.3); `havlak` and `deltablue` are limited by the
evaluator's 50,000,000-step fuel budget rather than by the clock
([`53`](../../docs/53-are-we-fast-yet-report.md) §53.3, [`53`](../../docs/53-are-we-fast-yet-report.md) §53.3). A
`--fuel` on `beck test` is owed by three benchmarks now.

**`havlak` runs a cheaper configuration than the suite publishes, and says so.**
[`53`](../../docs/53-are-we-fast-yet-report.md) §53.3 measured why: the published 50 discarded runs exhaust
the evaluator's 50,000,000-step fuel budget after 13.7 s in a release build, and six of them fit
where eight do not. Two of its three tests exist to make the reduction honest rather than
convenient — one asserts the verified numbers do not depend on that parameter, and the other
asserts each discarded run does the whole job.

**Two of the nine run at a size the suite publishes for one iteration rather than at its default.**
`mandelbrot` verifies at size 1 (128) rather than 500 (191), and `nbody` after one advance
(`-0.16907495402506745`) rather than 250,000 (`-0.1690859889909308`). Both of those are Are We Fast
Yet's own published values for those sizes — the suite verifies against all of them — but the
default sizes are what its published results are about, and these are not those. `mandelbrot` at
size 500 exhausts the evaluator's 50,000,000-step fuel budget after about 5.5 s in a release build
([`docs/53`](../../docs/53-are-we-fast-yet-report.md) §53.3).

**There is no comparative number, and there will not be one here yet.**
[`docs/25`](../../docs/25-benchmarks-and-expressiveness.md) §25.9 holds every comparative claim
until a second backend exists, because the tree-walker is a placeholder and a number about it is a
number about scaffolding. What `measure_awfy.rs` prints is wall-clock of this binary on this
machine, and it says so.
