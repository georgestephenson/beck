# 70–79 — Phase 3, parts 39–48: how the evaluator got fast

**Built.** Ten pieces of work on the tree-walking evaluator, over ten reports, consolidated here
into one. Three removed a quadratic, seven removed a constant, and four changes inside them were
built, measured and deleted. Together they are **5.2× on `fib(30)`** and a factor of three to five on every
benchmark in the tree, and they moved the sentence "the interpreter is a placeholder" from an
explanation for an order of magnitude to an explanation for a factor of five.

> **This document replaces ten reports** — 70 through 79 — that were one per change. They are gone
> rather than archived, and this is what they established. It is the first consolidation under
> [`AGENTS.md`](../AGENTS.md)'s rule that a report is for a phase or a subsystem: a 4%-faster
> binding is a changelog entry, and ten of them are a chapter.
>
> | Was | Now |
> |---|---|
> | 70 last-use moves · 71 strings · 79 a lambda is a frame | §70.2, the three quadratics |
> | 73 closures share their code · 74 the cost of a call · 75 what the profiler said · 76 the record and the read · 77 a `let` is a slot · 78 a record is a permutation | §70.3, the constants |
> | 72 a value is sixteen bytes | §70.3 and §70.6 |
> | each report's "measured and thrown away" | §70.4 |
> | each report's gates | §70.6 |
> | 78.6's finding about gates that cannot fail | §70.7 |

## 70.1 The method, which is the durable part

Every one of the ten followed the same loop, and the loop is worth more than any single change in
it:

1. **Ask what the operation should cost**, not how to make this one faster. Three of the ten found
   that the answer was "a different design" rather than "the same design, tuned".
2. **Profile before choosing.** `callgrind` ranks candidates in ten minutes; four reports of
   reasoning had ranked them wrong.
3. **Measure the wall clock before believing the profile.** An instruction count and a clock are not
   the same quantity: 4.5% of `richards`'s instructions were worth **1.5%** of its clock — *a factor
   of three*, and the ratio to plan with. The removed instructions were a tight, register-bound loop,
   which is the kind a modern core was already hiding.
4. **Measure at two sizes.** One measurement cannot tell linear from quadratic; two can, and the
   second costs a minute.
5. **Rotate the order of the binaries under test.** A fixed order gives later positions a
   systematically warmer machine, and on a shared runner that bias is the same size as the effects
   being reported: it turned a "−3.0%" into −0.2% when corrected.

Rules 3 and 5 are in [`AGENTS.md`](../AGENTS.md) because they were learned here, expensively.

## 70.2 The three quadratics

The accumulator idiom is how every loop in Beck is written, and it was `O(n²)` three different ways.
Each fix is a *shape* change, so each is gated on the shape rather than on a rate.

**The list (was report 70).** `list_append` copied the whole list because the evaluator lent every
value it read. The compiler now computes which read of a local is its **last**
([`beck_core::liveness`](../compiler/crates/beck-core/src/liveness.rs)) and hands the value over
instead, so a list nobody else holds is pushed into:

| n | before | after |
|---:|---:|---:|
| 1,000 | 2.1 ms | 1.6 ms |
| 4,000 | 33 ms | 6.7 ms |
| 16,000 | 530 ms (extrapolated) | 27 ms |
| 64,000 | ~8.5 s (extrapolated) | **99 ms** |
| per doubling | ×2.5 → ×3.7, heading for ×4 | **×2.0** |

**The text (was report 71).** `Value::Str` was an `Arc<str>`, so `+` allocated and copied both sides
and `str_len` counted characters every time. It carries its character count and its ASCII flag now,
and holds a `String` with spare capacity. Building text and walking it by character index are both
linear; a non-ASCII string gained a character index, so `str_slice` past an accent is `O(len)`
rather than `O(start)`.

**The fold (was report 79).** Three reports after the recursive form became linear, the same loop
written as a `fold` was still quadratic — because the liveness analysis could not see that a
lambda's own parameter is dead after its last read. It is linear now: **289 ms → 9 ms** over 8,000
elements, and **640 → 18** evaluator steps per element, flat in n.

