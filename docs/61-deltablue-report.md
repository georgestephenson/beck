# 61 — Phase 3, part 30: DeltaBlue, and Are We Fast Yet complete

**Built.** [`compiler/awfy/deltablue.beck`](../compiler/awfy/deltablue.beck) — the fifth and last of
Are We Fast Yet's macro-benchmarks.

**Are We Fast Yet is fourteen of fourteen.** [`25`](25-benchmarks-and-expressiveness.md) §25.9
scheduled the harness for Phase 3 and [`53`](53-are-we-fast-yet-report.md) stood up the nine
micro-benchmarks; [`57`](57-richards-report.md), [`58`](58-json-report.md),
[`59`](59-havlak-report.md), [`60`](60-collision-detection-report.md) and this one are the five
macro-benchmarks. Nothing of the suite is unported.

## 61.1 What it is, and why it was last

An incremental constraint solver. Variables are linked by constraints of differing strengths; adding,
removing or editing one makes the planner work out which constraints can be satisfied and in what
order, then execute that plan. `chainTest` builds a chain of equalities and drives a value from one
end to the other a hundred times; `projectionTest` builds `n` scaled projections sharing one scale
and one offset and then changes each of them in turn.

[`58`](58-json-report.md) §58.5 called it the largest of the three that were left and named why in
one line:

> variables point at constraints and constraints back at variables, both mutated during planning

That is a **cyclic** object graph, and unlike Richards' task chain it is the data structure rather
than a way of writing one. The answer is [`57`](57-richards-report.md) §57.2's, twice: two arenas —
`Map[Int, Variable]` and `Map[Int, Constraint]` — with integer handles. **A cycle becomes two maps
that refer to each other by name**, which is what a cycle is once you can no longer follow a pointer.

Two other simplifications fell out of the shape rather than being chosen:

- **A `Strength` is its arithmetic value.** The original is a symbol, an identity dictionary and a
  table lookup, and the only thing anybody ever asks of a strength is that number. `stronger` is `<`.
- **The constraint hierarchy is a `union` and two flags.** Java has an abstract class, a unary
  subclass, a binary subclass and four concrete leaves; Beck has `union Kind` with four variants and
  one record, because the difference between unary and binary is which fields are used rather than
  which methods exist.

## 61.2 The only benchmark in the suite that verifies against nothing

`DeltaBlue.java`:

```java
public boolean innerBenchmarkLoop(final int innerIterations) {
  Planner.chainTest(innerIterations);
  Planner.projectionTest(innerIterations);
  return true;
}
```

There is no constant. The oracle is the six `throw new RuntimeException` inside the planner and its
two tests — a cycle encountered, a required constraint unsatisfied, and the four value checks. **A
port that dropped an assertion would pass while solving nothing.**

So all six are kept, as a `raise` — which is [`45`](45-error-rows-report.md)'s row label and is what
makes "this did not throw" a thing a `test` block can say: `expect (try: benchmark(20)) == Ok(value=true)`.

And because the assertions *are* the oracle, the file asserts that they **fire**. A `raise` that
could never happen would verify exactly as much as no `raise` at all, and this benchmark has no
other check to fall back on. `and the assertions the benchmark relies on are reachable` provokes
both of the value checks and expects the `Err`.

That is the same discipline [`59`](59-havlak-report.md) §59.5 arrived at from the other direction —
there, a workload with no oracle; here, an oracle with no workload to check it against — and it is
the second time in three benchmarks that the suite's own verification turned out not to be enough on
its own.

## 61.3 What it costs, and the wall it hits

| `n` | |
|---|---|
| 20 — what the file gates on | 0.7 s debug |
| 200 | 1.6 s release |
| 1,000 | 8.9 s release |
| **12,000 — the suite's own measurement size** | **exhausts the fuel budget**, after 13.0 s release |

`rebench.conf` gives DeltaBlue `extra_args: 12000`, and it does not fit in the evaluator's
50,000,000-step budget. This is the **third** benchmark the budget refuses, after `mandelbrot` at
size 500 ([`53`](53-are-we-fast-yet-report.md) §53.3) and `havlak` at its published configuration
([`59`](59-havlak-report.md) §59.3).

