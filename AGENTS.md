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
- [`compiler/`](compiler/) is the built Phase 1: the compiler and the runtime it targets.
  [`docs/19-phase-1-report.md`](docs/19-phase-1-report.md) records what it does, what it refuses to
  claim, and the corrections Phase 1 makes to the design documents.
- Design decisions are numbered in [`docs/10-decisions.md`](docs/10-decisions.md). If a change
  contradicts one, say so rather than quietly diverging.

## Standards for changes

- Claims in docs are stated from evidence. If you write a number, it must be reproducible —
  `phase0/tests/measure.sh` is where the Phase 0 numbers come from; the Phase 1 numbers come from
  `cargo test` and the commands quoted in [`docs/19-phase-1-report.md`](docs/19-phase-1-report.md).
- The two harnesses are the project's conscience (§4.8, §8.3): `compiler/crates/beck-cli/tests/`
  holds the differential and replay-determinism suites. Keep them green.
- Say plainly when something is written but unproven. "Built" and "runs" and "measured" are three
  different claims.
