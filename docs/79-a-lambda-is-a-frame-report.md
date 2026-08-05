# 79 — Phase 3, part 48: a lambda is a frame

**Built.** The accumulator idiom written as a **fold** was `O(n²)`, three reports after
[`70`](70-last-use-moves-report.md) made the recursive form linear. It is linear now: 289 ms → 9 ms
over 8,000 elements, and 640 → 18 evaluator steps per element.

This report started as [`78`](78-a-record-is-a-permutation-report.md) §78.6's leftover — that none
of the three annotating passes walks a `test` block — which is fixed here and is worth nothing on
its own. Fixing it made a probe that should have got faster stay quadratic, and that is what
uncovered the real one.

## 79.1 The defect

[`19`](19-phase-1-report.md) §19.4 found a fold copying its accumulator and called it "a semantic
defect, not a backend one". [`69`](69-standard-library-imports-report.md) §69.7 found it again in
`list_append`. [`70`](70-last-use-moves-report.md) fixed it: the compiler marks the read of a local
nothing reads again, the frame hands the value over instead of lending it, and `list_append` pushes
into a list nobody else holds.

It fixed one of the two ways a Beck program writes a loop.

```text
def upto(i, n, done):                       # ← linear since docs/70
    if i >= n: return done
    return upto(i + 1, n, list_append(done, i))

list_fold(xs, [], lambda acc, x: list_append(acc, x))    # ← still quadratic
```

Measured at three sizes, release, before this change:

| elements | recursive | ×/doubling | fold | ×/doubling |
|---|---|---|---|---|
| 2,000 | 4 ms | | 23 ms | |
| 4,000 | 5 ms | ×1.23 | 77 ms | **×3.37** |
| 8,000 | 6 ms | ×1.22 | 289 ms | **×3.76** |

A doubling that costs 3.7× is `O(n²)`, and the fold form is the one `list_fold` — a primitive the
language offers — invites you to write.

## 79.2 Why the analysis could not see it

[`70`](70-last-use-moves-report.md)'s rule 2: *a `lam` body is not analysed against the enclosing
frame.* A closure outlives the expression that built it and may be called any number of times
later, so every variable it reads has to stay lent. The implementation of that rule collected every
variable mentioned anywhere inside any lambda and excluded all of them from marking — and its own
comment said what that cost: "a lambda's own parameters are counted too, which costs a missed move
and never an unsound one".

`acc` is a lambda's own parameter and nothing else. The exclusion written to protect a closure's
*captures* was swallowing the closure's *bindings*, so nothing inside any lambda was ever marked,
and the missed move was the whole idiom.

The rule was right about one thing and wrong about another, and the two are separable:

- A variable a lambda takes **from the scope around it** belongs to a frame that outlives the call.
  It must stay lent. Rule 2 is about this and remains exactly as it was.
- A variable **the lambda itself binds** — its parameters, its `let`s, its match arms — lives in the
  frame that call makes. `Env::call_frame` builds it fresh and it dies with the call, which is the
  same argument [`70`](70-last-use-moves-report.md) already makes for a *definition's* parameters.
  A definition is a lambda; this is the same frame.

## 79.3 What replaces it

One word in the condition, and a walk that starts over at each lambda:

```rust
c.last_use = own.contains(&v) && !captured.contains(&v) && !live.contains(&v);
```

`own` is what this frame binds. A lambda's body now gets its own analysis — its own `live` set
starting empty, its own `captured` set for the lambdas nested inside *it*, and its own `own` — and
outwards it contributes what it always did: every variable it takes from the enclosing scope
becomes live there, for as long as the closure exists.

That third condition is what makes the flag's promise true rather than accidentally true. **The
evaluator would refuse the unsound move anyway**: a closure holds an `Arc<Env>` of its captured
environment, so that environment is shared, and `Env::read` empties a frame only when
`Arc::get_mut` proves nothing else holds it. But [`70`](70-last-use-moves-report.md) states the
flag as a property of the *program* — "it says nothing about other frames, other calls or the heap;
a backend must check that separately" — and a flag that is only true because one backend is careful
is a trap for the second one.

Nothing about the *runtime* changed. No new IR node, no evaluator case, no primitive: the fold
becomes linear because `list_append` now receives a list the frame handed over, and pushing into
one is what it has done since [`70`](70-last-use-moves-report.md).

## 79.4 The `test` block half

[`78`](78-a-record-is-a-permutation-report.md) §78.6's finding, fixed: `liveness::mark_program`,
`frames::reserve_program` and `fields::order_program` all iterated `Program::defs`, and a `test`
block's clauses are in `Program::tests`. Every expression inside a `test` block therefore ran on
the paths those three passes exist to replace — no moves, no reserved slots, no record layouts.

`TestDef::cores_mut` is the walk they now share. One more thing was needed and is easy to miss: the
runner wraps each clause in a lambda **at the moment it evaluates it**, so that lambda does not
exist when the pass runs and has to ask for its own reservation —
`locals: beck_core::frames::locals_of(code)`.

`liveness`'s own docstring already claimed to "mark every definition and test in a checked
program". It had never marked a test. The comment was written for the intent and the code for the
`defs`, and nothing compared them.

**On its own this is worth nothing measurable**, and §79.5 says so with numbers: a `test` block
admits only `given`, `when`, `stub` and `expect`, so its expressions are calls into definitions
that were already annotated. It matters because "which optimisations apply" should not depend on
which side of a keyword the code was written, and because the *lambda* inside an expectation is
real code — which is how this change found §79.1.

