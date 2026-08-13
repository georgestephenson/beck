# 108 — A closure arrives, and it does not leave

**Built.** A `lambda` compiles, to **both** code generators: a closure is an object holding the
lambda's **rank** and its captures, applying one is a switch on that word into a direct call, and
`map_list`, `filter_list`, `list_fold`, `list_all` and `list_any` are five generated loops that go
through it. [`08`](08-roadmap.md) §8.5.5's Lane E row read "closures, the effects, `Html` and
growing a collection"; this is the first of those four.

The decision everything else follows from is
[`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md)'s, spent again: a value here is an
**offset**, and the arena crosses a pipe as bytes. So a closure cannot hold a pointer to code, and
applying one cannot be an indirect call. What it holds instead is a number — where its lambda comes
in the program's lambdas — and neither emitter generates a function pointer, a jump table in data, or
a relocation the arena would have to carry.

The line this feature draws about itself is in the title: **a closure never crosses the boundary**. It
may be built, bound, captured and applied inside one compiled call, and every place the host would
have to read one back — a parameter, a result, a record's field, a list's element, a map's key or
value — refuses it by name. Nothing in the host changed; there is no closure to marshal.

**The numbers are small and the vocabulary moved a lot**, which is
[`107`](107-a-map-arrives-read-only-report.md)'s pattern one feature on. Across the tree
**605 → 619 definitions** compile. Of the **96 refusals that blamed a closure** beforehand, **11 now
compile**, **52 are refused at the boundary above** — the design's own line — and **33 were
re-refused for a deeper reason that had always been true of them**: 14 for `sort_by`, 12 inherited
from a callee, 5 for growing a collection, and two for a primitive that is not one of the scalar
ones. The eleven include `examples/todo.beck`'s `remaining` and `corpus/05-poll.beck`'s `tally`.

The finding is §108.8, and it is not about closures: the gate
[`106`](106-lists-arrive-read-only-report.md) §106.7 built — *every type a refusal blames for having
no layout is asked whether it has one* — **fired on this change**, because giving a closure a shape
made "a closure, which has no layout here" false while the refusal that said it was still there.

---

## 108.1 The shape

| Word | What |
|---|---|
| 0 | the lambda's **rank** among the program's lambdas |
| 1.. | one word per captured value, in `VarId` order |

A rank rather than an address, for the reason above. The order of the captures is decided in
[`heap::Lambda`](../compiler/crates/beck-llvm/src/heap.rs) — the same place a record's field order and
a variant's tag are decided — because the code that *builds* a closure and the code that *reads its
captures back* are two functions in each emitter and four in all, and a layout with four spellings is
the drift this workspace spends its gates on.

Two things are not in the object, and both are deliberate:

- **The captures' types.** A capture is a `VarId`, which is a slot rather than a name, so there is no
  declaration to look a type up in. The survey takes it from a *use*: every free variable of a
  `lam` is read somewhere under it — that is what makes it free — so `heap::var_types` walks the
  body for a `Var` node and records the type it is read at. It may be under a nested `lam` rather
  than at the top, which is why the walk is the whole subtree.
- **A code address**, as above.

## 108.2 A rank is ordered the way the evaluator compares two closures

`beck_core::core::Closure`'s `Ord` is its **parameters**, then **where its body starts**. The
captured frame is deliberately not in it, so two closures from one `lam` are equal however
differently they were built — and `Prim::Eq` goes through `cmp`, so that is what `==` answers too.

Ranks are assigned in exactly that order, over the whole program, in `heap::survey`. That makes the
comparison of two closures one word comparison: `Repr::order` answers `Order::Call("beck.fn.cmp")`,
and that function loads two ranks and compares them unsigned. It agrees with the evaluator because
the *order* of the ranks is the order of the code positions, not because anything transcribes a
span.

This is [`101`](101-the-heap-report.md)'s "a tag is a variant's rank **by name**" argument in a
second place, and it is the reason `captures_ignored` is in the differential: two closures of one
lambda with different captures must compare **equal**, which is surprising enough that a compiled
backend guessing would guess otherwise.

## 108.3 Applying one is a switch, and both hops are tail calls

An application compiles to a call into `beck.apply.<family>`, generated once per **family** — a
family being a set of param reprs and a return repr, interned by those rather than by the type as
written, because an effect row is not part of a machine shape. `(Int) -> Int` and `(Int) -> Int ! io`
are one family, and a lambda inferred at one of them has to be applicable at a site that says the
other or the switch would be missing the arm for the closure standing in front of it.

The switch has one arm per rank of the family **that became code**: a rank whose definition was
refused has no function to call, and an arm calling one would be a link error rather than a refusal.
Two kinds of arm:

- a `lam` has its own `beck.lam.<rank>`, which takes the closure so it can read its captures;
- a **definition named as a value** — `map_list(xs, double)` — is called directly, with no closure
  operand, because a definition closes over nothing and a wrapper would be a second copy of a body
  that already exists.

Both hops are tail calls: the call into the application (`musttail` under `tailcc`, `return_call` in
Cranelift) and the arm inside it. [`27`](27-the-walls-come-down-report.md) makes "a call in tail
position is free" a property of the *language*, and an application is a call — so a loop written as a
closure calling itself must not grow the stack. §108.7 is the gate, and it was checked by making it
red.

The default arm traps with `Trap::NoSuchLambda` rather than being an LLVM `unreachable`. It is
unreachable for a stronger reason than `Trap::NoMatchData`'s — this module built the closure, so its
rank came from the same table the switch was written from — and it is a trap for the same reason:
`unreachable` is the optimiser's licence to do anything at all with the path a compiler bug reached.
The span index it stores is past the end of the table on purpose, because there is no source position
for a wrong rank and the host reads an index it cannot find as `Span::NONE`.

## 108.4 Five loops, and what each one costs

The higher-order list primitives are generated per family rather than written inline, for the reason
a list's comparison is: the emitters' own output is straight-line, and a loop inline would be the
first place either needed `phi` nodes of its own.

| Primitive | The loop | What it allocates |
|---|---|---|
| `map_list` | one answer per element | the list, allocated up front from the input's length |
| `filter_list` | the answers that are `true` | a list with room for **every** element; the header is written at the end |
| `list_fold` | the accumulator through every element | nothing |
| `list_all`, `list_any` | one loop and a flag | nothing |

`filter_list` is the one with a decision in it. The arena needs a size before it has elements, and a
filter's size is not known until the predicate has run — so either the predicate runs twice per
element (count, then fill) or the list is allocated for all of them and the count is written
afterwards. It is the second: **one pass**, and the words past what was kept are arena nobody reads,
bounded by the input's length and given back when the arena is reset. A predicate called twice would
be a cost the evaluator does not have, to save memory the next allocation does not need.

`list_all` and `list_any` are one function and a flag, which is how `beck-eval` writes them too —
and they **short-circuit**, which that file documents as a promise rather than an optimisation.

Every call into the application is followed by a look at the error cell. A closure can trap, and a
loop that carried on would run the remaining iterations of a program that has already failed.

`sort_by` and `list_flat_map` are refused, each with what it needs rather than a shrug:
`list_flat_map` answers a list whose length is the sum of the lists its function answers, which is
growing a list under another name; `sort_by` decorates each element with its key and merges
**stably** — `beck-eval` is explicit that stability is what makes the order total without a second key
— so it is a sort written in two emitters rather than a pass over a list. It is the next one to
build, not one that cannot be.

## 108.5 What compiles now, and what the refusals say instead

| | Before | After |
|---|---|---|
| Definitions compiled across the tree | 605 | **619** |
| Refusals | 785 | 771 |
| Refusals blaming a closure | 96 | **52**, all at a boundary |
| Refusals blaming `sort_by` | 0 | 14 |

Reproduced with `beck native <file> --backend cranelift` over `corpus/`, `lib/`, `awfy/`, `clbg/`,
`examples/` and `sicp/`, counting the two lists the report prints.

The eleven definitions that moved from refused to compiled are `awfy/bounce.beck`'s `step`,
`clbg/knucleotide.beck`'s `entries` and `frequency_lines`, `corpus/04-kanban.beck`'s `in_column`,
`corpus/05-poll.beck`'s `tally`, `corpus/06-inventory.beck`'s `low`,
`corpus/28-catalogue.beck`'s `all_stocked`, `corpus/31-tenants.beck`'s `mine`,
`examples/documented.beck`'s `chargeable`, `examples/todo.beck`'s `remaining` and
`sicp/felleisen.beck`'s `account`. Three more compile because they called one of those eleven.

The 52 that remain are the boundary, and they are mostly one shape: a definition whose *parameter* is
a function — `lib/collections.beck` is built out of them. That is the design's line rather than an
oversight, and §108.9 is what would have to change to move it.

## 108.6 What it costs against the tree-walker

`cargo test --release --test measure_native -- --nocapture what_a_closure_costs`:

| benchmark | size | evaluator | native | ratio |
|---|---|---|---|---|
| `apply_often` | 20,000 | 15.81 ms | 60.2 µs | **262.7×** |
| | 160,000 | 126.48 ms | 207.7 µs | **609.1×** |
| `mapped` | 2,000 | 350.4 µs | 93.3 µs | 3.75× |
| | 16,000 | 3.06 ms | 395.3 µs | 7.74× |
| `folded` | 2,000 | 379.2 µs | 76.3 µs | 4.97× |
| | 16,000 | 3.32 ms | 303.0 µs | 10.95× |

`apply_often` is the row about closures: a loop whose work *is* building and applying one. The
evaluator makes a `Value::Closure` per `lam` evaluated and pushes an environment frame per
application; here it is one arena object of one word and a switch into a direct call.

**Every ratio here grows with the size, and that is not the loop getting better** — it is
[`93`](93-llvm-backend-report.md) §93.1's pipe. A call costs a fixed round trip through a child
process (36 µs when that report measured it), which at 20,000 applications is most of the native
column's 60 µs and at 160,000 is a seventh of 208 µs. The two list rows are worse off for a second
reason: 16,000 elements is 128 KB of arena crossing the pipe in each direction, so `mapped` and
`folded` measure the marshalling at least as much as the loop. **The honest claim from this table is
the ratios at the larger size, and only as a lower bound.**

## 108.7 The gates

- **`native.rs::the_two_backends_agree_on_closures`** and
  **`cranelift.rs::the_three_backends_agree_on_closures`** — **1,108 calls each**, over a program
  written so that each thing that can be wrong separately is wrong separately: a closure with no
  captures beside one with two, a definition named as a value, two closures of one family so the
  switch has more than one arm, one applied twice, a closure inside a closure capturing from both
  levels, an element that is an offset, reals (so a word becomes a `double` on the way in and is
  normalised on the way out), the empty list, a closure that **traps**, and the four comparisons.
- **`native.rs::a_loop_costs_its_answer_and_one_closure`** — a shape gate with no clock in it: a fold
  that builds nothing costs **one closure and its answer, the same bytes at 200 elements and at
  1,600**, and a map costs the list it answers with and one closure and nothing else. A loop that
  allocated per iteration fails here and answers correctly everywhere.
- **`native.rs::a_tail_call_through_a_closure_costs_nothing`** and its Cranelift twin — ten million
  applications in tail position. **Checked by making it red**: with the application's own call site
  emitted as an ordinary call, it answers `SIGSEGV` at that size. The arm *inside* the application is
  the hop `tailcc` would have got right on its own, which is why it is `musttail` — enforced by the
  assembler rather than hoped for ([`93`](93-llvm-backend-report.md) §93.4).
- **`native.rs::a_closure_does_not_cross_the_boundary`** — the five boundaries, each refused by name
  and with its reason, and a control that compiles so the test cannot pass against a backend that
  refused everything.
- **`cranelift.rs::the_two_emitters_accept_and_refuse_the_same_definitions`** — with the two closure
  programs added to its list, so the subset stays one subset. The two lower an application
  differently: one writes a `switch` and the other a chain of comparisons.
- **`measure_native.rs::what_a_closure_costs_against_the_tree_walker`** (release-only) — §108.6's
  table, asserting only that each shape is faster than the tree-walker at both sizes. A threshold on
  a ratio dominated by a round trip would be a gate on this machine's pipe.

## 108.8 The finding: a gate that fired, on the reason rather than the answer

[`106`](106-lists-arrive-read-only-report.md) §106.7 found a refusal whose stated reason was false and
built a gate for the class:
`native.rs::a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one`. Every type a refusal
blames for having no layout is asked whether it has one, and the refusal is deleted if it does.

That gate went red here, and it was right to. `Heap::repr` used to answer "a function value, which is
a closure" for a function type — a refusal about the *layout* — and after this change a closure has a
layout. What is true now is narrower and needed writing in two parts: the shape **exists**, and the
**boundary** is what refuses it. So the gate now asserts both — `repr` answers, `Heap::crossing`
refuses — and the reason a definition sees names the boundary rather than the layout.

This is worth more than the feature's own tests, because it is the second time this gate has changed
a *sentence the compiler tells a reader* rather than a behaviour. A refusal is a claim, and the only
thing that keeps a claim true when the code under it moves is a test that asks whether it is still
so.

Two smaller things came out of the same commit:

- **`what_the_heap_does_not_reach_is_refused_by_name` lost a row.** That test's own documentation says
  each row "goes red the day its row starts compiling, which is the day the row should be deleted" —
  and `mapped`, a definition that is `map_list(xs, double_it)`, started compiling. It moved from the
  refusal list to the control list in the same commit, which is the third row that has made that
  journey after `docs/105`'s and `docs/106`'s.
- **`free_vars` was written twice about to be written a third time.** `beck_core::plan` had a private
  copy for deciding what a dataflow operator has to be handed; a closure needs the same answer for
  deciding what an object has to carry. It is `beck_core::core::free_vars` now, public, with the two
  callers named in its documentation — because a second walk that disagreed about one construct would
  give a compiled closure a field the evaluator's environment does not have.

## 108.9 What this does not establish

- **Nothing about a closure crossing the boundary**, because none can. Every case in the
  differential is a definition that builds a closure, applies it, and answers with something the
  host can read. That is the whole of what a program can *observe* about a closure anyway, but it
  means the marshalling half of this feature is not tested — it does not exist.
- **Nothing about `sort_by` or `list_flat_map`**, which are refused, and nothing about a definition
  whose parameter is a function, which is refused.
- **Nothing about `Html`**, which is the next row of Lane E and needs a tree of children rather than
  a closure ([`105`](105-text-on-the-heap-report.md) §105.10).
- **Nothing about how many arms an application can afford.** Every family in this tree has one or
  two, so the difference between a chain of comparisons and a jump table has not been measured and
  is not claimed either way.
- **Nothing about the effects**, which are the other unbuilt row and are not a closure's problem.

## 108.10 What this corrects

- [`101`](101-the-heap-report.md) §101.5's table of what the heap does not reach has a closure row,
  and [`105`](105-text-on-the-heap-report.md) §105.10, [`106`](106-lists-arrive-read-only-report.md)
  §106.8 and [`107`](107-a-map-arrives-read-only-report.md) §107.7 each carry "closures" in the list
  of what is left. All four stand as history; this is where the correction lives.
- **`Heap::repr` no longer refuses a function type**, and any reading of those reports that took "a
  closure has no layout" as a permanent fact is wrong twice over — it has one, and what refuses it is
  `Heap::crossing`. §108.8 is the gate that insisted on the distinction.
- [`08`](08-roadmap.md) §8.5.4's Wave 4 paragraph and §8.5.5's Lane E row both listed four things;
  they list three now.
- `beck_llvm::heap`'s own module documentation said "a `list`, a `Map`, a closure, `Html` and `Unit`
  are refused". Two of those five had already acquired layouts in
  [`106`](106-lists-arrive-read-only-report.md) and [`107`](107-a-map-arrives-read-only-report.md)
  without that sentence being edited, which is a documentation drift this commit fixes along with its
  own row: what is refused there is `Html` and `Unit`.

## 108.11 What would have to change to let a closure cross

The refusal is not a gap in an emitter: it is what the host would have to do with one. Decoding a
reply into a `beck_core::Value::Closure` means producing a `params`, a `body`, an `env` and a frame
size — an evaluator's closure, out of bytes. The module knows all four (a rank names a lambda, and
the lambda's captures are a table), so it is *possible*; what it costs is the host learning the
lambda table and a second place where "what a closure is" is written down.

The thing to notice is that it buys almost nothing on its own. A definition whose parameter is a
function is refused at the *signature*, and the caller of one is a compiled definition that already
has the closure in the arena — so the case that matters is a compiled definition calling another
compiled definition through a function parameter, and that needs no marshalling at all. It needs the
signature rule relaxed for a **call between compiled definitions** while the worker's protocol keeps
refusing one, which is a distinction the `Signature` type does not currently make. That is a
different change from this one and a smaller one than it looks.
