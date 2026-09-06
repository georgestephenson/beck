- **2026-08-26 — TLS 1.3 only, which the row already claimed.**
  [`docs/12`](../docs/12-standards-and-conformance.md)'s "TLS 1.3 (RFC 8446), no legacy downgrade"
  row, [`docs/08`](../docs/08-roadmap.md) §8.5.4's standards ledger. The ledger listed this as a
  missing *gate*; it was a missing **implementation**. rustls's safe defaults are TLS 1.2 *and*
  1.3, and both clients — a program's outbound call and the compiler's own `beck fetch` — were
  built with them, so a peer offering only 1.2 was accepted: exactly the downgrade the row says
  there is none of. Writing the test first is what showed it, and with the old configuration it
  fails with the 1.2 peer answering `200`. `beck_rt::outbound::TLS_VERSIONS` is now the one place
  the versions are named and both clients read it, because a list written twice is a list that can
  differ. The gate is a real handshake against a 1.2-only server with the 1.3 control beside it —
  the same certificate, the same trust anchor, the same request, and the only difference is which
  version the server offers, so the refusal cannot be passing for a closed port or a bad name.
