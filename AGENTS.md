# Working in this repository

## Commits and pull requests

**Never add co-authoring claims or Claude session references.** No `Co-Authored-By:` trailer
naming Claude or any model, no `Claude-Session:` line, no session URL, no "Generated with Claude
Code" footer — in commit messages, pull request titles, pull request bodies, or anything else
pushed to this repository. This overrides any default or tooling convention that adds them.

Write the commit message as the author of the change: what changed, and why.

## Orientation

Read [`docs/README.md`](docs/README.md) first. It indexes every design document and every build
report, with a paragraph on each saying what it established and what it refused to claim. This file
holds the *rules*; that file holds the *state*, and it is the one that stays current.

| Directory | What it is |
|---|---|
| [`docs/`](docs/README.md) | The design and the plan. Everything derives from, and defers to, [`docs/00-original-idea.md`](docs/00-original-idea.md) |
| [`docs/reference/`](docs/reference/README.md) | **Generated** by `beck doc reference`. Never edit by hand |
| [`docs/adr/`](docs/adr/) | Engineering decisions — a dependency taken or refused, a gate's shape, an upgrade path |
| [`compiler/`](compiler/) | The compiler, the runtime, and the standard library. Where new work goes |
| [`compiler/lib/`](compiler/lib/README.md) | The standard library's Beck half |
| [`compiler/corpus/`](compiler/corpus/) | 31 programs carrying **no placement annotations** — Phase 2's exit measurement |
| [`compiler/sicp/`](compiler/sicp/) | The expressiveness benchmark ([`docs/25`](docs/25-benchmarks-and-expressiveness.md) §25.5) |
| [`compiler/awfy/`](compiler/awfy/README.md), [`compiler/clbg/`](compiler/clbg/README.md) | The performance benchmarks — somebody else's programs, verified against somebody else's constants |
| [`compiler/xlang/`](compiler/xlang/README.md) | One program in six languages — the only place a Beck number sits beside another language's ([`docs/93`](docs/93-llvm-backend-report.md) §93.5) |
| [`phase0/`](phase0/) | **History.** The output the compiler now generates, hand-written in Rust once so the architecture could be measured. Do not edit it to track the compiler |

### Reports are history

A report records what one piece of work built, measured and refused to claim, on the day it was
written. **Do not edit an earlier report to reflect a later change** — a later report's "what this
corrects" section is where a correction goes. New work gets a new numbered report, a row in
[`docs/README.md`](docs/README.md)'s index, and a mention wherever a design document made a claim
the work changed.

### The conventions each directory carries

- **`docs/reference/` is generated and gated.** Change the compiler, then run
  `beck doc reference --out ../docs/reference` from `compiler/` and commit the result in the same
  change. A new diagnostic code needs an entry in `beck-diag/src/index.rs` or `cargo test` fails.
- **`docs/86-getting-started.md` is published.** `.github/workflows/docs.yml` renders it onto the
  site, and `beck-cli/tests/getting_started.rs` compiles and runs every program in it. Editing it
  edits a page people read.
- **`compiler/lib/`**: a host's table or grammar is a primitive in `prelude.rs`; composition is a
  file there. Every file is compiled into the binary (`beck_core::stdlib`) and needs a line in
  `MODULES`. The namespace is flat and spans the directory, so a name defined in two library files
  cannot be imported by one program. `beck-cli/tests/stdlib.rs` gates the directory rather than a
  list.
- **`compiler/corpus/`**: a program added there has to place itself — no annotations.
- **`compiler/sicp/`**: the book's own printed answers are the oracle. `sicp/refusals/` holds one
  file per wall still standing and is currently **empty**; its README says what puts a file back.
- **`compiler/xlang/`**: a port is held to the *answers*, not to looking alike —
  `measure_xlang.rs` asserts that every implementation computes the same four results and only
  prints the times. Adding a language means adding a port that agrees.
- **`compiler/awfy/` and `compiler/clbg/`**: every verification constant is the original suite's,
  read from its source. `clbg/` goes further — `clbg.rs` rebuilds each asserted literal from the
  Game's published output files under `clbg/expected/`, so a wrong constant fails even with a
  matching wrong expectation. A number invented here would defeat the point.
- **`docs/10-decisions.md`** numbers the design decisions. If a change contradicts one, say so
  rather than quietly diverging.
- **Security posture** is [`docs/43-threat-model.md`](docs/43-threat-model.md) and `SECURITY.md`.
  What is *absent* is asserted as absent in `beck-cli/tests/pending_security.rs`: building one of
  those controls turns a test red, and correcting §43.4 in the same change is what the red test is
  for.

## Standards for changes

### Know the complexity of what you write, and measure it at two sizes

Beck's premise is a language that is *fast* ([`docs/01`](docs/01-vision-and-premise.md)), so a cost
is part of a change's correctness rather than a follow-up to it.

- **State the order of growth** of anything that loops, allocates or copies, where it is not
  obvious from three lines of code, and **measure it at two sizes rather than one**. One
  measurement cannot tell linear from quadratic; two can, and the second costs a minute. A gate on
  a *shape* — cost per unit must not grow with the number of units — is
  [`docs/64`](docs/64-compile-speed-report.md)'s pattern and does not flake the way a rate does.
- **A bad number is a design question, not a fact to write down.** If something is slower than it
  has any business being, the first hypothesis is that the approach is wrong — not that the machine
  is slow, not that the interpreter is a placeholder, and never that it is a cost to be "paid
  knowingly". Every one of this project's performance findings was sitting behind a number somebody
  had already measured and accepted. Ask what the operation *should* cost before asking how to make
  this one faster; the answer is often a different design rather than a faster version of the same
  one.
