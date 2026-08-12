# 106 — A map arrives read-only

**Built.** `Map[K, V]` is a value both code generators compile — `map_len`, `map_get`,
`map_contains`, `map_keys`, `map_values`, all six comparisons and the empty literal — with
`map_insert`, `map_remove` and `map_merge` refused by name, on the rule
[`105`](105-lists-arrive-read-only-report.md) §105.5 gave `list_append`.
[`101`](101-the-heap-report.md) §101.5's collection row is gone.

The number that says why this one mattered: of the **1,026 refusals** across the tree before it,
**472 blamed a `Map`** — more than the next four causes together. There is now **no refusal anywhere
in the tree that blames a collection for having no layout**.

**403 → 452 definitions** compile, and the corpus goes **30 → 53 across 31 of its 35 files**, from
19. The row inside that number is the one to read: **nine corpus programs now compile their
`apply_event`** — the step function of a `durable` fold, `(State, Envelope[Event]) -> State` — which
is the first time anything at the centre of what Beck is *for* has compiled to machine code. What
still does not is `view`, because a page is `Html`.

The finding is §106.5, and it is the **fourth** occurrence of one defect: a record's field
comparison compared a reference by its **offset**. [`105`](105-lists-arrive-read-only-report.md)
§105.4 named what would prevent a fourth; this is that fix, built, plus the fourth occurrence it
caught on the way in.

---

## 106.1 The shape

| Word | What |
|---|---|
| 0 | how many entries |
| 1..1+n | every key, **in key order** |
| 1+n..1+2n | every value, in the same order |

Two runs rather than interleaved pairs, and both halves of that decision earn their keep: the keys
being contiguous is what makes the search a binary one with a stride of one, and the values being
contiguous is what makes `map_keys` and `map_values` one `memcpy` each into a fresh list.

Key order is not a convenience — it is the contract. `PMap::iter` is in key order and `PMap`'s `Ord`
is `self.iter().cmp(other.iter())`, so a sorted run *is* the tree, flattened, and a comparison
walking it pair by pair gives the answer the evaluator gives. The host writes the keys by iterating
the `PMap`, which is already sorted; nothing sorts at run time, which is also why §106.3 refuses the
non-empty literal.

A `Repr::Map(u32)` indexes a table of `(key, value)` repr **indices**, and those indices are into the
same table a list's element uses — so `Map[Str, Int]` asks for `Str`'s word comparison, which is the
one a `list[Str]` would have asked for. One table, one comparison per repr.

## 106.2 What compiles now that did not

| | after lists | after maps |
|---|---:|---:|
| programs with at least one compiled definition | 53 | **67** |
| definitions compiled | 403 | **452** |

| | before | after | why |
|---|---:|---:|---|
| [`corpus/`](../compiler/corpus/) — 35 files | 30 | **53** | in **31** files rather than 19 — a fold's state is a `Map` |
| [`awfy/`](../compiler/awfy/README.md) | 130 | **144** | `richards` 23 → 27, `deltablue` 17 → 21, `havlak` 10 → 13 |
| [`lib/`](../compiler/lib/README.md) | 63 | **68** | `lib/http.beck` had nothing before — a request is headers |
| [`sicp/`](../compiler/sicp/) | 87 | **89** | |

The refusal profile is the better measure, because it says what *kind* of thing is left:

| Leading cause | before text | now |
|---|---:|---:|
| a collection with no layout | 603 | **0** |
| inherited — "calls X, which does not compile" | 263 | 451 |
| an effect, a signal, `Html` — "not one of the scalar primitives" | 55 | 105 |
| generic over a type parameter | 69 | 68 |
| **grows** a map or a list | 41 | 118 |
| a closure | 56 | 59 |

Two rows are worth reading together. The collection row is **empty**, and the "grows" row nearly
tripled — which is the same definitions, re-refused for the reason that is actually true of them.
That is the shape of a wall being replaced by a decision.

## 106.3 What it answers, and what it refuses

