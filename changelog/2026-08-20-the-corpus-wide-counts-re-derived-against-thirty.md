- **2026-08-20 — The corpus-wide counts, re-derived against thirty-seven programs.**
  `corpus/37-ledger.beck` is the 37th, and [`DEFECTS.md`](../DEFECTS.md)'s `corpus-wide-counts-drift`
  says what that costs. It had drifted again, and this time in three directions at once: the
  WebAssembly emitter's corpus denominator read **195** in [`docs/08`](../docs/08-roadmap.md), **213**
  in [`docs/93`](../docs/93-the-native-backends-report.md) and **220** in
  [`docs/103`](../docs/103-the-wasm-emitter-report.md) and [`docs/README`](../docs/README.md) — three
  numbers for one quantity, in four places.
  Re-derived: the native backends compile **975** definitions against **143** refused, **all
  thirty-seven** corpus programs compile their `apply_event` and **twenty-five of thirty-seven**
  their `view`; the WebAssembly emitter is measured against **225** corpus definitions, of which
  **220** are refused for one shape — a parameter that lives on the heap, where §103.6 said 138 of
  195 were a `Str` parameter; and the corpus places **382** definitions and signals, not 373, with
  the tier table moved with it — `any` 201 (52.6%), `server` 83, `data` 61, `client` 37.
  [`docs/65`](../docs/65-the-editor-report.md)'s rename figure was the worst of them, **316 of 325**
  against a true **366 of 376**, and the reason is worth the entry: the test that derives it never
  printed it. The count lived in a *comment* beside an assertion with a floor of 250, so the only
  way to read today's value was to edit the test. It prints it now, as `native.rs` and
  `wasm_backend.rs` print theirs, which is the cheapest half of the gate that entry still owes.
