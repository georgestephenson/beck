# 105 — Text on the native heap

**Built.** A `Str` is a value both code generators compile: a layout, a literal pool, `+`, the six
comparisons, `str_len`, `str_is_empty`, `str_slice`, `str_contains`, `str_starts_with` and
`str_ends_with`. [`101`](101-the-heap-report.md) §101.5's table of what the heap does not reach had
six rows, and text was the first of them; this removes that row and leaves five.

The number to read first is not the speedup. It is that the **corpus went from 4 compiled
definitions to 28**, across 17 of its 35 files rather than 3 — because a corpus program is a fold
and a view, and what a fold's commands carry is text.

The second number to read is that **one of the three benchmarks in §105.7 is slower than the
tree-walker, on purpose, and gets slower as it grows**. §105.6 is why, and it is not a defect in
this backend so much as an ability the tree-walker has that an arena cannot be given cheaply.

---

## 105.1 The shape

Two header words and the bytes, padded so the next object still starts on a word:

| Word | What |
|---|---|
| 0 | how many **bytes** the text is |
| 1 | how many **characters** it is |
| 2.. | the bytes, then zero padding to a whole word |

Both counts are stored, and the second one is the whole reason this is a design decision rather
than a memcpy. `beck_core::core::Text` caches a character count precisely because `str_len` used
to be `chars().count()`, which made `while i < str_len(s)` walk the string once per iteration —
[`70`](70-the-evaluator-gets-fast-report.md) §70.2's quadratic. A compiled `str_len` that counted
would put that quadratic back in one implementation and not the other, and
`AGENTS.md`'s rule is that a performance defect the semantics force survives into every backend. So
the count is in the header and `str_len` is a load.

Storing both counts also buys the ASCII test for nothing, which is worth stating because it looks
like a coincidence and is not. A UTF-8 string has as many bytes as characters exactly when every
character is one byte. So `bytes == chars` **is** `is_ascii`, and where the evaluator carries a flag
this carries the same fact as an equality between two words it was already storing. A character
index is then a byte index, and `str_slice` on the text every program in this tree actually holds is
a range rather than a walk.

There is no NUL terminator and nothing looks for one. That is asserted rather than assumed: the
differential's alphabet contains `"a\0b"`, and an implementation that reached for `strlen` anywhere
would answer a one-character string and nothing else in the suite would notice.

## 105.2 A literal is the host's, at a fixed offset

