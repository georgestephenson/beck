- **2026-08-12 · #49 — Text is on the heap, and the read-only collections follow.** A `Str`
  compiles — layout, literal pool, comparisons, ten primitives — then read-only lists and maps,
  then the primitives those layouts had unlocked (`unwrap_or`, `is_some`, `str`, `str_join`,
  `str_repeat`), three of which were refused for reasons that were false
  ([`docs/93`](../docs/93-the-native-backends-report.md) §93.9). 283 → 625 across the rounds;
  differentials reach 3,382 text calls on all three backends. Record fields compared by offset
  found in both emitters — `Repr::order` is now the only place a comparison is named — and the
  evaluator's `str_slice` was charged the length the caller wrote rather than what it takes, found
  by the differential and gated in `interp`.
