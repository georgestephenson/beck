# 59 — Phase 3, part 28: Havlak, and a workload with no oracle

**Built.** [`compiler/awfy/havlak.beck`](../compiler/awfy/havlak.beck) — the third of Are We Fast
Yet's five macro-benchmarks, verified against the suite's own two numbers: **1,605 loops over 5,213
nodes**. Are We Fast Yet is twelve of fourteen.

[`58`](58-json-report.md) §58.5 recommended attempting this one first of the three that were left,
and the reason was not size. It was the only one with something cheap to learn on the first morning:

> Whether that fits the evaluator's 50,000,000-step fuel budget is *unknown* and is the first thing
> to find out.

It does not, and §59.3 is the measurement. The recommendation was right for the reason it gave.

## 59.1 What it is

Havlak's loop-recognition algorithm: a depth-first numbering of a control-flow graph, a
classification of every edge as a back edge or not, and then union-find with path compression
collapsing each loop body into its header. The graph is built by `LoopTesterApp` — ten parallel loop
trees, each ten deep, each of those five base loops of ten blocks — and comes to 5,213 nodes.

Three shapes the port needed, on top of the five rules in
[`awfy/README.md`](../compiler/awfy/README.md):

- **An object graph keyed by identity is a `Map` keyed by name.** The original numbers blocks in an
  `IdentityDictionary<BasicBlock, Integer>`, and `BasicBlock.customHash` returns its `name` — so a
  `Map[Int, Int]` is that dictionary with the identity written down.
- **Union-find is a `Map` threaded through the walk.** Path compression is mutation used as an
  optimisation, so the compression is a fold over the nodes collected on the way up rather than a
  write through a pointer. The same nodes are repointed at the same parent.
- **`som.Set` is a `list` with a membership check**, because that is what `som.Set` is — a `Vector`
  with `hasSome` on `add`. A `Map` would order the members by value where the original orders them
  by insertion, and this algorithm *iterates* them.

One detail is transcribed rather than improved, and it is worth naming because improving it would be
so tempting. `UnionFindNode.findSet` collects the nodes it walks past and then repoints them at
**the starting node's `parent` field** — not at the root it found:

```java
nodeList.forEach(iter -> iter.union(parent));   // `parent` is *this*'s field
```

A port that repointed at the root would compress harder and be a different algorithm. The Beck reads
`node_at(f, start).parent` once, before the compression, for exactly that reason.

## 59.2 What was not needed

`som.Vector` was not ported, and [`58`](58-json-report.md) §58.5's open question turns out to have an
answer for this benchmark: a `Vector` here is only ever appended to and indexed, which is what
Beck's `list` is. The question that mattered was about `som.Set` and `som.Dictionary` — hash
structures whose *order* is observable — and both of those are ported rather than swapped for a
`Map`, per §59.1.

So the decision [`58`](58-json-report.md) §58.5 deferred splits in two, and only half of it was
real. `som.Vector` ≈ `list`, and reaching for `list` is reaching for the language rather than for
Rust. `som.Set` and `som.Dictionary` are the benchmark's own data structures, and reaching for `Map`
would be the substitution Are We Fast Yet forbids.

## 59.3 The measurement, which was the point of doing this one first

`Havlak.java`'s configuration at `innerIterations == 1` is `main(1, 50, 10, 10, 5)`: one dummy run
over the small graph, one counted run over the big one, and **fifty discarded runs** over the big one.

| | |
|---|---|
| The published configuration, 50 discarded runs | **exhausts the fuel budget**, after 13.7 s in a release build |
| 6 discarded runs | fits — 13.7 s |
| 8 discarded runs | does not fit |
| 0 discarded runs, which is what the file gates on | 11.0 s release, 39.6 s debug |

So the published workload needs roughly **eight times** the evaluator's 50,000,000-step budget. That
budget is a deliberate runaway-program backstop (`interp.rs::DEFAULT_FUEL`), nothing exposes it to a
caller, and `mandelbrot` at size 500 hit the same wall in [`53`](53-are-we-fast-yet-report.md) §53.3.
This is the second benchmark it refuses and the first *macro*-benchmark, which makes it a pattern
rather than an incident: **a `--fuel` on `beck test`, or a way for a program to declare its own
budget, is now owed.** It is not built here.

`havlak.beck` therefore runs `find_loop_iterations = 0` and says so in the file. Two of its three
tests exist to make that reduction honest rather than convenient:

