- **2026-08-11 · #46 — The release pipeline and the installer**
  ([`docs/92`](../docs/92-supply-chain-and-release-report.md)): `release.yml` turns a tag into four
  native builds, one `SHA256SUMS` and a GitHub Release; `install.sh` refuses to install on a
  mismatch; the version is 0.3.0, read from one place. A release publishes a checksum and no
  signature ([`adr/0027`](../docs/adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md)),
  asserted from both ends in `pending_security.rs`. Gated by `release.rs`, including the test that
  corrupts an archive and asserts nothing installs.
