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
  Phase 3's test construct, its general slicer, its incremental view engine, that engine's shared
  dataflow, and the language's own means of abstraction — tail calls, reals, polymorphism,
  parameterised types and traits. The build reports —
  [`19`](docs/19-phase-1-report.md), [`20`](docs/20-phase-2-report.md),
  [`22`](docs/22-phase-3-report.md), [`23`](docs/23-general-slicer-report.md),
  [`24`](docs/24-incremental-views-report.md), [`26`](docs/26-arrangement-sharing-report.md),
  [`27`](docs/27-walls-report.md), [`31`](docs/31-tail-calls-report.md),
  [`32`](docs/32-numeric-tower-and-polymorphism-report.md),
  [`33`](docs/33-effect-polymorphism-and-list-patterns-report.md),
  [`34`](docs/34-generated-documentation-report.md),
  [`36`](docs/36-parameterised-types-report.md), [`37`](docs/37-traits-report.md),
  [`39`](docs/39-bounds-report.md), [`40`](docs/40-traits-across-modules-report.md),
  [`41`](docs/41-generic-arithmetic-report.md), [`44`](docs/44-wave-0-report.md),
  [`45`](docs/45-error-rows-report.md) and [`46`](docs/46-standard-library-report.md), indexed in
  [`docs/README.md`](docs/README.md) — record what each
  phase does, what it refuses to claim, and the corrections it makes to the design documents.
  Add a new report to that list and to the index; do not extend it with another "and".
  Phase 3 is **four bullets built, half of a fifth, most of a sixth, and a seventh that has
  started**: the test construct, the general slicer (Phase 2's debt), the means-of-abstraction
  bullet and `Result`/error rows are built; the standard library has its first half and a wall of
  its own (docs/46); the incremental-views bullet has its engine but not its read models, pgwire
  or fusion; the expressiveness suite runs two chapters of SICP. Seven of the fourteen are
  untouched, and docs/26 §26.9 names them one at a time. **Wave 0** (docs/08 §8.5.4) is also
  built — a bounded front end, an injected clock, a threat model, a disclosure policy and an
  identifier profile — and is debt rather than a phase bullet, so it is in docs/44 and not in that
  list. All six of
  [`docs/25`](docs/25-benchmarks-and-expressiveness.md)'s walls are down, and so are the three that
  removing them wrote (docs/33, docs/36, docs/41); docs/41 §41.7 names what stands in their place —
  and docs/46 §46.5 has added one more, found by writing a library rather than a compiler: a
  trait's declared row is a bound, so a fallible operation cannot be a trait method.
  Reports are history: a later phase's correction to an earlier one goes in the later report, not
  into the earlier text.
- [`docs/reference/`](docs/reference/README.md) is **generated** by `beck doc reference` from the
  compiler's own tables and checked in. Never edit it by hand: change the compiler, then run
  `beck doc reference --out ../docs/reference` from `compiler/` and commit the result in the same
  change. A new diagnostic code needs an entry in `beck-diag/src/index.rs` or `cargo test` fails.
  [`docs/34-generated-documentation-report.md`](docs/34-generated-documentation-report.md) records
  what is generated, what is written, and what it does not do.
- [`compiler/lib/`](compiler/lib/README.md) is the standard library's **Beck half**: a host's table
  or grammar is a primitive in `prelude.rs`, and composition is a file there. Each carries its own
  `test` blocks, and `beck-cli/tests/stdlib.rs` gates the directory rather than a list — a file
  added there is gated by being there.
- [`compiler/corpus/`](compiler/corpus/) is 30 programs — 29 single files and one three-module
  project — carrying **no placement annotations**, and the measurement behind Phase 2's exit
  criterion. A program added there has to place itself.
- [`compiler/sicp/`](compiler/sicp/) is the expressiveness benchmark
  ([`docs/25`](docs/25-benchmarks-and-expressiveness.md) §25.5): chapters of SICP in Beck, with the
  book's own stated answers as the oracle, and one file per remaining wall in `sicp/refusals/` whose
  test asserts the wall is still there. A wall coming down is a test that starts failing. All six
  §25.6 measured are down, and so are the three the removals wrote, so `sicp/refusals/` is
  **empty** — which claims that every wall this project has found has been removed, and not that
  Beck expresses SICP. `sicp/refusals/README.md` holds that distinction and says what puts a file
  back.
- Security posture is [`docs/43-threat-model.md`](docs/43-threat-model.md) (who is defended
  against, and who is not) and `SECURITY.md` (how a report reaches us). What is *absent* is asserted
  as absent in `beck-cli/tests/pending_security.rs`: building one of those controls turns a test
  red, and correcting docs/43 §43.4 in the same change is what the red test is for.
- Design decisions are numbered in [`docs/10-decisions.md`](docs/10-decisions.md). If a change
  contradicts one, say so rather than quietly diverging. Engineering decisions — a dependency
  taken or refused, a gate's shape, an upgrade path — are recorded in [`docs/adr/`](docs/adr/).

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
  [`docs/31-tail-calls-report.md`](docs/31-tail-calls-report.md) §31.5–§31.6 and
  [`docs/32-numeric-tower-and-polymorphism-report.md`](docs/32-numeric-tower-and-polymorphism-report.md)
  §32.5 and
  [`docs/33-effect-polymorphism-and-list-patterns-report.md`](docs/33-effect-polymorphism-and-list-patterns-report.md)
  §33.6 and
  [`docs/36-parameterised-types-report.md`](docs/36-parameterised-types-report.md) §36.6 and
  [`docs/41-generic-arithmetic-report.md`](docs/41-generic-arithmetic-report.md) §41.4.
- The harnesses are the project's conscience (§4.8, §8.3): `compiler/crates/beck-cli/tests/` holds
  the differential, replay-determinism, backend-seam, scaling, security, corpus, placement-property,
  general-slicer, incremental-analysis, incremental-engine, shared-arrangement, subscription,
  view-metrics, SICP, tests-in-Beck, UI, workflow-cross-check, documentation and
  diagnostic-snapshot suites, plus the two release-only measurement suites (`measure_phase2`,
  `measure_incremental`). Keep them green.
- The CI workflow is an artefact too, and Phase 2 found that it had never run
  ([`docs/20-phase-2-report.md`](docs/20-phase-2-report.md) §20.4 item 8). If you change
  `.github/workflows/`, run the steps you changed by hand before trusting them.
- `beck-rt` must not depend on any backend crate. Execution goes through
  `beck_core::backend::Backend`, and `tests/backend_seam.rs` drives the runtime with an
  implementation that is not the evaluator so the seam stays load-bearing (docs/19 §19.9). Anything
  the runtime needs to *know* about a backend goes on that trait — `Backend::stack_bytes` is how the
  runtime sizes a thread for the tree-walker without naming it (docs/31 §31.3).
- A program's own behaviour is asserted in Beck, not only in Rust. `beck test` runs `test` and
  `property` blocks ([`docs/21-tests-in-beck-and-proof.md`](docs/21-tests-in-beck-and-proof.md)
  §21.2–§21.3); a change to what a program *means* should move a test in the program, and
  `compiler/crates/beck-cli/tests/tests_in_beck.rs` is where the construct itself is held to account.
- Say plainly when something is written but unproven. "Built" and "runs" and "measured" are three
  different claims.
- A code comment states the point — a constraint, an invariant, a non-obvious why — and the
  context a reader needs, nothing else. Never narrate history ("this was broken, now it works"),
  a review, or a conversation ("we decided…"); that belongs in the commit message, an ADR
  ([`docs/adr/`](docs/adr/)), or a report. Docs and comments are the current state of things.

## Working in an isolated or cloud environment

- **The first `cargo` command downloads the pinned toolchain** (`rust-toolchain.toml`, 1.94.1,
  ~2 minutes). Do not run a second `cargo` or `rustup` process until it finishes: concurrent
  first-runs race inside rustup and corrupt the install. Repair:
  `rustup toolchain uninstall 1.94.1 && rustup toolchain install 1.94.1`.
- **Verification, cheapest first** (from `compiler/`): `cargo test -p <crate>`, then
  `cargo test --workspace --all-targets`, `cargo clippy --workspace --all-targets` and
  `cargo fmt --all --check` before pushing. CI denies warnings.
- **Environment-dependent suites degrade by skipping, and a skip prints itself** — read the
  output for it:
  - Kubernetes conformance (`beck-infra/tests/conformance.rs`): skips without a cluster;
    `BECK_REQUIRE_CLUSTER=1` forbids the skip. Do not claim this rung ran without one.
  - Postgres log contract (`beck-rt/src/log.rs`): runs only with `BECK_PG=<url>`
    (`BECK_REQUIRE_PG=1` forbids the skip). A local server works: `initdb`/`pg_ctl` as the
    `postgres` user, in a directory that user can traverse (`/tmp`, not a root-owned dir).
  - Compose parity needs Docker; the thin-client budget needs `brotli` (apt-installable).
- **The measurement suites are release-only by convention**: the reproducible form is
  `cargo test --release --test <suite> -- --nocapture`. They also run in debug under the full
  suite with their tables swallowed; that is expected.
- **The network is proxied and partial.** crates.io and the toolchain host work; docs hosts may
  not. Read a dependency's API from its vendored source under `~/.cargo/registry/src/`.
- **Run CI steps you change by hand** (the §20.4 rule above): the deterministic gates need only
  `python3` (with PyYAML) and git, so they run anywhere.
