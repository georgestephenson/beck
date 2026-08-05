# 72 — Phase 3, part 41: a value is sixteen bytes, and the budget counts work

**Built.** Four things, one measurement each:

| | |
|---|---|
| **`Value` is 16 bytes, not 48** | The record variant went behind a pointer. Peak memory **−23%** on the graph benchmark, and it is faster too |
| **`--fuel` charges for work** | A primitive that touches `n` elements costs `n`, so the budget bounds what a program can make the evaluator do rather than how many nodes it visits |
| **Non-ASCII text has an index** | `str_slice` into a string with an accent in it is `O(len)` rather than `O(start)` — [`71`](71-strings-report.md) §71.5's first row |
| **A scaling gate with no clock in it** | The accumulator's shape, asserted through the budget: same answer on every machine |

The first is what [`70`](70-last-use-moves-report.md) and [`71`](71-strings-report.md) were not
about. Those two removed *asymptotic* defects; this is the constant beside them, and the constant
had a factor of three in it.

## 72.1 A `Value` was 48 bytes because of a variant most values are not

```rust
Data { ty: Arc<str>, variant: Option<Arc<str>>, fields: Arc<BTreeMap<…>> }   // 40 bytes, inline
```

A Rust enum is as wide as its widest variant, so **every** `Value` was 48 bytes: an `Int`, a `Bool`,
the elements of a million-element list, and every binding in every call frame. Records are the
widest thing the language has and the rarest thing in a hot loop, which is exactly the shape that
should be behind an indirection — so `Value::Data(Arc<Record>)` it is, and a `Value` is now a
discriminant and a pointer.

| | before | after |
|---|---|---|
| `Value` | 48 bytes | **16** |
| a frame binding, `(VarId, Value)` | 56 | **24** |
| a 100,000-element list of integers | 4.8 MB | **1.6 MB** |

Measured, release, median of five, peak resident set per process:

| | time | peak memory |
|---|---|---|
| `awfy/havlak.beck` | 10.77 s → **10.16 s** (−5.6%) | 38.0 MB → **29.1 MB** (−23.4%) |
| `awfy/deltablue.beck` | 0.172 s → **0.153 s** (−11.0%) | 11.2 → 11.2 MB |
| `clbg/knucleotide.beck` | 2.23 s → **2.14 s** (−3.7%) | 10.9 → 11.0 MB |
| `lib/decimal.beck` | 1.131 s → **1.106 s** (−2.2%) | 11.8 → 11.9 MB |
| `awfy/json.beck` | 0.387 s → 0.387 s | 15.7 → **14.8 MB** (−5.7%) |
| `clbg/pidigits.beck` | 4.34 s → 4.39 s (+1.1%) | 10.9 → 10.9 MB |

The two that move are the two that hold many values at once: Havlak's 5,213-node control-flow graph
and Json's parsed document. Everything else is flat because its live set is small and 11 MB of the
figure is the binary and the standard library rather than the program. **A third off the memory of
every value the language holds** is the claim; a few percent of time is what that bought here, and
it would buy more on a program bigger than a benchmark.

## 72.2 The budget counts work now

[`70`](70-last-use-moves-report.md) §70.7 recorded the defect while explaining why a gate could not
use it:

> over the same loop the wall clock quadruples per doubling and the step count exactly doubles,
> because a primitive that copies ten thousand values is one step

So `--fuel` bounded *nodes visited*, and a program could do unbounded work inside a bounded number
of them. It now charges the length the caller chose: `sort_by` over a list costs its length,
`concat_lists` costs what it concatenates, a digest costs its bytes, and `list_append` costs nothing
when it pushes and its length when it copies.

**What it does not charge is the point.** `list_get`, `list_len`, `map_len` and `str_len` are
constant-time, and charging them a length would make an ordinary indexed loop over a long list run
out of fuel for doing nothing wrong.

**The first version of this got it wrong in exactly that way**, and the tree caught it: `list_slice`
was charged the length of the list it sliced *from*. `clbg/knucleotide.beck` reads every k-mer of a
10,245-element sequence with `str_join(list_slice(chars, i, k), "")`, so it was charged 10,245 where
the work is `k ≤ 18` — a 500× overcharge that took the benchmark from fitting the default budget to
needing 178,000,000. A slice costs what it takes, not what it is taken from. Corrected:

| | fuel needed, with work counted |
|---|---|
| `clbg/knucleotide.beck` | 3,466,084 |
| `clbg/pidigits.beck` | 18,488,800 |
| `awfy/havlak.beck` | 23,957,911 |
| `awfy/json.beck` | 1,730,221 |
| `clbg/fasta.beck` | 1,641,470 |

