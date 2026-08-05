# 70 — Phase 3, part 39: last-use moves, and an audit of what else was quadratic

**Built.** The compiler computes which read of a local is its **last**
([`beck_core::liveness`](../compiler/crates/beck-core/src/liveness.rs)), the evaluator hands that
value over instead of lending it ([`Env::read`](../compiler/crates/beck-core/src/core.rs)), and
`list_append` pushes into a list nobody else holds. The accumulator idiom every loop in Beck is
written as goes from **`O(n²)` to `O(n)`**.

This is [`69`](69-standard-library-imports-report.md) §69.7, which measured the defect and did not
fix it:

> `list_append` copies the whole list. […] Every one of them is therefore **O(n²) in time**. […]
> The real fix is one of two real changes, and both are somebody's next piece of work rather than
> this one's.

§69.7 named last-use moves as the one to take and said why: it costs the reads nothing, and
uniqueness information is what lets a *compiled* backend turn a functional update into an in-place
write, which is Koka's Perceus and Roc's opportunistic mutation rather than Clojure's vector. This
is that, at interpreter scale.

## 70.1 The analysis, and the three rules that make it sound

[`liveness.rs`](../compiler/crates/beck-core/src/liveness.rs) walks a definition's body **backwards
over evaluation order**, carrying the set of variables read after the current node. A `Var` whose
variable is not in that set is the last read of it, and is marked. `false` is always safe, and
anything the pass does not understand stays `false`.

Three rules carry the correctness, and two of them were wrong first:

1. **Branches are alternatives.** A read in the `then` arm is a last read if the variable is not
   read *after the whole `if`*, whatever `alt` does, because only one arm runs.
2. **A closure's captures are never moved.** A `lam` may be called any number of times later, so
   every variable any lambda mentions is excluded outright — and that has to be a **pre-pass**
   rather than something the backward walk discovers, because a closure is created *before* the
   reads that follow it, and walking backwards meets those reads first. Getting this wrong marks a
   read that happens after a closure captured the same binding.
3. **Evaluation order is what the evaluator does, not what reads naturally.** Verified arm by arm
   against `Interp`: arguments left to right, a record's fields in order, `with`'s base before its
   fields, a map literal's key before its value — and **a call's arguments before its callee**,
   which is the one that caught me (§70.2).

The flag promises one thing: *on every path that evaluates this node, no later evaluation in this
body reads that binding.* It says nothing about the heap, so the evaluator checks ownership
separately — [`Env::read`](../compiler/crates/beck-core/src/core.rs) empties a frame only when
`Arc::get_mut` proves nothing else holds it. A frame captured by a closure is read from, never
emptied. **A wrong analysis therefore cannot corrupt another frame**; the binding goes missing and
the next read of it is an unbound-variable error, which is how both of §70.2's bugs announced
themselves.

## 70.2 The two bugs, and which one the harness found

**A definition's body is itself a `lam`.** [`Def::body`](../compiler/crates/beck-core/src/check/mod.rs)
is "the whole definition as a lambda, so evaluating the name yields a callable value" — so rule 2's
pre-pass saw the outermost lambda, excluded every variable in the function, and marked precisely
nothing. The pass ran, reported no marks, and was a no-op; the fix is to look through that first
lambda, whose frame is built fresh per call and dies with it. **Found by measuring rather than by
testing**: the accumulator was still quadratic after the change, which is a thing a test asserting
"the answers are right" would never have said.

**A call's arguments are evaluated before its callee.** `Interp::step` evaluates every operand and
*then* the function, so a stub can answer "with what?" (§21.3 rule 4). Walking backwards therefore
has to meet the callee first. Written the intuitive way round, the last read of `f` in
`f(x, g(f))` looks like the inner one, so the value moves there and the call has nothing left to
call. That is exactly `accumulate` — SICP exercise 1.32, `combiner(term(a), accumulate(combiner,
…))` — and **`sicp/ch1.beck` failed within seconds of the first full run**, against the book's own
answers. [`25`](25-benchmarks-and-expressiveness.md) §25.5 adopted SICP as an *expressiveness*
benchmark; here it caught a miscompile, which is the argument for a corpus with an outside oracle in
one line.

Both are now unit tests in `liveness.rs`, in the shape that found them.

## 70.3 What it buys

The accumulator loop, release build, median of five, startup subtracted:

| n | before | after |
|---|---|---|
| 1,000 | 2.1 ms | 1.6 ms |
| 4,000 | 33 ms | 6.7 ms |
| 16,000 | 530 ms (extrapolated) | 27 ms |
| 64,000 | ~8.5 s (extrapolated) | **99 ms** |
| per doubling | ×2.5 → ×3.7, heading for ×4 | **×2.0** |

