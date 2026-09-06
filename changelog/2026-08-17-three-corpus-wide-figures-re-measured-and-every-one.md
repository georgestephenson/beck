- **2026-08-17 — Three corpus-wide figures re-measured, and every one of them had drifted.**
  [`docs/08`](../docs/08-roadmap.md) §8.5.6's first decay direction — a document behind the code — swept
  while re-running the measurement suites beside the join above. The placement share was written as
  43% across 176 placements and quoted as 44%; it is **52% across 353**
  (`measure_phase2`). The native backends' figures were 941 definitions compiled, nine corpus folds
  and twenty-one of thirty-two views; they are **963, all thirty-four, and twenty-four of
  thirty-four** (`BECK_REQUIRE_LLVM=1 … --test native`). No solver and no emitter moved: the corpus
  did, and the programs added to it since — recursive types, traits, error rows, structured
  concurrency, identity, presence, and now a relationship — are mostly pure definitions, so the
  numerators grew faster than the totals. Corrected in place in
  [`docs/20`](../docs/20-phase-2-report.md), [`docs/93`](../docs/93-the-native-backends-report.md),
  [`docs/08`](../docs/08-roadmap.md), [`docs/86`](../docs/86-getting-started.md) and the index, each with
  the command that reproduces it. **Nothing gates any of them**, which is why all three drifted
  together; the suites print them and assert shapes.
