# 58 — Phase 3, part 27: Json, and the three macro-benchmarks that are not here

**Built.** [`compiler/awfy/json.beck`](../compiler/awfy/json.beck) — the second of Are We Fast Yet's
five macro-benchmarks, verified against the suite's own count. Are We Fast Yet is **eleven of
fourteen**.

**Not built: CD, DeltaBlue and Havlak.** [`57`](57-richards-report.md) §57.5 assessed all four
remaining macro-benchmarks and said none was blocked by the language. That was right, and it was not
the whole story: they are also *large*, and §58.5 below sizes each one from its own source rather
than leaving "large" to be imagined. This report is honest about delivering one of the four it set
out to deliver.

## 58.1 What Json is, and why it is not `json_parse`

25,820 characters of minified RAP protocol traffic, parsed, with `verifyResult` asking four
questions of the result: that it is an object, that its `head` is an object, that its `operations`
is an array, and that the array has **156** entries.

Beck has a `json_parse` **primitive**. [`46`](46-standard-library-report.md) §46.2 put it there on
purpose — a grammar is somebody else's, so it belongs on the host's side of
[`lib/README.md`](../compiler/lib/README.md)'s division — and calling it here would have made this
benchmark a measurement of `serde` and a claim about nothing. That is exactly the substitution Are
We Fast Yet's methodology exists to forbid: the suite is written against a common subset *so that*
what gets compared is implementations rather than standard libraries.

So the parser is written out: `read_value`, `read_array`, `read_object`, `read_string`,
`read_escape`, `read_number`, `read_fraction`, `read_exponent` — the original's functions, with the
original's names.

The prelude already owning the names `Json`, `JsonNull`, `JsonObject` and the rest is what forced
the port's types to be called after the *Java* classes instead: `JsonValue` and `ParseException`.
That collision is a small thing and it is the right way round — the benchmark is not allowed to be
the standard library, and now it cannot accidentally be.

## 58.2 The one liberty, and it is the one that was forecast

[`57`](57-richards-report.md) §57.5 predicted this exactly:

> `str_slice` indexes by **character**, so a character-at-a-time reader over 25 KB is quadratic.
> `str_chars` gives a `list[Str]` whose `list_get` is an index, so the fix is one line at the top
> and the rest is a transcription.

That is what the port does. `Reader` holds `str_chars(text)` — the input as one character per
element — and a position. Everything else reads as the Java does, because `list_get` is an index
into a `Vec` and `input.substring(index, index + 1)` was one too.

The forecast being right is worth as much as the port. §57.5 was written from the Java source and
from what [`55`](55-bignums-report.md) §55.5 had already found out about `str_slice`; nothing had
been attempted. A prediction that survives contact is a prediction the next assessment can be
trusted a little further — which is why §58.5 below is worth writing.

The second difference is [`57`](57-richards-report.md) §57.2's third rule again, applied to a parser
rather than to a scheduler: **the reader is a value**. Java mutates `index` and `current` on a
field; here every function that consumes a character returns the reader it left behind, and
`Parsed` and `Captured` are the two records that carry a result beside one.

## 58.3 What it costs

`beck test awfy/json.beck` is **1.7 s** in a debug build — the third most expensive file in the
directory, behind `richards` (21 s) and ahead of `towers` (0.4 s). One pass over 25,820 characters,
building about two thousand values.

No comparative claim, unchanged: [`25`](25-benchmarks-and-expressiveness.md) §25.9 holds those until
there is a second backend.

## 58.4 What the port does less of, and one thing it does more

**Less.** `JsonObject` in the original keeps a `HashIndexTable` beside its two vectors and updates
it on every `add`; `field` here is a linear scan of the names. The trace has 156 operations and its
objects are small, and `verifyResult` performs exactly three lookups — so the table is work the
original does and this does not. It is named here rather than left in the code, because "the port
is faster" and "the port does less" are the same sentence and only one of them is honest.

**More.** The benchmark's own input contains **no backslash in 25,820 characters**, so
`read_escape` is never reached by it. The escapes are checked by their own `test` block instead —
including that `A` is *refused*, which is the original's behaviour too and would otherwise be
an untested branch that could accept more than the benchmark does.

## 58.5 The three that are not here, sized

[`57`](57-richards-report.md) §57.5 said what each needs. This says how big each is, counted from
the Java sources, and which published configuration it would verify against — so that "not done" has
a number attached rather than a shrug.

