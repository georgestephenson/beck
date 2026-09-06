- **2026-08-20 — The sweep that found the nested-loop join is a gate, and the cost it leaves is
  the last one.**
  [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.3 found the defect the view algebra
  was missing an operator for by **sweeping the tree by hand**: a per-element function that reads the
  accumulator is a different function after every event, so its whole collection is reapplied. That
  sweep was read three times and went stale twice, always in the flattering direction — the third
  reading found a site that had arrived with `awareness(f)` one change after the second and that
  nothing re-ran the sweep to catch.
  It is now `Plan::reapplied_per_event`, and
  `incremental.rs::no_program_in_the_tree_reapplies_a_collection_per_event` is what re-runs it:
  **42 programs across `corpus/` and `examples/` plan, and none of them reapplies a collection per
  event — against 8 sites in 8 programs with the recognition switched off.** The second number is
  what makes the first mean anything, and it is carried by the green run rather than promised by it.
  `beck explain cost` prints its capture lines from the same computation the gate counts, so the
  report and the gate cannot disagree — §99.9 item 2's lesson applied to a second reader instead of
  rediscovered.
  **What the zero exposes is the entry.** Every one of those 42 programs still has an operator
  costing `O(n)` per event and they all have the *same* reason: a recompute needs a `list` and an
  arrangement is a keyed collection. That is now the only per-event linear cost in the tree.
  [`docs/23`](../docs/23-incremental-views-report.md) §23.8 measured it when the engine landed, named
  the fix — the delta at the top of the plan **is** the patch set, so an engine emitting patches from its
  own output changes skips the assembly and the diff together — and called it "a known piece of work
  rather than an open question". It had **no position in [`docs/08`](../docs/08-roadmap.md) §8.5's
  order**, which is verbatim the failure mode that section opens by describing. It has one now, as an
  **F** item ahead of Mode B's codegen, because §23.8 says it is the same work that kernel needs.
