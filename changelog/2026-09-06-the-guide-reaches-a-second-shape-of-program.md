- **2026-09-06 — The getting-started guide reaches a second shape of program.**
  [`86`](../docs/86-getting-started.md)'s closing section named its own largest gap: "it covers one shape of
  program — one fold, one view, one command union", with two folds, a module boundary, a trait, an
  outbound call, a macro and a `parallel:` scope existing and documented in the reports rather than
  in the guide. §86.7–§86.10 are a second worked program that reaches all six, each arriving where a
  reader would want it rather than as a tour stop: a book club whose leaderboard is a second
  `durable` fold over the same log, which then looks a nomination up at two services in a
  `parallel:` scope, tells its chat channel in JSON it did not write out by hand, and finishes split
  across two files with `beck iface` between them. That bullet is deleted rather than annotated; the
  section keeps the one that matters, which is that the exit criterion still needs a person.
  **The gate is what makes it a claim.** `getting_started.rs` compiles every program and runs its
  tests as before, and now compiles them the way `beck test` does — through the project pipeline
  when a block has imports, so `import http` in the guide is the standard library a reader gets, and
  a block whose first line names a file is a module the next block may import. A fourth test asserts
  the six from the **compiled** program rather than from the text: two `durable` signals, a
  definition that crossed an own-module boundary, `net.out` and `spawn` in a row, an `impl` the
  block's source spells out, and an `impl` for a type it declares that the source does *not* spell
  out — which is a macro having written it, and is the assertion that goes red if `derive_json:` is
  replaced by the impl it generates.
  Two defects were found by writing it and are recorded rather than fixed:
  [`DEFECTS.md::an-imported-impl-is-visible-only-if-its-trait-was-imported-first`](../defects/an-imported-impl-is-visible-only-if-its-trait-was-imported-first.md)
  — swapping two `import` lines is the difference between `B0387` and `ok` — and
  `DEFECTS.md::a-stub-in-a-library-test-is-accepted-and-can-never-fire` (fixed since, so the entry
  is gone — the id is a name rather than a path for exactly this reason).
  One absence too: [`08`](../docs/08-roadmap.md) §8.5.4 now carries **a stub that fails**, because
  the guide has a rejection for "the catalogue is down" that no test can reach, and
  [`22`](../docs/22-phase-3-report.md) §22.6's list of what a stub cannot do did not have it.
  Three documents were corrected in place: §86.6's file listing had not carried `sbom.cdx.json` or
  `styles.css` since they were added, the guide's next-steps table said the corpus had thirty-four
  programs when it has 39<!--c:corpus-programs-->, and [`11`](../docs/11-language-tour.md) §11.1 said "no wildcard
  imports, ever" beside two import forms the compiler does not have — every import is a wildcard
  import, which is exactly what the guide's program collides with.
