- **2026-08-19 — Both ends of a group, answered without the group.**
  [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's other two aggregates, and
  the last of that item the language has a spelling for. `list_min` and `list_max` over the same
  `filter_list` the recogniser already reads — bare, or over a `map_list` of it — compile to
  `Op::GroupBy`: one entry per group, holding a **multiset** of what its rows projected to, of which
  `min` and `max` are the two ends. It is the first right side in this algebra that is not an index
  over the collection, so the join above it is a plain `Matching::Unique` — `Some` for a group with
  rows and `None` for one without, which is what `list_min` already answered for a list and for an
  empty one. No syntax, and no edit to any program.
  **`max` costs what `min` costs, and §99.9 said it would not.** The asymmetry it forecast is real of
  the design it forecast it for — a range over an index keyed `(group, value)` can be entered from
  its start and not from its end, because there is no successor of an arbitrary `Value` to bound it
  with — and it dissolves under the rule the *count* established: an aggregate is the reading
  operator's and never the index's, and a tree an operator builds itself is bounded at both ends by
  construction. The design was asymmetric; the problem was not. Corrected in place in §99.9 item 6.
  `corpus/36-auction.beck` is the program — the lowest and the highest bid on every lot, with no
  `group by` and no `min by` in the file. `scaling.rs::asking_a_group_for_one_end_does_not_build_it`
  measures a **new low** landing on a pile of 200 bids and of 1,600 — the worst case, because the
  answer moves and the page is reassembled — at **72 backend steps at both sizes against 4,097 and
  32,097** with the operator switched off, and one entry copied out of an arrangement either way;
  on a clock that is **22 µs against 95 µs at 200 bids and 56 µs against 698 µs at 1,600**, 4.3× then
  12.5× (`measure_incremental::what_answering_a_group_from_its_ends_saves`, whose slow side is the
  same three operators written through a `let`, so the two plans differ in the aggregate and nothing
  else).
  Two gates hold what the corpus-wide differential cannot. A bid *between* the standing ends moves
  the group and neither answer, so the operator publishes nothing and the join, the loop and the page
  do not run — `incremental_engine.rs::a_bid_between_the_ends_does_not_re_render_the_page`, which is
  about the operator's output rather than its cost. And the **multiset** rather than a set: two bids
  of the same amount are two bids, so
  `incremental_engine.rs::a_maintained_extreme_per_group_survives_the_events_that_take_it_down`
  withdraws the standing minimum, the standing maximum, half of a tie and the last bid on a lot from
  a written log — deleting the multiplicity leaves the corpus-wide differential **green** and turns
  that one red, which was checked rather than assumed.
  Two things fell out of it. A site nested inside one that **failed** is now tried, where before
  every nested site was skipped: `list_min` over a filter whose projection reads the loop's element
  is not an aggregate anything can maintain, and refusing the whole body would have taken the index
  down with it and left the loop at `O(n)` per event. And two lookups that index one collection by
  the same key now build **one** index rather than two — `Core` numbers variables per definition, so
  `lambda b: b.lot` and `lambda c: c.lot` reached the hash-consing key as different strings, and an
  arrangement is memory per subscriber as well as work per event. §99.5 decision 4 records it and
  `incremental.rs::two_lookups_by_the_same_key_share_one_index` is the gate.
