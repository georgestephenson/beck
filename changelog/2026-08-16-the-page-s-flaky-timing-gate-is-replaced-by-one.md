- **2026-08-16 · #63 — The page's flaky timing gate is replaced by one with no clock in it.**
  `measure_native.rs::what_a_page_costs_against_the_tree_walker` asserted a ratio of ratios over
  four wall-clock medians and went red 2 runs in 20 on an unchanged binary under load — a page sits
  near 0.8×, where the number is mostly the runner, not the backend. The claim is now
  `native.rs::a_page_of_keys_and_handlers_costs_equal_bytes_for_equal_rows`: equal steps must cost
  equal bytes of arena at 200, 400 and 600 rows of
  [`viewfix::PAGE`](../compiler/crates/beck-cli/tests/support/viewfix.rs), checked against a known
  quadratic before being trusted. 0 of 20 red under the load that reddened the old one.
