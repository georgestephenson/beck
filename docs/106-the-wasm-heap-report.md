# 106 — The heap on the third target

**Built.** A value representation in linear memory, text and the collections, closures through an
indirect call table, and the whole of it held to the tree-walker by a differential of
**22,714 calls run in a real WebAssembly engine** — where the scalar half
([`103`](103-the-wasm-emitter-report.md)) ran 12,852.

The number that matters is the other one. `beck-wasmgen` compiled **0 of the corpus's 237**
definitions; it now compiles **212<!--c:wasm-compiled--> against 25<!--c:wasm-refused--> refused**,
which is [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md)'s "what would reverse it"
arriving as a measurement. It is not yet reversed: **nothing loads the module**. `beck-wasm`'s
kernel still interprets, the bundle format is unchanged, and the four host effects have no import to
ask through. Those are §106.8, and they are what is left.

## 106.1 The decision §5.1 left open, and why it is neither of its two answers

[`05`](05-tier-lowering.md) §5.1 asks for "the component's pure code compiled to WASM (**GC proposal
where available; Perceus-style refcounting fallback**)".
[`adr/0032`](adr/0032-the-webassembly-heap-is-the-arena-in-linear-memory.md) takes neither. A value
that does not fit in a register is a **byte offset into the module's own linear memory**, laid out
by [`beck_llvm::heap`] — the same arena
[`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md) already decided for the two native
backends, with the same headers, the same field order, the same tags and the same `0`-means-nothing
rule.

The argument against the GC proposal is not availability — it has been in Chrome and Firefox since
late 2023, which is the bar
[`adr/0030`](adr/0030-the-webassembly-emitter-writes-its-own-bytes.md) applied to `return_call` and
passed. It is what a `ref` costs at the two boundaries this backend exists to cross:

- **It re-generates the marshalling `adr/0026` removed.** A GC value is a `(ref $Point)` and nothing
  outside the module can read one — no `DataView`, no `Uint8Array`, no `Heap::decode`. Every
  crossing would need an exported accessor per type and per field, generated into the module, and a
  host walking a type table to drive them. That is precisely the "type-directed walk… generated for
  every layout in the program" that decision names as the largest piece of work it avoided, and it
  would arrive here **larger**, because Mode B's host has to read a whole `Html` tree back and not
  merely one reply.
- **It forks the one layout.** [`beck_llvm::heap::Repr::machine`] answers `Scalar::Int` for every
  reference, which is what lets three emitters share one `Repr`. Under GC there is no `i64` to
  answer with, so `Repr` grows a fourth machine type and the shared module becomes a module with a
  target in it.

Refcounting loses for the reason `adr/0026` already gave and for one that is new: a count is a word
in **every object**, so an object built by a browser and an object built by a worker would be
different bytes for the same value, and the host's encoder would have to know which backend it was
talking to.

What the arena costs is named rather than argued away: nothing is reclaimed within a call, and
`p.with(x = …)` is always a fresh object. What makes that a bounded cost rather than a leak is Mode
B's own shape — a render is one call, and the kernel resets the bump pointer to the end of the
arguments before the next one, which is the classic per-frame arena.

## 106.2 The memory *is* the arena, and that is the whole marshalling layer

`Heap::encode_args` produces a byte string whose byte `i` is offset `i`. So the host writes that
blob at **address zero** of the module's memory, sets an exported bump-pointer global past it, and
calls; the answer comes back as the used prefix of the same memory, which is the blob `Heap::decode`
reads. Neither function is new and neither has a WebAssembly case in it.

That is the property this design was chosen for, and it is worth stating as a *measurement of the
diff*: the differential's driver is **eleven lines** of JavaScript that write bytes and read bytes
back, and it contains no notion of a record, a list or a tag. Every one of the 22,714 comparisons
goes through the same encoder the native host uses.

Two consequences follow that the native backends do not have:

- **The literal pool is a data segment.** The pool is a function of the program, so it belongs to
  the module rather than to every request. A host that writes a blob anyway writes the same bytes
  over it.
- **The memory grows.** `adr/0026` reserves 256 MiB up front and argues that untouched reservation
  is free; in a browser tab it is not, because a `WebAssembly.Memory` costs what it declares. So the
  module declares **one page** and `memory.grow`s under the bump pointer. The bound is unchanged —
  the declared maximum is `heap::ARENA_BYTES` exactly — so a program that runs out gets
  `Trap::HeapExhausted` and the message naming the same number, which
  `running_out_of_heap_is_a_message_and_not_a_fault` asserts by exhausting it.

## 106.3 A closure is a table, and that is the one thing not transcribed

[`93`](93-the-native-backends-report.md) §93.8's rule is that the *subset* is written twice on
purpose and the *layout* once. This work is mostly the first kind — a second implementation of
`map_insert`'s rebalance, of a stable merge sort, of a UTF-8 whitespace scan — and one place is
neither.

The two native backends compile an application to a **switch on the closure's rank** into a direct
call per rank, because their arena crosses a pipe as bytes and may therefore hold no code address.
A **table index is not a code address either**: it is an integer that means the same thing at both
ends of the same pipe. So the rank travels unchanged into one `funcref` table per closure family,
and applying a closure is:

```wat
local.get 0        ;; the closure
i32.wrap_i64
i64.load           ;; its rank, which heap::CLOSURE_HEADER put in word 0
;; …the guard…
return_call_indirect (type 3) (table 0)
```

`return_call_indirect`, so §93.4's guarantee survives the indirection: a loop written as a closure
calling itself is a jump, not a frame.

Three things fall out of it, and the third is the interesting one:

1. **A rank the family has no arm for is a null element**, and `ref.is_null` turns that into
   `Trap::NoSuchLambda` rather than an aborted instance — which is the property that trap was
   written for and the native backends' `switch` default gives it for a different reason.
2. **A definition named as a value needs an arm**, where the native backends need none. A table has
   one signature and a compiled definition does not take a closure, so `map_list(xs, double)`
   compiles a one-instruction thunk that drops the closure and jumps to `double`. That is a cost
   the switch did not have, and it is two bytes per definition-used-as-a-value.
3. **One table per family rather than one table**, because `call_indirect`'s runtime type check
   would otherwise be the thing that catches a wrong rank — and an engine's type-mismatch trap
   aborts the instance, which is exactly what §103.2 says a Beck failure must never be.

## 106.4 What was different about the target, and what was not

| | LLVM and Cranelift | WebAssembly |
|---|---|---|
| Where the arena is | a `malloc`'d buffer the host allocates | the module's **own** linear memory |
| How it is sized | reserved at 256 MiB | one page, grown under the bump pointer |
| A data pointer after an allocation | re-read, defensively | **stable**, because a memory only grows |
| Applying a closure | a switch on the rank | a `funcref` table and `return_call_indirect` |
| A byte comparison | `memcmp` | a loop, because there is no `memcmp` |
| A bulk copy | `memcpy` | `memory.copy`, which is the one bulk operation there is |
| The literal pool | written into every request | a **data segment** |

Rows three and seven are the two places this target is *simpler*. A WebAssembly memory never moves
what is already in it, so a data pointer taken before an allocation is still correct after one —
which removes a class of defect the native backends carry a comment about at four sites. And a pool
that belongs to the module is a pool nothing has to copy per call.

## 106.5 Four defects, each of which passes almost every case

The differential is the point, so what it caught is the report. Every one of these produced a
*plausible* answer over a wide input set and a wrong one over a narrow one.

- **`select` takes its first operand when the condition holds**, and eight sites had the operands
  the wrong way round. The failure is silent and local: `str_slice(s, 0, 1)` answered `""`, and
  `str_repeat` clamped to a million by taking the million. Nothing about the shape of the code says
  which way round it is, which is why the fixture that found it is a *sweep* over clamps rather than
  a case.
- **`i64.rem_s` on `i64::MIN`'s magnitude.** `str(n)` divides the magnitude out digit by digit, and
  the magnitude of `i64::MIN` is `2^63` — a value only unsigned arithmetic reads correctly.
  `i64.rem_s` renders its last digit as `(`. One input in the whole `Int` range is wrong, and it is
  the one `scalar::ints` puts first.
- **A block whose branches each answer a value needs a block type.** Two of the map's did not have
  one, and this is the cheap version of the mistake: the engine refuses the module rather than
  running it. §103.7 made the same observation about an opcode.
- **`i32.wrap_i64` applied twice**, which is also a validation error rather than a wrong answer.
  Both of these are the argument for a differential that *loads* what it emits, which
  [`adr/0030`](adr/0030-the-webassembly-emitter-writes-its-own-bytes.md) is the decision behind.

The fifth is not a defect but is worth the same sentence: `Repr::order`'s exhaustive match is copied
into this backend as `rt::cmp_helper`, so a new reference kind is a compile error **here** as well
as there. §93.8 recorded the same defect three times before that rule existed; this is the second
backend to inherit it rather than rediscover it.

## 106.6 What it compiles

| | Compiled | Refused | Was |
|---|---|---|---|
| [`corpus/`](../compiler/corpus/) — 39<!--c:corpus-programs--> applications | **212**<!--c:wasm-compiled--> | 25<!--c:wasm-refused--> | 0 and 237 |
| [`awfy/`](../compiler/awfy/) — Are We Fast Yet | 348 | 54 | 58 and 344 |

The ceiling is 219 and 18, which is what `beck-llvm` compiles over the same corpus. The gap is
seven definitions and every one of them is on the list in §106.8: three host effects, a `raise`,
and three definitions that call one of those.

## 106.7 The fixtures are not this suite's

`support/{heapfix,textfix,listfix,mapfix,clofix,viewfix,genfix}` are the programs `native.rs` and
`cranelift.rs` already point at, and `wasm_backend.rs` is the third caller. That is the same
argument `support/scalar.rs` was shared under, one subset up: a second copy of "what the heap subset
is" would be a second opinion, and the three suites would drift on exactly the cases nobody thought
to write twice.

It also means the layout traps those fixtures were built for are asked of this backend without
anybody re-deciding them: `Ranked`'s variants are declared out of alphabetical order and `Key`'s
fields are, so a backend that made either one a *declaration* index answers two definitions
backwards and nothing else.

## 106.8 What is not built

- **The four host effects.** `now`, `uuid`, `secret_env` and `http_fetch` are upcalls on the native
  backends; here they are imports the loader supplies, and nothing supplies them yet. Three corpus
  definitions are refused for this and three more for calling those three.
- **`raise` and `try`.** The native backends unwind through an error cell that is already an
  unwinder; here the trap globals are that cell, and what is missing is the lexical handler — a
  `block` a failure branches out of, which is a control-flow shape rather than a layout.
- **Bundle format 2 with the type table.** A compiling client backend needs the types the bundle
  erases, which [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) anticipated as "a version
  bump the format was built to take".
- **The kernel loading a compiled component.** `beck-wasm` still interprets `Core`, so `adr/0022` is
  **not reversed** — this is the half of it that runs, and nothing runs it.
- **The runtime library's fifteen primitives**, refused for the link line exactly as `sin` and `cos`
  are ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)): a
  WebAssembly module reaches `beck-prim` only as an import the bundle does not carry.
- **The two collection primitives `93` refuses** — `list_zip` has no pair type to lay out and
  `list_flat_map` grows a list under another name — and the JSON pair, for the same reasons stated
  there.
- **A measurement.** Nothing here has been timed. §106.1 names reclamation as the arena's cost and
  says a measurement is what would reverse the decision; that measurement does not exist, so the
  claim in this chapter is agreement and not speed.

### What this corrects, elsewhere

| Where | What |
|---|---|
| [`103`](103-the-wasm-emitter-report.md) | "**Built, for the scalar subset**" and "it compiles 0 of the corpus's 237 definitions" were true of the emitter without a heap in it. §103.6's table and §103.8's first two bullets are this chapter's |
| [`05`](05-tier-lowering.md) §5.1 | "GC proposal where available; Perceus-style refcounting fallback" is answered, and the answer is neither — [`adr/0032`](adr/0032-the-webassembly-heap-is-the-arena-in-linear-memory.md) |
| [`94`](94-the-client-report.md) §94.15 | "It compiles the **scalar subset**… so it compiles **none of the corpus**" is no longer true; "nothing loads its output" still is, and that is what keeps `adr/0022` standing |
| [`93`](93-the-native-backends-report.md) | The third emitter shares the heap as well as the monomorphiser, the trap codes and the fixtures — what it does not share is where the arena lives |
| [`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md) | "A fixed reservation nothing can invalidate… 256 MiB of untouched reservation costs nothing on any system this runs on" is false of a browser tab, which is why this target grows instead |
