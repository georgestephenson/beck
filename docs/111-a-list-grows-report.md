# 111 — A list grows, and the refusal was about a layout

**Built.** `list_append` compiles, to **both** code generators, and the accumulator every loop in
this language is written as is **linear**.

It had been refused since [`106`](106-lists-arrive-read-only-report.md) §106.5, and refused for a
reason that turned out to be about the wrong thing. The reason on record was *ownership*: the
tree-walker pushes in place when [`70`](70-the-evaluator-gets-fast-report.md)'s last-use analysis
proves the accumulator is nobody else's, and an arena cannot prove that — so an append here would
copy, the idiom would be `Θ(n²)` where the evaluator is `Θ(n)`, and
[`69`](69-standard-library-imports-report.md) §69.7's quadratic would be rebuilt on purpose. Every
sentence of that is true. What none of it establishes is the conclusion, because **the copy was
forced by the layout rather than by the absence of an analysis**.

A list was one word — the count — with the elements after it. With the count in front of the
elements, an append can copy them or it can overwrite the count, and the second is visible to
everything else holding that list. Separate the two and a third answer appears:

| | word 0 | word 1 | word 2.. |
|---|---|---|---|
| **header** | how many elements | the data block's offset | — |
| **data block** | how many it can hold (`cap`) | how many are written (`used`) | the elements |

A header is written once and never touched again. Appending writes at index `used` — **a slot no
header covers**, because every header over a block has a count of at most `used` — bumps `used`, and
answers a *new* header. Nothing any reader can see is ever rewritten. That needs no ownership
analysis, no reference count and no last-use flag: it is sound by the shape of the writes.

**Across the tree, 711 → 895 definitions compile and refusals go 707 → 523.** That is the largest
jump any of these rounds has produced, and it is not because `list_append` appears in 184
definitions: it appears in 65, and the rest inherited the refusal from a callee that had it.

---

## 111.1 What the old reason got right, and where it stopped

[`106`](106-lists-arrive-read-only-report.md) §106.5 and [`107`](107-a-map-arrives-read-only-report.md)
§107.4 state the rule this backend is held to, and it is a good rule: **this backend does not ship an
operation whose asymptote is worse than the evaluator's.** A program that reads collections gets
compiled; one that builds them keeps the tree-walker, and the refusal says which.

Applied to `list_append`, the argument ran: the evaluator is `O(1)` amortised because it can push in
place; pushing in place needs to know nobody else holds the list; an arena has no ownership in it;
therefore `O(n)`; therefore refuse. The step that does not follow is the second one. **In-place
mutation is one way to make an append cheap and it is not the only one** — it is the one a `Vec`
behind an `Arc` has available, because a `Vec` is a length beside its buffer and the length is the
thing that has to change.

What an arena of offsets has instead is that *allocation is free and nothing is ever freed*. A new
two-word header per append is cheaper than an ownership analysis and does not need one. Seeing that
took separating "what this list is" from "where the elements are", which is a layout question, and
the refusal had filed it as an analysis question.

§111.6 is the part of this worth carrying: the refusal was **inherited from the evaluator's
implementation strategy**. `beck-eval` solves this with uniqueness because `beck-eval` is written in
Rust over `Arc<Vec<T>>`; the backend copied the shape of that answer, found it unavailable, and
stopped. [`106`](106-lists-arrive-read-only-report.md) §106.7's gate asks whether a refusal's stated
reason is *true*; every sentence of this one was, and the refusal was still wrong.

## 111.2 Why it is sound, in one paragraph

Two lists can share a block. Call their counts `a` and `b`, and the block's `used` count `u`.

Every header over a block was written either by an allocation (`count = used`, both fresh) or by an
append (`count = used + 1` at the moment it bumped `used`), so **every header's count is at most
`u`**, and a reader of a list of count `c` reads slots `0 … c-1`, all of which are `< u`. A slot is
written exactly once — at index `used`, by the append that then bumps `used` — so no slot a reader
can see is ever written again. An append from a header whose `count < u` finds the slot already
taken and copies instead. Nothing else mutates anything.

The one mutable word in the design is `used`, it only ever grows, and it is read and written by one
compare-and-store. `beck-eval`'s equivalent is `Arc::get_mut`, and the two agree by a differential
rather than by argument (§111.5).

## 111.3 What it costs: one load, once per operation

An element used to be an add away from the list's offset; it is now a load away — the block's offset
— and then the same add. That load is paid **once per operation and not once per element**, because
every generated loop takes the data pointer before it starts, and `beck.list.data` is the one place
either emitter computes it.

What is *not* free is the size of an empty list, which the shape gates state outright:

| | before | after |
|---|---|---|
| a list of `n`, freshly allocated | `8 + 8n` | `32 + 8n` |
| one element sliced out | 16 | 40 |
| a row of the todo page | 96 | 144 |

Three gates in `native.rs` carry those constants, and each was updated rather than relaxed, because
what they assert is that the number does not grow with `n` and that claim is unchanged. The
per-element word is the same word it was.

## 111.4 The accumulator, measured

