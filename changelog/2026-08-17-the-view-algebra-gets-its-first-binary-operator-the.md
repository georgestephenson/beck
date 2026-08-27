- **2026-08-17 — The view algebra gets its first binary operator: the join a loop already
  contained.** [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 items 1, 4 and 5.
  Every operator the engine had took **one** collection, so a program relating two of them left the
  algebra: the loop body read the accumulator, the per-element function captured the state node, and
  a node that moves on every event makes that function a *different* function — so the whole
  collection was reapplied whatever changed. A nested-loop join with no index, per event.
  `Op::Join` is the operator and `beck_core::relate` is the recognition: `for x in xs:` whose body
  asks `map_get(m, k(x))` compiles to a join over an index, with **no edit to any program and no new
  syntax**, and the conditions are stated as what an expression *reads* rather than as a shape, so a
  lookup written three definitions deep behind a `match` is still found. Maintained from both sides
  per §99.5's bilinear rule — a left row that moved is looked up once, a right row that moved reaches
  exactly the left rows waiting on its key through a reverse index — so neither side costs the
  collection. The index needed no new operator: the right side is a `Map` field of the accumulator
  and `map_values`'s arrangement is already keyed by the join key, which is why §99.9's `arrange_by`
  moved *behind* the join rather than in front of it, and `examples/board.beck` is now named as the
  program waiting for it (it groups by column, which is not a lookup). Held by
  `scaling.rs::maintaining_a_view_whose_loop_looks_something_up_costs_the_same_at_any_size`, which
  measures `27-review.beck` at 200 and 1,600 rows **with the operator on and off**: 19 units of
  maintenance either size against **415 and 3,215** refused, so the gate carries its own evidence
  that it can fail ([`docs/82`](../docs/82-the-edge-report.md) §82.10) and proves the off switch
  [`docs/08`](../docs/08-roadmap.md) §8.3 item 8 requires — `Relate::Refuse`, reachable as
  `beck explain query --no-join` and `beck explain cost --no-join`. Correctness is the differential
  as before: `incremental_engine.rs` folds a generated log one event at a time and compares the
  maintained page with the recomputed one byte for byte. `corpus/34-assignments.beck` is the program
  §99.9 asked for, one that is *about* a relationship — many issues waiting on one person, so a
  rename is one entry moving on the right and several rows on the left, which `27-review`'s unique
  key cannot reach.
