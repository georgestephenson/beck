- **2026-08-25 — Measured: a nested typed macro is charged for output nobody gets.**
  [`DEFECTS.md`](../DEFECTS.md), [`docs/102`](../docs/102-the-macro-interpreter-report.md) §102.9. A
  `typed macro` whose argument is another one is expanded in the probe and again in the real check,
  so nesting `d` deep costs `2^d` expansions. §102.9 had recorded that as work "charged honestly
  against" F17's production budget; it is not honest, because that budget bounds what expansion
  **produces** and the probe's output is thrown away. Timed through `beck check` on a macro
  producing three nodes: 97 ms at thirteen deep, 175 ms at fourteen, and at fifteen a `B0214`
  refusal — `2^15 × 3 ≈ 98,000` against a budget of 100,000 — for a program whose whole expansion is
  about forty-five nodes, followed by a second error because a spent budget makes the macro's own
  `refuse` fire on a type it can no longer see. Recorded rather than fixed, with both gates a fix
  owes: a nesting sweep in `compile_speed.rs`'s shape form, **and** `macro_bomb.rs`'s doubling typed
  macro still refused — because the probe's charge is exactly what the first version of that work
  deleted, so the obvious fix reopens the hole the obvious gate was written for.
