- **2026-08-18 — Six documents' corpus-wide counts were two programs out of date.**
  `corpus/35-workload.beck` is the 35th corpus program, and adding one moves every figure derived
  from the whole corpus. Re-deriving them found that three were already wrong *before* it:
  the native backends compile **968** definitions against 137 refused, not 941
  ([`docs/93`](../docs/93-the-native-backends-report.md), [`docs/08`](../docs/08-roadmap.md),
  [`docs/README`](../docs/README.md)); the WebAssembly emitter is measured against **213** corpus
  definitions, not 195 ([`docs/103`](../docs/103-the-wasm-emitter-report.md), `docs/README`); and the
  corpus places **362** definitions and signals, not 353, with the tier table moved with it
  ([`docs/20`](../docs/20-phase-2-report.md)). All corrected in place, and
  [`DEFECTS.md`](../DEFECTS.md)'s `corpus-wide-counts-drift` records why it recurred — every one of
  these numbers is printed by a release-only suite and gated by nobody — with the marker convention a
  fix needs before a test can check prose against a measurement.
