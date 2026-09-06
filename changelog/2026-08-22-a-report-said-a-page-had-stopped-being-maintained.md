- **2026-08-22 — A report said a page had stopped being maintained, and the plan it compiles to
  was byte-identical.**
  `DEFECTS.md::class-list-recomputes`, closed — and the entry's own premise was the defect. It said
  a class written as [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.4 asks —
  `class=["flex", "gap-2", done_class(t)]` — "turns a page into a recompute". It does not. The
  `str_join` that list lowers to sits **inside the per-element function of a maintained
  `map_list`**, applied to what moved and nothing else, and `beck explain query` on the documented
  shape and on the workaround beside it prints the same plan, operator for operator.
  What was wrong is the incrementality analysis: it blocked on the primitive's **name**, so the
  same report said "3 of this view's 11 operators update from the change itself" in its headline
  and "recompute" in its verdict row, about one program. `rule_of` now asks what the join is
  applied *to*: a join over a maintained collection reduces it to one string and has no delta rule
  to have; a join of a fixed list of parts is a function of those parts, which is what every
  "pointwise" row in the table already means. It is `to_str` with three arguments.
  `incremental.rs::a_join_of_a_fixed_list_is_pointwise_and_a_join_over_a_collection_is_not` holds
  both directions in one helper, so a rule that looked at the name alone fails whichever answer it
  gives — checked by making every `str_join` pointwise, and then every `str_join` a blocker.
  **The cost of believing the report was a worse program.** `examples/todo.beck` had been rewritten
  to repeat its shared classes in both arms of an `if`, with a comment explaining why. It writes
  §104.4's own shape again, and the class enumeration follows it through the join, the call and the
  branch — which is the step no program in the tree had exercised. §104.4 item 4 now says the
  shared half need not be repeated, and `ui.rs`'s comment no longer claims the all-literal fold is
  the difference between a maintained page and a recomputed one.
