# ADR 0027 — A release publishes a checksum, and not a signature

**Superseded by [0028](0028-a-release-carries-provenance-and-still-no-signature.md)**, which took the
third of the three routes below — GitHub's artifact attestations — for the reason this record gives
for deferring it, once the pipeline it needed existed.

**Status:** superseded
**Date:** 2026-08-10
**Context:** [`104`](../104-the-release-and-the-installer-report.md),
[`28`](../28-releases-and-deployment.md) §28.2, [`99`](../99-supply-chain-report.md) §99.5 and
§99.7, [`43`](../43-threat-model.md) §43.4,
[`0023`](0023-tls-and-the-signature-it-brings.md)

## The decision

The compiler's release publishes, per target, a `.tar.gz` and one `SHA256SUMS` covering all of them.
[`install.sh`](../../install.sh) verifies the archive against that file and refuses to install on a
mismatch. **Nothing is signed**, and the fact that nothing is signed is written into the installer,
the release notes, the threat model and a test.

This is a decision and not an oversight, because this project *has* a signer.
[`99`](../99-supply-chain-report.md) §99.5 built `beck sign` and `beck verify` — a Sigstore-shaped
signature over a digest, in the form `cosign verify --key` reads, checked by `openssl` as well as by
this repository's own code. A reader who knows that would reasonably assume a released binary
inherits it.

## Why it does not

**`beck sign`'s subject is an OCI manifest digest.** It takes the layout `beck image` wrote, reads
the manifest, and signs *that*. A tarball on a releases page is not a manifest, is not in a layout,
and has no digest the command knows how to reach. Signing one is not a flag on an existing command;
it is a second subject type, a second payload shape, and a second verification path — and every one
of those has to be got right or it is worse than nothing, because a signature that verifies the
wrong thing is a claim nobody can audit.

Three cheaper-looking routes were considered and none of them is cheap:

- **Sign `SHA256SUMS` with the existing keyed signer.** Closest, and still a new subject: the
  payload `beck sign` builds is simple-signing over an image reference, so the file would either
  carry a lie in its `critical.image` field or need a different payload — at which point
  `cosign verify --key` is no longer the consumer, and §99.5's whole argument for that shape ("a
  consumer needs no Beck-specific tool") is gone.
- **`cosign sign-blob` in the workflow.** Adds a tool this repository deliberately does not depend
  on ([`99`](../99-supply-chain-report.md) §99.1 spends the argument that a build which executes
  nothing has nothing a compiler cannot do), and moves the trust into a key nobody has generated.
- **GitHub's artifact attestations.** The right answer eventually and a Phase 4 item
  ([`08`](../08-roadmap.md) §8.5.4 Wave 5), because it is the *provenance* half — a builder
  identity and a transparency log — rather than a signature bolted on beside a checksum.

## What the checksum is worth, stated exactly

> A checksum published beside the artefact it describes proves the download was not corrupted in
> transit. It proves nothing about the release page. Whoever can rewrite the tarball can rewrite the
> line describing it.

That sentence appears in `install.sh`, in `release/README.md`, in the release notes the workflow
writes and in [`43`](../43-threat-model.md) §43.4, because the failure mode of this decision is a
reader assuming the opposite. What it *does* buy is real and worth keeping: a truncated download, a
corrupted mirror and a proxy that rewrote a byte are all caught, and the installer treats a missing
SHA-256 tool as fatal rather than installing anyway — an installer that skips verification when the
tool is absent has taught its users that verification is optional.

## Consequences

- **Positive.** No key to generate, hold, rotate or leak before there is a release to sign. No new
  dependency. The gap is small, named in four places, and asserted as an absence in
  `pending_security.rs`, so building the control turns a test red and forces the documents to be
  corrected in the same change.
- **Negative, and this is the real cost.** A compromised release page is undetectable by anything
  the project ships. [`43`](../43-threat-model.md) §43.4 now says so under its own heading rather
  than leaving it to be inferred from a supply-chain report.
- **The upgrade path is not this ADR's to take.** It is one subject decision — what a signature over
  a *release* is about — plus a transparency log, and both belong with the registry push
  [`99`](../99-supply-chain-report.md) §99.7 already lists. Revisit this record then; do not extend
  `beck sign` quietly in the meantime.
