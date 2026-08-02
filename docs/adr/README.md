# Architecture decision records

One file per engineering decision: context, the decision, consequences — a few lines each.
Records are immutable; a reversal is a new record that supersedes the old one.

The split this directory creates:

- **Main docs and code comments state the current truth.** No "this used to be", no "we decided
  after discussion" — a comment or doc says what is, and links here when the why needs history.
- **ADRs hold the why.** The circumstances, the alternatives, the reason.
- **[`docs/10-decisions.md`](../10-decisions.md) (D1–D18) stays as is** — the design decisions
  that shaped the language, made before this convention existed. New *design* decisions may still
  earn a D-number; ADRs record *engineering* decisions: a dependency taken or refused, a gate's
  shape, an upgrade path.
- **Phase reports stay as is** — they are history by charter and are not rewritten.

| # | Record |
|---|---|
| [0001](0001-adopt-adrs.md) | Adopt architecture decision records |
| [0002](0002-salsa-0.28.md) | Salsa spine on salsa 0.28 |
| [0003](0003-redb-held-at-2.md) | redb held at major version 2 |
| [0004](0004-full-cargo-deny-gate.md) | The full cargo-deny check gates the compiler workspace |
| [0005](0005-workflows-cross-check.md) | Workflows cross-check each other's YAML validity |
| [0006](0006-ci-measurements-lane.md) | A release-profile measurements lane in CI |
| [0007](0007-evaluator-stack-is-declared-not-discovered.md) | The evaluator's recursion bound is a count, and its stack is declared on the backend seam |
| [0008](0008-numeric-operators-resolved-ad-hoc.md) | Numeric operators are resolved from their operands, not from a type class |
| [0009](0009-generated-reference-documentation.md) | Reference documentation is generated from the compiler, and checked in |
