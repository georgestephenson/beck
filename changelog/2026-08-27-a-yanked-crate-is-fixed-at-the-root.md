- **2026-08-27 — `chacha20 0.10.1` was yanked upstream, and both lockfiles moved to `0.10.2`.**
  [`docs/07`](../docs/07-dependencies.md), [`compiler/deny.toml`](../compiler/deny.toml)'s
  `yanked = "deny"`. A yank is retroactive: nothing in this repository changed, and
  `cargo-deny`'s advisories check went red on `compiler/Cargo.lock` and `phase0/Cargo.lock`
  together — the crate arrives in both through `rand 0.10.2` → `postgres-protocol` and
  `tungstenite`, so a branch that touches neither lockfile still fails. Fixed at the root rather
  than muted, which is what `deny.toml`'s comment beside the empty `ignore` list asks for: the
  allowance that outlives its reason is the one that waves the *next* yank of the same crate
  through. `0.10.0` and `0.10.1` are both yanked and `0.10.2` is not, so the change is one version
  and one checksum per lockfile and nothing else.
  `cargo-deny` is not installed on the machine this was fixed on, so the property it asserts was
  checked directly instead: every one of the 605 crates the two lockfiles take from crates.io, read
  against the sparse index, is at a version that is not yanked. The sweep was run against the old
  lockfile first, where it reports the one crate CI rejected, so it is not passing by looking at
  nothing.
