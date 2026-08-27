- **2026-08-21 — The corpus-wide counts, re-derived against thirty-eight programs — and this time
  nothing had drifted.**
  `corpus/38-backorders.beck` is the 38th, and [`DEFECTS.md`](../DEFECTS.md)'s
  `corpus-wide-counts-drift` says what that costs. Every figure was re-read with the new program
  held out of the corpus and came back exactly what the documents said — **975/143**, **225**,
  **382** and **366 of 376** — which is the first clean re-derivation in the entry's history and is
  the entry's own argument rather than a reason to close it: what made it clean is that the last two
  re-derivations left every figure printed by a test rather than kept in a comment.
  Re-derived with the program in: the native backends compile **982** definitions against **143**
  refused, **all thirty-eight** corpus programs compile their `apply_event` and **twenty-six of
  thirty-eight** their `view`; the WebAssembly emitter is measured against **232** corpus
  definitions, of which **227** are refused for one shape — a parameter that lives on the heap; the
  corpus places **393** definitions and signals, with the tier table moved with it — `any` 208
  (52.9%), `server` 84, `data` 63, `client` 38; and **377 of 387** corpus names rename, the ten that
  decline unchanged in their reasons.
