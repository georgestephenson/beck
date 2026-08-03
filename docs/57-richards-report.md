# 57 — Phase 3, part 26: Richards, and what a mutable object graph costs

**Built.** [`compiler/awfy/richards.beck`](../compiler/awfy/richards.beck) — the first of Are We
Fast Yet's five **macro**-benchmarks, ported and verified against the suite's own two constants.

[`53`](53-are-we-fast-yet-report.md) §53.6 recorded all five as "not ported, not attempted and not
declined … nothing is known about whether they are expressible", and that sentence was the largest
unqualified admission in that report. This answers it for one of them, and §57.5 answers it for the
other four with evidence rather than silence.

## 57.1 Why Richards is the one to do first

The nine micro-benchmarks are loops. Richards is a **program**: six tasks — an idler, a worker, two
handlers and two devices — passing packets to one another until a counter runs down. It is in the
suite because it is a mutable object graph, and it is the shape Beck has least of:

- a linked list of task control blocks, walked by following a `link` field;
- a chain of packets per task, spliced between queues by mutating `link`;
- four closures, each of which reaches *back into the scheduler* to change another task's queue,
  another task's state, and two counters — and then returns which task runs next.

Java says all of that with assignment. Beck has none, so the port is a rewrite. What it verifies
against is `Scheduler.java`'s own `start`: `queuePacketCount == 23246 && holdCount == 9297`.

**It passed both on the first run.** That is worth stating precisely rather than as a boast: two
independent counters agreeing exactly, after a rewrite of the control flow, is not something a port
with a dropped state transition does — the counts are sensitive to every scheduling decision in
33,000 activations.

## 57.2 The three rules the port needed, and the one that is new

`awfy/README.md` already had four rules from the micro-benchmarks. Two of them carried over:

- **A mutable field is a rebuilt record.** Every setter becomes a value returned.
- **A pointer chain is a `list`.** `Packet.link` exists only to be "the next packet in this queue",
  and a packet is in exactly one queue at a time — so the field *disappears into the list that
  holds it*. Nothing is lost. [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.6's
  cost, that a `list` cannot share a suffix, is paid on queues a few packets long.

The third is new, and it is the whole of what "no mutation" costs this benchmark:

> **A closure over mutable state is a function of the state.**

Java's `ProcessFunction` is `(Packet, RBObject) -> TaskControlBlock`: it *returns* the next task and
changes the scheduler as a side effect. Here each one is
`(Scheduler, Task, list[Packet], …) -> Step`, where `Step` is the scheduler **and** the next task.
The record exists because the second of those two things has to be said out loud.

That is a threading discipline, not a wall. Nothing was inexpressible; what it cost is that every
function that used to be a statement is now a value returned, and the file is about 500 lines to the
original's 415 across six files.

**One place the port is cleaner than the original**, and it is the language's doing rather than the
porter's. Java needs a `ProcessFunction` field on the task, four lambdas, four `RBObject` subclasses
and a cast at the top of each. Beck's task carries a `union TaskData` with four variants, and
`dispatch` **matches on it** — the tag, the private data and the choice of function are one thing
instead of three, and the cast is gone because there is nothing to cast.

The other thing worth recording: `schedule` is a `while` loop of about 33,000 iterations, written as
a tail call. This is the first benchmark in the directory where [`31`](31-tail-calls-report.md) is
load-bearing rather than tidy — a non-tail transcription would want 33,000 host frames, and
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)'s budget is nowhere near that.

## 57.3 What it costs to run

`cargo test --release --test measure_awfy -- --nocapture`, median of five, beside the nine:

| benchmark | `beck check` ms | `beck test` ms | difference |
|---|---|---|---|
| mandelbrot | 4.0 | 4.6 | 0.6 |
| nbody | 4.9 | 6.7 | 1.8 |
| list | 3.3 | 46.2 | 42.9 |
| storage | 3.3 | 50.0 | 46.7 |
| bounce | 4.0 | 51.9 | 47.9 |
| sieve | 3.3 | 52.0 | 48.7 |
| queens | 3.4 | 64.6 | 61.2 |
| permute | 3.3 | 94.7 | 91.4 |
| towers | 4.0 | 157.1 | 153.1 |
| **richards** | **7.9** | **5,359.9** | **5,351.9** |

**Thirty-five times the next slowest**, and about 21 s in a debug build — which is the *gate*'s cost
too: the `awfy` suite goes from 2.6 s to 21.8 s.

That number is not a performance claim and [`25`](25-benchmarks-and-expressiveness.md) §25.9's rule
is unchanged — there is nothing to compare it to until a second backend exists. What it *is* is the
shape of the work: 33,000 task activations, each rebuilding a `Task` record, a `Scheduler` record
and one or two queues. A language with mutation writes a field; this writes six.

Whether that gap is the language or the tree-walker is exactly what cannot be said yet, and saying
it now would be the mistake §25.3 warns about.

## 57.4 What the port does not change

Every scheduling decision is the original's, and three that would have been tempting to simplify are
not:

