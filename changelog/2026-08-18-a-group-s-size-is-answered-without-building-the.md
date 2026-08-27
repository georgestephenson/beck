- **2026-08-18 — A group's size is answered without building the group.**
  [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's first aggregate, and the
  leftover item 3 handed it: `arrange_by` turned the scan into an index probe and then materialised
  the group in order to count it, so an event still cost the size of the pile it landed on. A
  `list_len` over the same `filter_list` is now recognised as an aggregate rather than as a group —
  `Matching::Count` — and the join keeps a tally per key beside its reverse index, moved by ±1 as the
  index moves. `corpus/35-workload.beck` is the program it exists for: every person, and how many
  issues name them, with the set of people coming from the data rather than written out. It copies
  **one** entry out of an arrangement at 200 issues and at 1,600, against **202 and 1,602** for the
  same page whose count is wrapped so the recogniser reads it as a group — same characters rendered,
  one plan that builds the pile to measure it and one that does not; 2.8× then 4.9× on a clock
  (`measure_incremental::what_counting_a_group_saves`).
  `scaling.rs::counting_a_group_does_not_build_it` is the shape gate and carries that contrast in the
  same run. The tally lives on the **join** rather than on the index, which is the finding worth
  keeping: an operator reads its inputs' values and changes and never their private state, because an
  index in the shared dataflow is not the reading engine's cell at all.
  `incremental_engine.rs::a_maintained_count_per_group_survives_the_events_that_take_it_down` folds a
  written log rather than a generated one, because whether a generated `Closed` names an issue that
  exists is the seed's business and not the test's — deleting the decrement turns both red, which was
  checked rather than assumed. §99.9 now also records that `min`/`max` per group are an `arrange_by`
  keyed by `(group, value)` and therefore the **cheap** ones rather than the hard ones it first
  called them, and that `sum` owes a decision per numeric type before an operator: a `Float` sum
  cannot be maintained by subtraction at all, and an `Int` one can be arithmetically but passes
  through different intermediate values from a recompute, which `checked_add` can turn into a
  disagreement about whether the program failed.
