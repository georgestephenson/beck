# ADR 0032 — The WebAssembly heap is the same arena, in linear memory, and it grows

**Status:** accepted
**Date:** 2026-09-06
**Context:** [`05`](../05-tier-lowering.md) §5.1, [`106`](../106-the-wasm-heap-report.md),
[`103`](../103-the-wasm-emitter-report.md) §103.8,
[`0022`](0022-mode-b-ships-the-backend-it-has.md),
[`0026`](0026-the-native-heap-is-an-arena-of-offsets.md),
[`0030`](0030-the-webassembly-emitter-writes-its-own-bytes.md)

## The decision

[`05`](../05-tier-lowering.md) §5.1 asks for "the component's pure code compiled to WASM (**GC
proposal where available; Perceus-style refcounting fallback**)". Neither is taken. A value that
does not fit in a register is a **byte offset into the module's own linear memory**, laid out by
[`beck_llvm::heap`] — the same arena the two native backends read, with the same headers, the same
field order, the same tags and the same `0`-means-nothing rule.

Four consequences are the decision rather than the implementation:

1. **The memory *is* the arena.** Byte `i` of the module's memory is offset `i`. `heap::encode_args`
   already produces a blob whose byte `i` is offset `i`, so the host writes that blob at address
   zero, sets an exported bump-pointer global past it, and calls. The answer comes back the same
   way: the used prefix of the memory *is* the blob `heap::decode` reads.
2. **The literal pool is a data segment**, not something the host writes. The pool is a function of
   the program, so it belongs to the module; a host that writes it anyway writes the same bytes.
3. **The memory grows; it is not reserved.** [`0026`](0026-the-native-heap-is-an-arena-of-offsets.md)
   reserves 256 MiB up front and argues that untouched reservation is free. In a browser tab it is
   not: a `WebAssembly.Memory` costs what it declares. So the module declares one page and
   `memory.grow`s under the bump pointer. The **bound** is unchanged — the declared maximum is
   `heap::ARENA_BYTES` exactly — so a program that runs out gets `Trap::HeapExhausted` and the
   message that names the same number.
4. **A closure is applied through a table**, one `funcref` table per closure family, indexed by the
   rank in the closure's first word. The native backends switch on that rank into a direct call
   because their arena crosses a pipe and must hold no code addresses; a table index is not a code
   address either, so the rank travels unchanged and `call_indirect` replaces the switch.

## Why not the GC proposal

Not availability. WebAssembly GC has been in Chrome and Firefox since late 2023, which is the same
bar [`0030`](0030-the-webassembly-emitter-writes-its-own-bytes.md) applied to `return_call` and
passed. The argument is what a `ref` costs at the two boundaries this backend exists to cross.

- **It re-generates the marshalling [`0026`](0026-the-native-heap-is-an-arena-of-offsets.md)
  removed.** A GC value is a `(ref $Point)`, and nothing outside the module can read one: no
  `DataView`, no `Uint8Array`, no `heap::decode`. Every crossing would need an exported accessor
  per type and per field, generated into the module, and a host walking a type table to drive them.
  That is precisely the "type-directed walk… generated for every layout in the program" that
  decision names as the largest piece of work it avoided, and it would arrive here **larger**,
  because Mode B's host has to read a whole `Html` tree back and not merely one reply.
- **It forks the one layout.** [`heap::Repr::machine`] answers `Scalar::Int` for every reference,
  which is what lets three emitters share one `Repr`. Under GC there is no `i64` to answer with, so
  `Repr` grows a fourth machine type and the shared module becomes a module with a target in it.
  ADR 0026 calls the layout "a contract between three parties"; this would make it a contract
  between two and a half.
- **It buys reclamation, which is the honest cost of refusing it** — named below.

## Why not Perceus-style refcounting

[`0026`](0026-the-native-heap-is-an-arena-of-offsets.md) already refused reference counting for the
native backends: a count in every object, an increment on every field read, a decrement on every
path out of every function, and "a decision to take *after* a measurement rather than instead of
one". Nothing in that reasoning is about the target, and no measurement has arrived since.

What is new here is that it would also **fork the layout** in the most literal way — a refcount is a
word in every object, so an object built by a browser and an object built by a worker would be
different bytes for the same value, and the host's encoder would have to know which backend it was
talking to.

## What it costs, named

- **Nothing is reclaimed within a call.** A `view` that allocates a million nodes holds a million
  nodes until the call returns. What makes that a *bounded* cost rather than a leak is Mode B's own
  shape: a render is one call, and the kernel resets the bump pointer to the end of the arguments
  before the next one — the classic per-frame arena. It is exactly [`0026`](0026-the-native-heap-is-an-arena-of-offsets.md)'s
  cost, with a browser's page budget rather than a server's `malloc` behind it.
- **`p.with(x = …)` is a fresh object, always**, for [`0026`](0026-the-native-heap-is-an-arena-of-offsets.md)'s
  reason: an arena with no ownership in it cannot prove nobody else holds an offset.
- **A `Str`'s bytes are read one at a time.** There is no `memcmp` and no `memchr` to call, so the
  comparison and the search are byte loops in the module. `memory.copy` is the one bulk operation
  WebAssembly gives, and concatenation and slicing use it.
- **The memory is exported, so a page that can reach the instance can read the whole arena.** That
  is true of the kernel today as well — a `wasm32` tree-walker holds the same values in the same
  memory — so it is not a new disclosure, but it is a reason a compiled component is not a sandbox
  for the data inside it.

## What would reverse it

A measurement, which is the same answer [`0026`](0026-the-native-heap-is-an-arena-of-offsets.md)
gives. If a real Mode B application's render allocates enough that a per-frame arena is the wrong
shape — a long-lived component that renders continuously without returning, or a `view` whose
working set exceeds what a tab will grow to — then reclamation is worth its bookkeeping and the
question becomes which kind. The GC proposal would then be a **second** representation rather than a
replacement, because the server tier still marshals bytes; refcounting would be one representation
with a wider object, and is the cheaper of the two to reverse into.
