- **2026-08-16 · #68 — `Core` compiles to WebAssembly, for the scalar subset.** A third emitter
  ([`docs/103`](../docs/103-the-wasm-emitter-report.md), `beck-wasmgen`), over the same layout module,
  trap codes, monomorphiser and fixtures the two native backends use, with the binary format
  written by hand and no runtime taken as a dependency
  ([`adr/0030`](../docs/adr/0030-the-webassembly-emitter-writes-its-own-bytes.md)). `beck native
  --backend wasm --out <dir>` writes `module.wasm` and a readable `module.wat` rendered from the
  same instruction list. The gate is `wasm_backend.rs`: **12,852 calls agreed with the tree-walker
  in a real WebAssembly engine** — value or failure *and its message*, reals crossing as bit
  patterns — plus a million-deep tail recursion proving `return_call` is a jump. It compiles **0 of
  the corpus's 195 definitions** and 58 of `awfy/`'s, because the heap is not laid out on this
  target, so [`adr/0022`](../docs/adr/0022-mode-b-ships-the-backend-it-has.md) is **not** reversed and
  Mode B still ships the interpreter; `docs/93` §93.15, `docs/94` §94.15, `docs/12` §12.3 and
  `docs/08` corrected in place. The suite skips without a JavaScript engine and
  `BECK_REQUIRE_WASM_RUN=1` forbids the skip, which CI now sets.
