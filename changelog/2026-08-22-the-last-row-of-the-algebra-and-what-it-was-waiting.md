- **2026-08-22 — The last row of the algebra, and what it was waiting for was a name.**
  [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7's second half, and
  §99.4's table is now complete. §99.5 decision 2 deferred `distinct` because it "needs a count per
  distinct value, which is a new kind of arrangement" — and by the time it was wanted that
  arrangement was not new: `Op::GroupBy` had kept a multiset per group since `min` and `max`
  landed. What actually stood in the way was that **nothing in the language named the question**.
  Both of `lib/collections.beck`'s duplicate-dropping functions are folds, and a fold is one opaque
  operator the plan rebuilds in full on every event.
  **So the work was a decision, not an operator.** `unique(xs)` keeps the order the list had;
  `elements(set_of(xs))` sorts; both are maintainable, so nothing about the engine forced the
  choice. `list_unique` takes `unique`'s answer and that function's body is now a call to it, so no
  third answer entered the language — `list_sum`'s rule applied to an order instead of to a total.
  `interp.rs::a_unique_list_keeps_the_order_it_was_given_and_not_the_values_own` is that sentence as
  assertions.
  `Op::Distinct` publishes each value at the **smallest input key** holding it, which makes its
  output a sub-order of its input's exactly as `filter_list`'s is — so nothing downstream had to
  learn anything, and `list_len` over it is the arrangement's size rather than a recompute.
  `corpus/39-topics.beck` shows the topics its notes are filed under: **62 backend steps at 200
  notes and at 1,600, against 2,280 and 17,680** for the same program with the dedup written as a
  fold, measured on the worst case — a note that *moves* a topic's published occurrence.
  **One defect, found by the corpus-wide differential rather than by a test anybody wrote.** A value
  can leave a key that another value is arriving at — one row changing what it contributes is
  exactly that — so every departure has to be applied before any arrival, or the arriving value is
  inserted and then removed again.
  `incremental_engine.rs::a_maintained_set_of_values_survives_the_events_that_move_where_each_one_sits`
  is the written log that holds it, and it was run against the interleaved settle before being
  trusted.
  There is no off switch and that is itself the finding: `list_unique` *names* the operator, so
  nothing is decided on the program's behalf and [`docs/08`](../docs/08-roadmap.md) §8.3 item 8 has
  nothing to ask for — which is also the sixth time §99.8's solver rungs have not come due.
