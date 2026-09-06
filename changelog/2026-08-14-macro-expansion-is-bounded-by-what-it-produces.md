- **2026-08-14 · #54 — Macro expansion is bounded by what it produces** (`B0214`), closing
  [`docs/14`](../docs/14-review-findings.md)'s F17: 100,000 nodes per module, against a measured
  largest real expansion of 138. Gated in both directions by `macro_bomb.rs`, and the
  `pending_security.rs` F17 test is deleted, which is what that file's rule asks for.
