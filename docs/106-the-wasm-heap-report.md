# 106 — The heap on the third target

**In progress.** The heap for the WebAssembly emitter: a value representation in linear memory, text
and the collections, and closures through an indirect call table — the work
[`103`](103-the-wasm-emitter-report.md) §103.8 names as "the whole of the remaining work" and
[`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) names as what would reverse it.

[`adr/0032`](adr/0032-the-webassembly-heap-is-the-arena-in-linear-memory.md) is the memory model
[`05`](05-tier-lowering.md) §5.1 left open, taken: neither the GC proposal nor a refcounting
discipline, but the arena [`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md) already
lays out, in the module's own linear memory, growing rather than reserved.

This document is written as the work lands.