- **the verified numbers do not depend on it** — `benchmark(1) == benchmark(0)`, because the
  discarded runs are given a fresh loop structure and nothing reads it;
- **each discarded run does the whole job** — `discarded_total(1) == 1602`, so the parameter is a
  count of real work rather than a count of skipped work.

The second of those was written because of §59.5.

## 59.4 What the port does less of

`ControlFlowGraph.addEdge` appends every edge to a `Vector<BasicBlockEdge>` that **nothing ever
reads**. The port keeps a count instead of a list. That is allocation the original does and this does
not — about 21,000 objects — and it is named here for [`58`](58-json-report.md) §58.4's reason: "the
port is faster" and "the port does less" are the same sentence and only one of them is honest.

## 59.5 The finding: a workload with no oracle

The first version of the discarded-run loop was:

```beck
def repeated(cfg: Cfg, i: Int, n: Int) -> Int:
    if i >= n:
        return i
    return repeated(cfg, ignore(find_loops(new_finder(cfg, empty_lsg())).lsg.count), n)
```

It passes the run's own **loop count** back as the loop counter. A run finds about 1,600 loops, so
`i` becomes 1,602 on the first pass, `i >= n` is immediately true, and the loop runs **once instead
of fifty times**.

Every verified number was still right. `benchmark(50)` returned `LoopCount(loops=1605, nodes=5213)`
and the test passed — in 4.3 s, which is what gave it away, because 51 runs of a 5,213-node graph do
not take 4.3 s.

**The class of bug is the interesting part, and it is specific to benchmarks.** `verifyResult` checks
an *answer*; the discarded runs produce no answer, by construction — that is what "discarded" means.
So the benchmark's own oracle is structurally blind to whether its main workload ran at all. Every
other assertion in the file was blind to it too, because none of them is about the loop.

The defence is the one the file now has: **carry the discarded result somewhere a test can see it.**
`repeated` sums the counts, `main` uses the sum, and `discarded_total(1) == 1602` asserts that one
discarded run finds the 1,601 loops of the big graph plus its root. The arithmetic ties it to the
verified 1,605 — which is that same run plus the three loops the *small* graph contributes before
`constructCFG` grows it — so the two numbers are one algorithm rather than two coincidences.

This generalises past this file: **any benchmark whose work is discarded needs an assertion that the
work happened**, and the suite's own `verifyResult` cannot be that assertion. Of the twelve ported,
`havlak` is the only one with a discarded workload, so the rule has exactly one instance — but it
would have been cheaper to know before writing it than after.

## 59.6 What is **not** built

| | Status |
|---|---|
| CD and DeltaBlue | **not ported.** [`58`](58-json-report.md) §58.5 sizes both; neither is blocked by the language |
| The published `find_loop_iterations = 50` | **not run**, and §59.3 measures why — it needs about eight times the fuel budget |
| `--fuel`, or a program-declared budget | **not built**, and now owed by two benchmarks rather than one |
| `som.Vector` as a port | **not needed** here, per §59.2. `som.Set` is ported rather than swapped |
| The `Vector<BasicBlockEdge>` nothing reads | **not built**, per §59.4 |
| The CLBG harness | **not stood up**, unchanged |
| Any comparative number | **none**, unchanged. §59.3's figures are wall-clock of this binary on this machine |

## 59.7 What this corrects

- **[`58`](58-json-report.md) §58.5's Havlak row is discharged, and its recommendation was right.**
  Attempting this one first cost less than attempting it third would have, because what it teaches
  is a fact about the *evaluator* rather than about Havlak.
- **[`58`](58-json-report.md) §58.5's open question about `som.Vector` splits**, per §59.2: a
  `Vector` is a `list`, and a `Set` is not a `Map`.
- **[`53`](53-are-we-fast-yet-report.md) §53.3's fuel finding is now a pattern.** One benchmark
  refused by the budget is an incident; two, one of them a macro-benchmark at its published
  configuration, is a missing feature.
- **[`08`](08-roadmap.md) §8.4's Phase 3 row moves again.** Are We Fast Yet is twelve of fourteen.

## 59.8 What Phase 3 is still not

Unchanged from [`58`](58-json-report.md) §58.8. The standard-library bullet's library half is done
and its harness half is twelve of fourteen. The exit criterion — an outside developer building a
non-trivial app from documentation alone — is not met and is not closer.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
