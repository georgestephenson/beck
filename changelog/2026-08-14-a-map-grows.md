- **2026-08-14 · #54 — A map grows**: `map_insert`, `map_remove` and `map_merge` compile as the
  weight-balanced tree `beck_core::pmap` already is, so a fold that keeps a map is Θ(n log n)
  ([`docs/93`](../docs/93-the-native-backends-report.md)). 895 → 1,137; refusals 523 → 281. Gated by
  `a_fold_over_a_map_is_not_quadratic` — 4.9× the arena for 4× the entries, no clock in it.
