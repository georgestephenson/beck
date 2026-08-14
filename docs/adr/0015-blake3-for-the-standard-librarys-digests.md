# 0015 — BLAKE3 for the standard library's digests, and no signature library yet

**Context.** [`docs/07`](../07-dependencies.md) says cryptography is delegated to `ring` /
`aws-lc-rs` rather than hand-rolled, and that instruction is right about the thing it is about:
nobody here is going to implement a primitive. It was written before there was a standard library,
and it does not say *which* dependency the standard library's `digest` should be — a hash, a MAC, an
AEAD and a signature scheme are four decisions, and only the first two are needed by
[`46`](../46-standard-library-report.md).

BLAKE3 is already a first-party dependency and has been since Phase 1. It computes the signal
graph's stable node ids (`signal.rs`), the interface digest (`iface.rs`), the structural hash of a
value (`core.rs`) and the split's fingerprint, and [`docs/48`](../48-identity-report.md)'s
`SignedIdentity` already uses `blake3::keyed_hash` for exactly the construction this ADR is about.

**Decision.** The standard library's `digest` and `digest_keyed` are BLAKE3 and keyed BLAKE3, from
the `blake3` crate already in the workspace. No new dependency is taken. Hex and base64url are
written here (`beck-prim/src/digest.rs`, which was `beck-core`'s until the runtime library gave the evaluator and a compiled program one implementation to share — [`0029`](0029-the-runtime-library-is-linked-and-owns-the-arena.md)) against RFC 4648's own test vectors, because an alphabet is
not cryptography and a third dependency for sixty lines of table lookup is a supply-chain edge for
nothing.

**Alternatives.**

*Take `ring` or `aws-lc-rs` now, per [`07`](../07-dependencies.md).* Rejected as premature rather
than as wrong. Both are the right answer to the question those libraries exist for — asymmetric
signatures, key agreement, AEAD — and none of that is what a `digest` primitive needs. Taking one
for SHA-256 would mean a second hash function in a tree that has one, a C or assembly build
dependency in a workspace that currently has neither, and a decision about *which* of the two taken
under a standard library's time pressure rather than under the requirement that actually needs it.

*Write the hash too.* Never considered seriously, and named here so the line is visible: the
division is "a primitive is somebody else's table" for exactly this reason. A hand-rolled BLAKE3 is
the failure mode `07` was written to prevent.

*SHA-256 for interoperability.* Rejected for now, and it is the alternative most likely to come
back. Nothing here has to interoperate: these digests name values inside one program. The day a Beck
program has to verify a signature somebody else produced — a JWT, a webhook, a package digest — the
requirement is a specific algorithm, not a general "crypto library", and it arrives together with
the asymmetric half below.

**Consequences.**

[`07`](../07-dependencies.md)'s crypto row is now **narrower than it reads**: `ring`/`aws-lc-rs`
remains the decision for signatures, key agreement and AEAD, and is not the decision for the
standard library's digests. That correction is recorded in
[`46`](../46-standard-library-report.md) §46.16 rather than by editing the design document.

**What this does not decide, and what is now blocked only on it.**
[`docs/08`](../08-roadmap.md) §8.5.4's Wave 3 says the OIDC relying party is waiting on "the
signature library — and TLS, because JWKS is fetched over it … those two are one dependency decision
taken together". This ADR does not take that decision and does not narrow it: asymmetric signature
verification and a TLS stack are still one ADR, still unwritten, and nothing in `digest.rs` is a
step towards either. What has changed is that the *symmetric* half is no longer entangled with it.

`digest_keyed` is domain-separated with `blake3::derive_key` under a context string that is not the
runtime's. A token minted by a program and a credential minted by `SignedIdentity` are therefore
mutually unverifiable, which is the intended relationship between two systems that happen to share
a hash function.