| Compiled | How |
|---|---|
| `map_len` | a load of the header word |
| `map_get` | a **binary search**, then an `Option[V]` without a branch |
| `map_contains` | the same search, tested against `-1` |
| `map_keys`, `map_values` | one `memcpy` of a run into a fresh list |
| `==` `!=` `<` `<=` `>` `>=` | `beck.map.cmp.N` — key then value at each entry, then length |
| `{}` | one allocation and a zero |
| a `Map` field, a map in a variant, `with` over one | the layout was already there |

`map_get` is [`105`](105-lists-arrive-read-only-report.md)'s address trick with one more step: the
search answers an index, the value lives `n` words past the keys, and the address handed to the load
is a `select` between that cell and the map's own header. A miss loads the count into a word the
`None` tag means nobody reads.

**Only `{}` compiles as a literal.** A map's keys are laid out in key order and a literal's keys are
expressions, so a non-empty one would have to sort at run time — a sort in emitted code, twice, for
a form that is almost always written empty. Every `durable` fold in this tree starts at `{}`, which
is the case that mattered; the rest is refused by name until something needs it.

Refused: `map_insert`, `map_remove` and `map_merge` — see §106.4 — and, still, a closure, an effect,
`Html`, and a type parameter.

## 106.4 Growing a map, refused, and the argument is not quite `list_append`'s

[`105`](105-lists-arrive-read-only-report.md) §105.5 refused `list_append` because the tree-walker
pushes in place when `liveness` proves the accumulator is a last use, and an arena cannot prove
that. The map case is *worse* than that, and the difference is worth stating rather than assuming.

`list_append` is `O(n)` here and `O(1)` amortised in the evaluator, so the gap is one factor of `n`
and it comes from an analysis the front end already has half of. `map_insert` is `O(n)` here and
`O(log n)` in the evaluator, and the gap is **not** an ownership question: a `PMap` is a
weight-balanced tree that shares every subtree it did not touch, and a sorted run in an arena has no
subtrees to share. Even with a perfect last-use analysis, inserting into a flat sorted run is a
copy. So this refusal would survive the fix §105.5 asks for, and the thing that would remove it is a
different representation — a tree in the arena, which is a design and not a patch.

The rule is the same either way and worth restating because it now applies twice: **this backend
does not ship an operation whose asymptote is worse than the evaluator's.** A program that reads
collections gets compiled; one that builds them keeps the tree-walker, and the refusal says which.

## 106.5 The fourth occurrence, and the fix that was promised

A record's field comparison compared a reference by its **offset**, so two equal values compared
unequal whenever they were allocated at different places. That is now the fourth time:

| | where | caught by |
|---|---|---|
| [`104`](104-text-on-the-heap-report.md) §104.5 | Cranelift, a `Str` field | the differential's pairs |
| [`105`](105-lists-arrive-read-only-report.md) §105.4 | LLVM, a `list` field | the differential's pairs |
| here | Cranelift, a `Map` field | the differential's pairs |

[`105`](105-lists-arrive-read-only-report.md) §105.4 said what would stop it: "a `Repr::reference()`
accessor that the comparison is written against instead of a match". That is
[`Repr::order`](../compiler/crates/beck-llvm/src/heap.rs) now. It answers one of three things —
compare the words (signed or not), take `beck_core`'s order key first, or **call this symbol** — and
it is the only place in either backend that names a comparison function. Every consumer matches on
those three cases rather than on `Repr`'s six, so a new reference kind is a compile error *in
`Repr::order`*, where its comparison has to be named, and nowhere else.

It was built before `Repr::Map` was added, and it caught the fourth occurrence anyway — because one
of the five sites had not yet been converted when the map differential ran. The order of events is
the useful part: the accessor did not prevent the defect, it made the defect **one line to fix in
one place** instead of a hunt. What prevents the fifth is that all five sites now go through it.

`Function::wants` is the same idea for the other half: recording that a repr's comparison has to
exist is one method rather than three call sites, and `reachable` is one fixed point over layouts,
word comparisons and maps together — because a record with a `Map[Str, Point]` field needs that
map's comparison, which needs `Str`'s and `Point`'s.

