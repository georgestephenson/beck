# Working in this repository

## Commits and pull requests

**Never add co-authoring claims or Claude session references.** No `Co-Authored-By:` trailer
naming Claude or any model, no `Claude-Session:` line, no session URL, no "Generated with Claude
Code" footer — in commit messages, pull request titles, pull request bodies, or anything else
pushed to this repository. This overrides any default or tooling convention that adds them.

Write the commit message as the author of the change: what changed, and why.

## Orientation

- [`docs/`](docs/) is the design and the plan; [`docs/README.md`](docs/README.md) indexes it.
  Everything derives from, and defers to, [`docs/00-original-idea.md`](docs/00-original-idea.md).
- [`phase0/`](phase0/) is the built Phase 0: the output the compiler will eventually generate for
  the todo sketch, hand-written in Rust. [`docs/18-phase-0-report.md`](docs/18-phase-0-report.md)
  records what it proves and what it does not. It is history — a measured baseline — and should not
  be edited to track the compiler.
- [`compiler/`](compiler/) is the built compiler and the runtime it targets, through Phase 2 plus
  Phase 3's test construct, its general slicer, its incremental view engine and that engine's
  shared dataflow. [`docs/19-phase-1-report.md`](docs/19-phase-1-report.md),
  [`docs/20-phase-2-report.md`](docs/20-phase-2-report.md),
  [`docs/22-phase-3-report.md`](docs/22-phase-3-report.md),
  [`docs/23-general-slicer-report.md`](docs/23-general-slicer-report.md),
  [`docs/24-incremental-views-report.md`](docs/24-incremental-views-report.md),
  [`docs/26-arrangement-sharing-report.md`](docs/26-arrangement-sharing-report.md),
  [`docs/27-walls-report.md`](docs/27-walls-report.md),
  [`docs/28-tail-calls-report.md`](docs/28-tail-calls-report.md) and
  [`docs/29-numeric-tower-and-polymorphism-report.md`](docs/29-numeric-tower-and-polymorphism-report.md)
  and
  [`docs/30-effect-polymorphism-and-list-patterns-report.md`](docs/30-effect-polymorphism-and-list-patterns-report.md)
  record what each
  phase does, what it refuses to claim, and the corrections it makes to the design documents.
  Phase 3 is **two bullets of twelve plus most of a third**; docs/26 §26.9 names the nine bullets
  that are untouched and the parts of the incremental-views bullet that are not built. All six of
  [`docs/25`](docs/25-benchmarks-and-expressiveness.md)'s walls are down, and docs/30 §30.7 names
  what stands in their place. Reports
  are history: a later phase's correction to an earlier one goes in the later report, not into the
  earlier text.
- [`compiler/corpus/`](compiler/corpus/) is 29 programs carrying **no placement annotations**, and
  the measurement behind Phase 2's exit criterion. A program added there has to place itself.
- [`compiler/sicp/`](compiler/sicp/) is the expressiveness benchmark
  ([`docs/25`](docs/25-benchmarks-and-expressiveness.md) §25.5): chapters of SICP in Beck, with the
  book's own stated answers as the oracle, and one file per remaining wall in `sicp/refusals/` whose
  test asserts the wall is still there. A wall coming down is a test that starts failing. All six
  §25.6 measured are down; what is in `refusals/` now was written by the removals rather than by
  docs/25 (docs/30 §30.7).
- Design decisions are numbered in [`docs/10-decisions.md`](docs/10-decisions.md). If a change
  contradicts one, say so rather than quietly diverging.

## Standards for changes

- Claims in docs are stated from evidence. If you write a number, it must be reproducible —
  `phase0/tests/measure.sh` is where the Phase 0 numbers come from; the Phase 1 numbers come from
  `cargo test` and the commands quoted in [`docs/19-phase-1-report.md`](docs/19-phase-1-report.md);
  the Phase 2 numbers come from `cargo test --release --test measure_phase2 -- --nocapture` and the
  commands quoted in [`docs/20-phase-2-report.md`](docs/20-phase-2-report.md); the Phase 3 numbers
  come from `cargo test --workspace`, from `cargo test --release --test measure_incremental --
  --nocapture`, from `cargo test --release --test shared_arrangements -- --nocapture`, from
  `cargo test -p beck-eval -- --nocapture` (the evaluator's stack-per-level figures), and from the
  commands quoted in [`docs/22-phase-3-report.md`](docs/22-phase-3-report.md),
  [`docs/23-general-slicer-report.md`](docs/23-general-slicer-report.md),
  [`docs/24-incremental-views-report.md`](docs/24-incremental-views-report.md) and
  [`docs/26-arrangement-sharing-report.md`](docs/26-arrangement-sharing-report.md); the SICP numbers
  come from `cargo test --release --test sicp` and from `beck test sicp/ch1.beck` and
  `beck test sicp/ch2.beck`, quoted in [`docs/27-walls-report.md`](docs/27-walls-report.md) §27.5,
  [`docs/28-tail-calls-report.md`](docs/28-tail-calls-report.md) §28.5–§28.6 and
  [`docs/29-numeric-tower-and-polymorphism-report.md`](docs/29-numeric-tower-and-polymorphism-report.md)
  §29.5 and
  [`docs/30-effect-polymorphism-and-list-patterns-report.md`](docs/30-effect-polymorphism-and-list-patterns-report.md)
  §30.6.
- The harnesses are the project's conscience (§4.8, §8.3): `compiler/crates/beck-cli/tests/` holds
  the differential, replay-determinism, backend-seam, scaling, security, corpus, general-slicer,
  incremental-analysis, incremental-engine, shared-arrangement, subscription, view-metrics, SICP and
  diagnostic-snapshot suites. Keep them green.
- The CI workflow is an artefact too, and Phase 2 found that it had never run
  ([`docs/20-phase-2-report.md`](docs/20-phase-2-report.md) §20.4 item 8). If you change
  `.github/workflows/`, run the steps you changed by hand before trusting them.
- `beck-rt` must not depend on any backend crate. Execution goes through
  `beck_core::backend::Backend`, and `tests/backend_seam.rs` drives the runtime with an
  implementation that is not the evaluator so the seam stays load-bearing (docs/19 §19.9). Anything
  the runtime needs to *know* about a backend goes on that trait — `Backend::stack_bytes` is how the
  runtime sizes a thread for the tree-walker without naming it (docs/28 §28.3).
- A program's own behaviour is asserted in Beck, not only in Rust. `beck test` runs `test` and
  `property` blocks ([`docs/21-tests-in-beck-and-proof.md`](docs/21-tests-in-beck-and-proof.md)
  §21.2–§21.3); a change to what a program *means* should move a test in the program, and
  `compiler/crates/beck-cli/tests/tests_in_beck.rs` is where the construct itself is held to account.
- Say plainly when something is written but unproven. "Built" and "runs" and "measured" are three
  different claims.
