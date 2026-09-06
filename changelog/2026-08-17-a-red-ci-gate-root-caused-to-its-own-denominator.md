- **2026-08-17 — A red CI gate root-caused to its own denominator.**
  `measure_native.rs::what_an_appended_accumulator_costs_against_the_tree_walker` failed on CI for
  three runs and passed everywhere else, reporting "the ratio collapsed, which is what an append
  that copies looks like" about an append nothing had touched. It gated on how the **speedup**
  moved between two sizes — a quotient of two measurements — so it imported the noise of both, and
  the noisy one was the denominator: on a contended runner the evaluator's 2,000-element median came
  out **20.7×** slower than on a developer machine while its 8,000-element median came out only
  **12.0×** slower, because the first thing measured in a process pays for warm-up. The small
  speedup inflated and the ratio-of-ratios fell through the bound. The native column — where the
  property actually lives — said the same thing on both machines throughout: **1.68× per element on
  CI, 1.54× locally**, against the 4× a copy would cost. So the assertion moves to per-element
  compiled cost at two sizes, which is machine-independent and is the instrument the rest of this
  project's shape gates use (`scaling.rs`, 3× bound); the speedups are still printed, because they
  are what this suite is *for*. [`docs/13`](../docs/13-testing.md) §13.7 names this trap and
  [`docs/64`](../docs/64-compile-speed-report.md) §64.1 names the cure — gate the shape, print the
  rate — and a gate that divides by something it does not need is a third way to get it wrong.
