# ADR 0030 — The WebAssembly emitter writes its own bytes, and its host is whoever loads it

**Status:** accepted
**Date:** 2026-08-16
**Context:** [`05`](../05-tier-lowering.md) §5.1, [`103`](../103-the-wasm-emitter-report.md),
[`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md),
[`0022`](0022-mode-b-ships-the-backend-it-has.md),
[`0026`](0026-the-native-heap-is-an-arena-of-offsets.md)

## The decision

Beck's third emitter (`beck-wasmgen`) writes the **WebAssembly binary format directly** — no
assembler, no `wat2wasm`, no encoder crate — and it does **not** run what it emits. There is no
WebAssembly runtime in this workspace and none is taken as a dependency to test one: the host is
whoever loads the module, which for Mode B is a browser and for the differential is a JavaScript
engine found on the path.

Three consequences are part of the decision rather than of the implementation:

- **A trap is a value in an exported global**, not a WebAssembly trap. `unreachable` and
  `i64.div_s` abort the *instance*; a Beck program that overflows has failed the way its type says
  it can. The codes are [`beck_llvm::Trap`]'s and the message comes from `Trap::message`, so three
  backends decode one wire.
- **A tail call is `return_call`**, so the emitted module needs a runtime with WebAssembly 2.0's
  tail calls. Every engine Mode B has ever run in has had them since 2023.
- **The emitter is a crate of its own**, not a module of `beck-wasm`. `beck-wasm` is the kernel —
  a crate compiled *to* `wasm32-unknown-unknown`; this one is compiled *for the host* and produces
  wasm. Putting them together would make one crate two things and would compile a code generator
  into every browser download.

## Why

**Writing the bytes.** The binary format is a documented byte string and the encoder is a page:
sections, LEB128, and a closed list of opcodes. A dependency here would be one more crate in the
SBOM for a job a `Vec<u8>` does, and it would put a version of somebody else's encoder between the
compiler and the artefact a browser loads. This is the same argument
[`92`](../92-supply-chain-and-release-report.md) makes for `beck image` writing an OCI layout in
one process, and it has the same second half: **the gate reads a rendering** — the listing
`beck native --backend wasm --out` writes is generated from the same instruction list the encoder
walks, so the text and the bytes cannot disagree about what was emitted.

**Not running it.** [`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md) refused to
*run* emitted code in process because turning a pointer into a function is `unsafe`, and answered
with a child process and a pipe. That answer is not available here and is not needed: a
WebAssembly module has no meaning without a host, and the host that matters is the one the code
was compiled for. So the artefact is the deliverable, and the differential drives it through a
JavaScript engine — the same execution environment production uses, rather than a second one taken
as a crate so that a test can avoid depending on the first.

The cost is that the suite **skips** where there is no engine, which is the tradeoff every
environment-dependent gate in this repository makes ([`19`](../19-phase-1-report.md) §19.4 item
10): a printed skip, and `BECK_REQUIRE_WASM_RUN=1` to forbid it.

**The trap in a global.** The two native backends store a trap in an error cell the host reads out
of the arena. There is no arena here — the scalar subset has no heap — so the same three facts
(which failure, where, and the value a `no match` reports) are three exported globals. Exported
rather than returned because a compiled function returns its *value*: a second return value would
be a different signature for every definition, and a sentinel would be a value the language can
legitimately produce.

## What this does not decide

**It does not reverse [`0022`](0022-mode-b-ships-the-backend-it-has.md).** Mode B still ships the
interpreter, because the heap is still not laid out on this target and a component's `view` is
nothing but heap. ADR 0022's "what would reverse it" names a `Core → WASM` backend; this is the
half of one that has no heap in it, and [`103`](../103-the-wasm-emitter-report.md) §103.6 measures
what that buys a real program today, which is nothing.

**It does not choose a memory model.** §5.1 says "GC proposal where available; Perceus-style
refcounting fallback". Nothing here chooses between them, because nothing here allocates. When
something does, [`0026`](0026-the-native-heap-is-an-arena-of-offsets.md)'s arena is the layout
already written down and shared, and whether a browser's linear memory should hold it is the
question that decision will have to answer.

## What would reverse it

An engine-in-workspace — Wasmtime, which [`07`](../07-dependencies.md) §7.3 already names for the
*server* tier — would make the differential runnable without a JavaScript engine and would remove
the skip. That is a reason to take the dependency when the server tier needs it, and not a reason
to take it now: the browser is where this code runs, and a test against the engine that will not
run it would be the weaker gate.
