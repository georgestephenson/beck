- **2026-08-18 — `arrange_by`, and the join a filter already was.**
  [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 3, and the program it named:
  `examples/board.beck` renders three columns out of one map of cards, and each is
  `filter_list(map_values(b.cards), lambda c: c.column == n)` inside a loop over the columns — a
  many-to-one equi-join over an index nobody built, so the loop's function captured the accumulator
  and every event re-scanned every card once per column. The recogniser now reads a `filter_list`'s
  **predicate** the way it already read a `map_get`: an equality with one side over the filtered
  element and one over the loop's is a key and a probe. `Op::ArrangeBy` builds the index — the same
  arrangement `sort_by` builds, iterated there and probed here — and the join answers with the group,
  which is the rows the predicate would have kept in the order the collection held them, because `==`
  is the `Value` order the arrangement is a `BTreeMap` in. **4.5–4.9× less work per event at 200
  cards and at 1,600** with the cards spread over the columns, **1.1×** with every card in the one
  column the event touches: it removes the scan and leaves the group, which is §99.9 item 6's.
  `scaling.rs::a_group_a_loop_filters_for_costs_the_group_and_not_the_collection` is the shape gate —
  the group's size is paid and the collection's is not, at two sizes, with the growing case beside it
  as the gate's own evidence it can fail — and the board joins `fusion.rs` and
  `incremental_engine.rs`'s differentials, which is where a wrong group would show as a wrong page.
  Rungs 0–1 of §99.8's ladder still did not come due, and §99.8 now says why rather than predicting
  otherwise: a join inferred from a loop has the loop's order to preserve, so which side is the left
  is fixed before any cost is consulted.
