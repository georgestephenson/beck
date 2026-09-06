- **2026-08-14 · #54 — `raise` and `try:` compile**, as a fourteenth trap code and a handler
  label; unwinding costs nothing per frame, and a caught raise from 3,000 frames is 17.0× the
  tree-walker ([`docs/93`](../docs/93-the-native-backends-report.md)). 688 → 711. Gated by the
  failure differentials (84 calls each) and `unwinding_costs_nothing_per_frame`.
