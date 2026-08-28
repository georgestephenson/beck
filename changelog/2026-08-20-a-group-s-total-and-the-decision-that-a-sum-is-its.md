- **2026-08-20 — A group's total, and the decision that a sum is its answer.**
  [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's last aggregate, and the
  only one that owed a **decision** rather than an operator. The two edges it named were real: a
  float total maintained by adding what arrived and subtracting what left is not the number a left
  fold produces, and an integer one passes through intermediate values `checked_add` raises on, so
  the maintained plan and the recompute could disagree about whether the program *failed*. Both are
  edges of the fold rather than of the sum. **A sum is its answer, not the order it was added in**:
  `list_sum` is the exact total and raises only when *that* leaves `Int`, which makes it a function
  of the numbers alone — and a **conservative extension** of `+`, with the same answer wherever the
  fold has one and an answer for `[Int_MAX, Int_MAX, -Int_MAX]`, where the fold has none.
  `interp.rs::a_sum_is_its_answer_and_not_the_order_it_was_added_in` is that sentence at the two
  functions themselves. **`Float` gets no `sum`**, because there the same definition would *disagree*
  with the fold rather than extend it — a different number in the last bits, on ordinary inputs
  ([`docs/46`](../docs/46-standard-library-report.md) §46.16).
  `Agg::Sum` therefore keeps a running total and **no multiset**, since a sum does not care which
  distinct values its group holds: `corpus/37-ledger.beck` shows every account's balance in **47
  backend steps at 200 postings and at 1,600, against 2,060 and 16,060** with the operator switched
  off (`scaling.rs::totalling_a_group_does_not_build_it`, which measures both settings). Unlike the
  extremes there is no worst case to choose — every posting moves its account's total, so an ordinary
  event is already the reassembling one.
  Two things it does that the extremes do not. An empty group is `0` rather than `None`, so the join
  above it reads a missing entry as a *value* — `Matching::Total`, gated by
  `incremental.rs::a_total_is_a_group_by_probed_as_a_value_rather_than_an_option`. And a total no
  `Int` holds is **published rather than raised**: the operator maintains every group while the
  recompute only sums the groups the loop reaches, so raising at maintenance time would fail renders
  that never asked. The raise lands at the probe, and
  `incremental_engine.rs::a_total_outside_int_fails_where_it_is_asked_for_and_nowhere_else` holds the
  two plans to the same failure as well as to the same answer.
