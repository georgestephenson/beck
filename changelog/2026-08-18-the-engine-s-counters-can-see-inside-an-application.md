- **2026-08-18 — The engine's counters can see inside an application, and two gates stopped looking
  away.** [`DEFECTS.md`](../DEFECTS.md)'s `work-cannot-see-inside-an-application`, opened earlier today
  and closed here. `Work` counted what the engine did *to* its arrangements and stopped at the
  boundary of a call, so `examples/board.beck` with the join refused reported the identical four
  numbers at 200 cards and at 1,600 while its clock moved tenfold — and both of this session's shape
  gates had to be written against something other than their own off switch, because the off switch
  was invisible. `Backend` now carries a defaulted `steps()` in the shape `intercepting` and
  `stack_bytes` already established: the tree-walker publishes what its calls spent of their own
  evaluation budget, a compiling backend answers `None`, and `Prepared` takes the counter at prepare
  time so `Engine::render` can subtract. `Work::steps` is that difference, deliberately **not** in
  `Work::total` — it is a different unit by three orders of magnitude and would drown every gate that
  reads the total.
  `scaling.rs::the_work_a_render_reports_includes_what_happened_inside_it` asserts the blindness from
  both ends: the four counters identical at either size, `steps` growing with the collection. Both
  operator gates now measure `Relate::Refuse` directly, which is what §8.3 item 8 asks of an off
  switch: **98 steps at 200 cards and at 1,600 against 12,830 and 101,030** for the join's index, and
  **44 against 1,253 and 9,653** for the group's count. The variant-program contrast the aggregate
  gate needed is gone; `measure_incremental` prints `steps` beside the clock, where the two now agree
  about shape.
