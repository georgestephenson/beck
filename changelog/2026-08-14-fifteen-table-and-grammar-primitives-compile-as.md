- **2026-08-14 · #58 — Fifteen table-and-grammar primitives compile, as calls into a linked
  runtime library.** `beck-prim` is the same crate the evaluator calls, so backend agreement on a
  digest is one function rather than a differential's claim
  ([`docs/93`](../docs/93-the-native-backends-report.md) §93.12,
  [`adr/0029`](../docs/adr/0029-the-runtime-library-is-linked-and-owns-the-arena.md)). A linked
  `digest` is 274 ns against 5.2 µs asked across the worker's pipe
  (`measure_native.rs::what_a_linked_primitive_costs`); no pointer crosses the ABI, and the crate
  has no `unsafe`. 905 → 941 definitions compile; refusals 173 → 137.
