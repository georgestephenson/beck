- **2026-08-14 · #54 — A view compiles, as the call that builds it**, baked by the evaluator's own
  `beck_core::html::element` ([`docs/93`](../docs/93-the-native-backends-report.md)). 650 → 688, and
  21 of the 32 corpus programs compile their `view`. Not faster than the tree-walker
  (0.80×–1.33×), and §93.5 says why that is the design.
