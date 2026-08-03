# 53 — Phase 3, part 23: Are We Fast Yet, in Beck

**Built.** Nine of Are We Fast Yet's fourteen benchmarks, ported and verified against the original
suite's own constants, in [`compiler/awfy/`](../compiler/awfy/) with a gate and a measurement.

[`50`](50-collections-and-dates-report.md) §50.6 and [`52`](52-crypto-and-identifiers-report.md)
§52.6 both ended on the same sentence — the benchmark harnesses "remain the largest thing owed on
this bullet" — and [`25`](25-benchmarks-and-expressiveness.md) §25.9 schedules them for Phase 3 with
the number held until Phase 4. This is the harness, and the number is held.

The interesting part is not the timing table, which measures a placeholder and says so. It is that a
suite deliberately built against a *common subset* of language features turns out to be a good
detector of what a language does not have: three things came out of porting it, and only one of them
is about speed.

## 53.1 What was ported, and what it is verified against

Are We Fast Yet is nine micro-benchmarks and five macro-benchmarks. The nine are here:

| | What it exercises | The suite's own `verifyResult` |
|---|---|---|
| `bounce` | allocation and field update | `1331 == (int) result` |
| `list` | calls and linked nodes | `10 == (int) result` |
| `mandelbrot` | float loops and bit packing | `result == 128` at size 1 |
| `nbody` | float arithmetic and `sqrt` | `result == -0.16907495402506745` after one advance |
| `permute` | recursion and array swaps | `(int) result == 8660` |
| `queens` | backtracking search | `(boolean) result` |
| `sieve` | array writes in a loop | `669 == (int) result` |
| `storage` | allocation | `5461 == (int) result` |
| `towers` | stack discipline and recursion | `8191 == (int) result` |

**Every constant in that column was read out of the original's source, not remembered.** The Java is
MIT-licensed and public, and the whole value of a benchmark port is that the number is somebody
else's; a suite verified against numbers the porter chose is a suite that verifies nothing.
[`awfy/README.md`](../compiler/awfy/README.md) carries the provenance and the licence notice, and
`awfy.rs::every_benchmark_names_the_suite_it_is_a_port_of` fails if a file stops saying whose
benchmark it is.

`storage.beck` additionally pins **the generator itself**. `som.Random` ships its own test vector —
its `main` checks the first nine draws — and two of the nine benchmarks are downstream of it, so a
transcription slip in three lines of arithmetic would present as a wrong benchmark rather than as a
wrong generator. Those nine numbers are a `test` block.

## 53.2 What the port had to change, and what that cost

Are We Fast Yet's common subset assumes mutable arrays, mutable object fields and bitwise operators.
Beck has none of the three, so the directory follows four rules rather than letting each file
improvise — [`awfy/README.md`](../compiler/awfy/README.md) states them and this is what they came to:

- **A mutable array is a `Map[Int, T]`**, with an absent key reading as the Java's zero-initialised
  element. Four of the nine need one.
- **A mutable object is a record, rebuilt.** Where the original mutates two things at once — an
  array *and* a counter, in `permute` — the port carries both in one record, which is a language
  without mutation making you write down what the original leaves implicit.
- **A bitwise operator is arithmetic**, and §53.5 says why that is a finding rather than a
  workaround.
- **A number is the suite's.**

Two changes are not mechanical and are worth naming, because a port that quietly did something
easier would be the failure mode this directory exists to avoid.

**`queens` does not undo.** The original sets three arrays on the way down and unsets them on the way
back up. The port passes the board the caller still holds, so backtracking needs no undo at all. The
search visits the same nodes in the same order; it does four fewer assignments per rejected
placement. That is a real difference in the work performed, it favours Beck, and it is here rather
than in a footnote for that reason.

**`towers` keeps its exceptions.** The original throws if a disk lands on a smaller one — an
assertion that the algorithm did what it claims. A port that dropped it would pass while moving
nonsense. `raise` is [`45`](45-error-rows-report.md)'s row label, so `benchmark` publishes
`raises(TowersError)` by inference and its test faces it with `try:`. This is the first program in
the tree where an effect row exists because a *benchmark* had an invariant.