Three is enough to stop calling it a finding and start calling it a missing feature. **`beck test`
needs a `--fuel`, or a program needs a way to declare its own budget.** The budget itself is right —
a runaway program should stop — but a backstop that nothing can raise is a ceiling, and a benchmark
suite is precisely the kind of legitimate program that lives above it. It is not built here, and it
is now owed by three of the fourteen.

Everything below the wall scales linearly, which is the other half of the evidence: `n = 200` at
1.6 s and `n = 1,000` at 8.9 s is the shape a constraint solver should have, so what stops 12,000 is
the budget rather than the algorithm.

## 61.4 What was transcribed rather than tidied

One detail is worth naming because it is exactly the kind a port gets wrong by improving it.
`ScaleConstraint` overrides `inputsDo` to include its scale and its offset — and does **not** override
`inputsHasOne`, which `inputsKnown` is built on. So the planner asks whether the single binary input
is known and never asks about the scale or the offset.

That asymmetry looks like an oversight in the original. It is also load-bearing: `inputsKnown`
decides which constraints may enter a plan, so tidying it changes which plans are producible and
therefore what the benchmark measures. The port keeps it, with a comment saying so.

## 61.5 What is **not** built

| | Status |
|---|---|
| The suite's own measurement size | **not run** for `deltablue` (12,000), `havlak` or `mandelbrot`, and §61.3 is why for the first two |
| `--fuel`, or a program-declared budget | **not built**, and now owed by three benchmarks |
| The CLBG harness | **not stood up**, unchanged from [`53`](53-are-we-fast-yet-report.md) §53.6. It is what is left of [`25`](25-benchmarks-and-expressiveness.md) §25.9's Phase 3 row |
| Compile-speed budgets, and the Felleisen table | **not built**, unchanged. §25.9 schedules both beside the harnesses |
| Any comparative number | **none.** Fourteen benchmarks now run and not one of them is compared to anything, which is §25.9's rule and is the whole reason the harness could be adopted before the backend |

## 61.6 What this corrects

- **[`58`](58-json-report.md) §58.5 is fully discharged.** All three of the benchmarks it sized are
  built. Its blocking verdict was right for two of them and wrong for CD, which
  [`60`](60-collision-detection-report.md) §60.6 records.
- **[`08`](08-roadmap.md) §8.4's Phase 3 row is met for Are We Fast Yet.** "Are We Fast Yet and CLBG
  harnesses against the evaluator" is half done: the first is complete, the second is untouched.
- **[`53`](53-are-we-fast-yet-report.md) §53.6's first row is gone.** "The five macro-benchmarks —
  not ported, not attempted and not declined … nothing is known about whether they are expressible"
  ends here. All five are expressible; four needed an arena, one needed a `list` where a pointer
  chain was, and one needed two primitives the language did not have.
- **The fuel budget is a missing feature rather than a series of incidents**, per §61.3.

## 61.7 What the whole harness establishes, and what it does not

Fourteen benchmarks, ported from a suite built to be hard to game, each verified against the
original's own answer. What that establishes is **expressiveness**: every one of Are We Fast Yet's
common-subset features has a Beck shape, and the five that looked least likely — mutable object
graphs, a cyclic constraint network, a red-black tree with deletion, union-find with path
compression, a hand-written parser — all have one.

It establishes nothing about speed, and [`25`](25-benchmarks-and-expressiveness.md) §25.9's rule is
unchanged after fourteen ports: no comparative claim until there is a second backend. What exists
now is the *baseline* that rule was waiting for, taken before the thing it measures got good, which
is §25.3 item 1's whole argument.

Three things the suite found on the way, none of which was about performance: `and` and `or` did not
short-circuit ([`53`](53-are-we-fast-yet-report.md) §53.5), the language had no `sin`, `cos` or
real-to-integer truncation ([`60`](60-collision-detection-report.md) §60.2), and it has no bitwise
operators. That is what a common-subset suite is *for*, and it is worth more than the timing table.

## 61.8 What Phase 3 is still not

The standard-library bullet is **done**: its library half closed with
[`56`](56-decimal-report.md), and its harness half is Are We Fast Yet complete — with CLBG, the
compile-speed budgets and the Felleisen table still outstanding under
[`25`](25-benchmarks-and-expressiveness.md) §25.9 rather than under this bullet.

The exit criterion — an outside developer building a non-trivial app from documentation alone — is
not met and is not closer.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