## 70.3 The constants, in the order they were found

| Change | What it was doing | What it bought |
|---|---|---|
| **A closure shares its code** (73) | `Value::Closure` deep-copied the whole function body on every construction | **−56% to −72%** on every benchmark. One line; the largest single improvement this project has measured |
| **A call is a frame and a jump** (74) | a name lookup, three allocations, a virtual call and a copy of the parameter list | 287 ns → **174 ns**; every benchmark **−17% to −27%** |
| **A modern allocator** (75) | glibc `malloc` was **35% of every instruction executed** | mimalloc, `−3.7% to −15.7%` ([`adr/0019`](adr/0019-a-modern-allocator-for-the-evaluator.md)) |
| **A record is a small sorted array** (75) | a `BTreeMap` per record | part of the same run; the iteration order it fixes is load-bearing for replay determinism and the state digest |
| **A record literal sorts once** (76) | a search per field on every construction | **4% to 8%** on record-heavy programs, nothing elsewhere — which is the right shape |
| **A `let` is a slot** (77) | a binding cost 134 ns: an allocation and a vector push per `let` | **32 ns**; every benchmark **−4.2% to −8.2%**, and the program that binds nothing does not move |
| **A record is a permutation** (78) | the field order was re-derived per literal | **4.5% of instructions**, worth **1.5%** of the clock on the one program that builds a million records |
| **A `Value` is sixteen bytes** (72) | the record variant made every value 48 bytes | peak memory **−23%** on the graph benchmark, and faster |

## 70.4 What was built, measured and thrown away

Four changes were written, benchmarked and deleted, and they are the reason this document is worth
keeping.

- **An argument stack** — push arguments onto one buffer owned by the interpreter and drain them
  into the frame, which removes an allocation per call and had been written up two reports earlier
  as the obvious next move. It made a call **20% slower**, 185 → 220 ns: a `RefCell` borrow per
  argument (argument evaluation nests, so the borrow cannot be held across it) and a drop guard cost
  more than the allocation they removed.
- **A small-vector for arguments** — 1–3.5% *slower*, and inline capacity 2 no better.
- **A smaller `Core`** — 152 bytes to 80 by sharing the inline `Ty`. Neutral at run time and **6.6%
  slower to check**, because every node gains an allocation at construction.
- **The obvious permutation** — evaluate the fields into one vector and select them into a second.
  Shorter than what is there, and *slower than doing nothing at all*.

A fifth deserves the same shelf: a list of four "next things" turned out to be **three-quarters
wrong**, which a profiler said in ten minutes after four reports of reasoning had ranked them.

Nothing in this project has produced better evidence for the rule that a candidate you have only
*counted* has not been measured.

## 70.5 Where it left the evaluator

[`25`](25-benchmarks-and-expressiveness.md) §25.3 measured `fib(30)` at **4.120 s, about 33×
CPython**. The same program after this series:

```console
$ beck test fib.beck
0.797s                        # 0.004s of which is compile and harness
```

Beside four other interpreters — same algorithm transliterated, whole process minus start-up, best
of five:

| | `fib(30)` — calls | binary trees, depth 16 — allocation | a counting loop, 10⁶ — arithmetic |
|---|---|---|---|
| Beck | 794 ms | 104 ms | 437 ms |
| CPython 3.11 | 108 ms (**7.3×**) | 21 ms (**5.1×**) | 163 ms (**2.7×**) |
| Ruby 3.3 | 80 ms (**9.9×**) | 19 ms (**5.4×**) | 32 ms (**13.5×**) |
| PHP 8.4 | 47 ms (**17×**) | 26 ms (**4.0×**) | 9 ms (**47×**) |
| Node 22 | 11 ms (**73×**) | 12 ms (**8.5×**) | 13 ms (**34×**) |

