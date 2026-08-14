# 119 — A list comes apart, and a refusal that had stopped being true

**Built.** `case [first, *rest]` compiles, to **both** code generators. A list pattern was the last
pattern form the native backends refused — nesting, guards and alternatives all arrived with
[`90`](90-nested-patterns-report.md) and [`91`](91-guards-and-alternatives-report.md) — and it was
refused with a sentence that had been false for three reports:

> `matches a list pattern, and a collection is not on this heap yet`

A collection has been on this heap since [`106`](106-lists-arrive-read-only-report.md). The work
below is small; **the finding is that nothing noticed**, and §119.4 is the gate that would have.

**Across the tree, 889 → 905 definitions compile and refusals go 189 → 173.** Six of those are the
list patterns themselves and **ten are definitions that were only ever waiting on them** — which is
the ordinary shape of this backend's fixed point, and a useful reminder that a refusal's cost is
never just itself.

---

## 119.1 What a list pattern is, at the machine

Three steps, and the order is the whole safety argument:

1. **The length.** `[a, b]` is a list of exactly two; `[a, *rest]` is a list of at least one. That
   is the evaluator's own rule and it is one comparison — `eq` or `sge` against a constant.
2. **The elements**, read *after* the length test has proved they are there. A pattern that loaded
   first would read past the end of the data block on a short list, and `[]` is where that shows.
3. **The tail**, when there is a binder for it.

Everything else a list pattern can contain — a constant, a constructor, a nested list — is
[`90`](90-nested-patterns-report.md)'s recursion, unchanged. `probe` calls itself on each element
with the element's repr, which is what makes `[Some(x), *_]` and `[[a, *_], *_]` work without a line
about either.

## 119.2 The tail is copied, and that is not a shortcut

`*rest` binds a **fresh list**, built by `beck.list.copy` — the same helper `list_drop` uses.

The tempting alternative is a borrowed suffix: a list is a header of `[count, data]` and a data
block of `[cap, used, elements…]`, so a header pointing `items.len()` words further into the block
would address the right elements with no copy at all. It is wrong, and for a reason worth writing
down: the block's **own** header carries `used`, which is what
[`113`](113-a-list-grows-report.md)'s append reads to find the slot no reader can see. A suffix
header offset into the element run would have an element read as that count, and an append onto it
would write wherever that element pointed.

So the tail is copied — and this costs nothing against the tree-walker, because the evaluator copies
it too: `Arc<Vec<_>>` cannot share a suffix either, which [`27`](27-the-walls-come-down-report.md)
§27.3 recorded as `O(n)` per step and `O(n²)` for a fold written over one. Both backends are the
same shape of slow, which is the property that matters for a differential.

## 119.3 The gates

- **`native.rs::the_two_backends_agree_on_list_patterns`** and
  **`cranelift.rs::the_three_backends_agree_on_list_patterns`** — 50 calls each over a fixture
  written around what a *length test* can get wrong rather than around whether destructuring works:
  the exact-length rule beside the at-least rule at every length around their boundary; the empty
  list, where an element read before the length test would fault; a tail of nothing and a tail of
  everything; two fixed elements before the tail, where an off-by-one in the copy's start shows; a
  constant inside a pattern, so an element is tested and not merely bound; a nested list, which
  reads a copy out of a copy; and text elements, so the element's repr is an offset rather than an
  integer.
- **Arm order is part of it.** `[]`, `[x]`, `[a, b]` and `[first, *rest]` overlap, and `described`
  has all four in one `match` — so an emitter that compiled the at-least rule where the exact one
  belonged would answer the wrong branch rather than failing.

## 119.4 The finding: the gate was type-shaped and the lie was prose-shaped

[`105`](105-text-on-the-heap-report.md) §105.4 found a refusal whose stated reason was false, and
the response was a gate:
`native.rs::a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one`. It takes every type a
refusal blames for having no layout and asks whether it has one. It has fired once already, when
[`108`](108-closures-arrive-report.md) gave a closure a shape.

It could not fire here. *"A collection is not on this heap yet"* **names no type** — there is no
`Ty` in it to resolve and ask about — so a gate that resolves types had nothing to look at, and the
sentence stayed true-sounding through [`106`](106-lists-arrive-read-only-report.md),
[`107`](107-a-map-arrives-read-only-report.md) and [`113`](113-a-list-grows-report.md).

So the gate grew a second half, in the corpus pass where every refusal in the tree is already
collected: **a list of sentences this backend may no longer say about itself.** "not on this heap",
"no collection", "text is not" — a refusal reason containing any of them fails the suite, by name.
Checked by putting the old refusal back, where it names the six definitions it used to refuse.

The general lesson is the one [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5 keeps
teaching in new places: a gate tests the *shape of the gap* it was written for. This one was written
for a reason that named a type, so it covered reasons that named types. What it did not cover was a
reason that named a **class** — and a class is exactly what a report retires.

## 119.5 What this does not establish

- **Nothing about a pattern over a `Map`.** There is no map pattern in the language; if one arrives
  it is a different form with a different test.
- **Nothing about sharing a suffix** (§119.2). Both backends copy, and a language that wanted
  `[first, *rest]` to be cheap in a loop would need a list representation that can share a tail —
  which is [`27`](27-the-walls-come-down-report.md) §27.3's open question and not this one.
- **Nothing about the remaining refusals.** 173 stand, and the largest class by far is a **function
  value at a boundary** — every `parameter f is a function value`, and every `Seq[T]` in SICP's
  chapter 3, whose `Cons(head, rest: () -> Seq[T])` puts a closure in a *field*. That is
  `Heap::crossing`, it is a true refusal rather than a stale one, and what would move it is
  compiling definitions that are called only by other compiled definitions — which need never
  marshal a closure at all.

## 119.6 What this corrects

- **The refusal itself**, in both emitters: *"a collection is not on this heap yet"*, false since
  [`106`](106-lists-arrive-read-only-report.md).
- **`beck_llvm::heap`'s pool walk** carried a comment reading *"a list pattern is refused by both
  emitters, and is walked anyway"*. The second half is still the reason; the first half is not.
- [`05`](05-tier-lowering.md) §5.2's running correction said what is still the tree-walker's is
  *"every effect and every operation that grows a collection"*. Neither has been true since
  [`113`](113-a-list-grows-report.md), [`114`](114-a-map-grows-report.md) and
  [`116`](116-the-host-answers-back-report.md), and that section is a **design document** rather
  than a report — so it is corrected in place, in the appended-quote form it already uses, rather
  than here. This bullet says so because a correction nobody can find is not one.
