# 106 — The heap on the third target

**Built.** A value representation in linear memory, text and the collections, closures through an
indirect call table, failure, and the four host effects as imports — the whole of it held to the
tree-walker by a differential of **22,814 calls run in a real WebAssembly engine**, where the scalar
half ([`103`](103-the-wasm-emitter-report.md)) ran 12,852.

The number that matters is the other one. `beck-wasmgen` compiled **0 of the corpus's 237**
definitions; it now compiles **217<!--c:wasm-compiled--> against 20<!--c:wasm-refused--> refused**,
which is [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md)'s "what would reverse it"
arriving as a measurement. It is **two short of what `beck-llvm` compiles over the same corpus**,
and both of those two are `str_to_int` — linked there, and there is no link line here
([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)).

It is not yet reversed: **nothing loads the module**. `beck-wasm`'s kernel still interprets and the
bundle format is unchanged. Those are §106.8, and they are what is left.

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

## 106.4 A question is an import, and in production nothing crosses at all

`now`, `uuid`, `secret_env` and `http_fetch` are the four things a computation cannot answer.
[`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md) put the compiled program
in **another process**, so the native backends write a question frame into the arena and block on a
pipe. There is no other process in a browser tab and no pipe to block on: the loader is in the same
tab, holds the memory, and can be called. So the frame's five fields —
[`beck_llvm::Question`]'s `op`, `span`, the answer's shape, a failure's shape, and the name of the
type a failure raises — become the arguments of **one import**, `beck.upcall`, and the answer is
its result with its bytes appended at the mark.

Two things follow, and the second is the one worth stating:

- **The host is not written twice.** `beck_llvm::service::answer` services this backend's questions
  and the native worker's. It is handed a `Question` and a `Heap` and gives back an `Answer`, and
  the only per-primitive code in it is four lines. What made that possible is the **shapes**: a
  word per argument and a word saying what each word *is*, so nothing on the host side has a table
  of what `secret_env` takes.
- **In production the arena does not travel.** §93.15's measured decision — a question sends the
  live arena when, and only when, an argument could point into it — exists because a pipe copies.
  Here the host *is* holding the memory, so a question copies nothing at all whatever its
  arguments are. The differential's driver copies, because its engine is another process; that is
  the harness's cost and not the design's.

A module that asks nothing declares nothing, which
`only_a_module_that_asks_declares_an_import` is the gate for — and it is decided from the
definitions the fixed point *kept*, because an import is a function the loader has to supply and a
browser should not be asked for one a refused definition wanted.

## 106.5 A handler is a block a failure branches out of

`raise` and `try:` are the one control-flow shape a `block` is for here. The native backends unwind
through an error cell that was already an unwinder and branch to a *label*; there are no labels, so
a handler is two nested `block`s — the inner one is the failure exit and the outer one carries the
answer past it. The success path builds its `Ok` and `br`s over the handler; every trap check under
the handler `br`s to the inner block's end instead of returning.

That makes the handler **lexical** in the strongest possible sense
([`38`](38-literature-survey.md) §38.4): the distance to it is a number counted where the block is
written, so there is not merely no dynamic search — there is nowhere to search. What it costs is
that the emitter has to know how deep it is, which is one counter maintained where instructions are
pushed.

The globals are cleared **before** the `Err` is allocated, and that ordering is load-bearing: every
allocation checks them, so a failure that stops here would otherwise make the next call look like it
was still failing.

## 106.6 What was different about the target, and what was not

| | LLVM and Cranelift | WebAssembly |
|---|---|---|
| Where the arena is | a `malloc`'d buffer the host allocates | the module's **own** linear memory |
| How it is sized | reserved at 256 MiB | one page, grown under the bump pointer |
| A data pointer after an allocation | re-read, defensively | **stable**, because a memory only grows |
| Applying a closure | a switch on the rank | a `funcref` table and `return_call_indirect` |
| A byte comparison | `memcmp` | a loop, because there is no `memcmp` |
| A bulk copy | `memcpy` | `memory.copy`, which is the one bulk operation there is |
| The literal pool | written into every request | a **data segment** |
| A question | a frame in the arena and a blocking pipe | one **import**, called in the same tab |

Rows three and seven are the two places this target is *simpler*. A WebAssembly memory never moves
what is already in it, so a data pointer taken before an allocation is still correct after one —
which removes a class of defect the native backends carry a comment about at four sites. And a pool
that belongs to the module is a pool nothing has to copy per call.

## 106.7 Four defects, each of which passes almost every case

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

## 106.8 What it compiles

| | Compiled | Refused | Was |
|---|---|---|---|
| [`corpus/`](../compiler/corpus/) — 39<!--c:corpus-programs--> applications | **217**<!--c:wasm-compiled--> | 20<!--c:wasm-refused--> | 0 and 237 |
| [`awfy/`](../compiler/awfy/) — Are We Fast Yet | 391 | 11 | 58 and 344 |

The ceiling is 219 and 18, which is what `beck-llvm` compiles over the same corpus. The gap is
**two** definitions: `parse_amount`, which calls `str_to_int`, and the one that calls it. That
primitive is one of the fifteen the runtime library computes, and a WebAssembly module reaches
`beck-prim` only as an import the bundle does not carry — the same sentence `sin` and `cos` are
refused under.

## 106.9 The fixtures are not this suite's

`support/{heapfix,textfix,listfix,mapfix,clofix,viewfix,genfix}` are the programs `native.rs` and
`cranelift.rs` already point at, and `wasm_backend.rs` is the third caller. That is the same
argument `support/scalar.rs` was shared under, one subset up: a second copy of "what the heap subset
is" would be a second opinion, and the three suites would drift on exactly the cases nobody thought
to write twice.

It also means the layout traps those fixtures were built for are asked of this backend without
anybody re-deciding them: `Ranked`'s variants are declared out of alphabetical order and `Key`'s
fields are, so a backend that made either one a *declaration* index answers two definitions
backwards and nothing else.

## 106.10 The type table, and why it is not a type table

[`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) named the next two steps: "**Types are
erased from the bundle.** A compiling client backend needs them, so it needs bundle format 2… the
bundle grows a type table under a new format version." Both halves of that sentence turn out to be
wrong, and finding out why is what this section is.

