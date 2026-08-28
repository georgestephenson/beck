- **2026-08-21 — The page's children were copied, not shared, and "n handles" was never true.**
  [`docs/23`](../docs/23-incremental-views-report.md) §23.8 has always named the one cost a maintained
  view does not remove: `html_el` is pointwise, so one event reassembles every element from the page
  down to the list. It described that as "`n` handles are copied". It was not handles.
  `Html::Element` held `children: Vec<Html>` — **owned subtrees** — so `child` deep-copied each child
  it was given, and because every enclosing element re-copied what its own children had just copied,
  one event rebuilt *every node of the page* and the cost compounded with nesting depth. A column
  counting entries cannot see that, which is how it sat behind an accepted number.
  Children are `Vec<Arc<Html>>` now, and an untouched subtree costs a refcount. Same harness, same
  program: **one event on a 5,000-row page goes 14,827 µs → 697 µs**, and the maintained-to-recomputed
  ratio goes 3.4× → 35.0×. The cold render halves too (50,845 µs → 24,370 µs), because
  `beck_core::html::element` is the single function the evaluator *and* both native backends assemble
  a page with — the reason that function was written once.
  **The gate counts allocations, not microseconds**, because §13.7 forbids a shared runner a timing
  threshold and identity is the fact the cost follows from:
  `incremental_engine.rs::one_event_allocates_a_handful_of_html_nodes_whatever_the_page_holds` reports
  **9 new nodes on a 200-row page and 9 on a 1,600-row page** — measured at two sizes, since one
  measurement cannot tell a constant from a linear one. Putting the copy back reports **211 and
  1,611**, which is the shape of the gap it exists to catch.
  This does **not** make a maintained view `O(δ)` end to end, and [`docs/08`](../docs/08-roadmap.md)
  §8.5.4's item stays open: the assembly is `n` refcounts instead of `n` subtree copies, which is a
  smaller `n` and still an `n`. What closes it is the engine emitting patches from its own output
  changes, and this was the representation change that had to come first.