## 79.5 What it buys

The fold, by the §79.1 measurement:

| elements | before | after | ×/doubling before | after |
|---|---|---|---|---|
| 2,000 | 23 ms | **5 ms** | | |
| 4,000 | 77 ms | **6 ms** | ×3.37 | **×1.24** |
| 8,000 | 289 ms | **9 ms** | ×3.76 | **×1.38** |

**32× at 8,000 elements, and unbounded beyond** — it is an asymptote rather than a constant.

And the same shape without a clock in it, which is the gate. `--fuel` charges a primitive for the
work it does over a length the caller chose ([`72`](72-space-and-constants-report.md)), so a copy
per append is visible to the budget:

| | 1,000 elements | 8,000 elements |
|---|---|---|
| before | 640 steps/element | **5,120 steps/element** |
| after | **18** | **18** |

Eight times the elements costing eight times as much *each* is the quadratic, stated in integers
that are the same on every machine.
`accumulating_inside_a_fold_costs_the_same_per_element_however_long_it_gets` in
`beck-cli/tests/scaling.rs` is the gate, bounded at 25 steps an element.

**Nothing in the tree got faster**, and that is worth its own sentence. Release, 20+ runs in
rotation with a control, per [`78`](78-a-record-is-a-permutation-report.md) §78.6: `richards`,
`havlak`, `json`, `deltablue`, `pidigits`, `knucleotide`, `decimal`, `ch1` and `bounce` are all
inside their own noise, and `beck check` is too. The reason is in the source: the tree contains
eight `list_fold`s and **not one of them folds into a list**. They accumulate into `Map`s, which
are persistent and share structure, so they were never quadratic. Every loop in `lib/`, `awfy/`,
`clbg/`, the corpus and both SICP chapters is written the recursive way —
[`70`](70-last-use-moves-report.md)'s way. The defect was real and this repository had, by habit,
walked around it.

## 79.6 How it is tested

Three levels, and the middle one is the interesting one.

**The flag**, in `liveness`'s unit tests: a lambda's own parameter is moved on its last read; a
lambda moves its parameter and *lends* its capture in the same body; a parameter a deeper lambda
reads goes back to being lent. Removing `own` from the condition turns three of them red.

**The program**, in a new suite `beck-cli/tests/moves.rs`, six tests, none about speed — a fold's
answer asserted on its contents rather than its length, a list passed to a lambda twice, a closure
built inside a lambda and called after the fold has finished, a parameter read twice, a lambda's
free variable outliving two calls, and a lambda written inside a `test` block.

**And what those six cannot tell you.** Removing `own` does *not* turn any of them red, because the
evaluator refuses the move independently (§79.3). That is a good property of the runtime and a bad
property of a test, so the guard is asserted where it is observable — on the flag — and this
paragraph is here so the next person does not read six green tests as evidence for something they
do not check.

Plus the other 49 suites: SICP's two chapters build closures for a living, the fourteen Are We Fast
Yet benchmarks and eight Benchmarks Game ports check their own published constants, and
`beck explain incremental` and `beck explain place` are **byte-for-byte identical** over all 66
corpus, example, SICP, library, `awfy/` and `clbg/` programs — 132 outputs.

## 79.7 What this corrects

- **[`70`](70-last-use-moves-report.md) §70.6's audit missed this.** It listed what was still
  quadratic — strings, twice — and what was measured and found sound. The fold form of its *own*
  idiom was neither, because the audit asked which data structures were slow rather than which
  ways of writing the same loop were.
- **[`19`](19-phase-1-report.md) §19.4's defect has now been found three times**, in the fold, in
  `list_append` and in the fold again. Each time it was one shape — an accumulator copied instead
  of handed over — reached by a different route.
- **[`78`](78-a-record-is-a-permutation-report.md) §78.7's list is one item shorter**, and the item
  it named as "not built" was worth nothing on its own, exactly as forecast. What it was worth was
  finding this.

## 79.8 What is not built

| | |
|---|---|
| A `Map` or a record handed over the same way | **not built.** `worth_moving` covers `List`, `Str` and `Data`; a `Map` is persistent so an insert shares rather than copies, and nothing has measured whether a move would beat that |
| A lambda's own bindings sharing the caller's frame | **not built.** A closure call still allocates its own frame, and [`74`](74-the-cost-of-a-call-report.md) §74.6 is why two allocations a call is the floor |
| An interned field name, a `with` placed by index | **not built**, per [`78`](78-a-record-is-a-permutation-report.md) §78.7 |
| A variable read that is an index | **still recommended against**, per [`78`](78-a-record-is-a-permutation-report.md) §78.7 |

## 79.9 What this establishes

**That an idiom is a unit of measurement.** Every performance report in this project since
[`69`](69-standard-library-imports-report.md) has measured a *program* or an *operation*, and this
one was found by measuring the same loop written two ways and noticing that only one of them had
been fixed. The suite was green, the gate in `scaling.rs` was green, `--fuel` was linear, every
benchmark in the tree was unaffected — because all of them were written the way that worked.

[`78`](78-a-record-is-a-permutation-report.md) §78.8 said the remaining named items were all under
one per cent and the next order of magnitude was not in this evaluator. That is still true of the
*named* items. This one was not on the list.