Every program in the tree still fits the **unchanged** 50,000,000 default, so nothing was
recalibrated to make this land — which is the evidence that the charge is the size of the work
rather than a tax on it.

## 72.3 Non-ASCII text is indexed

[`71`](71-strings-report.md) fixed `str_slice` for ASCII by making a character index a byte index.
Anything with an accent in it still walked from the start, so scanning it was still quadratic.

A non-ASCII `Text` now carries the byte offset of every 32nd character: `n / 8` bytes, chunked
rather than one entry per character, and a slice jumps to the nearest chunk and walks at most 32.
Built **eagerly**, in the pass that already counts the characters, and that is a decision worth its
sentence: a lazy index would be interior mutability inside a `Value`, and a `Value` is a `Map` key.
It would have been *harmless* — `Text` orders and hashes by its bytes and nothing else — but
"harmless interior mutability in a key" is a sentence every reader and `clippy::mutable_key_type`
has to re-check, and it cost less to spend an eighth of the bytes than to keep explaining it.

Appending maintains the index in `O(other)` rather than rebuilding it, including the transition
where an ASCII string first meets a character that is not: the left half's offsets are its character
numbers, so they are written down rather than walked for.

| scanning by index, non-ASCII | before | after |
|---|---|---|
| per doubling, n = 2,000 → 16,000 | quadratic | **×1.59, ×1.94, ×2.06** |

## 72.4 A gate that needs no clock

Because the budget can now see a copy, the accumulator's shape can be asserted through it:
`the_budget_itself_shows_that_accumulating_is_linear` runs 1,000 and 8,000 elements under a budget
of 20 steps each — measured at 14 either side — and a copy per append needs `n / 2`. It fails the
moment the move is switched off, and it gives the same answer on every machine: no 3× slack for a
shared runner, no [`13`](13-testing.md) §13.7 caveat.

It runs **beside** the two wall-clock gates rather than replacing them, because they see different
things. Fuel counts what the evaluator was asked to do; the clock counts what it cost. An allocation
per step or a cache miss per element is invisible to the first and real in the second — §72.1 is
exactly such a change, and no fuel assertion would have noticed it.

## 72.5 What this corrects

- **[`70`](70-last-use-moves-report.md) §70.7's "it cannot be a fuel assertion"** was true of a
  budget that counted nodes and is no longer true of this one.
- **[`70`](70-last-use-moves-report.md) §70.9's "a cost model anywhere — not built"** is half built:
  the *budget* bounds work now. There is still no cost model the compiler reasons with.
- **[`71`](71-strings-report.md) §71.5's "a character index for non-ASCII text — not built"** is
  built.
- **[`62`](62-fuel-report.md)'s budget means something different**, and the number is the same. It
  bounded nodes and now bounds work, so the same 50,000,000 refuses a smaller class of runaway
  program than it used to admit.

## 72.6 What is not built

| | |
|---|---|
| A cost model the *compiler* reasons with | **not built.** The budget is a runtime backstop; nothing predicts a program's cost before running it |
| Charging for allocation | **not built.** `work_of` charges for elements touched, not bytes allocated, so §72.1's kind of improvement is invisible to it — which is why the wall-clock gates stay |
| A smaller `Core` | **not built.** A `Core` node is 152 bytes, most of it an inline `Ty` of 80. It is compile-time memory rather than run-time, [`64`](64-compile-speed-report.md) measured the front end at 12 µs a line, and nothing is complaining yet — but it is the same shape of finding as §72.1 and the next place to look |
| One allocation per call frame | **not built.** `Env::extend` allocates a `Vec` and an `Arc` for every call. A frame is small and short-lived, which is the profile an arena or an inline-capacity vector fits; both are a dependency or a hand-rolled structure, and neither has been measured against the alternative yet |
| Interning `Arc<str>` field names | **not built.** Every record carries `Arc<str>` keys and every `with` clones them; a symbol table would make them a `u32`. `Value` is 16 bytes either way, so this is a `Record` improvement rather than a `Value` one |

## 72.7 What this establishes

**That the constants were worth the same attention as the asymptotics.** Three quadratics came out
of this branch by asking what an operation should cost; this one came out of asking what a *value*
should cost, and the answer — a discriminant and a pointer — was three times smaller than what was
there. Neither question is clever. Both were available to anybody who measured, and neither had been
asked.
