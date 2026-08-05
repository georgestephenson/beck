# 78 — Phase 3, part 47: a record is a permutation

**Built.** A record literal sorted its field names on every construction. Which order they belong
in is written in the source, so it is now decided once, by a pass, and spent as a permutation.

It is worth **about 1.5%** on the one program in the tree that builds a million records, and
**nothing measurable** anywhere else — against **4.5% of the instructions** removed, which is the
more useful half of this report. [`76`](76-the-record-and-the-read-report.md) §76.4 said an
instruction profile ranks candidates and the wall clock decides between them; §78.4 puts a ratio on
the gap, and §78.7 turns the same instrument on
[`77`](77-a-let-is-a-slot-report.md) §77.7's own next item and recommends **not building it**.

## 78.1 What a record literal cost

`callgrind` over `awfy/richards.beck`, which is the most record-shaped program in the tree:

| | |
|---|---|
| record literals evaluated in one run | **1,166,277** |
| instructions inside the sort they ran | **416,262,745** — 5.62% of the whole process |
| per record, to order three or four names | **357 instructions** |

Three hundred and fifty-seven instructions is not a bad sort. It is `sort_unstable_by`'s prologue
over a slice of four, run a million times: a pattern-defeating quicksort that recognises it has
almost nothing to do and hands the slice to an insertion sort, which then makes three comparisons
and — because a literal's fields are usually written in declaration order and declaration order is
usually not name order — moves them.

What makes it worth removing is not that it is slow. It is that **the answer is a constant.**
`Ball(x=…, y=…, x_vel=…, y_vel=…)` puts its fields in the order `x, x_vel, y, y_vel` every single
time it runs, because that is a fact about four names in a source file. [`AGENTS.md`](../AGENTS.md)
says to ask what an operation *should* cost before asking how to make this one faster; a record
literal should cost the fields it evaluates, and nothing for deciding where they go.

Records are held in name order rather than declaration order for the reason
[`54`](54-ordering.md) gives and [`50`](50-collections-and-dates-report.md) §50.5 pinned as a test:
the order is the `Map`'s iteration, the value order, the state digest and the patch stream. It is
not a detail of the evaluator, which is why the permutation has to be exactly the sort's and is
tested as such.

## 78.2 What replaces it

`beck_core::fields`, a pass beside [`70`](70-last-use-moves-report.md)'s liveness and
[`77`](77-a-let-is-a-slot-report.md)'s frames, running where they run — once, on the finished
program, before any backend sees it. For each `Make` node it computes where each written field
belongs and packs the answer into `Core::order`, **four bits per field**:

```rust
let map = if order != fields::UNORDERED && pairs.len() <= fields::MAX_ORDERED {
    fields::place(&mut pairs, order);   // no comparison, no allocation
    Fields::from_sorted(pairs)
} else {
    Fields::from_pairs(pairs)           // the old path, unchanged
};
```

Four decisions are worth their sentences.

**It costs no memory.** A `u32` on `Core` fits in the padding `last_use: bool` already left, so a
`Core` node is 160 bytes either way — checked rather than hoped, since
[`75`](75-what-the-profiler-said-report.md) §75.4 measured that a `Core` node's width is worth
6.6% of *checking* even when it is worth nothing at run time.

**Eight fields, and a sentinel that cannot be a layout.** Every nibble of a real layout is below
`MAX_ORDERED = 8`, so `u32::MAX` — `0xf` in every position — is not a permutation of anything and
is safe to mean "no layout". A record wider than eight sorts at run time. That is a constant in a
pass and not a limit in the language: nothing declares a record that wide, and one that did would
be slower rather than refused.

**Not answering is always allowed.** [`77`](77-a-let-is-a-slot-report.md) §77.3's rule, kept: a
repeated field name has no layout, a wide record has no layout, and a program built by something
that never runs the pass has no layout. All three sort, exactly as everything did before this
existed. There is one synthesised record in the tree — the splitter's fused state, made after the
pass has run — and it asks for its own layout rather than going without one.

**Evaluation order is untouched.** A field expression can `raise`, so which of two fallible fields
fails first is observable, and it is still the order they are written in. The values are all
produced first and *then* placed, which is the whole reason §78.3 is about an allocation.

## 78.3 The version that was built first, measured, and thrown away

The obvious implementation evaluates the fields into one vector and selects them out of it into a
second, in the finished order. It is shorter than what is here. It is also **slower than doing
nothing at all**:

`awfy/richards.beck`, 32 interleaved runs, four binaries in rotation (§78.6), against the unchanged
compiler:

| | min | median |
|---|---|---|
| unchanged, measured against a copy of itself | −1.3% | +0.1% |
| **selecting into a second vector** | **+2.2%** | **+1.9%** |
| **permuting the vector in place** | **−3.3%** | **−1.5%** |

An allocation is dearer than an insertion sort over four names. The second vector cost more than
the whole of what the sort cost, which is [`75`](75-what-the-profiler-said-report.md)'s finding
arriving from the other direction: `malloc` was 35% of that profile, and a change that removes 4.5%
of the instructions while adding one allocation to the hottest constructor in the language is not a
saving — it is a loss of about the same size.

