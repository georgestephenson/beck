- **2026-08-13 · #50 — A closure compiles, and it does not leave**: a rank and its captures,
  applied by a switch into a direct call, refused by name at every boundary the host would read
  one across ([`docs/93`](../docs/93-the-native-backends-report.md)). `concat_lists` and `sort_by`
  follow — one refused for a reason that was false — and the gate that asks whether a refusal's
  reason is *true* fired for the first time (§93.14). 605 → 646 across the two rounds. Gated by
  the closure differentials (1,178 calls each) and shape gates with no clock in them.
