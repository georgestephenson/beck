- **2026-08-19 — The corpus-wide counts, re-derived against thirty-six programs.**
  `corpus/36-auction.beck` is the 36th, and [`DEFECTS.md`](../DEFECTS.md)'s `corpus-wide-counts-drift`
  says what that costs: every figure derived from the whole corpus moves and nothing gates it. It had
  already drifted again — [`docs/93`](../docs/93-the-native-backends-report.md)'s headline read 963 while
  [`docs/08`](../docs/08-roadmap.md), [`docs/README`](../docs/README.md) and §93.6's own table read 968.
  Re-derived: the native backends compile **972** definitions against **140** refused, **all
  thirty-six** corpus programs compile their `apply_event` and **twenty-five of thirty-six** their
  `view`; the WebAssembly emitter is measured against **220** corpus definitions, not 213; and the
  corpus places **373** definitions and signals, not 362, with the tier table moved with it — `any`
  196 (52.5%), `server` 82, `data` 59, `client` 36. §93.6's Compiled column now says it is the
  reading taken when each row landed rather than today's, which is what made it look like a
  contradiction rather than a history.
