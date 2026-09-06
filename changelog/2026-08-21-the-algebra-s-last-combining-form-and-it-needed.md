- **2026-08-21 — The algebra's last combining form, and it needed nothing added to the
  language.**
  `filter_list(xs, lambda x: not map_contains(m, k(x)))` is the difference by key and its mirror is
  the intersection — [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7, and
  the row of §99.4 that had no operator. Every operator before it waited on a *spelling*: `min`,
  `max` and `sum` each became a primitive before an operator could read them. `map_contains` was
  already one, so this was the shortest of the five to land.
  `Op::Restrict` is **the one binary operator whose output is one of its inputs**, which is what
  §99.5 decision 2 meant by "no representational change at all" and is why a `filter_list` can have
  it where the rule that a join's element is a *row* says a `filter_list` may not have a join: a
  join would need a projection underneath to hand the element back, per element, per event.
  `incremental.rs::a_difference_and_the_intersection_beside_it_are_one_index_and_no_rows` counts
  that rather than asserting it — the recognised plan is held to the refused plan's number of
  per-element operators — and holds the other half too: the two opposite questions
  `corpus/38-backorders.beck` asks the stock are answered from **one** `map_values`.
  **The cost is all on the right, and measuring the other side would have measured nothing.** An
  order arriving moves the left side, which the refused `filter_list` already handles per delta
  because its capture did not move; a *delivery* changes the predicate itself and reconsiders every
  order ever placed. `scaling.rs::stocking_one_item_does_not_reconsider_every_order` measures one:
  **134 backend steps at 200 orders and at 1,600, against 10,064 and 80,064** with the operator
  switched off. The operator holds no copy of what it filters — a dropped row comes back from the
  left input, which is already holding it — so a *stale* value is the one failure it could have,
  and the event that produces it is written into
  `incremental_engine.rs::a_maintained_difference_survives_the_events_that_move_it_from_the_right`
  rather than left to a generator. Both gates were run against a broken right-hand pass and a
  broken read-back before being trusted.
  **What is left of item 7 is `distinct`, and it moved rather than shrank**: the count-per-distinct-value
  arrangement decision 2 said it would need is already built (it is `Op::GroupBy`'s multiset), so
  what remains is a spelling — `lib/collections.beck` has two duplicate-dropping functions with
  different answers, both maintainable, and picking one is a decision of the kind `list_sum` took.