Read it for the shape rather than the ranking: **within a small multiple of the bytecode
interpreters and nowhere near the JIT**, with calls still the worst axis because those three execute
a call as a bytecode dispatch into a frame they built once, and this walks a tree. Every caveat
§25.3 attached still applies — transliterations rather than tuned ports, `beck test` includes
parsing and checking, and this is still an interpreter standing in for a compiler. [`25`](25-benchmarks-and-expressiveness.md)
§25.9 holds every comparative *claim* until there is a second backend; this is a measurement rather
than a claim.

## 70.6 The gates it left behind

| Gate | What makes it go red |
|---|---|
| `scaling.rs` | The **shape** gates: building a list by accumulation, folding one, and text, each asserted to cost the same per element however long it gets. Checked against the old evaluator, where the list gate fails at **6.5×** |
| `liveness.rs` unit tests | Eight, including a callee read after its arguments — the shape of the second miscompile this analysis produced — and three that go red if a lambda's own parameter stops being movable |
| `frames.rs` | Five tests, none about speed: what a *program* would see if a slot were wrong, which is a closure quietly answering with somebody else's value |
| `records.rs` and the permutation's own test | **All 40,320 arrangements** of a record at the packing's full width, plus what a program sees if the layout is wrong |
| `moves.rs` | Six tests on a fold's *contents* rather than its length |
| The other 48 suites | The differential harness, the corpus, SICP against the book's printed answers, fourteen Are We Fast Yet benchmarks and eight Benchmarks Game ports against published constants. A miscompile in any of this shows up as a wrong answer there — which is how the one real miscompile was caught |
| `--fuel` | The budget charges for *work*: a primitive touching `n` elements costs `n`, so a scaling gate can assert a shape with no clock in it and get the same answer on every machine |

## 70.7 A gate that could not fail, twice over

The permutation's first gate was green and worthless, for two independent reasons, and both are
general:

**The test data.** Every record it used had a layout that is *its own inverse* — true of any single
transposition, and of most small records — so inverting the pass, which is the classic way to get a
permutation wrong, changed nothing any of them could see. It needed a record whose fields are
written `c, a, b`, because that is a three-cycle.

**Where the literals were written.** None of the three annotating passes walks a `test` block:
`liveness::mark_program`, `frames::reserve_program` and `fields::order_program` all iterate
`Program::defs`, and a `test` block's clauses live in `Program::tests`. So every literal in the gate
took the fallback path and the pass under test was never called.

This is the pattern [`AGENTS.md`](../AGENTS.md) points at: when you write a gate, ask what would
have to be true for it to go red, and check that the thing you are guarding against would make it
so. It is cited from five other reports for that reason.

## 70.8 What this corrected, elsewhere

- [`69`](69-standard-library-imports-report.md) §69.7's measured-but-unfixed quadratic is fixed.
- **`STACK_BYTES`' derivation was wrong** — the evaluator's ceiling had been measured in a debug
  build, so the release ceiling was a fiction.
  [`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)'s property holds in both
  profiles now.
- [`62`](62-fuel-report.md)'s budget means something different and the number is the same: it counts
  work rather than nodes.
- **Every wall-clock number in reports 69 through 78 is superseded by the one after it**, which is
  the strongest argument for this document existing: ten reports each correcting the previous one's
  figures is nine corrections nobody needs to read.
- [`19`](19-phase-1-report.md) §19.4's defect — an analysis that cannot see through a lambda — was
  found three times in this series before it was fixed.

## 70.9 What is not built

- **A cost model.** `--fuel` charges for work, and there is still no published table of what an
  operation costs.
- **A walker over `Clause`.** All three annotating passes skip `test` blocks, so a file whose tests
  *are* the work — most of `compiler/lib/` — gets no layout, no reserved slot and no move in them
  (§70.7). Named, not fixed.
- **A compiler.** This is a tree-walker that got fast; [`93`](93-llvm-backend-report.md),
  [`97`](97-cranelift-report.md) and [`101`](101-the-heap-report.md) are the other answer, and the
  gap between them is what §25.9 is holding.