## 106.6 The numbers, and the one this cannot measure

`cargo test --release --test measure_native -- --nocapture --test-threads=1`, Ubuntu clang 18.1.3,
`-O2`, median of seven runs at the small size and three at the large one.

| benchmark | entries | evaluator | native | ratio |
|---|---:|---:|---:|---:|
| `lookup` — 2,000 searches | 250 | 1.452 ms | 44.5 µs | **32.7×** |
| | 2,000 | 1.436 ms | 130.2 µs | **11.0×** |
| `spin` — the control: the same loop, no search | 250 | 885 µs | 47.1 µs | 18.8× |
| | 2,000 | 1.235 ms | 117.9 µs | 10.5× |
| `walk` — read every entry through `map_keys` | 250 | 1.368 ms | 60.6 µs | **22.6×** |
| | 2,000 | 75.483 ms | 3.195 ms | **23.6×** |

**The control is the point, and it says this table cannot answer the question it was written to
answer.** `spin` does the same loop over the same map and searches nothing, and it costs within a
few percent of `lookup` at both sizes — so at 250 and 2,000 entries a binary search is *smaller than
the tail-recursive loop that calls it*, and nothing here distinguishes halving from scanning. The
first version of this measurement asserted a ratio under 3.0 and got 2.94, which would have been a
gate passing for the wrong reason: what it was measuring was the map crossing the pipe, eight times
bigger at the large size.

So the shape claim is made where it needs no clock:
`a_lookup_costs_the_same_whatever_the_map_holds` reads the **arena**. A lookup leaves **16 bytes** —
`Some`'s tag and its payload — on a map of 200 and on a map of 1,600, so nothing is built per entry;
and `map_keys` leaves 1,608 then 12,808 bytes, which is its answer and not its input.

`walk` is the row to read for a different reason: 1.37 ms to 75.5 ms for eight times the entries is
**55×**, on *both* backends. That is `map_keys` inside a loop, which allocates the whole key list on
every step — a quadratic in the **program**, not in either implementation, and exactly the shape
[`19`](19-phase-1-report.md) §19.4 calls a semantic defect rather than a backend one. It is in the
fixture deliberately, because a reader who sees `map_keys(m)` in a loop should see this number.

## 106.7 What this corrects

- [`101`](101-the-heap-report.md) §101.5's `Map[K, V]` row is gone, and with it the last collection
  row. Its stated reason — "a `PMap` is a weight-balanced tree with structural sharing; the same
  ownership question one level up" — was right about the *writing* half and did not need to be true
  of the reading half, which is what §106.4 separates.
- [`104`](104-text-on-the-heap-report.md) §104.3 quotes a refusal from `corpus/01-counter.beck`
  blaming `Session.claims` for being a `Map`. That definition compiles.
- [`105`](105-lists-arrive-read-only-report.md) §105.4's named prevention is built.
- **`Html` was refused for a reason that named the wrong thing.** It fell through to "returns
  `Html`, which is not a type this module declares" — a true sentence about the path taken and a
  misleading one about the cause, since `Html` is a builtin and what it lacks is a layout. It is
  named now, and says what it follows: the collections rather than text.
- [`08`](08-roadmap.md) §8.5.5's Lane E row: what is left is closures, the effects, `Html`, and
  growing a collection.

## 106.8 What this establishes

That reading a `Map` is compiled and growing one is not, over 898 calls per backend on an alphabet
containing the empty map, a prefix pair, maps differing only in a value, and — for the search — keys
that are present, below every key, above every key, and **between two**, which is the case a window
that shrinks wrongly never leaves.

And that nine corpus folds compile, which `a_corpus_fold_compiles` asserts by name — with the other
side asserted too, so it cannot pass by everything compiling: no corpus `view` does.

It establishes nothing about a view, which builds `Html`; nothing about a fold that *inserts*, which
most of the other twenty-six do; and nothing about how a compiled binary search scales, which §106.6
says plainly rather than inferring from a number that was measuring something else.
