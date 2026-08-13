# `release/`

The executable half of [`docs/28-releases-and-deployment.md`](../docs/28-releases-and-deployment.md)
§28.2, kept out of the workflow on purpose.

A release workflow is the one artefact in this repository that **cannot be run before it is used**:
it is triggered by a tag, so the first time it executes is the first release. Everything else here
is run by hand once before it is trusted ([`19`](../docs/19-phase-1-report.md) §19.4 item 10). So the
parts that *can* be executed live in these two files and in [`../install.sh`](../install.sh), and
[`.github/workflows/release.yml`](../.github/workflows/release.yml) is reduced to a schedule over
them.

| File | What it does | Run it |
|---|---|---|
| [`version.sh`](version.sh) | Prints the version under `[workspace.package]` in `compiler/Cargo.toml` — the one place a release number is read from | `release/version.sh` |
| [`build.sh`](build.sh) | Builds `beck` for one target and writes `dist/beck-<version>-<target>.tar.gz` and its `.sha256` | `release/build.sh --out dist` |
| [`../install.sh`](../install.sh) | Downloads a release for this platform, checks it against `SHA256SUMS`, installs it | `curl -fsSL …/install.sh \| sh` |

The two properties worth knowing:

- **The tag and the workspace version cannot disagree.** `build.sh --expect-tag v0.3.0` refuses
  unless `compiler/Cargo.toml` says `0.3.0`, so a mistyped tag is a failed build rather than a
  binary whose own `--version` contradicts the page it came from.
- **A checksum is not a signature, and the provenance is.** `SHA256SUMS` proves a download was not
  corrupted in transit and says nothing about the release page. What does is the SLSA build
  provenance the workflow attests over every artefact that file lists
  ([`109`](../docs/109-provenance-report.md),
  [`adr/0028`](../docs/adr/0028-a-release-carries-provenance-and-still-no-signature.md)) — a
  Sigstore-signed statement, in a public transparency log, that this repository's release workflow
  produced those digests. `install.sh` checks it when asked
  (`BECK_VERIFY_PROVENANCE=1`, which needs `gh`) and not by default. The release *listing* is still
  signed by nobody: that is [`99`](../docs/99-supply-chain-report.md) §99.7's remaining row, and
  [`104`](../docs/104-the-release-and-the-installer-report.md) §104.6 says why `beck sign` cannot
  take it.

`compiler/crates/beck-cli/tests/release.rs` is the gate: it runs `build.sh` against a wrong version,
runs `install.sh` against a corrupted archive and against a verifier that refuses, and asserts that
the platforms `install.sh` offers are exactly the ones the workflow builds.
