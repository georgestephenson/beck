- **2026-09-13 — The runtime serves OpenMetrics, and the roadmap stops naming work that is done.**
  `/_beck/openmetrics` serves OpenMetrics 1.0.0 text beside the dashboard's JSON and the OTLP
  export, which closes [`12`](../docs/12-standards-and-conformance.md) §12.8's chartered row and one
  of the two export gaps [`101`](../docs/101-the-public-surface.md) §101.8 named. It adds no
  measurement: every value was already recorded on the serving path, so a second exposition is a
  second *spelling*.
  **Both exports read one table.** `Telemetry::scalars` and `Telemetry::histograms` are the list,
  and `otlp_metrics` was rewritten to read them, so a counter added in future reaches both formats
  by being added once. A gate for that would have been the alternative, and a structure that cannot
  drift is better than a test that notices when it has.
  **Fetching the specification changed the output twice**, which is the finding. Written from
  memory it had `# HELP` before `# TYPE` (the spec says the ordering SHOULD be TYPE, UNIT, HELP) and
  rendered an `le` with Rust's `{:e}` — `1e-6`, `1.6e-5`, `5.24288e-1`. OpenMetrics pins those label
  values to **Canonical Numbers**: Go's `%g`, with a `.0` where there is no decimal point, so
  exponent form exactly when the decimal exponent is below -4 or at least 6. The spec's own examples
  are the boundaries — `0.0001` and `1e-05`, `100000.0` and `1e+06` — and they are the test's
  oracle, for the reason `clbg/` rebuilds its constants from the Game's own output. Rust's two
  formats are each half of the rule: `{}` never uses an exponent and `{:e}` always does, so
  `canonical_number` reads the mantissa and exponent off one and chooses.
  Three gates in `telemetry.rs`, each checked against the mistake it is for: the `le` rendering
  against the published values (moving either threshold turns it red), the specification's MUSTs
  over the emitted bytes (dropping `_total`, emitting per-bucket instead of cumulative counts, or
  letting a dot survive transliteration each turn it red), and the two exports agreeing. **What is
  not held is a foreign reader** — nothing here is a scraper, so this is
  [`adr/0030`](../docs/adr/0030-the-webassembly-emitter-writes-its-own-bytes.md)'s position about a
  format with no local reader, and §12.8's row says so and names `promtool check metrics` as what
  would close it.
  **Three stale claims in [`08`](../docs/08-roadmap.md) §8.5.4 were corrected while working in it**,
  because §8.5.4 is the one place in `docs/` that holds an order and a stale order misdirects
  whoever reads it next. Grammar-aware fuzzing was listed as due and is **built** —
  `grammar_fuzz.rs`, three gates green — its trigger having fired some time ago. The presence
  roster's row claimed a successor that requires a join against presence to be refused; §99
  decision 3 established that it does not, and `corpus/33-awareness.beck` joins against a roster
  today. And the Prometheus endpoint this change builds was on the free list.