- **The idler's control word is stirred with `^ 53256`.** Beck has no bitwise operators
  ([`53`](53-are-we-fast-yet-report.md) §53.5), so `xor_of` is eight lines of arithmetic — the same
  eight as `mandelbrot.beck`, repeated rather than shared, because `awfy/README.md`'s rule is that
  each benchmark is a self-contained program and a shared helper would make the directory a library.
  It is tested against what the operator means, not only through the benchmark.
- **The worker fills all four bytes of every packet**, wrapping its letter counter at 26, exactly as
  the original does. The bytes are never read back for anything but the handler's transfer, and
  dropping the loop would have made the benchmark faster and different.
- **The task is written back before its function runs.** `markWaiting` reads the current task out of
  the scheduler, so a function that ran against a stale copy would silently lose the state
  transition that `runTask` had just made. In Java this is free — there is one object. Here it is an
  ordering rule, and it is the single place a port of this is most likely to be quietly wrong.

## 57.5 The other four, assessed rather than attempted

[`53`](53-are-we-fast-yet-report.md) §53.6 said nothing was known. Having done Richards, something
is — because Richards *is* the hard part of three of the four. Line counts are of the Java sources.

| | Size | What it needs | Blocked? |
|---|---|---|---|
| **Json** | `JsonPureStringParser` 354, `JsonValue` 160, `JsonObject` 177, `JsonArray` 101, plus the smaller value classes | A recursive-descent parser over a ~25 KB minified literal. The parser is pure and would port almost directly — but `str_slice` indexes by **character**, so a character-at-a-time reader over 25 KB is quadratic. `str_chars` gives a `list[Str]` whose `list_get` is an index, so the fix is one line at the top and the rest is a transcription | **No.** Needs the input read as a `list[Str]` first |
| **DeltaBlue** | `Planner` 308, `Variable` 100, plus the constraint hierarchy | An incremental constraint solver over a **cyclic** object graph: variables point at constraints and constraints point back, both mutated during planning. Richards' answer applies — an arena keyed by identity, which is what `Scheduler.tasks` is — but the cycles are the data structure rather than an implementation detail, so it is the largest of the four | **No.** Needs an arena, and `som.Vector` |
| **Havlak** | `HavlakLoopFinder` 318, `LoopTesterApp` 123, `ControlFlowGraph` 70, plus the block and loop types | Loop recognition over a control-flow graph, with **union-find and path compression**. Path compression is mutation used as an optimisation, so a pure version is either a `Map` rebuilt per find or the algorithm without compression — and the second changes the work the benchmark measures | **No**, but the faithful version needs the compression to be a threaded `Map` |
| **CD** | `RedBlackTree` 406, `CollisionDetector` 182, `Simulator` 26, plus the vector and motion types | Aircraft collision detection over a **mutable red-black tree**. A persistent red-black tree is textbook and would be less code than the original — which is the problem: it would not be the same benchmark. A faithful port keeps the rotations and the parent pointers, which is the arena again | **No.** Needs an arena, and `som.Vector` |

**None of the four is blocked by the language.** What three of them share is a dependency this
directory does not have: `som`'s collection classes — `Vector` 248 lines, `Dictionary` 247,
`IdentityDictionary` 31. Porting those is the next thing owed here, and it is a genuine question
whether they belong in `awfy/` at all or whether the benchmarks should use `lib/collections.beck` —
which would be *faster and wrong*, because Are We Fast Yet exists to compare implementations rather
than standard libraries, and swapping its collections for ours is exactly the substitution its
methodology forbids.

That question is named and not answered here.

## 57.6 What is **not** built

| | Status |
|---|---|
| CD, DeltaBlue, Havlak, Json | **not ported.** Assessed in §57.5 with what each needs; none blocked by the language |
| `som.Vector`, `som.Dictionary`, `som.IdentityDictionary` | **not ported**, and three of the four need them. About 530 lines, with the methodological question in §57.5 attached |
| The CLBG harness | **not stood up**, unchanged from [`53`](53-are-we-fast-yet-report.md) §53.6 |
| Any comparative number | **none**, unchanged. §57.3 is wall-clock of this binary on this machine |
| Richards at more than one iteration | **not run.** The suite's harness runs `start()` `innerIterations` times; this runs it once, which is what the published constants verify against |

## 57.7 What this corrects

- **[`53`](53-are-we-fast-yet-report.md) §53.6's first row is out of date in the direction that
  matters.** "The five macro-benchmarks … nothing is known about whether they are expressible" is
  now one ported and four assessed. Reports are history, so the correction is here.
- **[`08`](08-roadmap.md) §8.4's Phase 3 row moves again.** Are We Fast Yet is ten of fourteen.
- **[`awfy/README.md`](../compiler/awfy/README.md) gains a fifth port rule**, §57.2's, which none of
  the nine micro-benchmarks needed.
- **[`31`](31-tail-calls-report.md) gains its first benchmark.** Tail calls were measured on
  synthetic recursion and on SICP; `schedule` is 33,000 iterations of somebody else's loop, and it
  is the first place in the tree where the feature is load-bearing for a third-party program.

## 57.8 What Phase 3 is still not

Unchanged from [`56`](56-decimal-report.md) §56.8 except where this touches it. The standard-library
bullet's library half is done and its harness half is ten of fourteen. The exit criterion — an
outside developer building a non-trivial app from documentation alone — is not met and is not
closer.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
