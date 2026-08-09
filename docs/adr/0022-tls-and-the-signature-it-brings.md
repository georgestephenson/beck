# ADR 0022 — TLS, and the signature that comes with it

**Status:** accepted
**Date:** 2026-08-09
**Context:** [`94`](../94-oidc-relying-party-report.md), [`07`](../07-dependencies.md) §7.2,
[`48`](../48-identity-report.md) §48.5, [`49`](../49-http-client-report.md) §49.6,
[`0004`](0004-full-cargo-deny-gate.md),
[`0013`](0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md)

## The decision

**rustls**, with **aws-lc-rs** as its cryptography provider, and Mozilla's trust anchors compiled
in as data (`webpki-roots`). One decision, taken once, for two capabilities:

1. **Transport security on an outbound call.** `beck_core::net::Request` gains a `tls` field and
   `lib/http.beck` gains `over_tls`, so a Beck program says which of its peers it reaches privately.
2. **Asymmetric signature verification.** `aws_lc_rs::signature` verifies the RSA and ECDSA
   signatures on an OIDC ID token, so `beck-rt`'s relying party needs **no second crate** for the
   half that is cryptography.

[`08`](../08-roadmap.md)'s Wave 3 row had said the relying party needed "the signature library —
and TLS, because JWKS is fetched over it … those two are one dependency decision taken together".
They are one because rustls's provider is a signature library, which is the argument
[`0015`](0015-blake3-for-the-standard-librarys-digests.md) made about BLAKE3 one layer down: the
cheapest cryptographic dependency is the one already in the graph.

## Why these, and why not the alternatives

**Why rustls** is [`07`](../07-dependencies.md) §7.2's row and this ADR does not relitigate it:
memory-safe, no OpenSSL build, and the ecosystem's default. **Why aws-lc-rs** is the same row —
docs/07 wrote "rustls (+ `aws-lc-rs`)" — and following it rather than substituting `ring` keeps one
choice in one place. `ring` would have built faster and needed no CMake; it is a smaller, quieter
library and would have served identically here. The reason it is not chosen is that a dependency
table nobody follows is a dependency table, and diverging from §7.2 for a build-time convenience is
how one becomes fiction.

**Why not `jsonwebtoken` or `openidconnect`.** D6 names `openidconnect` and it is a good crate. It
also brings its own HTTP client, its own async model and its own opinions about where a token is
stored — and [`49`](../49-http-client-report.md) built the outbound call *on this project's own
seam* so that a request is stubbable, bounded at 8 MiB and 10 s, and made of a host the cluster's
egress rule already names. A relying party built on somebody else's client would have none of those
properties, and adding them back is more work than the 500 lines the protocol actually is. What is
genuinely hard about OIDC is the cryptography and the claim checks; the first is aws-lc-rs's and the
second is a list. So D6's "the audited `openidconnect` Rust crate" is **narrowed** here rather than
followed, and [`94`](../94-oidc-relying-party-report.md) §94.7 records that as a correction rather
than leaving the decision log to disagree with the tree.

**Why not the `rsa` crate** for the RSA half, which would have been the pure-Rust answer:
RUSTSEC-2023-0071 is open against it, and [`0004`](0004-full-cargo-deny-gate.md)'s advisory gate has
an empty ignore list on purpose. The advisory is about private-key operations and a relying party
only verifies, so the exposure would have been nil and the argument would have had to be made in
`deny.toml` — which is exactly the shape of muting this project has refused.

## The cost, named

- **CMake, at build time.** `aws-lc-sys` builds C with CMake, so building this repository now needs
  `cmake` as well as a C compiler. That is a **new** requirement — [`0017`](0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md)'s
  SQLite and [`0019`](0019-a-modern-allocator-for-the-evaluator.md)'s mimalloc need `cc` and not
  CMake — and it is the one real cost here. GitHub's `ubuntu-latest` has it; a developer on a bare
  container does not.
- **Licences.** rustls `Apache-2.0 OR ISC OR MIT`, aws-lc-rs and aws-lc-sys a conjunction of ISC,
  Apache-2.0, MIT and BSD-3-Clause, rustls-webpki ISC, `webpki-roots` **CDLA-Permissive-2.0** —
  every one inside [`0004`](0004-full-cargo-deny-gate.md)'s allowlist, which already carried the
  CDLA row in anticipation. The gate is what enforces this rather than this sentence.
- **A dev-dependency: `rcgen`.** `beck-rt`'s TLS test makes a certificate authority a moment before
  it uses it. A checked-in certificate would have been one fewer dependency and would expire, and a
  test that expires is a test that fails on a date nobody chose. It is `[dev-dependencies]`, so it
  is in no artefact and in no bill of materials.
- **A second place trust is decided.** Mozilla's root list now decides who may answer to a name a
  Beck program wrote. That is a real transfer of trust and it is stated in `beck-rt/src/outbound.rs`
  rather than implied.

## What this deliberately does *not* buy

- **TLS on the way in.** `beck run` still serves plaintext HTTP; §6.5's gateway terminates TLS in
  front of it, and that is unchanged. A listener that speaks TLS is a different decision, and this
  one does not quietly make it.
- **A configurable trust store.** There is no flag to add a certificate authority, no
  `--insecure`, and no SNI override: the name verified is `Request::host`, which
  [`0013`](0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md) already made the atom
  the call performs and the peer in the generated NetworkPolicy. Introducing a way to reach a host
  under a name the deployment was not told about would undo that ADR from the other end.
- **Certificate pinning, OCSP, or CRLs.** Named as absent so nobody assumes otherwise.

## What would reverse it

A build environment where CMake is genuinely unavailable, or a rustls release that changes its
default provider. Either is a swap of one feature flag plus this file's second paragraph: the
signature code names `aws_lc_rs::signature`, `ring` exposes the same three items under the same
names, and `beck-rt/src/oidc.rs` is where both would be edited.