What is here permutes the vector that already exists, following the permutation in its cycles. The
bookkeeping a cycle-following permutation needs — which positions are already placed — is the
packed layout itself, copied into a local and rewritten as it goes, so it is a `u32` in a register
and the whole operation touches no memory but the elements it moves.

## 78.4 What it buys, and what the profile promised

Release, 30 interleaved runs in rotation, both statistics. The **control** column is the unchanged
binary measured against a byte-identical copy of itself in the same run: it is the noise floor, and
without it none of the rest of this table can be read.

| | before | after | change (min) | change (median) | control (min / median) |
|---|---|---|---|---|---|
| `awfy/richards.beck` | 1,179 ms | **1,161 ms** | **−1.5%** | **−1.1%** | 0.1% / 0.2% |
| `awfy/havlak.beck` | 2,119 ms | 2,114 ms | −0.2% | −0.5% | 1.1% / 0.2% |
| `awfy/json.beck` | 74 ms | 75 ms | +0.7% | −2.0% | 0.4% / −1.8% |
| `awfy/cd.beck` | 138 ms | 138 ms | +0.1% | +1.8% | −0.2% / −0.6% |
| `clbg/knucleotide.beck` | 493 ms | 492 ms | −0.2% | −0.8% | −1.6% / −1.6% |
| `lib/decimal.beck` | 193 ms | 193 ms | −0.2% | −0.5% | 1.3% / −0.7% |
| `clbg/pidigits.beck` | 802 ms | 801 ms | −0.1% | −0.7% | −0.3% / −0.7% |

**One row is a result and the rest are not.** `richards` moves by more than its control in three
separate rotated runs — −1.9%, −1.1% and −1.5% by median, against controls of −0.7%, +0.2% and
+0.1% — so it is about **1.5%**, and it is the program that builds 1.17 million records.
`pidigits` builds none and is the control that matters. Everything between them is inside its own
noise, and this measurement says nothing about those rows in either direction.

Compile time is unchanged: a whole-program walk that writes a `u32` per record literal is
−3.1% to +1.6% on `beck check` across four files, against noise of up to 5.2% on the smallest.

**And the correction this report exists for.** The same `richards` run under `callgrind`, before
and after:

| | before | after |
|---|---|---|
| instructions | 7,413,079,618 | **7,083,216,351** — −4.5% |
| inside `memcmp` | 215,279,906 | **159,814,341** — −26% |
| wall clock | 1,179 ms | 1,161 ms — **−1.5%** |

**4.5% of the instructions were worth about 1.5% of the clock — a factor of three.** The removed
instructions were a tight, predictable, register-bound loop, which is the kind a modern core
retires several of per cycle while the interpreter around it waits on pointers. [`76`](76-the-record-and-the-read-report.md)
§76.4 estimated the same effect at "6.21% of instructions worth about 2%"; this is the first time
both numbers have been measured on the same pair of runs rather than one estimated from the other,
and three is the ratio to plan with.

## 78.5 How it is tested

The permutation is small, self-contained code — right for the shapes somebody thought to write down
and wrong for one of the ones they did not — so it is tested exhaustively rather than by example:
**all 40,320 arrangements of a record at the packing's full width**, each placed and compared
against the order it must produce. Plus the identity, a repeated name, a record one field too wide,
and a literal inside a lambda, which is the walk — a lambda's body being the one child held behind
an `Arc`.

`beck-cli/tests/records.rs` is the other half, five tests, none about speed. They assert what a
*program* would see if the layout were wrong: that three ways of writing the same literal are one
value, that `<` between records is decided by name order from either notation, that a nine-field
record still orders by name through the fallback, that `with` and a literal agree, and that the
first *written* fallible field is still the one that fails.

That file's first draft could not fail, twice over, and both reasons are §78.6.

`beck explain incremental` and `beck explain place` are **byte-for-byte identical** over all 44
corpus, example, SICP and library programs — both subcommands, 88 outputs.

## 78.6 What this found

**A gate that could not fail, for two different reasons.**

The first was in the test data. Every record the gate used had a layout that is *its own inverse* —
which is true of any single transposition, and true of most small records — so inverting the pass,
the classic way to get a permutation wrong, changed nothing any of them could see. It needed a
record whose three fields are written `c, a, b`, because that is a three-cycle. The mutation now
turns the suite red, and did not before.

The second was in where the literals were written. **None of the three annotating passes walks a
`test` block.** `liveness::mark_program`, `frames::reserve_program` and now
`fields::order_program` all iterate `Program::defs`, and a `test` block's clauses live in
`Program::tests` — so a record literal, a binding or a last read written inside a `test` block gets
no layout, no reserved slot and no move, and takes the fallback. Instrumenting `place` is what
showed it: for the literals written in the test block, it was never called.

Nothing measured here is affected, because a benchmark's work is in its `def`s and the `test` block
is the line that calls them. But it is a real gap with a real cost for a file whose tests *are* the
work, which is most of `compiler/lib/`, and it is named here rather than fixed: covering it means a
walker over `Clause`, which is a change to all three passes and belongs in one of its own.