A string literal cannot be allocated where it is written. The arena is reset before every call
([`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md)), so the first iteration of a loop
would allocate `"x"` and the second would allocate it again — and a loop that allocates a constant
is a loop whose memory grows for no reason. It cannot be a global either, because a value here is an
**offset into the arena** and a constant living somewhere else could not be one.

So the literals of a module are a **pool**, and the host writes it as the first bytes of every
request's heap, at offsets decided when the module was emitted. Compiled code refers to a literal by
a constant. Neither emitter generates a byte to build one.

The pool is interned by `beck_llvm::heap::survey`, before either emitter writes anything, which is the same
rule a layout's index follows and for the same reason: it makes the pool a function of the *program*
rather than of which definitions turned out to compile.
`the_literal_pool_is_a_function_of_the_program` is the gate, and its discriminating assertion is
that the **survey alone** already holds every literal — including a pattern's. `case "one":` is not
an expression, so the walk that collects these reaches it only because it was told to; deleting that
one line leaves a pool that emission fills in as it goes, which happens to work and quietly makes
the pool a function of the fixed point. The gate was made red by hand to check it says so.

What the pool costs is §105.6's third item.

## 105.3 What compiles now that did not

`beck native <file>` over the corpus, the examples, all three SICP chapters, Are We Fast Yet, the
Benchmarks Game, the standard library and `xlang/`, against the same command with `Ty::STR` refused:

| | before | after |
|---|---:|---:|
| programs with at least one compiled definition | 30 | **50** |
| definitions compiled | 283 | **344** |

The 283 is [`101`](101-the-heap-report.md) §101.2's own number, reproduced by the same method, which
is the only reason to trust the delta.

Where it landed:

| | before | after | why |
|---|---:|---:|---|
| [`corpus/`](../compiler/corpus/) — 35 files | 4 | **28** | a command carries a `Str`, and 17 of them now have at least one |
| [`clbg/`](../compiler/clbg/README.md) | 48 | **63** | `fasta`, `knucleotide` and `revcomp` are string benchmarks |
| [`lib/`](../compiler/lib/README.md) | 43 | **52** | `money`, `format` and `text` had nothing before |
| [`examples/`](../compiler/examples/) | 2 | **11** | including `todo.beck`'s first compiled definition |
| [`awfy/json.beck`](../compiler/awfy/json.beck) | 0 | **3** | a parser's cursor is a `Str` and an index |

The corpus row is the one worth a sentence. [`101`](101-the-heap-report.md) §101.2 said "three
programs contribute four definitions between them … the number that matters is not this one",
because a fold's state is a `Map` and a view builds `Html`. Both of those are still true and both
are still refused — what changed is that a corpus program also has small definitions *about* its
commands, and those are text. `corpus/01-counter.beck` compiles one and refuses two, and the
refusal now reads

```
validate  parameter `p` is a `Proposal`, whose field `session` is a `Session`,
          whose field `claims` is a `Map`, and a collection is not on this heap yet
```

which is a better sentence than the one it replaced, because a `Map` is a thing somebody could go
and build.

## 105.4 What it answers, and what it refuses

| Compiled | How |
|---|---|
| a literal | an offset into §105.2's pool, written into the instruction |
| `a + b` | `beck.str.concat` — one allocation, two `memcpy`s |
| `==` `!=` `<` `<=` `>` `>=` | `beck.str.cmp`, which is `memcmp` over the shorter length and then the lengths |
| `str_len`, `str_is_empty` | a load of a header word |
| `str_slice` | `beck.str.slice`, in **characters**, clamped exactly where the evaluator clamps |
| `str_contains` | `beck.str.find`, a naive scan |
| `str_starts_with`, `str_ends_with` | one `memcmp` at a clamped offset, with no branch |
| a `Str` field, a `Str` in a variant, `with` over one | the layout was already there; text is one more word |
| a `case "one":` pattern | the same three-way comparison |

Ordering is bytes and then length, because `Text`'s `Ord` is `String`'s and UTF-8 sorts bytes and
code points the same way. The length has to decide *after* the bytes rather than before them, which
is the one place a plausible implementation is wrong in exactly one direction: `memcmp("ab", "abc",
2)` answers `0`.

The searches are byte searches, and that is correct rather than approximate: UTF-8 is
self-synchronising, so a well-formed needle cannot match starting inside a character.

Refused, each by name and with the reason rather than swept into "not a scalar primitive":

| | why |
|---|---|
| `str_split`, `str_chars` | answers with a list |
| `str_join` | reads one |
| `str_index_of` | answers with an `Option`, whose layout this backend resolves from a program's own types and not from the prelude's |
| `str_upper`, `str_lower` | Unicode case mapping is a table; folding ASCII only would disagree with the evaluator on the first letter that is not |
| `str_trim` | Unicode whitespace, for the same reason |
| `str_replace`, `str_repeat` | build text whose size is not a function of its arguments' sizes |
| `str`, `str_to_int` | the rendering has to be Rust's to the digit |

## 105.5 One layout, two runtimes — and what writing it twice caught

[`97`](97-cranelift-report.md) §97.3's rule holds: the *layout* is shared because it is a contract
with the host as well and three spellings of one contract drift, and the *emitters* are written
twice because an agreement by construction is worth nothing. Text is six small functions, and there
are two of each — `TEXT` in [`beck_llvm::emit`](../compiler/crates/beck-llvm/src/emit.rs) as LLVM IR
text, and `Text` in [`beck_clif::emit`](../compiler/crates/beck-clif/src/emit.rs) built through
Cranelift's `FunctionBuilder`.

Writing it twice found a bug within a minute of the second one existing, and it is [`97`](97-cranelift-report.md)
§97.4 happening again: the Cranelift **record comparison** compared a `Str` field by its *offset*.
Two equal strings allocated at different places compared unequal, so `Named(label="") <
Named(label="")` answered `true`. The LLVM emitter had the corresponding line and Cranelift's `_`
arm swallowed it, because a `Repr` that is carried in an `i64` looks exactly like an `Int` to a match
that is not asked to distinguish them. What caught it was the three-way differential's *pairs* —
`the_three_backends_agree_on_text` compares every ordered pair of a twelve-string alphabet — and
nothing in the single-string sweep would have.

## 105.6 What it costs, said plainly

**Building a string in a loop is `O(n²)` here and `O(n)` in the evaluator.** This is the largest
cost and it is the one [`101`](101-the-heap-report.md) §101.5 forecast for `list_append`, arrived at
one row up. [`70`](70-the-evaluator-gets-fast-report.md) gave the tree-walker an in-place `push_str`
when the liveness analysis proves nobody else holds the accumulator; an arena with no ownership in it
cannot prove that, so `acc + s` allocates the whole accumulator every step. §105.7's `grown` row is
that, measured: **1.06× at 1,000 steps and 0.17× at 4,000** — four times the work and twenty times
the time, which is what a quadratic looks like at two sizes. The measurement asserts that row is
*below* one, so a run in which the compiled accumulator caught up would go red: it would mean the
evaluator had lost its in-place append, which is a finding rather than good news.

**A slice of non-ASCII text is a walk.** `beck.str.byteof` is constant time when the two counts
agree and otherwise steps character by character, where `Text::byte_offset` has a chunked index and
answers in at most a stride. Every program in this tree slices ASCII, so this is a difference nothing
here pays; the fix, if a program ever needs it, is the same index in the same header, and the
trigger is a benchmark that indexes into non-ASCII text.

**The pool is on the wire on every call.** It is written in front of every request's heap, so a
program's literals are copied down the pipe whether or not the call touches one:
`examples/todo.beck` has 22 literals and pays 560 bytes, `corpus/28-catalogue.beck` 14 and 352,
`lib/dates.beck` 20 and 480. Against a 35.6 µs round trip this is not currently measurable, and it
is written down because it is a cost that grows with a program rather than with a call — the version
that does not pay it copies the pool into the arena once at startup and shifts every argument offset,
which is a protocol change rather than a tidy-up.

**A slice always allocates.** `str_slice(s, 0, str_len(s))` builds a copy where the evaluator's
`Value::str_` also builds one, so this is a match rather than a cost; but there is no substring that
shares its parent's bytes, and an arena of offsets could have one. It does not, because a shared
substring is a second kind of `Str` and the host would have to know about both.

**Memory is not reclaimed inside a call**, which is [`101`](101-the-heap-report.md) §101.6 and is
what makes the first item bite as hard as it does.

## 105.7 The numbers

`cargo test --release --test measure_native -- --nocapture --test-threads=1`, Ubuntu clang 18.1.3,
`-O2`, median of seven runs at the small size and three at the large one. Two sizes each per
`AGENTS.md`, and every ratio includes the pipe round trip.

| benchmark | size | evaluator | native | ratio |
|---|---:|---:|---:|---:|
| `walk` — index every character, compare each | 2,000 | 1.138 ms | 55.5 µs | **20.5×** |
| | 16,000 | 9.221 ms | 326 µs | **28.3×** |
| `hunt` — search a 2,000-byte string, repeatedly | 2,000 | 970 µs | 60.1 µs | **16.1×** |
| | 16,000 | 8.419 ms | 351 µs | **24.0×** |
| `grown` — append to an accumulator | 1,000 | 398 µs | 377 µs | 1.06× |
| | 4,000 | 1.245 ms | 7.506 ms | **0.17×** |

The first two ratios *grow* with size, which is the round trip being amortised away and is reported
rather than smoothed. The third shrinks, and §105.6 is why.

None of the three is asserted as a rate — [`13`](13-testing.md) §13.7 — and the shape claim is gated
where it needs no clock at all: `a_slice_costs_its_answer_and_not_the_string_it_came_from` counts the
**bytes** the arena holds after a loop that takes one character per step, at 200 characters and at
1,600, and finds 24 a character at both. A `str_slice` that copied what it was taken *from* would be
quadratic there, and the differential could not see it because copying too much still answers
correctly. That gate is written twice too, once per backend, because the allocator is.

## 105.8 What it found

**`str_slice` was charged the length the caller wrote rather than the length it takes.** The
evaluator's fuel accounting has a rule stated in its own comment one arm above — "a slice costs what
it *takes*, not what it is taken from" — and `Prim::StrSlice` did not follow it. So
`str_slice(s, 0, 1_000_000)` on a five-character string cost a million steps, and "from here to the
end", which is ordinarily written with a length nobody bounded, could exhaust a 50,000,000-step
budget on a program doing nothing. It was found by the differential answering with the string while
the evaluator answered "ran out of fuel", which is a pleasing way round: the compiled backend was
right and the accounting was wrong. `a_slice_is_charged_what_it_takes` is the gate, and it asserts
the control too — a slice of a 10,000-character string still costs 10,000.

**The Cranelift record comparison compared text by its offset**, which is §105.5.

Neither is in the feature being built, which is the pattern
[`27`](27-the-walls-come-down-report.md) records for five defects and
[`91`](91-guards-and-alternatives-report.md) §91.3 for one more: the interesting thing a piece of
work finds is usually next to it rather than in it.

## 105.9 What is not built, and the tests that say so

[`101`](101-the-heap-report.md) §101.5's table, less one row:

| Not built | Why it is not a layout problem |
|---|---|
| `list[T]` | the layout is a count and the elements; what is hard is `list_append`'s in-place push, which an arena cannot prove — and §105.6 is the same wall met from the text side |
| `Map[K, V]` | a `PMap` is a weight-balanced tree with structural sharing; the same ownership question one level up |
| a closure | a code pointer and a captured environment, and therefore an indirect call |
| `Html`, `Attr`, `Unit` | `Html` follows collections rather than text: a page is a tree of children |
| every effect | needs the host, which is on the other side of a pipe that carries values and not calls |

`what_the_heap_does_not_reach_is_refused_by_name` still asserts each of them by the reason its
refusal gives, and now asserts the removal from the other side as well: `names_it` — a `Str`
argument, a `Str` field and a record answer — is in the same fixture on the *compiled* side, so the
row's deletion is a thing the test says rather than a thing a reader infers from a row that is
missing.

## 105.10 What this corrects, elsewhere

- [`101`](101-the-heap-report.md) §101.5's first row is gone, and §101.2's "the corpus still compiles
  four definitions" is now 28. Both stand as history; this is where the correction lives.
- [`93`](93-llvm-backend-report.md) §93.6 and [`97`](97-cranelift-report.md) §97.7 each list text
  first among what bounds the native backends. It no longer does.
- [`08`](08-roadmap.md) §8.5.5's Lane E row reads "what is left is text, collections, closures and
  the effects". Three of those four.
- **Mode B codegen is not closer.** [`94`](94-mode-b-report.md) §94.8 waits on a heap that reaches
  `Html`, and [`101`](101-the-heap-report.md) §101.10 said a page is `Html` and `Str` all the way
  down. Half of that sentence is now false and the half that blocks Mode B is the other one:
  `Html` is a tree of children, so it follows the **collection** row and not this one. Nothing about
  [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) changes.

## 105.11 What this establishes

That a `Str` is an ordinary value in both code generators, agreeing with the tree-walker on 2,893
calls per backend over an alphabet chosen so that each of six specific mistakes would show — the
empty string, an embedded NUL, two-, three- and four-byte characters, a prefix pair, an odd padding
length, and a string that is also one of the program's own literals.

It establishes nothing about a page, a fold or a view, because those need a collection. And the row
it is most honest about is the one where it loses: an accumulator built in a loop is the shape every
Beck program is written in, and this backend does not have the analysis that makes it linear.
