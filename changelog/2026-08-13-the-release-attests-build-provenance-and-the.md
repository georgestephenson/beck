- **2026-08-13 · #52 — The release attests build provenance, and the installer can check it**
  ([`adr/0028`](../docs/adr/0028-a-release-carries-provenance-and-still-no-signature.md), superseding
  [`0027`](../docs/adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md)): `actions/attest`
  over the same `SHA256SUMS` that `install.sh` verifies, and `BECK_VERIFY_PROVENANCE=1` runs
  `gh attestation verify`. Written and not executed — no tag has been pushed. Gates in
  `release.rs` and `pending_security.rs`.
