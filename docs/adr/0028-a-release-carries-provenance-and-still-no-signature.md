# ADR 0028 — A release carries build provenance, and still no signature

**Status:** accepted — supersedes [`0027`](0027-a-release-publishes-a-checksum-and-not-a-signature.md)
**Date:** 2026-08-13
**Context:** [`92`](../92-supply-chain-and-release-report.md),
[`92`](../92-supply-chain-and-release-report.md) §92.11,
[`92`](../92-supply-chain-and-release-report.md) §92.6 and §92.15,
[`92`](../92-supply-chain-and-release-report.md) §92.15, [`12`](../12-standards-and-conformance.md) §12.6,
[`43`](../43-threat-model.md) §43.4

## The decision

The compiler's release attests **SLSA build provenance** over every artefact listed in
`SHA256SUMS`, using GitHub's `actions/attest`: an in-toto statement signed by a short-lived
Sigstore certificate whose identity is this repository's release workflow, recorded in the
public-good transparency log. [`install.sh`](../../install.sh) can check it —
`BECK_VERIFY_PROVENANCE=1`, which runs `gh attestation verify` with `--signer-workflow` — and
**does not by default**.

Nothing about the release *page* is signed. `SHA256SUMS` carries no signature, and no artefact
carries a `cosign`-readable one.

## What changed since 0027

[`0027`](0027-a-release-publishes-a-checksum-and-not-a-signature.md) considered three routes and
called this one "the right answer eventually and a Phase 4 item … because it is the *provenance*
half — a builder identity and a transparency log — rather than a signature bolted on beside a
checksum". Nothing about that reasoning has been revised. What changed is that
[`08`](../08-roadmap.md) §8.5.4's Wave 5 exists to arrange the Phase 4 gates *before* Phase 4, and
the one stated reason to wait — [`92`](../92-supply-chain-and-release-report.md) §92.15's "this repository has no release
pipeline to attach one to" — stopped being true when
[`92`](../92-supply-chain-and-release-report.md) built the pipeline.

The other two routes 0027 rejected stay rejected, for the reasons it gives: signing `SHA256SUMS`
with `beck sign` needs a payload that is not simple-signing over an image reference, and
`cosign sign-blob` adds a tool and a key this project would then have to hold.

## Why the subject is `SHA256SUMS`

`subject-checksums` reads that file and attests **one subject per line** — a name and a digest each.
So the set of digests the attestation vouches for is the set `install.sh` verifies against, read
from the same bytes, rather than two lists that happen to agree on the day they are written. A glob
over the tarballs would produce the same attestation today and a different one the day an artefact
stops being listed.

This also answers 0027's objection precisely. Its complaint was that a signature needs a *subject
the signer can take*, and that a tarball on a releases page is not an image manifest. A provenance
attestation's subject is a name and a digest, which a tarball has.

## Why verification is opt-in

`gh` is not a tool an installer piped from `curl` can assume is present, and the alternative — full
Sigstore verification in POSIX shell, with a Fulcio certificate chain and a Rekor inclusion proof —
is not a shell script anybody should trust.

The consequence is faced rather than hidden: **the ordinary install path establishes what it
established before**, that the download was not corrupted in transit.
`pending_security.rs::the_default_install_checks_a_checksum_and_not_the_provenance` asserts it by
running the installer with a verifier that refuses everything and watching the install succeed.

What is *not* done is the shape this project has already refused once: verifying when the tool
happens to be present and skipping when it does not. `BECK_VERIFY_PROVENANCE=1` with no `gh` is a
failed install, for the same reason a missing `sha256sum` is
([`92`](../92-supply-chain-and-release-report.md) §92.8) — an installer that skips
verification when a tool is missing has taught its users that verification is optional.

## `--signer-workflow`, and why the check is worth nothing without it

`gh attestation verify --repo` alone accepts **any** attestation this repository can produce.
Whoever could rewrite the release page could add a workflow, run it, and mint provenance for their
own tarball; the check would pass and mean nothing. `--signer-workflow` pins the signing identity
to `.github/workflows/release.yml`, so what is asserted is that *this* workflow, on this
repository, produced the bytes on disk.

`release.rs` reads the arguments the installer actually invoked the CLI with, rather than checking
that a call happens, because the flag is the whole of the property.

## Consequences

- **Positive.** A rewritten tarball on the release page no longer verifies, and the record that
  says so is in a public transparency log that the page's owner does not control. There is still no
  key to generate, hold or rotate: the certificate lives for minutes and the identity is the
  workflow.
- **Negative.** The reader has to ask for the check, and most will not. The gap is narrower than
  0027's and it is the same *kind* of gap, so it stays in [`43`](../43-threat-model.md) §43.4 and in
  `pending_security.rs` rather than being called closed.
- **Negative.** Verification now depends on GitHub's attestations API and on Sigstore's public-good
  instance being reachable. An air-gapped consumer has the checksum and nothing else, which is
  [`06`](../06-kubernetes-and-packaging.md) §6.7's problem rather than this one's.
- **Unchanged.** The image half still signs with `beck sign` over a manifest digest
  ([`92`](../92-supply-chain-and-release-report.md) §92.6), and the two do not meet: this record is about the
  compiler's own release, not about what a user's `beck build` produces.
- **What would supersede this.** A signature over `SHA256SUMS` in a form `cosign verify` reads, or a
  verifier the installer can run without `gh`. Both are still one subject decision plus a
  transparency log, and [`92`](../92-supply-chain-and-release-report.md) §92.15 still lists them.
