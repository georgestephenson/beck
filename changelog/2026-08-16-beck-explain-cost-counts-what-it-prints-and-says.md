- **2026-08-16 — `beck explain cost` counts what it prints, and says how often a capture moves.**
  The summary collected operators whose cost mentions `n entries copied` and the capture line was
  written after the count, so `corpus/27-review.beck` — the one program in the corpus that contains
  a join — was told **1 of 29** operators cost `O(n)` per event when two do, wrong in the
  reassuring direction. The tally is now derived from the same per-operator record the body is
  printed from, so the two cannot disagree, and it reports **2 of 29** with the two reasons named
  apart: an arrangement forced into a list is `docs/23` §23.8's constant factor, a per-element
  function that captured the state is a program that left the view algebra. The capture line also
  carries the **cadence** of what it captured — never, per subscription, or per event — traced back
  to a source in one pass over the plan's dependency order, so a captured `const`, a captured
  `session` and a captured *state* print three different sentences instead of one; §99.3's sweep
  found 18 capture sites of which only 3 are the expensive kind, and one of those is two hops from
  `#0`. Gated by `incremental.rs::the_tally_counts_every_line_the_report_prints`, which reads both
  numbers out of the printed text rather than recomputing either, and
  `a_capture_says_how_often_what_it_captured_moves`, which builds one program per cadence; both go
  red on the previous behaviour. Deletes `DEFECTS.md::cost-report-undercount`; item 2 of
  [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9, the instrument every item below
  it is read through.
