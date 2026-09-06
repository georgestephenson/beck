- **2026-08-21 — Reordering a keyed list was quadratic, and the measurement that missed it only
  ever moved one row.**
  `diff_keyed_from` named each child's current index by scanning the child list for it — one scan
  per child, so `O(w²)` over the window. Reversing 4,000 keyed rows cost **25,476 µs**, and each
  doubling of the rows roughly quadrupled the cost *per row* (1.01 µs/row at 500, 6.37 µs/row at
  4,000). A sort-toggle on a large table is exactly this case: it shares no prefix and no suffix, so
  the previous change's trim cannot help and the whole list is the window.
  Because a `Move` lifts a child out and re-inserts it at the front, the children nobody has claimed
  yet keep their relative order — so the distance a child sits ahead of where it belongs is the
  number of unclaimed children before it. That is a **rank query**, `O(log w)` against a Fenwick
  tree, and it makes the pass `O(w log w)`: **25,476 µs → 2,105 µs at 4,000 rows**, with per-row
  cost flat (0.39–0.56 µs) out to 8,000. Dropping the child list from the removal pass took the
  other quadratic with it.
  **Two gates, because the change owes two different promises.**
  `scaling.rs::reordering_a_keyed_list_costs_the_same_per_row_however_long_it_gets` compares per-row
  cost at 500 and 4,000 rows, so it fails on a change in the order of growth rather than on a slow
  machine — restoring the scan makes it red at 5.4×. And because reconciliation's output is a
  contract with a client that has already applied everything before it, the op stream had to be
  proved unchanged rather than assumed: the old scan is kept as the oracle in
  `diff::tests::the_rank_structure_and_the_scan_it_replaced_emit_the_same_ops`, which asserts the
  two streams equal over 300 generated cases. Round-tripping cannot do that job — emitting one
  redundant no-op `Move` leaves `round_trips_over_a_long_random_walk` green and turns the oracle
  test red.
  This **corrects [`docs/23`](../docs/23-incremental-views-report.md) §23.8**, which said the diff's
  residue could not be removed by a better differ. That was measured on single-row edits, and the
  quadratic was in the case the measurement did not cover.
