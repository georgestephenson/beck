# 101 — Phase 3, part 69: the heap, and a value with fields

**Built, and the codegen bullet has its first floor.** A `model`, a `union` and a `newtype` now
compile — to both code generators, over the same layout, held to the evaluator by the three-way
differential [`97`](97-cranelift-report.md) built. That is the item
[`93`](93-llvm-backend-report.md) §93.6 and [`97`](97-cranelift-report.md) §97.7 each named as the
single largest thing between the native backends and §5.2:

> **The heap, which is what bounds both code generators.** No record, list, string, map, union or
> closure compiles, and no effect does: there is no allocator and no collector behind either
> emitter.

Six of those seven are still true, and §101.5 says so first and with tests attached. What changed is
the two that carry every other one: **there is an allocator**, and there is a way for a value that
does not fit in a register to cross the pipe.

The interesting decision is not the arena. It is that a reference is an **offset** rather than a
pointer, which makes an object graph a flat byte string — so the host marshals a `Value` in Rust,
once, and neither emitter generates a line of code for getting one across.
[`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md) is that decision with its costs.

## 101.1 The shape

One `malloc` at startup, a bump pointer, and no free. Every object is a whole number of 8-byte
words:

| Word | What |
|---|---|
| 0 | the **tag** — which variant, and `0` for a record or a newtype |
| 1.. | one word per field, in the order [`Fields`](../compiler/crates/beck-core/src/core.rs) keeps them, which is **by name** |

A field word holds an `i64`, an IEEE `double`, a `Bool` as `0` or `1`, or the **offset** of another
object. Offset `0` is reserved, so no live object has it and an allocation that trapped can return
it. A scalar is what it was: an `Int` is an `i64` in a register, and
[`93`](93-llvm-backend-report.md) §93.5's numbers are still about the same code.

```text
    Segment(from=Point(x=1, y=2), to=Point(x=3, y=4))

    offset   0 ┌──────────────┐   reserved
             8 │ 0            │   Point   tag
            16 │ 1            │           x
            24 │ 2            │           y
            32 │ 0            │   Point   tag
            40 │ 3            │           x
            48 │ 4            │           y
            56 │ 0            │   Segment tag
            64 │ 8            │           from   ── an offset, not a pointer
            72 │ 32           │           to
               └──────────────┘
```

The protocol grew two fields: a request says how many bytes of heap follow its arguments, and a
reply says how many follow its answer. Both are zero for a call whose arguments and result are all
scalars, which is every call a program of arithmetic makes — and a module with no object in it gets
**no `malloc`, no globals and no allocator**, which `a_program_with_no_object_has_no_arena` asserts
on both backends rather than leaving to be believed.

The arena is reset to just past the arguments before every call. So the bound is per call, not per
process, and what a program can do inside one call is bounded by 256 MiB —
`Trap::HeapExhausted`, which is a message with a span and not a `SIGSEGV`.
`running_out_of_heap_is_a_diagnostic` is the gate, and it fires at eight million nodes.

## 101.2 What compiles now that did not

`beck native <file>` over the corpus, the examples, both SICP chapters, Are We Fast Yet, the
Benchmarks Game, the standard library and `xlang/`, against the same command on the commit before
this one:

| | before | after |
|---|---:|---:|
| programs with at least one compiled definition | 25 | **30** |
| definitions compiled | 187 | **283** |

Where it landed is the more useful half, because it is not spread evenly:

| | before | after | why |
|---|---:|---:|---|
| [`lib/dates.beck`](../compiler/lib/dates.beck) | 13 | **35** | a civil calendar is `Date` and `Time` records, and almost every function takes one |
| [`awfy/cd.beck`](../compiler/awfy/cd.beck) | 14 | **30** | vectors and motions, passed by value |
| [`sicp/ch2.beck`](../compiler/sicp/ch2.beck) | 6 | **17** | §2.1's rationals and §2.2's trees |
| [`awfy/deltablue.beck`](../compiler/awfy/deltablue.beck) | 12 | **17** | |
| [`awfy/nbody.beck`](../compiler/awfy/nbody.beck) | 4 | **11** | |
| [`awfy/bounce.beck`](../compiler/awfy/bounce.beck) | 1 | **7** | |
| [`awfy/list.beck`](../compiler/awfy/list.beck) | 0 | **6** | a linked list written as a `union` is a heap this backend has |
| [`corpus/`](../compiler/corpus/) — 32 programs | 0 | **4** | and §101.6 is why it is four |

[`97`](97-cranelift-report.md) §97.7's "all 32 corpus programs compile **nothing at all** between
them" is no longer true and is barely less damning: three programs contribute four definitions
between them, because a corpus program is a fold and a view, and a view builds `Html` out of `Str`.
The number that matters is not this one.

## 101.3 One layout, two emitters

[`97`](97-cranelift-report.md) §97.3 set the rule that the *subset* is written twice on purpose:
two emitters that agreed by construction would make the agreement gate worth nothing, and §97.4 is
what writing it twice found within a minute. This change had to decide whether a **layout** is that
kind of thing, and it is not.

A layout is a contract between **three** parties — the LLVM emitter, the Cranelift emitter, and the
host that writes a `Value` into it and reads one back. Two of those three are not emitters. Three
spellings of one contract is the drift this project spends its gates on, and the precedent was
already in the same crate: `Trap`'s codes have been one table since
[`93`](93-llvm-backend-report.md), written by both emitters and decoded by the host.

So `beck_llvm::heap` holds the layouts, the tags, the field order, the encoder and the decoder, and
the line is: **what a program means is written once; how one backend says it is written twice.**
`the_two_emitters_accept_and_refuse_the_same_definitions` runs over the two new fixtures as well,
so the two are still held to one subset.

What that bought is worth naming: because a heap value is an offset, **neither emitter generates
marshalling**. With pointers, the worker would have to relocate an incoming graph and serialise an
outgoing one, which is a type-directed walk emitted in LLVM IR text *and* in Cranelift IR for every
layout in the program. That was the largest piece of work in this change until it wasn't.

## 101.4 Agreeing with the evaluator, and the three ways a layout goes quietly wrong

The differential is the point, as it was in [`93`](93-llvm-backend-report.md): both backends are
called on the same arguments and the **whole outcome** is compared — the value, or the failure and
its message.

| | calls compared |
|---|---:|
| records, evaluator against LLVM | 429 |
| unions, evaluator against LLVM | 183 |
| records, three-way (evaluator, LLVM, Cranelift) | 429 |
| unions, three-way | 181 |

A wrong answer is the easy case. Three of the fixtures exist because a layout can be wrong in a way
that still looks plausible, and each of them makes one specific mistake visible:

**A tag is a variant's rank by name, not by declaration.** `Value`'s derived `Ord` compares a
record's type, then its variant **name**, then its fields — so `Big` sorts below `Small` whatever
order the `union` wrote them in. `Ranked` is declared `Small, Big, Nothing` on purpose, and a
backend that numbered variants as they were declared answers `<` backwards on it.

**A field's slot is its rank by name too.** `Key` is declared `score, name`, and
[`50`](50-collections-and-dates-report.md) §50.6 established that a record's *value* order is by
field name — so `name` decides before `score`. A layout in declaration order answers the opposite,
on a program that reads perfectly.

**A real on the heap has to be the one the evaluator would have built.** `Value::float`
canonicalises `-0.0` to `0.0` and every NaN to one NaN, so a real is **normalised on the way into a
field** — which is the one place [`93`](93-llvm-backend-report.md) §93.2's invariant has to be paid
for rather than argued away. `negated` makes a negative zero *on the heap* rather than receiving one
the host already canonicalised, which is §93.7's lesson applied a second time: a differential over a
boundary that normalises tests the boundary.

Both order rules are also pinned by an explicit assertion
(`a_layout_is_ordered_by_name_and_not_by_declaration`) rather than only by the differential, because
a differential compares two things against an **oracle** and cannot see the day the oracle moves.

## 101.5 What is not built, and the tests that say so

Six of the seven things §97.7 listed are still true, and a list in prose goes stale where a list
with a test attached cannot ([`83`](83-the-runtime-edge-report.md) §83.7):

| Not built | Why it is not a layout problem |
|---|---|
| `Str` | The layout is easy — a length and the bytes. What is hard is that `beck_core::Text` carries a **character** count, an ASCII flag and a chunked character index ([`71`](71-strings-report.md), [`72`](72-space-and-constants-report.md)), and `str_len` and `str_slice` have to answer exactly what they answer |
| `list[T]` | The layout is a count and the elements. What is hard is `list_append`, which is [`70`](70-last-use-moves-report.md)'s **in-place push when nobody else holds it** — and an arena with no ownership in it cannot prove that. Building a list in a loop would be `O(n²)` here where the evaluator is `O(n)`, which is [`69`](69-standard-library-imports-report.md) §69.7 reintroduced in a new place |
| `Map[K, V]` | A `PMap` is a weight-balanced tree with structural sharing ([`pmap`](../compiler/crates/beck-core/src/pmap.rs)); the same ownership question, one level up |
| a closure | Needs a code pointer and a captured environment, and therefore an indirect call — which is the first thing in this backend that is not a direct call to a name |
| `Html`, `Attr`, `Unit` | Follow text and collections |
| every effect | Needs the host, which is on the other side of a pipe that carries values and not calls |

`what_the_heap_does_not_reach_is_refused_by_name` asserts each of them, by the reason the refusal
gives, and asserts the control: a program where everything is refused would pass a list of refusals
without meaning anything, so `scalar_and_fine` has to still compile.

`beck native --call` cannot pass a record either, and refuses rather than guessing: a notation for
`Point(x=1, y=2)` on a command line is the language's own literal syntax written a second time in a
worse place.

## 101.6 What it costs, said plainly

**Memory is not reclaimed inside a call.** There is no collector. A loop that allocates a million
objects holds a million objects whether or not the program can still reach them, and the ceiling is
256 MiB. That is a real difference from the evaluator, which frees what an `Arc` drops.

**A reply carries the used arena, not the value.** The worker cannot tell which objects the answer
reaches without a walk it has no code for, so a call answering with an object sends back everything
it allocated. A call answering with a scalar sends nothing at all.

**`with` always builds a fresh object.** [`70`](70-last-use-moves-report.md)'s analysis lets the
tree-walker rebuild a record in place when the base is a last use and nobody else holds it, and
[`87`](87-the-chapter-that-argues-back-report.md) §87.5 made that hold for `x.with(f = g(x.f))` too.
Neither is available here, and the answers are identical — it is a cost, not a divergence.

**A value the host decodes is bounded at 2,048 deep**, which the evaluator is not. It is what stops
a compiler bug from becoming a blown host stack, and it is a limit on what a compiled definition may
*answer* with.

**The arena's base is loaded per access on one backend and hoisted on the other.** The LLVM emitter
writes text and can insert a line into the entry block after the fact; the Cranelift one marks the
load `readonly` — which is true, because `main` writes it before any compiled code runs — and lets
the alias analysis fold the repeats. Two answers to one question, and only the first is measured.

**The corpus still compiles four definitions.** A `model` having a layout does not make a fold
compile: a fold's state is a `Map`, its view is `Html`, and its commands carry `Str`.

## 101.7 The numbers

`cargo test --release --test measure_native -- --nocapture --test-threads=1`, Ubuntu clang 18.1.3,
`-O2`, median of seven runs at the small size and three at the large one. Two sizes each, per
`AGENTS.md`: one measurement cannot tell a constant from a slope. Every ratio includes the pipe
round trip, measured at **35.6 µs** in the same run.

None of these is asserted, and the reason is worth writing down because the sibling benchmark's
ratio *is*: the native side is `clang -O2` whatever profile `cargo` was run in and the evaluator is
not, so the same code answers 2,460× under `cargo test` and 120× under `cargo test --release`. A
gate on that number would be a gate on which profile ran it, which is
[`13`](13-testing.md) §13.7's "a gate that flakes gets deleted" arriving before the flake did.

| benchmark | what it does | size | evaluator | native | ratio |
|---|---|---:|---:|---:|---:|
| `build_and_sum` | allocates a spine of `Node`s and walks it | 10,000 | 11.91 ms | 193.4 µs | 61.6× |
| | | 100,000 | 136.55 ms | 1.367 ms | **99.9×** |
| `folded` | `with` in a loop, which is a fold's shape | 10,000 | 7.30 ms | 60.8 µs | 119.9× |
| | | 100,000 | 71.20 ms | 448.2 µs | **158.8×** |
| `scan` | the control: same loop, reads a field, allocates nothing | 10,000 | 4.37 ms | 48.4 µs | 90.2× |
| | | 100,000 | 43.74 ms | 168.4 µs | 259.8× |

Net of the round trip, an allocate-a-`Node`-and-a-`Leaf`-and-walk-them step is about **13 ns** and
the control's step is about **1.3 ns**. So the arena costs roughly ten times a bare loop step and is
still about a hundred times the tree-walker — which is the shape to expect and the reason the
control is in the table: without it, the first two rows would read as though compiling the *loop*
were the win.

What is **asserted** rather than printed is the shape, and there is a second gate with **no clock in
it at all**. `chain(n)` builds exactly `n` `Node`s and `n` `Leaf`s, so the arena it leaves behind is
a known number of bytes:

```text
chain(100) used 4024 bytes and chain(800) used 32024 — 40 bytes an element at both sizes
```

A layout that grew, an allocator that rounded, or a `with` that copied a spine would all show there,
and none of them needs the program to be timed. It is [`64`](64-compile-speed-report.md) §64.1's
pattern — gate the shape, print the rate — applied to memory.

## 101.8 What it found

**A `match` arm's guard has been carrying unresolved types since Phase 2.** `resolve_types` — the
pass that substitutes the solver's answers back into every `Core` node — walks `&mut a.body` for a
match arm and not `a.exprs_mut()`, so **every node inside a guard kept whatever type variable it had
when it was lowered**. Nothing had noticed, because nothing downstream read a node's type from
inside a guard; this backend does, and the symptom was a definition refused with
"``Option[?1]``, whose type is not known here" for a guard that was perfectly ordinary. It is
[`91`](91-guards-and-alternatives-report.md) §91.3 one walk further on — that report found fourteen
sites that walk an `Arm` and taught them all about guards, and this is the fifteenth, in a pass
neither of us thought to look at because it is not about liveness or slots or effects. Fixed, with
the reason on the line.

**A pattern test cannot be a conjunction, and that is a memory-safety requirement.** The scalar
emitters computed "does this pattern match?" as one boolean and branched once — which is right when
a pattern takes nothing apart. `Some(Circle(r))` does: it has to read a field, and reading the field
of a variant that is *not* present gives a word that means something else. If that word is an `Int`
it is a perfectly good `i64` being used as an offset, and following it leaves the arena. So a
pattern is now **control flow**: each test branches, and a field is read only in a block the tag
check dominates. §101.4's differential would not have found this — it is not a wrong answer, it is a
segfault waiting for the right `Int`.

**Two alternatives that bind the same name to different words need two arms.** `Circle(r) |
Square(r)` binds one `r` from two different objects, so a single block reached from both needs a
`phi` per binder — and the binders are discovered while emitting the test that produced them.
Rather than build that, an arm whose pattern takes a value apart is **emitted once per
alternative**, which is the same behaviour with no join to get wrong and a bound (16) on the
copying. An or-pattern of plain constants is still one `or` and one branch, so `case 1 | 2 | 3:`
emits what it always did.

## 101.9 What this corrects, elsewhere

| Where | What |
|---|---|
| [`93`](93-llvm-backend-report.md) §93.6, [`97`](97-cranelift-report.md) §97.7 | "there is no heap" — there is one, for records, unions and newtypes. Every other row of both tables stands, and §101.5 restates them with tests |
| [`97`](97-cranelift-report.md) §97.7 | "all 32 corpus programs compile **nothing at all**" — three of them compile four definitions. The sentence's point survives, and §101.6 says why |
| [`93`](93-llvm-backend-report.md) §93.3 | "59 programs assemble, 136 definitions compile" was a measurement of that day over a different file set; §101.2 is this day's, with the command |
| [`08`](08-roadmap.md) §8.5.4, §8.6 | The codegen bullet is no longer "both code generators and no heap". It is both code generators, a heap for the algebraic core, and no text, collections, closures or effects |
| [`05`](05-tier-lowering.md) §5.2 | "compiles to native binaries, one per `service`" is still not true and is closer: what stands between is §101.5's table rather than the whole idea of a heap |
| [`91`](91-guards-and-alternatives-report.md) §91.3 | Fourteen walks over an `Arm` were taught about guards. There was a fifteenth, and §101.8 is it |

## 101.10 What this establishes

That a value with fields compiles, on both code generators, to the same answers the tree-walker
gives — over 1,222 differential calls whose oracle is the evaluator and whose layout is checked
against the two orders (`Ord` on variants, `Ord` on fields) that a plausible-looking wrong layout
gets backwards. And that the way to get a value across a process boundary without generating code
for it is to stop it containing pointers.

What it does **not** establish is the sentence [`05`](05-tier-lowering.md) §5.2 wants. A record is
the first heap value and not the interesting one; a Beck program's state is a `Map` and its output
is `Html`, and both of those are text and collections, and text and collections are where
[`70`](70-last-use-moves-report.md)'s ownership question comes back. §101.5's second row is the next
piece of work and it is a *design* question rather than a layout: an arena that cannot prove
uniqueness makes `list_append` quadratic, and shipping that would be
[`69`](69-standard-library-imports-report.md) §69.7 rebuilt on purpose. Reference counting, a
uniqueness analysis carried from `beck_core::liveness`, or a persistent sequence — three answers,
none of them measured here, and [`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md)
deliberately leaves the choice to whoever measures first.

Mode B codegen is not closer than [`97`](97-cranelift-report.md) §97.7 said, and for the reason it
said: a page is `Html` and `Str` all the way down, and this heap holds neither yet.
