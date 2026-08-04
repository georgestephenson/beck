# Architecture decision records

One file per engineering decision: context, the decision, consequences — a few lines each.
Records are immutable; a reversal is a new record that supersedes the old one.

The split this directory creates:

- **Main docs and code comments state the current truth.** No "this used to be", no "we decided
  after discussion" — a comment or doc says what is, and links here when the why needs history.
- **ADRs hold the why.** The circumstances, the alternatives, the reason.
- **[`docs/10-decisions.md`](../10-decisions.md) (D1–D20) stays as is** — the design decisions
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
| [0010](0010-generic-arithmetic-through-a-prelude-trait.md) | Numeric operators resolve through a prelude trait — supersedes [0008](0008-numeric-operators-resolved-ad-hoc.md) |
| [0011](0011-identifiers-are-snake-case-in-the-python-surface.md) | Identifiers are `snake_case` in the Python surface and kebab-case in the S-expression one |
| [0012](0012-the-front-end-counts-its-own-recursion.md) | The front end counts its own recursion, against one ceiling and one declared stack |
| [0013](0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md) | The host of an outbound call is written at the call site, so the egress policy is derivable |
| [0014](0014-a-keyed-digest-is-the-one-declassifier.md) | A keyed digest is the one declassifier, and it is a capability |
| [0015](0015-blake3-for-the-standard-librarys-digests.md) | BLAKE3 for the standard library's digests, and no signature library yet |
| [0016](0016-the-language-server-speaks-json-rpc-directly.md) | The language server speaks JSON-RPC directly, and takes no LSP framework |
| [0017](0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md) | SQLite is a substrate for its transaction, not its speed — and durability is a type |
| [0018](0018-the-standard-library-is-carried-in-the-compiler.md) | The standard library's Beck half is carried in the compiler, and an import resolves against the caller's directory first |