**`list` deliberately does not use `list[T]`.** The benchmark walks a chain of allocated nodes one
link at a time, and [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.6 recorded that a
`list[T]` cannot share a suffix — so `list_drop` would copy where the original follows a pointer, and
the port would measure the copy. It is a `union Chain` with a `Nil`, which is
[`27`](27-walls-report.md)'s recursive type doing the job Java's `null` does, with no null.

## 53.3 The numbers, and what they are not

`cargo test --release --test measure_awfy -- --nocapture`, median of five, on the machine this was
written on:

| benchmark | `beck check` ms | `beck test` ms | difference |
|---|---|---|---|
| bounce | 4.5 | 59.6 | 55.1 |
| list | 3.9 | 51.3 | 47.4 |
| mandelbrot | 4.1 | 4.9 | 0.8 |
| nbody | 5.4 | 7.2 | 1.7 |
| permute | 3.4 | 105.3 | 102.0 |
| queens | 3.8 | 80.8 | 76.9 |
| sieve | 3.3 | 57.8 | 54.5 |
| storage | 3.7 | 56.2 | 52.5 |
| towers | 4.8 | 174.3 | 169.5 |

Four things that table is **not**:

1. **It is not a benchmark time.** It is the whole binary on the whole file, and the difference is
   the benchmark *plus the test harness around it*. It is reported as a difference for that reason.
2. **It is not comparable to anything.** [`25`](25-benchmarks-and-expressiveness.md) §25.9 holds
   every comparative claim until a second backend exists; the tree-walker is scaffolding, and
   §25.3's 33×-CPython measurement is why a number about it is a number about scaffolding.
3. **It is not at the suite's sizes for two of the nine.** `mandelbrot` verifies at size 1 and
   `nbody` after one advance. Both are values Are We Fast Yet itself publishes for those sizes —
   its `verifyResult` carries three sizes for `mandelbrot` and two for `nbody` — but its *published
   results* are about the defaults, and these are not those.
4. **It is not thresholded.** [`13`](13-testing.md) §13.7: a timing gate on a shared runner cannot be
   held honestly, and a gate that flakes gets deleted.

What stops `mandelbrot` reaching size 500 is worth stating precisely, because "too slow" is the kind
of sentence that turns out to be about something else. It is **not** the clock: it is the evaluator's
fuel budget, 50,000,000 steps, which is a deliberate runaway-program backstop
(`interp.rs::DEFAULT_FUEL`) and which size 500 exhausts after about 5.5 s in a release build.
Nothing exposes that budget to a caller, so the size is out of reach of `beck test` rather than out
of reach of the machine.

## 53.4 A float literal may carry an exponent

`nbody`'s constants are published in scientific notation — `4.84143144246472090e+00` — and Beck's
float literal was `[0-9][0-9_]*\.[0-9][0-9_]*`. **No exponent.** Every constant would have had to be
rewritten as a plain decimal on the way in.

They are the same number: `parse::<f64>` is correctly rounded, so `6.90460016972063023e-05` and
`0.0000690460016972063023` are the same `f64`. The objection is not arithmetic, it is that a
transcription a reader cannot check against the source by eye is a transcription with a place for an
error to hide, and this file's whole claim is that seventeen digits came out right.

So the lexer takes one: `1.5e-3`, and `1e6` for an exponent with no point. The second rule is there
because a literal whose value is not representable as an `Int` must not lex as one — without it,
`1e6` is the integer `1` followed by an identifier. `1.e6` is not a float in any of the languages
that have this notation and is not one here.

This is a small change and it is the whole language change in this report. It is recorded rather than
folded in silently because it is a **lexical** change: a program that used to be one thing is now
another, and the only reason that is safe is that `1e6` was previously a parse error at the point of
use rather than a legal expression.

## 53.5 Three findings, and only one of them is about speed

### `and` evaluates both operands

`Queens.java` reads:

```java
if (getRowColumn(r, c)) {
  …
  if (placeQueen(c + 1)) { return true; }
```

Two nested tests, and in Beck that reads as one conjunction. Written as one, the benchmark **runs out
of fuel**: `and` is a primitive over two `Bool`s, its arguments are evaluated before it is called, so
`free_at(b, r, c) and settles(…)` searches from squares that are already attacked and turns a
2,057-node search into an 8⁸ one.

This is not about queens. Beck's `and` and `or` do not short-circuit, which means:

- a guard does not guard — `if b != 0 and 10 / b > 1` divides by zero;
- a `raise` in the right operand fires when the left already decided the answer, so a `try:` is
  needed for a failure the program cannot reach;
- and an effectful right operand **performs its effects**, so an expression's inferred row is larger
  than the expression can actually reach.

Nothing in the tree was wrong because of it — the standard library's uses are all pure, total and
cheap, which is why three phases and six libraries never noticed. That is exactly the argument for a
benchmark suite: it is written by people who assume the common subset, and it assumed this.

It is **pinned rather than fixed**, in the shape [`50`](50-collections-and-dates-report.md) §50.5 used
for the record-ordering finding: `awfy.rs::and_evaluates_both_operands_and_a_guard_written_with_it_does_not_guard`
asserts the current behaviour, so changing it is a red test and a correction to this section rather
than a silent change of meaning in every program that guards with `and`. What fixing it would take is
one rewrite in the checker — `a and b` becomes `if a: b else: false` — and no new IR node, because
`CoreKind::If` is already there; what it needs before that is a decision about whether `and` stays a
name in the prelude when no source expression lowers to it, which is a published-surface question and
not a two-line one. It is named here as the next person's starting point, as §50.5 named `Ord`.

### Beck has no bitwise operators

`mandelbrot` packs escape bits eight to a byte and folds the bytes with exclusive-or. The checksum
*is* the answer, so the packing is not incidental — a port that counted escapes instead would be a
different benchmark with a different number.

`<<` is written as repeated doubling and `^` bit by bit, in Beck, in the benchmark file. `& 65535` in
`som.Random` is `% 65536`, which is the same operation on a value the recurrence keeps non-negative.
The replacements are tested against what the operators *mean* rather than only through the benchmark
that uses them, because a hand-written exclusive-or that is wrong in a way the checksum happens not
to catch is the worst outcome available here.

This is the largest gap between an original and its port anywhere in the directory, and it is a gap
rather than a wall: nothing is inexpressible, and `xor_of` is eight lines. Whether Beck should have
the operators is a question this report raises and does not answer — the honest summary is that one
benchmark in nine needed them, the workaround is exact, and no other program in the tree has ever
asked.

### An `if` inside an `if` is not a statement

The first draft of `queens` nested the two tests directly:

```beck
if free_at(b, r, c):
    if settles(r, c, occupy(b, r, c)):
        return true
return try_row(r + 1, c, b)
```

```
error[B0320]: the two branches mismatch: expected `Unit`, found `Bool`
```

The inner `if` is the last thing in the outer block, so the outer block's value is an `if` with no
`else`. The rule is defensible and the diagnostic is pointing at a real mismatch; what it does not do
is say that the *shape* is the problem, and the reader's fix — writing the fall-through by hand —
is not the one the message suggests. The port names the intermediate instead, which is what a strict
`and` was going to force anyway.

Recorded, not fixed, and smaller than the other two: it is a diagnostic that could say more, in the
shape [`50`](50-collections-and-dates-report.md) §50.5's missing-type-argument suggestion could.

## 53.6 What is **not** built

| | Status |
|---|---|
| The five macro-benchmarks — CD, DeltaBlue, Havlak, Json, Richards | **not ported.** Not attempted, not measured, and not declined: each is several hundred lines of mutable object graph, and porting them is what this directory is owed next. Nothing is known about whether they are expressible |
| The Computer Language Benchmarks Game harness | **not stood up.** [`25`](25-benchmarks-and-expressiveness.md) §25.2 rates it below Are We Fast Yet — entries are hand-tuned, so it measures effort as much as compilers — and two of its programs arrive here through Are We Fast Yet anyway. It remains on the Phase 3 list |
| Any comparative number | **none, deliberately.** §25.9's rule, unchanged |
| The suite's default sizes for `mandelbrot` and `nbody` | **not reached**, and §53.3 says what stops the first: the evaluator's fuel budget, measured, not the clock |
| A benchmark time that is the benchmark | **not measured.** What is printed is the binary on the file; separating the evaluation would need the evaluator driven in-process, which is a harness change rather than a measurement |
| Compile-speed budgets, and the Felleisen table | **not built.** [`25`](25-benchmarks-and-expressiveness.md) §25.9 schedules both alongside this for Phase 3, and §25.9 calls the Felleisen table the cheapest item in §8.4 and one that waits on none of the six walls — all of which are now down |

## 53.7 What this corrects

- **[`25`](25-benchmarks-and-expressiveness.md) §25.7's ordering is partly discharged.** Its
  instruction was to "hold CLBG and Are We Fast Yet until there is a backend for them to be about",
  which §25.3 item 1 immediately qualifies with "adopt the harnesses now, publish the numbers now,
  and make them unflattering". The two are only compatible if *adopting* and *publishing a
  comparative claim* are different acts, and this report treats them as different: the harness is
  here, the wall-clock is here, and the comparison is not.
- **[`50`](50-collections-and-dates-report.md) §50.6 and
  [`52`](52-crypto-and-identifiers-report.md) §52.6 lose their last shared row.** "The AWFY and CLBG
  harnesses remain the largest thing owed on this bullet" is now half true: AWFY is nine-fourteenths
  built and CLBG is untouched.
- **[`08`](08-roadmap.md) §8.4's Phase 3 row gains its first tick.** "Are We Fast Yet and CLBG
  harnesses against the evaluator", stood up in part.
- **Nothing in the design documents had to change for §53.4**, and that is the correction.
  [`02`](02-syntax.md) settles the *shape* of the surface — layout, block-passing, typed literal
  macros — and never writes down what a number looks like; the numeric literal grammar lives only in
  `lexer.rs`, and the generated reference derives from the compiler's tables rather than from its
  regexes, so it does not carry it either. A reader looking for "may a float have an exponent" has
  nowhere to look but the source. That is not fixed here — it is named, because it is the kind of
  gap [`34`](34-generated-documentation-report.md) exists to close and has not yet.
- **[`21`](21-tests-in-beck-and-proof.md) §21.2 gains a use nobody designed it for.** A `test` block
  is a program's own assertion about its meaning; here it is a *third party's* assertion about the
  program's meaning, carried into the file. Nothing had to change for that to work, which is the
  observation rather than the achievement.

## 53.8 What Phase 3 is still not

Unchanged from [`52`](52-crypto-and-identifiers-report.md) §52.8 except where this touches it. The
standard-library bullet's benchmark half has started; the exit criterion — an outside developer
building a non-trivial app from documentation alone — is not met and is not closer.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
