# ADR 0026 — The native heap is an arena of offsets, and the host marshals against it

**Status:** accepted
**Date:** 2026-08-10
**Context:** [`101`](../101-the-heap-report.md), [`93`](../93-llvm-backend-report.md) §93.6,
[`97`](../97-cranelift-report.md) §97.7, [`05`](../05-tier-lowering.md) §5.2,
[`43`](../43-threat-model.md), [`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md),
[`0024`](0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md)

## The decision

A value that does not fit in a register lives in one contiguous **arena**, and a reference to it is
a **byte offset into that arena** rather than a pointer. Three consequences follow, and they are the
whole of the design:

1. **Allocation is a bump pointer.** One `malloc` at startup, an offset that only goes up, and no
   free. The arena is reset to the end of the call's arguments before every call.
2. **An object graph is position-independent.** Nothing in it points anywhere, so the host can build
   one as a flat byte string and the worker can adopt it with a copy — and the worker can hand one
   back the same way.
3. **Marshalling is written once, in Rust, on the host.** `beck_llvm::heap` holds the layouts, the
   encoder and the decoder; neither emitter generates a line of code for getting a value across the
   pipe.

The layout itself — the tag in word 0, one word per field, fields in name order, a variant's tag
being its rank **by name** — is also decided once, in that same module, and is the one thing the two
emitters share.

## Why offsets rather than pointers

The pipe is the reason. [`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md) put the
compiled program in a **different process**, so a `Value` has to cross a byte stream in both
directions. With pointers, that crossing needs a type-directed walk in the worker to relocate
everything on the way in and to serialise everything on the way out — which means generating that
walk, in LLVM IR text *and* in Cranelift IR, for every layout in the program. With offsets there is
nothing to relocate: the bytes mean the same thing at either end.

That removed the largest piece of work in this change and, more usefully, removed a place for the
two emitters to disagree. What is left in each emitter is what each emitter is for — building an
object, reading a field, testing a tag, comparing two values.

## Why the layout is shared and the emitters are not

[`97`](../97-cranelift-report.md) §97.3 established the rule: the *subset* is written twice on
purpose, because two emitters that agreed by construction would make the agreement gate worth
nothing. A layout is the opposite kind of thing. It is a **contract between three parties** — the
two emitters and the host that writes a value into it — and a contract with three spellings is the
drift this project spends its gates on. The precedent is already there: `Trap`'s codes have been one
table since [`93`](../93-llvm-backend-report.md), read by both emitters and decoded by the host.

The line between the two is: *what a program means* is written once; *how one backend says it* is
written twice.

## What it costs

- **Memory is not reclaimed within a call.** A loop that allocates a million objects holds a million
  objects, whether or not the program can still reach them. The bound is
  `beck_llvm::heap::ARENA_BYTES` — 256 MiB — and exceeding it is `Trap::HeapExhausted`, which is a
  message with a span rather than a fault. The evaluator has no such limit, and that difference is
  written down in [`101`](../101-the-heap-report.md) §101.6 rather than argued away.
- **A reply carries the whole used arena, not the value.** The worker cannot tell which objects the
  answer can reach without a walk it has no code for, so a call that answers with an object sends
  back everything it allocated. A call that answers with a scalar sends nothing.
- **A record is rebuilt where the evaluator would mutate it.** `docs/70`'s last-use analysis lets the
  tree-walker rebuild a record in place when nobody else holds it; an arena with no ownership in it
  cannot prove that. `p.with(x = …)` is a fresh object here, always.
- **A load of the arena's base at every access.** One backend hoists it to the entry block and the
  other marks it readonly and lets the alias analysis fold it; neither is free.

## What it buys

- **The heap without a collector, and therefore without a collector's design.** A tracing collector
  needs a root set, a map of what is a pointer, and a decision about when it runs — three designs
  this backend has no measurement to choose between. A bump pointer needs none of them and is
  strictly what a bounded call needs.
- **No generated marshalling.** Which is the difference between this being one change and two.
- **A fixed reservation nothing can invalidate.** A value is an offset, so an arena that *moved*
  would be safe — but a fixed one is one fewer invariant to hold, and 256 MiB of untouched
  reservation costs nothing on any system this runs on.

## What was considered and refused

**Reference counting.** It would make `with` rebuild in place and make the reply carry only what is
reachable. It also puts a count in every object, an increment on every field read, and a decrement
on every path out of every function — and a compiled backend that is slower than the tree-walker
because of bookkeeping the tree-walker does with `Arc` for free would be a worse answer than none.
Nothing here has been measured yet ([`101`](../101-the-heap-report.md) §101.7), and this is a
decision to take *after* a measurement rather than instead of one.

**A uniform boxed value, with `Int` on the heap too.** It makes one representation for everything
and one comparison function for the whole language. It also allocates on every arithmetic operation
and would have thrown away every number [`93`](../93-llvm-backend-report.md) §93.5 measured. The
representation is static and by type: an `Int` is an `i64`, as it was.

**Generating the marshalling in each emitter.** The obvious design, and what pointers would have
forced. See above: offsets make it unnecessary, and unnecessary code in two IRs is two places for a
layout to be wrong.

**A `Str` and a `list` in the same change.** They fit this arena — a length and then the bytes — and
they are not here, because what makes them hard is not the layout ([`101`](../101-the-heap-report.md)
§101.5): a string is a character index and an ASCII flag that has to answer exactly what
`beck_core::Text` answers, and a list is `list_append`, which is `docs/70`'s in-place push and
therefore the ownership question this ADR just deferred.
