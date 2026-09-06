- **2026-08-18 — The engine's counters cannot see inside an application, recorded as a defect.**
  Found while measuring the above: with the join refused, `examples/board.beck` reports the *same*
  `Work` — 3 applications, 3 touched, 3 materialised, 3 recomputed — at 200 cards and at 1,600, while
  the clock over the same two renders goes from 2.3 ms to 21 ms, because the whole page is rebuilt
  inside one per-element function and the engine counts one application for it. `beck explain cost`
  is right about that plan and the counter is not, which makes every `scaling.rs` gate over an opaque
  operator blind to exactly what an opaque operator can hide.
  `measure_incremental::what_a_grouped_join_is_worth` prints the counters beside the clock so the two
  disagree in public, and [`DEFECTS.md`](../DEFECTS.md)'s `work-cannot-see-inside-an-application` names
  the gate a fix owes — two plans that do the same work must report the same `Work` — and why it is a
  `Backend` change rather than a line in the operator that found it.
