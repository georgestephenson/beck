- **2026-08-16 · #62 — The public surface is designed.** The boundary between a Beck backend and a
  non-Beck consumer is an opt-in `@public` family — `rest`, `mcp`, `grpc`, `events`, `sql` — each a
  rendering of the internal contract, gated by a foreign reader; GraphQL declined with the reason
  recorded ([`docs/101`](../docs/101-the-public-surface.md), D28). Design only — no annotation exists
  in the compiler, and §101.11 says so. `beck-rt/src/telemetry.rs`'s module doc corrected in place:
  OTLP export is pull-only.
