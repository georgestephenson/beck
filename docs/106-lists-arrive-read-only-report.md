# 106 — A list arrives read-only

**Built.** A `list[T]` is a value both code generators compile — a layout, literals, all six
comparisons, `list_len`, `list_is_empty`, `list_get`, `list_contains`, `list_index_of`,
`list_slice`, `list_take`, `list_drop` and `list_reverse` — and `list_append` is **refused**, by
name and with the reason. [`101`](101-the-heap-report.md) §101.5's table of what the heap does not
reach had six rows, [`105`](105-text-on-the-heap-report.md) removed one, and this removes half of
another: what is left of collections is the half that **grows** one.

**283 → 344 → 403 definitions** compile across the tree.
[`awfy/richards.beck`](../compiler/awfy/richards.beck) went from 12 to 23 in this change alone.

Three things in it are worth more than the feature, and none is about lists:

- **A refusal reason was false.** [`105`](105-text-on-the-heap-report.md) §105.4 refused
  `str_index_of` because "its `Option` has no layout here", and the prelude's `Option` has had one
  since [`101`](101-the-heap-report.md) — a definition returning `Option[Int]` compiled in the
  fixture *beside* it. Every gate was green, because each asserted that the refusal said something
  and none asked whether what it said was so. §106.7.
- **A refusal reason could not fire.** The higher-order half of the collection primitives is
  refused a line earlier, by its own argument, so the table's entry for them was unreachable prose.
  Deleted, which is [`89`](89-query-fusion-report.md) §89.5's answer to the same thing. §106.7.
