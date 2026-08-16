# Architecture decision records

One file per engineering decision: context, the decision, consequences — a few lines each.
Records are immutable; a reversal is a new record that supersedes the old one.

The split this directory creates:

- **Main docs and code comments state the current truth.** No "this used to be", no "we decided
  after discussion" — a comment or doc says what is, and links here when the why needs history.
- **ADRs hold the why.** The circumstances, the alternatives, the reason.
- **Phase reports stay as is** — they are history by charter and are not rewritten.

## Here or a D-number

Both registers hold decisions and both are numbered, so which one a decision goes in has to be
decidable by somebody who is not its author.

> **A D-number is a rule a Beck program lives under. An ADR is a choice only the compiler lives
> under.**

The test is one question: **could a user observe this without reading our source?** If yes it is
[`docs/10-decisions.md`](../10-decisions.md); if no it is a record here. A dependency taken or
refused, a gate's shape, an allocator, an upgrade path — nobody writing Beck can tell, so they are
ADRs. How `+` resolves, what an identifier may be spelled, where the host of an outbound call is
written — every program shows it, so those are D-numbers.

**This replaces "design decisions there, engineering decisions here"**, which was the rule from
[`0001`](0001-adopt-adrs.md) until [`102`](../102-styling-and-the-component-library.md)'s wave and
which is not decidable: whether a thing is design or engineering is a judgement about intent, and
the same judgement went both ways. At least six records here state a rule a program lives under —
[`0010`](0010-generic-arithmetic-through-a-prelude-trait.md) (how `+` resolves),
[`0011`](0011-identifiers-are-snake-case-in-the-python-surface.md) (identifier case),
[`0013`](0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md),
[`0014`](0014-a-keyed-digest-is-the-one-declassifier.md) (the one declassifier),
[`0017`](0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md) (durability is a type) and
[`0007`](0007-evaluator-stack-is-declared-not-discovered.md)/[`0012`](0012-the-front-end-counts-its-own-recursion.md)
(the recursion ceiling a program hits) — and under the rule above each would have been a D-number.

**They are not being moved, and the reason is the one that makes this directory worth having.** A
record here is immutable and is cited by identity: `adr/0007` and `adr/0012` from
[`front_end_bound.rs`](../../compiler/crates/beck-cli/tests/front_end_bound.rs), `adr/0013` from
[`lib/README.md`](../../compiler/lib/README.md), and more from `AGENTS.md` and the design documents.
Relocating a record to satisfy a rule written afterwards would break those citations *and* the
immutability that is the whole difference between the two registers. The rule governs what is filed
next; where a misfiled record's rule needs stating as current truth, the design document that owns
the subject states it and links here for the why — which is what every document is supposed to do
with an ADR anyway.

**What one record can do is both.** [`10`](../10-decisions.md) D23 decides that the standard library
needs no declaration and that the caller's directory wins — a rule every program lives under — and
[`0018`](0018-the-standard-library-is-carried-in-the-compiler.md) records that the library is carried
in the binary, with the three alternatives refused. Neither restates the other, and each links to
the other. That is the shape to copy.

## The conventions a record follows

- **The filename is `NNNN-slug.md` and the title states the same number.** The number is the
  record's identity and it is what citations name, so the two agreeing is not a formality:
  `0023` was titled `ADR 0022` — a real record's number — from the day it was written, and a
  reader following a citation landed on a page about the wrong decision.
  `docs.rs::an_adr_is_numbered_for_the_file_it_is_in_and_is_listed` is the gate, and it also
  refuses two records claiming one number and a record the index below does not name.
- **The `ADR ` prefix in the title is optional.** The first fifteen records were written without it
  and the rest with it; the gate reads the number either way rather than making thirty files agree
  about prose.
- **A record is immutable.** A reversal is a new record that supersedes the old one and says so,
  the way [`0010`](0010-generic-arithmetic-through-a-prelude-trait.md) supersedes
  [`0008`](0008-numeric-operators-resolved-ad-hoc.md) and
  [`0028`](0028-a-release-carries-provenance-and-still-no-signature.md) supersedes
  [`0027`](0027-a-release-publishes-a-checksum-and-not-a-signature.md).

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
| [0019](0019-a-modern-allocator-for-the-evaluator.md) | mimalloc is the `beck` binary's global allocator — a third of the evaluator was inside `malloc` |
| [0020](0020-the-read-model-speaks-pgwire-by-hand.md) | The read model speaks pgwire by hand, and answers to nobody — loopback only, off by default |
| [0021](0021-the-native-backend-writes-ir-and-runs-a-process.md) | The native backend writes LLVM IR as text and runs the result as a process, so `forbid(unsafe)` survives it |
| [0022](0022-mode-b-ships-the-backend-it-has.md) | Mode B's kernel is the evaluator compiled to WebAssembly, not a WebAssembly code generator — the mode's questions are not the backend's |
| [0023](0023-tls-and-the-signature-it-brings.md) | rustls with aws-lc-rs — one dependency for transport security *and* the asymmetric signature an OIDC relying party verifies |
| [0024](0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md) | Cranelift as a crate, emitting an object a linker turns into a program — the `unsafe` ADR 0021 refused was in *running* code, not in generating it |
| [0025](0025-deflate-so-the-image-build-needs-no-tools.md) | DEFLATE is taken and tar is written — the image build needs an inflater it cannot write and a byte-deterministic tar no library offers |
| [0026](0026-the-native-heap-is-an-arena-of-offsets.md) | The native heap is an arena of offsets and the host marshals against it — a value that carries no pointer crosses a pipe as bytes, so neither emitter generates a line of marshalling |
| [0027](0027-a-release-publishes-a-checksum-and-not-a-signature.md) | A release publishes a checksum and not a signature — `beck sign`'s subject is an image manifest, and a compiler release is a tarball |
| [0028](0028-a-release-carries-provenance-and-still-no-signature.md) | A release carries SLSA build provenance over `SHA256SUMS`, checkable on request and not by default — supersedes [0027](0027-a-release-publishes-a-checksum-and-not-a-signature.md) |
| [0029](0029-the-runtime-library-is-linked-and-owns-the-arena.md) | A primitive that is somebody else's table is linked rather than emitted or asked for, and the runtime library **owns** the arena so that no pointer crosses its ABI — which is what keeps `forbid(unsafe_code)` true |