Measured either side at n = 8,000: **385 ms → 15 ms, 25× faster**, and the gap grows with n because
the shape changed rather than the constant.

## 70.4 What it costs, which is the part that changed the design

The first version moved **every** last read, and the benchmark suite came back **6–13% slower**:
`pidigits` +2.7%, `decimal` +5.6%, `bignum` +11%, `havlak` +4.5%. That is the honest shape of the
trade: moving is strictly more work than cloning *at the read* — find the slot, prove the frame is
unshared, empty it — and it pays only when somebody downstream can use the sole ownership. The reads
that dominate a real program are of `Int`s, and moving one gains nothing.

So the move is now conditional on the value being worth moving — a `List`, which `list_append` can
push into, or a record, whose fields `with` can rebuild — and the slot is **tombstoned** rather than
removed, because `Vec::remove` shifts every binding above it on the hottest path in the interpreter.
Two further percent came back from `#[inline]` on the read itself, which is a deliberate exception
to [`Interp::leaf`](../compiler/crates/beck-eval/src/interp.rs)'s rule about keeping arms out of
line: that rule is about arms holding *values*, and this one holds a `bool` and a reference.

The result, medians of five, release:

| | before | after | |
|---|---|---|---|
| `lib/decimal.beck` | 1.109 s | 1.019 s | **−8.1%** |
| `clbg/fasta.beck` | 1.254 s | 1.218 s | −2.9% |
| `awfy/havlak.beck` | 9.736 s | 9.779 s | +0.4% |
| `lib/bignum.beck` | 0.229 s | 0.234 s | +2.2% |
| `clbg/pidigits.beck` | 4.164 s | 4.282 s | +2.8% |
| `clbg/knucleotide.beck` | 2.018 s | 2.087 s | +3.4% |

**Roughly neutral on today's programs, and that is the claim** — not a speed-up. The lists in this
tree are ten to twenty-five elements long, so the quadratic never bit here; what the change removes
is the ceiling on the first program that builds a long one. §70.7's gate is what keeps it removed.

## 70.5 The evaluator's ceiling was a fiction in a debug build

Found on the way, because the SICP suite's 1,000-deep spine started overflowing the stack, and worth
more than the change that surfaced it.

[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) promises that what stops a
deep program is a **counted ceiling** (`DEFAULT_MAX_DEPTH`, 4,000) with a diagnostic, rather than
whatever stack the process happened to have. `STACK_BYTES` was sized from
`the_depth_ceiling_fits_the_smallest_stack_we_run_on`, which measures the *cheapest* body a function
can have — one `if`, one add, one call — at about 7 KiB a level, making 64 MiB look like three times
what the ceiling needs.

An ordinary body costs far more. Measured on a `match` over a union whose arm calls through
`map_list`:

| | deepest recursion that runs |
|---|---|
| debug, 64 MiB | **1,000** — then the process aborts on a stack overflow |
| release, 64 MiB | 1,997 — the *counted* ceiling, reached |
| debug, 256 MiB | 1,997 — the counted ceiling, reached |

So the ceiling was unreachable in a debug build at a quarter of its nominal value, the failure was a
SIGSEGV rather than the diagnostic that ceiling exists to give, and **which programs ran depended on
how the compiler was built** — [`64`](64-compile-speed-report.md) §64.4's defect on the evaluator's
axis rather than the front end's. `STACK_BYTES` is 256 MiB now, both profiles reach the count, and a
3,000-deep recursion gets the ceiling's sentence instead of aborting. It is address space, and
`beck-cli` spawns one such thread per process.

My change did not cause this — the same program stopped at 1,002 levels before it — but it took the
margin from 0.2% to zero, which is how a knife-edge gate announces itself.

## 70.6 The audit: what else is quadratic

[`69`](69-standard-library-imports-report.md) §69.6's lesson applied systematically — every
primitive and structure asked what it *should* cost, and measured at two sizes where the answer was
not obvious.

**Two findings, both about strings, and neither is fixed.**

`Value::Str` is an `Arc<str>`: no spare capacity, and `+` is `format!("{x}{y}")`, so it allocates
and copies both sides. `str_len` is `chars().count()` and `str_slice` skips characters, so both are
`O(n)` in the string rather than in the answer.

| | measured | |
|---|---|---|
| building a string by `+` in a loop | ×2.1 → ×2.6 → **×3.5** per doubling (n = 8,000 → 64,000) | `O(n²)`, invisible below about 16,000 characters because a `memcpy` is fast next to an evaluator step |
| scanning a string by character index | ×2.2 → ×2.4 → **×2.7** per doubling (n = 4,000 → 32,000) | `O(n²)`, and it bites immediately: `str_len` in the loop guard and `str_slice` from the start each walk the string |