**The version is already spent.** [`beck_core::bundle::FORMAT`] is `2`, and has been since the
client learned to carry `reads_freshness`. The type table is format **3**, which is a detail — but
it is the kind of detail an ADR's forecast gets wrong for free and a reader should not have to
discover by reading the constant.

**A type table is not enough, and the reason is a property this project wanted.** What a host has
to hold in order to marshal against a module is not the types — it is the module's
[`beck_llvm::heap::Heap`]: the layouts, the element and entry tables, the closure families, and the
literal pool, each identified by an **index** the compiled code has baked into it. A view node's
deferred value stores `heap.word_of(v.ty)` in the arena and the host reads it back through
`heap.shape(at)`, so a host whose table is numbered differently decodes the right word as the wrong
thing.

Those indices are assigned by [`beck_llvm::heap::survey`], which walks **the whole program** —
deliberately, and §93.1 is the reason: "a layout's index is then a function of the program's own
order rather than of which definition happened to be emitted first, so the IR is the same bytes
twice." A bundle is a *slice*. Replaying the survey in the browser would need the program the
bundle exists not to carry, and resolving only the slice's own types produces a different numbering
for the same layouts.

So the bundle has to carry the `Heap`, and **a `Heap` cannot cross a wire**: its fields are
private, it derives no `Serialize`, and its only public constructor is `Heap::new`. Everything can
be *read* out of it — `layouts`, `lists`, `maps`, `families`, `strings` — and nothing can be put
back, because the only way to make a layout is to resolve a type and the only way to resolve a type
is to have the program.

That is one change in [`beck_llvm::heap`] and it is somebody else's file. Any of three would do,
and they are not equivalent:

1. `#[derive(Serialize, Deserialize)]` on `Heap` and its parts. Smallest, and it puts `serde` in a
   crate that has none.
