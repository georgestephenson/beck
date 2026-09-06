- **2026-08-14 · #55 — A `parallel:` child that fails stops its siblings** — the ones an ordered
  join would never have reached, so the scope's answer cannot race
  ([`docs/80`](../docs/80-structured-concurrency-report.md) §80.12). Costs about 1% on a program with
  no scope, flat across 10×. Gated by a count, not a clock
  (`concurrency.rs::a_failing_child_stops_its_siblings`); §80.9 records which wasm can have
  threads.