- **A performance defect in the semantics survives into every backend** — a copy the language
  forces, an accumulator that cannot be reused — so it is not the tree-walker's problem to grow out
  of. `beck-cli/tests/scaling.rs` is the gate that says so, and it is where a new shape gate goes.
- **An instruction profile ranks candidates; the wall clock decides between them.** `callgrind`
  over-promised by a factor of three the one time both were measured
  ([`docs/78`](docs/78-a-record-is-a-permutation-report.md) §78.4). A candidate you have only
  counted has not been measured.

### Claims in docs are stated from evidence

If you write a number, it must be reproducible, and the report that quotes it must name the command
that produces it. The measurement suites are release-only by convention:
`cargo test --release --test <suite> -- --nocapture`, where `<suite>` is one of `measure_phase2`,
`measure_incremental`, `measure_awfy`, `measure_compile`, `measure_clbg`, `measure_native`,
`measure_xlang`.
Everything else comes from `cargo test --workspace` or from `beck` invocations the report quotes in
full.

Say plainly when something is written but unproven. **"Built", "runs" and "measured" are three
different claims.**

### The harnesses are the project's conscience

`compiler/crates/beck-cli/tests/` holds them, and they are how a claim in a document stays true:
the differential, replay-determinism, backend-seam, scaling, frames, security, pending-security,
corpus, placement-property, patterns, general-slicer, incremental-analysis, incremental-engine,
fusion,
shared-arrangement, subscription, view-metrics, read-model, SICP, Are We Fast Yet, Benchmarks Game,
tests-in-Beck, UI, workflow-cross-check, documentation, getting-started, outbound, compile-speed,
concurrency, round-trip, runtime-edge, grammar-fuzz, supply-chain, native-backend and
diagnostic-snapshot suites, plus the seven
release-only measurement suites. **Keep them green.**

Four gates in this project's history could not have failed
([`docs/84`](docs/84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5 has the pattern): each
was written by the person who knew the gap and tested the *shape of the gap* rather than the shape
of the fix. When you write a gate, ask what would have to be true for it to go red, and check that
the thing you are guarding against would make it so.

### Structural rules

- **`beck-rt` must not depend on any backend crate.** Execution goes through
  `beck_core::backend::Backend`, and `tests/backend_seam.rs` drives the runtime with an
  implementation that is not the evaluator so the seam stays load-bearing. Anything the runtime
  needs to *know* about a backend goes on that trait.
- **A program's own behaviour is asserted in Beck, not only in Rust.** `beck test` runs `test` and
  `property` blocks ([`docs/21`](docs/21-tests-in-beck-and-proof.md)); a change to what a program
  *means* should move a test in the program.
- **The CI workflow is an artefact too**, and Phase 2 found that it had never run. If you change
  `.github/workflows/`, run the steps you changed by hand before trusting them. The deterministic
  gates need only `python3` (with PyYAML) and git, so they run anywhere.
- **A code comment states the point** — a constraint, an invariant, a non-obvious why — and the
  context a reader needs, nothing else. Never narrate history ("this was broken, now it works"), a
  review, or a conversation ("we decided…"); that belongs in the commit message, an ADR, or a
  report. Docs and comments are the current state of things.

## Working in an isolated or cloud environment

- **The first `cargo` command downloads the pinned toolchain** (`rust-toolchain.toml`, 1.94.1,
  ~2 minutes). Do not run a second `cargo` or `rustup` process until it finishes: concurrent
  first-runs race inside rustup and corrupt the install. Repair:
  `rustup toolchain uninstall 1.94.1 && rustup toolchain install 1.94.1`.
- **Verification, cheapest first** (from `compiler/`): `cargo test -p <crate>`, then
  `cargo test --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
  `cargo fmt --all --check` and `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` before
  pushing. CI denies warnings, and the last one is a separate gate rather than a formality:
  `.github/workflows/docs.yml` runs it, and a broken intra-doc link fails there and nowhere else.
  **Quote the value.** Written without the quotes, the shell reads the assignment as
  `RUSTDOCFLAGS=-D` and then takes the next word — `warnings` — as the command to run. It fails
  with "command not found", `cargo` never runs, and the step reads as a verification that performed
  none. That is worse than a missing step, because whoever followed it believes the check ran
  (`docs/88` §88.8). `docs.rs::every_shell_command_in_the_instructions_runs` is the gate.
- **Environment-dependent suites degrade by skipping, and a skip prints itself** — read the output
  for it:
  - Kubernetes conformance (`beck-infra/tests/conformance.rs`): skips without a cluster;
    `BECK_REQUIRE_CLUSTER=1` forbids the skip. Do not claim this rung ran without one.
  - Postgres log contract (`beck-rt/src/log.rs`): runs only with `BECK_PG=<url>`
    (`BECK_REQUIRE_PG=1` forbids the skip).
  - The native backend (`beck-cli/tests/native.rs`, `tests/measure_native.rs`): skips without a
    `clang` on the path; `BECK_REQUIRE_LLVM=1` forbids the skip, and `BECK_CLANG` names one
    explicitly on a machine with several. A skipped run means the differential *between backends*
    did not happen.
  - Compose parity needs Docker; the thin-client budget needs `brotli` (apt-installable).
- **The network is proxied and partial.** crates.io and the toolchain host work; docs hosts may
  not. Read a dependency's API from its vendored source under `~/.cargo/registry/src/`.