2. `Heap::parts()` and `Heap::from_parts()`, with the mirror types written where the bundle's other
   mirrors are ([`beck_core::bundle`]'s own argument for why a concrete wire type is worth having).
3. **`heap` as a crate of its own.** The layout is already "a contract between three parties"
   ([`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md)); a fourth is a browser, and a
   client kernel taking a dependency called `beck-llvm` in order to know what a record looks like
   is a name that has stopped describing the thing.

**What it costs the kernel is measured rather than guessed**, because that is the assumption
everybody makes first: adding `beck-llvm` to `beck-wasm` and calling `Heap::decode` grows the
`wasm32-unknown-unknown` kernel from **832,279 bytes to 832,482** — 203 bytes. The 21 MiB runtime
archive `beck_llvm::prim` embeds is dead code and is eliminated. Whatever decides between the three
options above, it is not size.

The rest of the design is settled and is not the blocker:

- The **compiler** compiles, not the browser. [`adr/0030`](adr/0030-the-webassembly-emitter-writes-its-own-bytes.md)
  refuses to "compile a code generator into every browser download", so the module is emitted where
  the bundle is built and travels in it.
- The module is attached at **compile time**, not served time. `AGENTS.md`'s structural rule is that
  `beck-rt` must not depend on any backend crate, and `beck-rt::http` is what serves the bundle — so
  the compiled slice belongs on the `Placed` program, put there by whoever compiled it.
- The kernel needs **one import**, not a second module of its own: the shim instantiates the
  component, and the kernel hands it a blob and cells and gets a cell and a blob back. The
  marshalling stays in the kernel, where the `Heap` is.

## 106.11 What is not built

- **The kernel loading a compiled component**, for §106.10's reason. `beck-wasm` still interprets
  `Core`, so `adr/0022` is **not reversed** — this is the half of it that runs, and nothing runs
  it.
- **The runtime library's fifteen primitives**, refused for the link line exactly as `sin` and `cos`
  are ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)): a
  WebAssembly module reaches `beck-prim` only as an import the bundle does not carry.
- **The two collection primitives `93` refuses** — `list_zip` has no pair type to lay out and
  `list_flat_map` grows a list under another name — and the JSON pair, for the same reasons stated
  there.
- **A measurement.** Nothing here has been timed. §106.1 names reclamation as the arena's cost and
  says a measurement is what would reverse the decision; that measurement does not exist, so the
  claim in this chapter is agreement and not speed.
- **A WebAssembly spec-suite run.** [`12`](12-standards-and-conformance.md) §12.3 pins core 3.0,
  and what exists is a differential against the *language's* semantics — a different claim from
  conformance to the format. The half of it that is cheap is paid on every run: the emitter's
  output is validated by a real engine, and two of §106.7's four defects were caught by that
  validation rather than by an answer.

### What this corrects, elsewhere

| Where | What |
|---|---|
| [`103`](103-the-wasm-emitter-report.md) | "**Built, for the scalar subset**" and "it compiles 0 of the corpus's 237 definitions" were true of the emitter without a heap in it. §103.6's table and §103.8's first two bullets are this chapter's |
| [`05`](05-tier-lowering.md) §5.1 | "GC proposal where available; Perceus-style refcounting fallback" is answered, and the answer is neither — [`adr/0032`](adr/0032-the-webassembly-heap-is-the-arena-in-linear-memory.md) |
| [`94`](94-the-client-report.md) §94.15 | "It compiles the **scalar subset**… so it compiles **none of the corpus**" is no longer true; "nothing loads its output" still is, and that is what keeps `adr/0022` standing |
| [`93`](93-the-native-backends-report.md) | The third emitter shares the heap as well as the monomorphiser, the trap codes and the fixtures — what it does not share is where the arena lives |
| [`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md) | "A fixed reservation nothing can invalidate… 256 MiB of untouched reservation costs nothing on any system this runs on" is false of a browser tab, which is why this target grows instead |
| [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) | "it needs bundle format **2**" — the format is already 2, for `reads_freshness`, so the table is format 3. And "the bundle grows a **type table**" is not sufficient: §106.10 is why a table of types cannot reproduce the indices a whole-program survey assigned |