Both are the same shape as the list defect and **neither is fixed by last-use moves**: an
`Arc<str>` cannot be pushed into however uniquely it is owned. The fix is a representation — an
`Arc<String>` with capacity, which makes the append case a push under exactly the ownership test
this change added, plus a cached character count and an ASCII flag so `str_len` is `O(1)` and
`str_slice` can index bytes. That is a change of its own, and it is the largest performance item
this project now knows about. `lib/text.beck`, `clbg/fasta.beck` and `clbg/revcomp.beck` are the
callers that would notice.

**And four things that were checked and are sound**, which is worth writing down because an audit
that only reports problems cannot be trusted about the rest:

| | |
|---|---|
| `PMap` — every `Map` in the language | A **weight-balanced** tree with a real `balance`: `O(log n)` insert and lookup, `O(1)` `map_len`, and `pmap.rs`'s header argues the choice against red-black and AVL. Building a map in ascending key order — the `Map[Int, T]`-as-array idiom `awfy/` and `clbg/` use — does not degrade |
| `list_get`, `list_len` | `O(1)` into a contiguous `Vec` |
| A frame chain — sequential `let`s | Measured at 200, 400 and 800 bindings: ×1.78, ×1.99. **Linear**, so the environment's parent chain is not the quadratic it looked like it could be |
| `sort_by`, `digest`, `concat_lists`, `str_join` | Linear or `n log n`, and each is inherent to the answer rather than to how it is computed |

The residual `n^1.35` in `check` along a long call chain stands where
[`64`](64-compile-speed-report.md) §64.3 left it: it is the front end's, not the evaluator's, and it
is recorded rather than fixed.

## 70.7 How it is tested

| | |
|---|---|
| `liveness.rs` | Eight unit tests: the only read, an earlier read, both branches, a read outliving a branch, a lambda's captures, a `let`'s binder, idempotence — and **a callee read after its arguments**, which is §70.2's second bug in the shape that found it |
| `scaling.rs` | `building_a_list_by_accumulation_costs_the_same_per_element_however_long_it_gets` — a **shape** gate beside the fold's, for the same reason [`13`](13-testing.md) §13.7 gives: eight times the elements must not cost three times as much per element. Checked against the old evaluator, where it fails at **6.5×** |
| Everything else | 48 suites, including the differential harness, the corpus, SICP's two chapters against the book's answers, all fourteen Are We Fast Yet benchmarks, eight Benchmarks Game ports against the Game's published files, and the standard library's own property tests. A miscompile in this analysis shows up as a wrong answer somewhere in that lot, which is exactly how §70.2's second bug was caught |

The gate cannot be a fuel assertion, and that is a fact worth keeping: over this loop the step count
is exactly linear either way, because a primitive that copies ten thousand values is **one step**.
Only wall clock sees it.

## 70.8 What this corrects

- **[`69`](69-standard-library-imports-report.md) §69.7's "not fixed" is fixed**, by the option it
  recommended and for the reason it gave.
- **[`69`](69-standard-library-imports-report.md) §69.7's cheap-fix note is completed.**
  `Arc::try_unwrap` on the append was "tried and measured at no change" because the caller's frame
  still held the list; it is the other half of this change rather than dead weight, and it now
  fires on every accumulator.
- **`STACK_BYTES`' derivation was wrong**, per §70.5, and its doc comment now says what it was
  measured against and what an ordinary body costs.
- **[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)'s property holds in both
  build profiles**, which it did not before.

## 70.9 What is not built

| | |
|---|---|
| A string representation that can be appended to | **not built**, per §70.6, and it is the next thing on this axis |
| Moving anything but a `List` or a record | **not built, deliberately.** §70.4 is the measurement: moving an `Int` costs more than cloning one, and nothing downstream can use the ownership |
| Reuse rather than rebuild | **not built.** A moved record's fields are rebuilt in place by `with`; a moved *list* is pushed into by `list_append` and nothing else. `list_slice`, `map_insert` and the rest still copy, and each is a separate small change under the same ownership test |
| The analysis in a compiled backend | **not built**, because there is no compiled backend. The flag is on `Core` and computed in `beck-core` rather than in the evaluator precisely so that Cranelift inherits it — which is where it is worth much more, since an in-place write there needs no `Arc` at all |
| A cost model anywhere | **not built.** `--fuel` counts nodes, and §70.7 is the proof that a node count cannot see a copy. A budget that bounds *work* is owed |

## 70.10 What this establishes

**That Beck can be pure and still write in place**, on the idiom the language actually uses, with
the ownership test done at runtime and the liveness proved at compile time. That is the mechanism
the premise needs: a language with no mutable sequence is only as fast as its compiler's willingness
to notice when a copy is unobservable.

**It does not establish that Beck is fast.** The tree-walker is still a placeholder, the numbers in
§70.4 are neutral, and §70.6 has the next quadratic already measured and waiting.
