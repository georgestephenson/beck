# 75 — Phase 3, part 44: what the profiler said

**Built, and half of what was planned was thrown away.**
[`74`](74-the-cost-of-a-call-report.md) §74.8 listed four things not built. This is that list taken
seriously — and the first thing it produced was the discovery that **three of the four were not
worth building**, which a profiler said in ten minutes and four reports of reasoning had not.

Every benchmark is **4.9% to 17.0% faster** than [`74`](74-the-cost-of-a-call-report.md) left it,
and **25.6% to 33.1%** faster than [`73`](73-closures-share-their-code-report.md).

## 75.1 The method, which is the point

The four previous performance reports found what they found by asking what an operation *should*
cost and measuring whether it did. That works, and it found a quadratic in long division, a
quadratic in list building, two in strings, a 48-byte `Value` and a per-call deep copy of a function
body. It also has a blind spot: it can only examine the operations somebody thought to name.

This one ran `callgrind` over `awfy/json.beck` — the first time this project has profiled itself.
The answer was one line long and not on anybody's list:

| | |
|---|---|
| glibc `malloc`, `free` and friends | **35%** of every instruction executed |
| `Interp::operand`, `step`, `leaf`, `eval`, `prim` | 26% |
| everything else | 39% |

About 1.06 million allocations for a benchmark that takes 95 ms. A tree-walking evaluator is an
allocator benchmark wearing a language's clothes: a call allocates its arguments and its frame, a
`let` allocates its scope, and a record, a list and a string each allocate on construction — nearly
all freed a few microseconds later.

## 75.2 A modern allocator

`mimalloc` is the `beck` binary's `#[global_allocator]`.
[`docs/adr/0019`](adr/0019-a-modern-allocator-for-the-evaluator.md) is the decision, what it costs
and what would reverse it. Release, minimum of five, interleaved:

| | glibc | mimalloc | |
|---|---|---|---|
| `awfy/havlak.beck` | 2.916 s | **2.457 s** | **−15.7%** |
| `awfy/json.beck` | 0.095 s | **0.083 s** | **−12.8%** |
| `lib/decimal.beck` | 0.233 s | **0.209 s** | **−10.3%** |
| `clbg/pidigits.beck` | 0.982 s | **0.890 s** | **−9.4%** |
| `awfy/deltablue.beck` | 0.054 s | **0.049 s** | **−8.9%** |
| `clbg/knucleotide.beck` | 0.584 s | **0.535 s** | **−8.4%** |
| `clbg/fasta.beck` | 0.330 s | **0.318 s** | **−3.7%** |
| `awfy/richards.beck` | 1.403 s | **1.355 s** | **−3.5%** |

The ordering is the interesting part: it is a ranking of how much each benchmark allocates.

## 75.3 A record is a small sorted array

The other half of the profile, on `awfy/havlak.beck`, was `BTreeMap` — its search, its insert, its
drop and the `memcmp` underneath them, about a fifth of the process between them. A record's fields
were a `BTreeMap<Arc<str>, Value>`, and a record is the wrong size for one: three to eight entries,
built once and read many times, where a B-tree pays a node allocation and a pointer chase per level
to buy an asymptote that never arrives.

`Fields` is a `Vec<(Arc<str>, Value)>` kept sorted by name, searched linearly by equality — length
first, then bytes, which is what `==` does and what `cmp` cannot. One allocation for the whole
record and the names next to each other in cache.

**Iteration order is unchanged**, which is what makes this a representation change rather than a
semantic one: the value order, the state digest and the wire format
([`beck_core::repr`](../compiler/crates/beck-core/src/repr.rs)) are bit-for-bit what the `BTreeMap`
gave. The 48 suites are the proof — a record that iterated differently would fail the
replay-determinism harness, the digest tests and the page snapshots at once.

Worth −3.5% on `havlak`, −2.8% on `json`, under 1% elsewhere and inside the noise on the two
benchmarks with no records in them. It is kept for being simpler and smaller as much as for being
faster.

## 75.4 What was measured and thrown away

Three of [`74`](74-the-cost-of-a-call-report.md) §74.8's four, and one that was not on it.

**A small-vector for call arguments.** [`74`](74-the-cost-of-a-call-report.md) §74.6 had already
found that an argument *stack* costs more than it saves; `SmallVec<[Value; 4]>` avoids the
allocation without any of that bookkeeping, and `Value` is 16 bytes, so four fit in 64. It made
every benchmark **1–3.5% slower**, and inline capacity 2 was no better: `Step` grows, and `step`
returns one per node evaluated. With mimalloc underneath, an allocation is cheaper than carrying
the space to avoid it.

**A smaller `Core`.** 152 bytes a node, 80 of it an inline `Ty` that the evaluator almost never
reads. Behind an `Arc` a node is **80 bytes** — and it is neutral at run time (−0.9% to +2.2%, no
signal) and **6.6% slower** to check `awfy/havlak.beck`, because every node's construction gains an
allocation. Reverted, all 54 lines of it. The reason it does not pay at run time is
[`73`](73-closures-share-their-code-report.md): a closure shares its body, so the hot nodes are the
same nodes every call and they stay in cache whatever they weigh.