**And a finding about the measurements themselves, which applies to more than this report.** The
first version of §78.4's table ran the binaries in a fixed order within each repetition, and
reported −3.0% on `havlak` and −2.9% on `richards`. Rotating the order — so each binary takes each
position equally often — collapsed `havlak` to −0.2% and left `richards` at −1.5%. A fixed order
gives the later positions a systematically warmer machine, and on a shared runner that bias is the
same size as the effects this project has been reporting.

Two things follow, and both are cheap:

- **Rotate the order** of the binaries under comparison, rather than measuring A then B.
- **Measure a control** — the unchanged binary against a byte-identical copy of itself, in the same
  run. Its number should be zero. When it is 1.3%, as it is on `awfy/deltablue.beck` at 45 ms, the
  honest report is that the measurement has nothing to say about that row, which is a different
  sentence from saying nothing changed.

The wall-clock numbers in [`69`](69-standard-library-imports-report.md)–[`77`](77-a-let-is-a-slot-report.md)
were taken without either, on the same runner. Nothing suggests any of them is wrong — most are far
larger than this bias, [`73`](73-closures-share-their-code-report.md)'s 56–72% and
[`74`](74-the-cost-of-a-call-report.md)'s 17–27% especially — but the ones in the low single digits
were measured with an instrument that has since been shown to have a bias of that size, and this
paragraph is the correction rather than a re-measurement of six reports.

## 78.7 What is not built, and one thing that should not be

| | |
|---|---|
| A `with` and a field access placed by index | **not built.** Both are searches with a compile-time answer, exactly as a literal was. Reading a field is 4.3 million `memcmp` calls in one `richards` run, 1.19% of its instructions — which by §78.4's factor of three is about 0.4% of the clock |
| Interned field names | **not built**, per [`76`](76-the-record-and-the-read-report.md) §76.7. It is what would make the row above worth doing rather than most of it |
| A `test` block's expressions annotated like a definition's | **not built**, §78.6 |
| A variable read that is an index | **measured, and it should not be built** — below |

[`77`](77-a-let-is-a-slot-report.md) §77.7's first row has been on the list since
[`76`](76-the-record-and-the-read-report.md), on the strength of `Env::read` being 3.4% of
`richards`'s instructions. §76.4's rule says to put a clock on it before writing a calling
convention on the back of a profile, and this is that clock: the same loop, 12 million reads,
varying only how far down the frame the binding is — because the scan is the only thing an index
removes.

Nine runs in rotation, minimum:

| bindings in the frame | a constant | reading the nearest | reading the furthest |
|---|---|---|---|
| 8 | 2,183 ms | 2,288 ms | 2,305 ms |
| 24 | 4,154 ms | 4,271 ms | 4,291 ms |

A variable read costs **about 9 ns** more than a constant, at both widths. The scan — the only part
an index removes — costs **a fraction of a nanosecond per binding passed**: 1.4 ns across seven
extra bindings and 1.7 ns across twenty-three.

So making the read an index saves under **2 ns of a 9 ns read**, and the other 7 ns is the walk and
the clone, which an index does not touch. §77.7 described the work as "`Core::locals` extended to a
slot per binding and a `(hops, slot)` on every `Var` — the same pass, more of it". It is: a calling
convention, a second annotation on the hottest node in the IR, and a fallback for every frame the
evaluator chains dynamically. For well under half a per cent.

**That row should come off the list, and the reason is the measurement rather than the size of the
job.** [`76`](76-the-record-and-the-read-report.md) named it from a profile;
[`77`](77-a-let-is-a-slot-report.md) looked straight at it and found the *write* was the expensive
half; this looked at what is left and found the scan was never the expensive part of the read
either. Three reports have now pointed at `Env::read`, and after
[`77`](77-a-let-is-a-slot-report.md) put a body's bindings in one frame there is not much there.

## 78.8 What this establishes

**That the tree-walker's remaining named items are all small, and that this is now measured rather
than suspected.** Six reports of performance work have taken it from
[`25`](25-benchmarks-and-expressiveness.md) §25.3's `fib(30)` at 4.12 s to about 0.8 s, and the size
of the findings has fallen all the way down: [`73`](73-closures-share-their-code-report.md) was one
line and 56–72%, [`74`](74-the-cost-of-a-call-report.md) five changes and 17–27%,
[`75`](75-what-the-profiler-said-report.md) an allocator and 4.9–17%,
[`76`](76-the-record-and-the-read-report.md) and [`77`](77-a-let-is-a-slot-report.md) 4–8%, and
this 1.5% on one program. Every remaining item on the list has now been measured at under one per
cent by the instrument that decides.

That is the argument [`25`](25-benchmarks-and-expressiveness.md) §25.9 has been waiting to be able
to make: the next order of magnitude is not in this evaluator, and the second backend is what the
comparative claims are being held for.

**And that a measurement needs a control.** This change is the first in the project whose effect is
smaller than the instrument's own bias, and finding that out required measuring nothing against
nothing. §78.6 is the cheaper half of this report and the half that applies to the next one.
