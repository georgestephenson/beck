# 109 — What a release can prove about itself

**Built.** Every artefact a release publishes is now the subject of a SLSA build provenance
attestation — an in-toto statement signed by a short-lived Sigstore certificate whose identity is
this repository's release workflow, recorded in the public-good transparency log. `install.sh` can
check it, and does not by default. [`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md)
is the decision; this is what building it found.

The row it fills has been empty for three reports with the same reason attached each time.
[`92`](92-sbom-report.md) §92.5 gave it: "this repository has no release pipeline to attach one to".
[`104`](104-the-release-and-the-installer-report.md) §104.6 built the pipeline and observed, in the
same sentence, that "that reason is gone, and the row is still empty". This is that row.

## 109.1 What the checksum could never do, stated once more

[`104`](104-the-release-and-the-installer-report.md) put the limit of a published checksum in a
sentence that appears in four places on purpose:

> A checksum published beside the artefact it describes proves the download was not corrupted in
> transit. It proves nothing about the release page. Whoever can rewrite the tarball can rewrite the
> line describing it.

The whole of what this change does is break the second half of that. A provenance attestation is not
published beside the artefact — it is in GitHub's attestations API and in a transparency log, signed
by an identity that is not "whoever can write to this repository's releases" but "this workflow file
on this repository". Rewriting the tarball and the sums line leaves the attestation describing the
bytes that were actually built, and `gh attestation verify` looks the artefact up by the digest it
computes from the file on disk.

What it does **not** do is make the release page trustworthy. Nothing signs the listing;
[§109.7](#1097-what-this-leaves-absent) is the honest version.

## 109.2 The subject is the sums file, and that is the only interesting design decision

`actions/attest` takes a subject three ways: a path (or glob), a digest, or a **checksums file**.
The release already assembles one — `SHA256SUMS`, concatenated from the per-target `.sha256` files
and re-checked with `sha256sum -c` before anything else happens — so the third is available, and it
is the one taken:

```yaml
- uses: actions/attest@v4
  with:
    subject-checksums: staging/SHA256SUMS
```

`subject-checksums` attests one subject per line. So the set of digests the attestation vouches for
**is** the set `install.sh` verifies against, read from the same bytes, rather than two lists that
agree on the day they were written. A glob over `staging/*.tar.gz` would produce an identical
attestation today and a different one the first time an artefact stops being listed — which is
exactly the failure [`92`](92-sbom-report.md) §92.2 designed the SBOM's gate against, in a different
document about a different artefact: an inventory assembled beside the thing it describes is one
that can be wrong about it, quietly.

`release.rs::the_pipeline_attests_the_file_the_installer_verifies_against` asserts the input by
name, and the mutation that motivates it is in [§109.5](#1095-what-each-gate-would-have-to-see-to-go-red).

## 109.3 Verification is opt-in, and a missing tool is fatal

`BECK_VERIFY_PROVENANCE=1` makes the installer run:

```
gh attestation verify <tarball> --repo <owner/repo> \
  --signer-workflow <owner/repo>/.github/workflows/release.yml \
  --predicate-type https://slsa.dev/provenance/v1
```

Three things about that are decisions rather than defaults.

**`--signer-workflow` is the whole of it.** `--repo` alone accepts any attestation this repository
can produce — including one minted by a workflow that whoever rewrote the release page also added.
The check would pass and assert nothing. So `release.rs` reads back the arguments the installer
invoked the CLI with, rather than asserting that a call happened; the gate is written against the
flag, because the flag is the property.

**It is off by default**, because `gh` is not a tool a script piped from `curl` can assume, and the
alternative — a Fulcio chain and a Rekor inclusion proof in POSIX shell — is not a thing to write.
The cost of that choice is stated in [§109.7](#1097-what-this-leaves-absent) and asserted as an
absence rather than described.

**Asked for and unavailable is a failed install**, not a skipped step. `gh` is resolved *before
anything is downloaded*, for the reason `install.sh` already resolves its SHA-256 tool that way and
says so in a comment: an installer that skips verification when the tool is missing has taught its
users that verification is optional. `release.rs` asserts the ordering too — a run that cannot
finish should not have fetched anything.

`BECK_GH` names the CLI, in the pattern `BECK_CLANG`, `BECK_LINKER` and `BECK_CHROME` already use
for a tool a harness has to reach on a machine with several. It is also what makes the whole
verification path testable on a machine that has never seen `gh` ([§109.4](#1094-what-was-executed-and-what-was-only-written)).

## 109.4 What was executed, and what was only written

The distinction [`AGENTS.md`](../AGENTS.md) insists on — "built", "runs" and "measured" are three
different claims — applied to this change, and it is unflattering in one specific place.

**Executed, on this machine:**

- `install.sh` against a fixture release with a stub verifier that **refuses**: exit 1, "the build
  provenance … did not verify", and no file in the install directory.
- The same with a stub that **agrees**: installed, and the recorded argument list contains
  `attestation verify`, `--repo` and `--signer-workflow` naming a workflow file that exists.
- `BECK_VERIFY_PROVENANCE=1` with `BECK_GH` pointing at a path that is not a file: exit 1, a message
  naming the variable and the CLI, and **nothing downloaded** — the failure happens before the
  fetch.
- The default path with a verifier that refuses everything: installed, and the verifier was never
  called. That is the absence, asserted as behaviour.
- Seven mutations of `install.sh` and `release.yml`, each producing exactly one failing test
  ([§109.5](#1095-what-each-gate-would-have-to-see-to-go-red)).
- Every workflow file, including the edited one, through PyYAML.

**Written and not executed:**

- **The attestation itself.** No tag has been pushed and no `workflow_dispatch` has been run, so
  `actions/attest` has never executed in this repository: not the OIDC exchange, not the Sigstore
  certificate, not the transparency-log entry, not the upload. What is checked is that the workflow
  parses, that the step's input is the file the installer reads, that the two permissions it needs
  are on the one job that needs them, and that nothing makes the step conditional.
- **`gh attestation verify` against a real attestation.** Every run above used a stub, because the
  fixture release is local and no attestation exists to look up. What the stub establishes is the
  installer's behaviour *around* a verifier — refusal stops the install, a missing tool is fatal,
  and the call is made with `--signer-workflow`. What it cannot establish is that the real CLI
  accepts the real bundle. That is one command on the day of the first release, and until then this
  is a written claim.
- **The permissions.** `id-token: write` and `attestations: write` have never been granted to a job
  that ran.

The dry-run path exists to shorten that list: the attest step is deliberately **not** guarded by
`if: github.event_name == 'push'`, so a `workflow_dispatch` run — which publishes nothing — still
mints an attestation over the artefacts it built. That is the only way any of the second list moves
into the first before a tag is cut, and `release.rs` asserts the guard's absence so it cannot be
added back without a red test.

## 109.5 What each gate would have to see to go red

[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5's pattern: write the gate against the
shape of the gap, then check that the gap would make it fire. Each row is a mutation that was
applied, run, and reverted.

| Mutation | What went red |
|---|---|
| `--signer-workflow` dropped from the installer's command | `the_installer_refuses_an_archive_whose_provenance_does_not_verify` |
| A failed verification warns instead of dying | Same |
| A missing `gh` silently turns verification off | `provenance_verification_is_refused_rather_than_skipped_when_the_tool_is_missing` |
| Verification defaults to **on** | `the_default_install_checks_a_checksum_and_not_the_provenance` — the absence gate, firing as designed |
| `if: github.event_name == 'push'` added to the attest step | `the_pipeline_attests_the_file_the_installer_verifies_against` |
| `subject-checksums` replaced by a glob over the tarballs | Same |
| `id-token: write` granted at the top of the workflow instead of on `publish` | Same |

The fifth row is the one worth recording, because **the first version of that assertion did not
catch it**. It sliced the step's text from `uses: actions/attest@` to the next `- name:` and looked
for `if:` — and a condition written *above* `uses:`, which is where a person would naturally put it,
was outside the slice. The gate passed on the mutation it existed for. It now slices the step the
way YAML delimits one, from its `- name:` to the next, and the mutation fires it.

That is [`85`](85-what-the-generator-found-report.md) §85.1's lesson arriving in a much smaller
place: a gate calibrated against the shape you happened to write tests that you wrote it, not that
the property holds.

## 109.6 The stub, and what a stub can be evidence for

`support::relfix` builds a fixture release — a tarball with a stub `beck` in it, a `SHA256SUMS`
beside it, reached over `file://` — and stands a shell script in for `gh` that records its argument
list and exits with a chosen code. `release.rs` and `pending_security.rs` both install from it,
which is why it is one module rather than two fixtures that could disagree about what a release
looks like.

The honest boundary: a stub verifier is evidence about **the installer**, not about the
verification. Every property in [§109.4](#1094-what-was-executed-and-what-was-only-written)'s first
list is a property of `install.sh` — what it does when a verifier says no, when one is absent, when
one is not asked for. Nothing here says an attestation verifies. That claim needs the first release.

This is the same shape as [`92`](92-sbom-report.md) §92.6's "nothing consumes it": the fix is to run
somebody else's client against the real artefact, and the release is when that becomes possible.

## 109.7 What this leaves absent

- **The default install is unchanged.** `curl … | sh` checks a checksum. The provenance is there
  and nobody is made to look at it, which is the narrowed form of the entry
  `pending_security.rs` used to hold and now holds again with a different name.
- **The release page is signed by nobody.** `SHA256SUMS` carries no signature of its own, and no
  artefact carries a `cosign`-readable one. A reader who checks the sums and stops has checked one
  file on the page against another file on the page. The signing machinery
  [`99`](99-supply-chain-report.md) §99.5 built still cannot take this subject —
  [`104`](104-the-release-and-the-installer-report.md) §104.6's finding, unchanged.
- **Verification needs the network and needs GitHub.** The attestation lives in GitHub's API and the
  trust root in Sigstore's public-good instance. An air-gapped consumer has the checksum.
- **SLSA Build L3 is not claimed.** What is built is provenance with a builder identity and a
  transparency log, which is the shape of the Build track's requirement, on GitHub-hosted runners
  whose isolation this project has not audited. [`12`](12-standards-and-conformance.md) §12.6 states
  the target; this report does not claim the level, and the Source track is untouched.
- **The SBOM is still unsigned and still version-less.** `actions/attest` has an SBOM mode, and the
  compiler's SBOM is written per *program* rather than for the release, so there is nothing here to
  point it at. [`92`](92-sbom-report.md) §92.5's rows are unmoved, including the mandatory-signature
  element [`42`](42-security-assurance.md) §42.7 retargeted at.
- **Trusted publishing** on crates.io is untouched, and remains the other half of Wave 5's
  supply-chain row. It is the one control that has to be configured *before* an action rather than
  after it, and nothing here changes that clock.

## 109.8 What this corrects, elsewhere

| Where | What changed |
|---|---|
| [`adr/0027`](adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md) | **Superseded** by [`0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md). Its reasoning is not revised — it named this route as the right one and deferred it, and the reason to defer expired |
| [`43`](43-threat-model.md) §43.4 | "A signature on the compiler you downloaded" is narrowed: the pipeline attests and the installer can check, so the absence is now the *default* rather than the mechanism |
| [`12`](12-standards-and-conformance.md) §12.6 | The SLSA row's "provenance attestations for compiler releases" is emitted rather than planned; the Source track and the level claim are not |
| [`42`](42-security-assurance.md) §42.7, §42.8 | "What is left here is the transparency log, the provenance statement and a signature over the SBOM" loses its first two items for the compiler's own release |
| [`28`](28-releases-and-deployment.md) §28.2 | Item 2's "a provenance attestation, **not**" is now built; the section's closing paragraph named a signature over `SHA256SUMS` as the next slice, and that is still the next slice |
| [`08`](08-roadmap.md) §8.5.4, §8.5.5 | Wave 5's supply-chain row loses the provenance half; Lane D's items lose "SLSA" and keep "SBOM/trusted publishing" |
| [`92`](92-sbom-report.md) §92.5, [`104`](104-the-release-and-the-installer-report.md) §104.6, [`99`](99-supply-chain-report.md) §99.7 | Reports, so not edited. Each says the provenance row is empty; it is this report that fills it, and §109.7 says what of those rows is still open |