**A global resolved to an index.** Not built, and now with a reason rather than a shrug: after
[`74`](74-the-cost-of-a-call-report.md) §74.4 made the name hash cheap, no hashing symbol appears
anywhere in the profile above 1%. There is nothing left there to win.

**One allocation per `let`.** Not built, same reason. `Env::extend` accounts for 40,388 of
`json`'s allocations against `bind`'s 137,785, and mimalloc charges about 15 ns for one. It is a
third-order term.

## 75.5 What this leaves

| | |
|---|---|
| Interned field names | **still not built**, and §75.3 is why it matters less: the `memcmp` the profile showed was mostly `Value::cmp` inside `PMap` — Beck's own `Map` with string keys — rather than field lookup. A `u32` symbol would need a process-wide interner and would put the value order, the digest and the wire format through it, which is [`54`](54-ordering.md)'s territory rather than a performance change |
| A variable read that is an index | **not built, and it is the largest thing left.** `Env::read` is 4.5% of `havlak` and it is a linear scan of a frame followed by a walk up the scope chain — so a read costs the depth of the `let` nesting around it. What it should cost is one indexed load, which means the checker assigning a slot per binding and a function's locals living in one frame. That is a change to the calling convention, not a constant |
| A compiling backend | the thing all of this is scaffolding for. [`25`](25-benchmarks-and-expressiveness.md) §25.9 still holds every comparative claim until there is a second one |

## 75.6 Where that leaves Beck against other languages

[`25`](25-benchmarks-and-expressiveness.md) §25.3 is the only place this project has put a number on
that, and it did it with `fib(30)` as a `test` block: **4.120 s, about 33× CPython**, with the
section's whole point being that the number measures scaffolding rather than a language.

The same program, on the same kind of container, now:

```console
$ beck test fib.beck
0.797s                        # 0.004s of which is compile and harness
```

**5.2× faster than §25.3 measured**, which is [`69`](69-standard-library-imports-report.md)–[`75`](75-what-the-profiler-said-report.md)
compounded. Beside four other interpreters, same algorithm transliterated, whole process minus that
runtime's own start-up, best of five:

| | `fib(30)` — calls | binary trees, depth 16 — allocation | a counting loop, 10⁶ — arithmetic |
|---|---|---|---|
| Beck | 794 ms | 104 ms | 437 ms |
| CPython 3.11 | 108 ms (**7.3×**) | 21 ms (**5.1×**) | 163 ms (**2.7×**) |
| Ruby 3.3 | 80 ms (**9.9×**) | 19 ms (**5.4×**) | 32 ms (**13.5×**) |
| PHP 8.4 | 47 ms (**17×**) | 26 ms (**4.0×**) | 9 ms (**47×**) |
| Node 22 | 11 ms (**73×**) | 12 ms (**8.5×**) | 13 ms (**34×**) |

Read it for the shape rather than the ranking. **Beck is now within a small multiple of the
bytecode interpreters and nowhere near the JIT**, and the multiple tracks exactly what these reports
have been fixing: allocation-heavy work is 4–5× off, arithmetic is 2.7× off CPython, and *calls* —
174 ns each after [`74`](74-the-cost-of-a-call-report.md) — are still the worst axis, because
CPython, Ruby and PHP all execute a call as a bytecode dispatch into a frame they built once, and
this walks a tree.

Every caveat [`25`](25-benchmarks-and-expressiveness.md) §25.3 attached still applies and none of
them have weakened: these are transliterations rather than each suite's own tuned ports, `beck test`
includes parsing and checking, and above all **this is still a tree-walking interpreter standing in
for a compiler**. §25.9 holds every comparative *claim* until there is a second backend, and this
section is a measurement rather than a claim. What has changed is that "the interpreter is a
placeholder" no longer explains an order of magnitude — it explains a factor of five, and
[`73`](73-closures-share-their-code-report.md) §73.8's point stands: a third of that order of
magnitude was one `clone`.

## 75.7 How it is tested

Nothing new. Neither change alters an answer, so the gate is that **all 48 suites still pass** —
and §75.3's is the change where that matters most, because a record's iteration order is load-bearing
for replay determinism, the state digest and the checked-in page snapshots.

## 75.8 What this corrects

- **[`74`](74-the-cost-of-a-call-report.md) §74.8's list was three-quarters wrong.** Two of its four
  entries do not pay and one is a rounding error; only interned field names survives, and §75.5
  restates even that one differently. A list of "what is not built" written from reasoning is a list
  of hypotheses, and this is what happened when they were tested.
- **Every wall-clock number in [`69`](69-standard-library-imports-report.md)–[`74`](74-the-cost-of-a-call-report.md)
  is historical**, on one more axis than before: they were measured under glibc's allocator. The
  shapes and the within-report ratios are unaffected.
- **[`72`](72-space-and-constants-report.md)'s peak-memory figures** are glibc figures. A memory
  number taken now is a number about mimalloc's page behaviour as well as about the program.

## 75.9 What this establishes

**That four reports of careful reasoning about performance missed a third of the cost, and ten
minutes with a profiler did not.** [`AGENTS.md`](../AGENTS.md)'s standard — know the cost of what
you write, and treat a bad number as a design question — found real defects five times, and it is
still the right first move. But it examines the operations somebody names, and the largest cost here
was not an operation at all: it was the sum of every small one. Both are needed, and the profiler is
the cheaper of the two to reach for first.