- **The declared depth ceiling was not reachable.** The host's decoder recursed, so `MAX_DEPTH =
  2048` was a claim about the *thread's stack* and a debug build aborted at about 1,600. It is
  iterative now, and the ceiling is a bound on the value in every profile. §106.6.

---

## 106.1 The shape

One header word and one word per element:

| Word | What |
|---|---|
| 0 | how many elements |
| 1.. | one word per element, in order |

Which is to say: a `Str`'s shape without the second count. What a word *means* is not in the object
— an `Int`, a real's bits, a `Bool`, or the offset of a string, a record or another list — so the
element type is carried in the `Repr` rather than at the machine. `Repr::List(u32)` indexes a table
of element reprs the way `Repr::Obj(u32)` indexes a table of layouts, and a `list[Int]` and a
`list[Point]` are two entries even though they are one shape.

That is the whole layout, and it is why four of the operations do not care what an element is:
allocating one, taking a clamped range out of one (`list_slice`, `list_take` and `list_drop` are one
function and three pieces of arithmetic) and turning one around are word moves. Everything that has
to know what a word means is **generated per element repr** — a three-way comparison over two words,
the lexicographic order over two lists, and a linear search — because taking the comparison as a
function pointer would be an indirect call, which is the one thing this backend does not have and is
exactly what a closure would need.

Ordering is `Vec<Value>`'s: element by element, and a list that is a prefix of another is less than
it. The prefix half is the one a plausible implementation gets wrong in exactly one direction, and
it is [`105`](105-text-on-the-heap-report.md) §105.4's `memcmp("ab", "abc", 2)` one type up.

## 106.2 What compiles now that did not

`beck native <file>` over the corpus, the examples, all three SICP chapters, Are We Fast Yet, the
Benchmarks Game, the standard library and `xlang/`:

| | before text | after text | after lists |
|---|---:|---:|---:|
| programs with at least one compiled definition | 30 | 50 | **53** |
| definitions compiled | 283 | 344 | **403** |

Where this half landed:

| | before | after | why |
|---|---:|---:|---|
| [`awfy/richards.beck`](../compiler/awfy/richards.beck) | 12 | **23** | a scheduler is a list of tasks read by index |
| [`awfy/cd.beck`](../compiler/awfy/cd.beck) | 30 | **38** | |
| [`lib/decimal.beck`](../compiler/lib/decimal.beck) | 6 | **13** | a decimal is a sign and a list of limbs |
| [`sicp/ch2.beck`](../compiler/sicp/ch2.beck) | 17 | **23** | §2.2 is lists all the way down |
| [`awfy/json.beck`](../compiler/awfy/json.beck) | 3 | **8** | |
| [`clbg/`](../compiler/clbg/README.md) | 63 | **72** | |

## 106.3 What it answers, and what it refuses

| Compiled | How |
|---|---|
| `[a, b, c]`, `[]` | one allocation, filled left to right — the order a trap has to be raised in |
| `==` `!=` `<` `<=` `>` `>=` | `beck.list.cmp.N`, element by element and then by length |
| `list_len`, `list_is_empty` | a load of the header word |
| `list_get` | an `Option[T]`, **without a branch** — see below |
| `list_contains`, `list_index_of` | `beck.list.find.N`, a linear scan |
| `list_slice`, `list_take`, `list_drop` | one clamped range and one `memcpy`, clamped where the evaluator clamps |
| `list_reverse` | a word-by-word walk |
| a `list` field, a list in a variant, `with` over one | the layout was already there; a list is one more word |
| `str_index_of` | the row [`105`](105-text-on-the-heap-report.md) got wrong (§106.7) |

`list_get` is the interesting one, and the trick is the **address** rather than the value. An index
outside the list would read a word that may be past the end of the arena, so the address handed to
the load is a `select` between the element's and the list's own *header*, which is always there. Out
of bounds therefore loads the length, stores it where `Some`'s payload would go, and tags the answer
`None` — and the host reads a variant's own fields and nothing else, so that word is never looked
at. A branch would be two allocations, two arena bumps and a join, for a value that fits in a
register either way.

Refused, each by name with the reason:

| | why |
|---|---|
| `list_append`, `concat_lists` | §106.5 — the decision, not a gap |
| `list_zip` | answers with a list of pairs, and there is no pair type to lay out |
| `map_list`, `filter_list`, `list_fold`, `sort_by`, `list_all`, `list_any`, `list_flat_map` | **nothing in the table** — see §106.7 |
| `str_split`, `str_chars` | answer with a list whose *elements* they also allocate, which is two loops rather than the one every list here builds |
| `str_join` | builds text whose size is a sum over a list |
| `Map[K, V]` | still the whole row: a `PMap` is a weight-balanced tree with structural sharing |

## 106.4 One layout, three runtimes — and what writing it twice caught again

[`97`](97-cranelift-report.md) §97.3's rule holds and paid again, in the same place and for the
third time: the **record comparison compared a reference field by its offset**.
[`105`](105-text-on-the-heap-report.md) §105.5 found it in Cranelift for a `Str`; this found it in
**LLVM** for a `list`, because a `Repr` carried in an `i64` looks exactly like an `Int` to a match
that is not asked to distinguish them, and each emitter's fall-through arm swallows whichever
reference kind was added last.

Both times the differential's *pairs* caught it — `Bag{items: []} < Bag{items: []}` answered `true`
— and both times the single-value sweep would not have. Three occurrences is a pattern rather than
a coincidence, and the shape of it is: **adding a `Repr` variant is a compile error at every site
that matches exhaustively and silence at every site with a `_`**. That is
[`91`](91-guards-and-alternatives-report.md) §91.3's `Arm` gaining a field, one enum over. What
would make the fourth impossible is a `Repr::reference()` accessor that the comparison is written
against instead of a match; it is not built, and it is named here so the next person does not have
to find it a fourth time.

## 106.5 `list_append` is refused, and text's `+` is not

The two decisions look inconsistent and are not, so both arguments are here.

[`69`](69-standard-library-imports-report.md) §69.7 measured the defect: `list_append` copies the
whole list, so the tail-recursive accumulator every loop in the language is written as is `O(n²)` in
time. [`70`](70-the-evaluator-gets-fast-report.md) fixed it for the tree-walker with last-use moves
— `liveness` proves the read of the accumulator is its last, so the append pushes into a list nobody
else holds. [`101`](101-the-heap-report.md) §101.5 then forecast exactly this: *"an arena with no
ownership in it cannot prove that… which is §69.7 reintroduced in a new place."*

So `list_append` is refused. A definition that grows a list stays with the evaluator, which is
`O(n)`, and the refusal names the reason. **Nothing else about a list is refused with it**: reading,
searching, slicing and comparing all compile, so a program that reads collections gets the backend
and one that builds them keeps the tree-walker's asymptote.

Text's `+` shipped instead, and [`105`](105-text-on-the-heap-report.md) §105.6 measured it at
**0.17× the tree-walker** at 4,000 appends. The difference is not principle but what refusing costs:
`+` is the *only* way to build a `Str`, so refusing it would mean a `Str` could be received and read
and never combined — no `"hello, " + name`. Refusing `list_append` costs a program the loop that
builds a list and leaves every other thing it does with one. When the same choice is available for
text — when there is another way to build a string — the same rule should apply to it.

**What would remove the asymmetry is the analysis, not another refusal.** `liveness`'s `last_use`
flag is on the `Core` tree already, computed in the front end and explicitly "a fact about the
program" rather than the evaluator's. What it does not answer is the heap half — its own
documentation says so: *"a value may still be shared, and a backend must check that separately."*
The evaluator checks with a refcount; an arena of offsets has none, and "is this list at the top of
the arena" is not the same question, because a value already handed to an earlier argument is still
at the top. That is the piece somebody has to design, and it is the whole of what stands between
this backend and a linear accumulator.

## 106.6 The ceiling that was not reachable

`Heap::decode` walked a reply by recursing, and `MAX_DEPTH = 2048` bounded that recursion. Adding a
`list` arm made the frame bigger, and a `cargo test` build then **aborted the process** on a value
800 deep — [`52`](52-crypto-and-identifiers-report.md) §52.6's "nine match arms cost a thousand
levels of recursion", in the host rather than in the evaluator.

Splitting the arms into separate functions bought back the 800. The measurement that followed is the
part worth recording: after the split a debug build managed **1,200 and aborted at 1,600**, against a
declared ceiling of 2,048. So the number in the source had never been the limit — the limit was the
thread's stack, and *which replies could be read depended on how the compiler was built*. That is
[`64`](64-compile-speed-report.md) §64.4's finding one subsystem over, and the property
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) states — a ceiling is declared,
not discovered — which this did not have.

The decoder is **iterative** now, with its stack on the heap: `MAX_DEPTH` bounds how deep a *value*
may be and nothing else, and it is the same bound in every profile.
`a_value_at_the_declared_ceiling_decodes_rather_than_aborting` builds a value exactly that deep by
hand — no toolchain, the same default-stack thread `cargo test` gives everything else — decodes it,
walks the answer to check it really is that deep, and asserts that one level further is a message.
It goes red on the recursive version, which is how it was checked.

## 106.7 What a refusal's reason is worth

Two findings, and they are the same finding from opposite ends.

**A reason that was false.** [`105`](105-text-on-the-heap-report.md) §105.4 refused `str_index_of`
because it "answers with an `Option`, whose layout this backend resolves from a program's own types
and not from the prelude's". The prelude's `Option` has had a layout since
[`101`](101-the-heap-report.md); `maybe(n) -> Option[Int]` compiles, and compiled in the fixture
beside the refusal while that sentence was being written. The wall was imaginary and
`str_index_of` compiles now — a search, a byte-to-character conversion, and the same branch-free
`Some`/`None` `list_get` builds.

Every gate around it was green. `what_the_heap_does_not_reach_is_refused_by_name` asserted the
reason *contained a string*; `the_two_emitters_accept_and_refuse_the_same_definitions` asserted both
emitters said something; `what_cannot_be_compiled_is_refused_by_name_and_with_a_reason` asserted the
reason was non-empty. All three assert that a refusal **said** something and none asks whether what
it said is **so** — which is [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5's
pattern in the one place this project had not looked for it: a proxy for a control is defeated by
naming, and a reason is a proxy for a fact.

`a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one` is the gate that would have gone
red. It asks the heap whether each blamed type really has no layout, and asserts the control the
missing assertion was: `Option[Int]` **does** have one, so no refusal may say otherwise.

**A reason that could not fire.** The table also had an entry for `map_list` and the six other
higher-order primitives — "takes a function, and a function value is a closure". True, and
unreachable: the emitter evaluates a primitive's *arguments* before it looks at the operator, so
`double_it` fails first with "is used as a value rather than called, and a function value is a
closure", which is both truer and more specific. A reason that cannot be produced is
[`89`](89-query-fusion-report.md) §89.5's rule that could not fire, and it gets the same answer —
deleted. The fixture now asserts the message that is actually produced.

## 106.8 The numbers

`cargo test --release --test measure_native -- --nocapture --test-threads=1`, Ubuntu clang 18.1.3,
`-O2`, median of seven runs at the small size and three at the large one. Two sizes each per
`AGENTS.md`, and every ratio includes the pipe round trip.

| benchmark | size | evaluator | native | ratio |
|---|---:|---:|---:|---:|
| `walk` — read every element by index | 2,000 | 1.469 ms | 41.1 µs | **35.8×** |
| | 16,000 | 11.616 ms | 320 µs | **36.3×** |
| `hunt` — search a 500-element list, repeatedly | 500 | 918 µs | 144 µs | **6.4×** |
| | 4,000 | 7.216 ms | 858 µs | **8.4×** |
| `windows` — a four-element slice at every position | 2,000 | 1.226 ms | 58.6 µs | **20.9×** |
| | 16,000 | 10.037 ms | 314 µs | **31.9×** |

There is no accumulator row, because `list_append` is refused — §106.5. The claim
[`105`](105-text-on-the-heap-report.md) §105.6 made about text's accumulator is now gated **with no
clock in it** as well, which is the correction §106.9 records:
`an_accumulator_costs_the_square_of_what_it_builds` reads the arena rather than the wall clock and
finds **15.9× the bytes for 4× the steps**.

The shape gates are the same two, one per type:
`a_list_slice_costs_its_answer_and_not_the_list_it_came_from` counts 16 bytes an element at 200 and
at 1,600, so a `list_slice` that copied what it was taken *from* would be quadratic there — which
the differential cannot see, because copying too much still answers correctly.

## 106.9 What this corrects

- [`105`](105-text-on-the-heap-report.md) §105.4's reason for refusing `str_index_of` is **false**,
  and §106.7 is the correction. The row is gone and the primitive compiles.
- [`105`](105-text-on-the-heap-report.md) §105.6's `str_split` reason — "answers with a list, and a
  collection is not on this heap yet" — is stale rather than wrong; the true reason is that it
  allocates the *elements* too.
- `measure_native.rs` asserted that text's accumulator is **slower** than the tree-walker. That was
  a gate on which profile ran it — it is slower in a release build and faster in a debug one, for
  exactly the reason that file's own Cranelift row already warns about — and it went red in the
  workspace sweep the day it was written. The assertion is gone; the row is still printed, and the
  claim it was evidence for is gated without a clock instead.
- [`101`](101-the-heap-report.md) §101.5's `list[T]` row is half gone. Its forecast — that the
  in-place append is what an arena cannot have — was exactly right, and §106.5 is that forecast
  cashed rather than argued with.
- [`08`](08-roadmap.md) §8.5.5's Lane E row: what is left is `Map`, closures and the effects.

## 106.10 What this establishes

That reading a collection is compiled and growing one is not, over 1,425 calls per backend on an
alphabet with an empty list, a prefix pair, and elements of every kind that is itself an offset —
text, a list, a record.

It establishes nothing about a fold or a view, which need a `Map`; nothing about
[`94`](94-mode-b-report.md) §94.8's Mode B codegen, which needs `Html`; and nothing about the loop
that builds a collection, which is the shape most Beck programs are written in and is the one thing
here that is refused rather than slow.
