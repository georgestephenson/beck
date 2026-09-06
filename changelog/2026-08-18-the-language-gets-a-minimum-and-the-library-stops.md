- **2026-08-18 — The language gets a minimum, and the library stops sorting to find one.**
  [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6 named the blocker on `min`
  and `max` per group as a **surface** rather than a delta rule: neither had a spelling the view
  engine could recognise, because `lib/collections.beck`'s `min_of` was `list_get(sorted(xs), 0)` — a
  sort and a copy of the whole list to answer a question about one element of it. `list_min` and
  `list_max` are primitives now, one pass and no allocation, and the library's two are one line over
  each. Neither takes a comparator, for `sort_by`'s reason one level down: ordering is the runtime's
  structural one ([`docs/54`](../docs/54-ordering.md)), so the smallest of a list needs nothing from the
  caller, and `Option` says an empty list has no answer rather than raising. Over 64,000 elements the
  library's minimum went from **151 ms of work above the baseline to 33 ms**
  (`beck test` over a generated list, at 16,000 and 64,000).
  `stdlib.rs::the_smallest_and_largest_are_the_runtimes_order_and_an_empty_list_has_neither` holds
  the three decisions; `lib/collections.beck`'s own tests are unchanged and still pass, which is what
  makes this a reimplementation rather than a new function.
  **[`compiler/lib/README.md`](../compiler/lib/README.md)'s division gains the row that was always
  missing.** It admitted a primitive only for a host's table or grammar, and that never explained
  `sort_by`, `filter_list` or `list_len` — every one of which is expressible in Beck and is a
  primitive anyway. The third row says why: the incremental engine maintains what it can
  **recognise**, and an aggregate written as a fold is one opaque operator recomputed in full.
  §46.16's set-cost row is corrected to the amended rule, and a set operation is still neither.
  §99.9's own design claim is corrected in the same change: the `arrange_by` keyed by `(group, value)`
  makes a group's **minimum** the first entry of its range and `O(log n)`, and does **not** do the
  same for its maximum — a `BTreeMap` prefix range is entered from its start and not its end, there
  is no successor of an arbitrary `Value` to bound with, and Beck has no descending order to key by.
  So `max` per group is a walk of the group or a maintained extreme with an `O(g)` repair, which is
  the decision §99.9 opened by calling a genuine one, now known to bite one of the two rather than
  both. Neither is built.
