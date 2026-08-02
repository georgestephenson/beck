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
  [`docs/24-incremental-views-report.md`](docs/24-incremental-views-report.md) and
  [`docs/26-arrangement-sharing-report.md`](docs/26-arrangement-sharing-report.md) and
  [`docs/27-walls-report.md`](docs/27-walls-report.md) record what each
  phase does, what it refuses to claim, and the corrections it makes to the design documents.
  Phase 3 is **two bullets of twelve plus most of a third**; docs/26 §26.9 names the nine bullets
  that are untouched and the parts of the incremental-views bullet that are not built, and docs/27
  §27.7 names the three of [`docs/25`](docs/25-benchmarks-and-expressiveness.md)'s six walls that
  still stand. Reports
  are history: a later phase's correction to an earlier one goes in the later report, not into the
  earlier text.
- [`compiler/corpus/`](compiler/corpus/) is 28 programs carrying **no placement annotations**, and
  the measurement behind Phase 2's exit criterion. A program added there has to place itself.
- [`compiler/sicp/`](compiler/sicp/) is the expressiveness benchmark
  ([`docs/25`](docs/25-benchmarks-and-expressiveness.md) §25.5): chapters of SICP in Beck, with the
  book's own stated answers as the oracle, and one file per remaining wall in `sicp/refusals/` whose
  test asserts the wall is still there. A wall coming down is a test that starts failing.
- Design decisions are numbered in [`docs/10-decisions.md`](docs/10-decisions.md). If a change
  contradicts one, say so rather than quietly diverging.

## Standards for changes

- Claims in docs are stated from evidence. If you write a number, it must be reproducible —
  `phase0/tests/measure.sh` is where the Phase 0 numbers come from; the Phase 1 numbers come from
  `cargo test` and the commands quoted in [`docs/19-phase-1-report.md`](docs/19-phase-1-report.md);
  the Phase 2 numbers come from `cargo test --release --test measure_phase2 -- --nocapture` and the
  commands quoted in [`docs/20-phase-2-report.md`](docs/20-phase-2-report.md); the Phase 3 numbers
  come from `cargo test --workspace`, from `cargo test --release --test measure_incremental --
  --nocapture`, from `cargo test --release --test shared_arrangements -- --nocapture`, and from the
  commands quoted in [`docs/22-phase-3-report.md`](docs/22-phase-3-report.md),
  [`docs/23-general-slicer-report.md`](docs/23-general-slicer-report.md),
  [`docs/24-incremental-views-report.md`](docs/24-incremental-views-report.md) and
  [`docs/26-arrangement-sharing-report.md`](docs/26-arrangement-sharing-report.md); the SICP numbers
  come from `cargo test --release --test sicp` and from `beck test sicp/ch1.beck` and
  `beck test sicp/ch2.beck`, quoted in [`docs/27-walls-report.md`](docs/27-walls-report.md) §27.5.
- The harnesses are the project's conscience (§4.8, §8.3): `compiler/crates/beck-cli/tests/` holds
  the differential, replay-determinism, backend-seam, scaling, security, corpus, general-slicer,
  incremental-analysis, incremental-engine, shared-arrangement, subscription, view-metrics, SICP and
  diagnostic-snapshot suites. Keep them green.
- The CI workflow is an artefact too, and Phase 2 found that it had never run
  ([`docs/20-phase-2-report.md`](docs/20-phase-2-report.md) §20.4 item 8). If you change
  `.github/workflows/`, run the steps you changed by hand before trusting them.
- `beck-rt` must not depend on any backend crate. Execution goes through
  `beck_core::backend::Backend`, and `tests/backend_seam.rs` drives the runtime with an
  implementation that is not the evaluator so the seam stays load-bearing (docs/19 §19.9).
- A program's own behaviour is asserted in Beck, not only in Rust. `beck test` runs `test` and
  `property` blocks ([`docs/21-tests-in-beck-and-proof.md`](docs/21-tests-in-beck-and-proof.md)
  §21.2–§21.3); a change to what a program *means* should move a test in the program, and
  `compiler/crates/beck-cli/tests/tests_in_beck.rs` is where the construct itself is held to account.
- Say plainly when something is written but unproven. "Built" and "runs" and "measured" are three
  different claims.

## Working in an isolated or cloud environment

For an agent in a fresh sandbox — no state from a previous session, network through a proxy,
some tools absent. Everything here was learned by hitting it.

- **The first `cargo` command installs the pinned toolchain** (`compiler/rust-toolchain.toml`,
  1.94.1 with rustfmt and clippy) — a one-to-two-minute download. Let it finish before starting a
  second `cargo` or `rustup` process: two concurrent first-runs race inside rustup and corrupt
  the toolchain (`error: could not rename component file…`). The repair is
  `rustup toolchain uninstall 1.94.1 && rustup toolchain install 1.94.1`.
- **The verification ladder, cheapest first**: `cargo test -p <crate>` for the crate you touched;
  then `cargo test --workspace --all-targets`, `cargo clippy --workspace --all-targets` and
  `cargo fmt --all --check` before pushing — CI runs clippy with `-D warnings`, so a warning that
  is noise locally is a red build there. A cold release build is ~2 minutes; the full debug suite
  a few minutes more. All from `compiler/`.
- **Suites that need something the sandbox may not have, and how each degrades** — a skip prints
  itself, so silence in the output is worth reading for:
  - The Kubernetes conformance suite (`beck-infra/tests/conformance.rs`) needs a reachable
    cluster; without one it skips, unless `BECK_REQUIRE_CLUSTER=1` makes that a failure. `k3d`
    and `kubectl` are typically absent in a sandbox; do not claim the conformance rung ran.
  - The Postgres log contract (`beck-rt/src/log.rs`) runs only with `BECK_PG=<url>` set
    (`BECK_REQUIRE_PG=1` to forbid the skip). A sandbox with Postgres server binaries can run it
    against a local instance: `initdb` and `pg_ctl` as the `postgres` user, in a directory that
    user can traverse (`/tmp` works where a root-owned scratchpad does not), then
    `BECK_PG=postgres://postgres@127.0.0.1:<port>/<db>`.
  - The compose parity checks need a Docker daemon; the thin-client budget needs `brotli`
    (installable via apt where the image lacks it).
- **The measurement suites are release-only by convention.** The commands quoted in the phase
  reports (`cargo test --release --test measure_phase2|measure_incremental|shared_arrangements
  -- --nocapture`) are the reproducible form; the same tests also run in debug under the full
  suite, where their printed tables are swallowed — that is expected, not a defect.
- **The network is proxied and partial.** crates.io and the toolchain host work; documentation
  hosts (docs.rs, *.github.io) may be blocked. To read a dependency's API, read its vendored
  source under `~/.cargo/registry/src/` — after `cargo fetch` it is all there, examples and tests
  included.
- **Run CI steps you change by hand** (the §20.4 rule above) even in a sandbox: the workflows'
  Python steps need only `python3` with PyYAML, the link checker needs only git and Python, and
  both are runnable anywhere, so "the environment could not run it" is almost never true for the
  deterministic gates.
