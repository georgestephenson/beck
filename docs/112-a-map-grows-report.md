# 112 — A map grows, as the tree it always was

**Built.** `map_insert`, `map_remove` and `map_merge` compile, to **both** code generators, and a
fold that keeps a `Map` is `Θ(n log n)` rather than `Θ(n²)`.

[`111`](111-a-list-grows-report.md) §111.7 said this one would not fall to the same trick, and it was
right: a list's refusal was about a *layout* and a map's is about a *structure*. A sorted run has to
shift however its header is arranged, so no amount of separating the count from the entries makes an
insert cheap. What removes it is the structure `beck_core::pmap` already uses — a **weight-balanced
tree**, whose insert rebuilds the path and shares every subtree it did not touch.

So a `Map` in the arena is now that tree: five words a node — a subtree size, a key, a value and two
children — with the same `DELTA` and `RATIO` the evaluator's own module argues for, and an empty map
is the offset `0`.

**Across the tree, 895 → 1,137 definitions compile and refusals go 523 → 281.** `examples/todo.beck`
compiles **eight of its nine definitions** — the fold, the ownership check, the query and the page —
and the one left is `validate`, for a Unicode table.

---

## 112.1 Why the list's answer does not work here

[`111`](111-a-list-grows-report.md) separated a list's count from its elements so that an append could
write a slot no reader could see. The move works because a list grows **at one end**, and there is
always a slot after the last element.

A map is ordered by key. An insert lands *in the middle*, and every entry after it shifts — so a
sorted run cannot be extended in place at any position other than the end, and a program inserting in
key order would be the only one that ever hit the fast path. The gap is `O(n)` against `O(log n)` and
it is a property of the run, not of where its count sits.

That is why §111.7 forecast this and why the forecast held. The two refusals looked identical in
`refusal()` — *grows a collection* — and were two different claims.

## 112.2 The tree, and what is generated per repr

| word | what |
|---|---|
| 0 | how many entries this subtree holds |
| 1 | the key |
| 2 | the value |
| 3 | the left child, or `0` |
| 4 | the right child, or `0` |

Insert walks down comparing keys, rebuilds the path on the way out, and rebalances at each step.
`O(log n)` fresh nodes; every other subtree is shared by offset. A node is never written after it is
built, so the map an insert was given is exactly what it was — which is the same soundness argument
[`111`](111-a-list-grows-report.md) §111.2 makes, and it comes for free here rather than needing a
design.

The division of labour is worth stating, because it is what kept this to one report rather than two:
**everything that moves nodes is one function for the whole module.** Rebalancing shuffles *words* —
sizes, keys, values and two children — and never asks what a key is. So `size`, `node`, `balance`,
`nth` and the in-order walk are written once; only `find`, `insert`, `remove`, `merge` and the
two-map order are generated per repr, because those are the ones that compare.

`map_remove` is the textbook deletion with the weights' rebalance: a node with two children is
replaced by the smallest of its right subtree, which that subtree's leftmost node is.

## 112.3 What it costs

`native.rs::a_fold_over_a_map_is_not_quadratic`, a gate with **no clock in it** — `grown(n)` is the
fold every `durable` state is, `n` inserts into an accumulator:

| entries | arena |
|---|---|
| 500 | 262,712 B |
| 2,000 | 1,279,432 B |

**4.9× for 4× the entries**, which is `n log n`. A copying insert costs sixteen. The assertion is
`< 2 × steps` rather than a tight bound on purpose: what separates linearithmic from quadratic here
is a factor of three, and a gate that split them at 5.1 would be measuring the balance constants
rather than the asymptote.

A lookup is `O(log n)` where the sorted run's binary search was `O(log n)` too — so reads are the same
asymptote with a worse constant (a pointer chase per level rather than an indexed probe), and
`map_keys`/`map_values` go from one `memcpy` to an in-order walk. Both are the price of the insert,
and both are stated here rather than left to be found.

## 112.4 The gates

- **`native.rs::the_two_backends_agree_on_maps`** and its Cranelift twin — **1,216 calls**, extended
  with the cases this feature has: `put`, `dropped`, `joined`, and
  - **`branched`**, which is the sharing argument as a program: two maps grown from one, answering
    with the original's length *and* both lookups, so a rotation that wrote through a node somebody
    else holds fails on the first case;
  - **`grown`**, the fold, against the evaluator's own answer;
  - **`descending`**, which inserts in *descending* key order — the case a tree that did not
    rebalance degenerates on, and the one a sorted run handled by accident.
- **`a_fold_over_a_map_is_not_quadratic`** — §112.3's clockless gate.
- The refusal lists moved `map_insert` to their control side, which is the fourth time this series
  has done that and the last collection there was to move.

## 112.5 The finding: a definition could take a runtime symbol's name

`awfy/richards.beck` has a definition called `dispatch`. Every user definition was mangled to
`beck.<name>`, and the module's own dispatcher is `beck.dispatch` — so the assembler answered
**"invalid redefinition of function 'beck.dispatch'"** for a program that had done nothing wrong.

It had been latent since [`93`](93-llvm-backend-report.md), and it surfaced here because a collision
needs *both* halves to exist: `dispatch` had never compiled before, so the name it would have taken
was never claimed. That is [`106`](106-lists-arrive-read-only-report.md) §106.7's shape one level
down — a rule nobody had tested at the production that reaches it — and it is the third time in this
series that making something compile has exposed a defect in something else.

The fix is a namespace rather than a prefix: a user definition is `beck.def.<name>`, and everything
either emitter generates for itself stays `beck.<something>`. Both emitters changed in the same way,
and `every_corpus_program_produces_a_module_llvm_accepts` is what went red.

## 112.6 What this does not establish

- **Nothing about the constants.** This is `beck_core::pmap`'s asymptote, not its speed. A node here
  is five words in an arena and there it is an `Arc` with a refcount, and neither has been measured
  against the other.
- **Nothing about a map that is *read* faster.** A binary search over a contiguous run is friendlier
  to a cache than a pointer chase, and §112.3 says so: the insert is what this bought.
- **Nothing about `{}` with entries in it.** A map literal with keys still has to sort at run time
  and is still refused by name — every `durable` fold in this tree starts at `{}`.
- **Nothing about the effects that reach the host**, which is what is left of Lane E.
