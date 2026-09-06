- **2026-08-14 · #55 — `parallel:` runs its children at the same time**, on a thread each, with
  fuel split rather than shared ([`docs/80`](../docs/80-structured-concurrency-report.md)). Two
  200 ms children take 201.1 ms against 400.7 ms in order; the compute crossover is measured at
  ~580 µs per child (`measure_concurrency.rs`). Gated by
  `concurrency.rs::two_children_actually_overlap`, a deadlock-or-pass no serial evaluator can pass
  at any speed.