| | Java lines | Verifies against | What it needs |
|---|---|---|---|
| **CD** | 878 across ten files — `RedBlackTree` **406**, `CollisionDetector` 182, `Motion` 116, and the geometry — plus `som.Vector` (248) | 42 collisions at **2 aircraft** over 200 frames, which is the smallest published configuration of any of the three | A red-black tree with **deletion**, in Beck. `CollisionDetector` needs `put` returning the previous value, `get`, `remove` and in-order iteration. Using a `Map` instead would be the forbidden substitution — the tree is a data structure the benchmark implements, not a library call |
| **DeltaBlue** | about 1,140 across twelve files — `Planner` 308, `BinaryConstraint` 155, `AbstractConstraint` 135, `Strength` 125 — plus `som.Vector` and `som.IdentityDictionary` | **nothing.** `innerBenchmarkLoop` runs `chainTest` and `projectionTest` and returns `true`; the oracle is the assertions *inside* the planner, so a port must keep every one of them or it verifies nothing | An arena for a **cyclic** graph — variables point at constraints and constraints back at variables, both mutated during planning. Richards' `Map` keyed by identity is the shape; the cycles are the data structure rather than an artefact |
| **Havlak** | about 770 across seven files — `HavlakLoopFinder` **318**, `LoopTesterApp` 123, `SimpleLoop` 118 — plus `som.Vector`, `som.Set` and `som.IdentityDictionary` | 1,605 loops and 5,213 nodes at `innerIterations == 1` | Union-find with path compression as a threaded `Map`. **And a workload the others do not have**: at that configuration `findLoops` runs **52 times over a 5,213-node graph**, which is arithmetic off `LoopTesterApp.main(1, 50, …)` rather than a measurement of Beck. Whether that fits the evaluator's 50,000,000-step fuel budget is *unknown* and is the first thing to find out — `mandelbrot` at size 500 did not ([`53`](53-are-we-fast-yet-report.md) §53.3) |

Two things follow that are worth stating as conclusions rather than as table cells.

**`som.Vector` is needed by all three and by neither of the two that are done.** 248 lines, and the
question [`57`](57-richards-report.md) §57.5 raised is still open and still unanswered here: whether
it belongs in `awfy/` as a port, or whether the benchmarks should use `lib/collections.beck`. The
answer is almost certainly the port — Beck's `Map` is a primitive backed by the host's `BTreeMap`,
so reaching for it is reaching for Rust — but that decision deserves to be taken where it can be
written down rather than in passing.

**Havlak should be attempted before CD and DeltaBlue**, despite being neither the smallest nor the
largest. It is the only one of the three with a *cheap* thing to learn first: whether its published
configuration runs at all under `beck test`. A day spent porting 770 lines to discover the fuel
budget refuses them is a day better spent knowing that on the first morning.

## 58.6 What is **not** built

| | Status |
|---|---|
| CD, DeltaBlue, Havlak | **not ported**, and §58.5 sizes each. None blocked by the language; between them about 2,800 lines of Java plus the collection classes |
| `som.Vector`, `som.Set`, `som.IdentityDictionary` | **not ported.** All three of the above need at least one, and the methodological decision about them is named in §58.5 and not taken |
| The CLBG harness | **not stood up**, unchanged from [`53`](53-are-we-fast-yet-report.md) §53.6 |
| A `HashIndexTable` for `json`'s objects | **not built**, deliberately — §58.4 says what that costs the comparison |
| JSON's `\uXXXX` escape | **not accepted**, which is the original's behaviour and is asserted rather than assumed |
| Any comparative number | **none**, unchanged |

## 58.7 What this corrects

- **[`57`](57-richards-report.md) §57.5's Json row is discharged**, and its forecast about
  `str_slice` was right — §58.2. The other three rows stand and are given sizes in §58.5.
- **[`08`](08-roadmap.md) §8.4's Phase 3 row moves again.** Are We Fast Yet is eleven of fourteen.
- **[`46`](46-standard-library-report.md) §46.2's division gets a case it did not anticipate.** A
  grammar belongs to the host — *unless* the program's whole purpose is to be the grammar, at which
  point using the primitive would measure the wrong thing. `json.beck` is the first program in the
  tree that deliberately declines a primitive, and the reason is methodological rather than
  technical.

## 58.8 What Phase 3 is still not

Unchanged from [`57`](57-richards-report.md) §57.8. The standard-library bullet's library half is
done and its harness half is eleven of fourteen. The exit criterion — an outside developer building
a non-trivial app from documentation alone — is not met and is not closer.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