`native.rs::an_appended_accumulator_is_linear`, a gate with **no clock in it**: `doubled_up` walks a
list and appends each element doubled, which is the idiom in question.

| elements | arena |
|---|---|
| 500 | 28,272 B |
| 2,000 | 113,136 B |

**4.0× for 4× the elements.** A copying append leaves about sixteen, which is what the *text*
accumulator beside it in the same file still does — `an_accumulator_costs_the_square_of_what_it_builds`
asserts the quadratic for `Str`, and the two tests are worth reading together: one asserts a
quadratic and one a linear, on the same shape, in the same backend, because only one of the two
layouts has been separated. §111.7 is what that implies for text.

And the wall clock, `measure_native.rs::what_an_appended_accumulator_costs_against_the_tree_walker`:

| benchmark | elements | evaluator | native | ratio |
|---|---|---|---|---|
| `doubled` — walk and append | 2,000 | 1.468 ms | 128.4 µs | **11.43×** |
| | 8,000 | 6.116 ms | 870.3 µs | **7.03×** |
| `summed` — the same walk, nothing built | 2,000 | 1.440 ms | 17.6 µs | **81.62×** |
| | 8,000 | 5.527 ms | 70.4 µs | **78.46×** |

The control is the row that makes the first one readable. `summed` holds its ratio flat over four
times the elements; `doubled` loses a third of it, and the arena gate above says the *allocation* is
linear — so what grows is the **reply**: eight thousand elements crossing a pipe and being decoded
into `Value`s, which is [`93`](93-llvm-backend-report.md) §93.1's round trip and the same thing
[`108`](108-closures-arrive-report.md) measured as `concat_lists` losing to the tree-walker outright.
A definition that appends and answers with a *number* keeps the 80×.

## 111.5 The gates

- **`native.rs::the_two_backends_agree_on_lists`** and
  **`cranelift.rs::the_three_backends_agree_on_lists`**, extended with the cases this feature has:
  - `appended`, the operation;
  - **`forked`**, which is the whole soundness argument in a program — two lists grown from one, so
    the first extends the block it stands at the end of and the second finds the slot taken and
    copies. It answers with the original's length *and* both results, so a backend that mutated what
    the original sees fails on the first case;
  - `doubled_up`, the accumulator, against the evaluator's own answer;
  - `named`, appending a `Str`, because an element's word is an offset and a block holds words;
  - `grown_bag`, appending to a list read out of a record — a header the emitter did not allocate.
- **`native.rs::an_appended_accumulator_is_linear`** — §111.4's clockless gate.
- The three arena-shape gates above, with their constants moved and their claims unchanged.
- **Four refusal lists** moved `list_append` to their control side and put `map_insert` in its place,
  which is what those lists are for.

## 111.6 What this corrects

- [`106`](106-lists-arrive-read-only-report.md) §106.5, [`107`](107-a-map-arrives-read-only-report.md)
  §107.4, [`101`](101-the-heap-report.md) §101.5 and [`105`](105-text-on-the-heap-report.md) §105.7
  all name `list_append` as refused, and three of them name the *reason* as ownership. They stand as
  history. The correction is that the reason was true and the conclusion did not follow: what forced
  the copy was the count sitting in front of the elements.
- [`08`](08-roadmap.md) §8.5.5's Lane E cell called the remainder "growing a collection" and called
  it a decision. Half of it was a layout.
- **`beck_llvm`'s module documentation** said every operation that grows a collection is refused. It
  is `map_insert` and its two siblings now, and §111.7 says what that one still needs.

## 111.7 What this does not establish

- **Nothing about a `Map`.** [`107`](107-a-map-arrives-read-only-report.md) §107.4's argument for
  `map_insert` is *not* the one this report corrects: a `PMap` is a weight-balanced tree that shares
  every subtree it did not touch, and the gap there is `O(log n)` against `O(n)` for reasons that
  survive any amount of layout separation — a sorted run has to shift. The thing that would remove it
  is a tree in the arena, which is still a design and not a patch.
- **Nothing about text.** A `Str` is a length and its bytes, which is exactly the shape this report
  separated for a list, so the same separation is available — and it is *not* obviously right, because
  a `Str`'s bytes are read by `memcmp` and `memcpy` in six runtime functions that currently need no
  indirection at all. [`105`](105-text-on-the-heap-report.md) §105.7's `grown` is still 0.17× and
  still asserted to be.
- **Nothing about `list_flat_map`**, which needs the length of an answer nobody has computed yet, and
  nothing about the effects that reach the host.
- **Nothing about how a shared block behaves under an adversarial program.** Two lists appending
  alternately from the same count each copy, which is `O(n)` per append — the evaluator does the same
  thing for the same reason (its refcount is 2), so the *asymptotes still match*, which is the rule.
  It is not a claim that the constant matches.
- **Nothing about memory.** A block that doubles holds up to twice what it needs, and an arena frees
  nothing, so a long accumulator leaves about 4× its elements behind. That is what §111.4's gate
  measures and asserts as linear; it is not a claim that it is small.
