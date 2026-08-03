# 0014 — A keyed digest is the one declassifier, and it is a capability

**Context.** §3.5's headline property is that a `secret[T]` cannot reach a browser: it is not
Sendable, so a boundary crossing that carries one is a compile error naming the flow. Three phases
have held that line without exception. [`docs/49`](../49-http-client-report.md) §49.4 hit the first
program that needed to *spend* a secret rather than hold one — a credential in a header — and closed
it without weakening the property, by moving *when* the secret is unwrapped: `HttpRequest.secrets`
travels apart and the runtime merges it at the edge.

Wave 2's crypto item asks a question that trick does not answer. A message authentication code is a
function of a key and a message whose **output is meant to be given to the party that must not learn
the key**: a session cookie, a signed download link, a webhook signature. There is no edge to defer
to, because the value being produced is the thing the program then puts in a page, a URL or a
`Set-Cookie`. Either the language can compute one, or [`docs/48`](../48-identity-report.md)'s
`SignedIdentity` — which is exactly this computation — stays permanently in Rust, and a Beck program
cannot issue its own tokens.

**Decision.** One primitive, `digest_keyed(key: secret[Str], message: Str) -> Str`, performs
`cap.sign`. It is the **only** function in the prelude whose parameter is a `secret[T]` and whose
result is not one, and `security.rs::exactly_one_primitive_turns_a_secret_into_something_that_is_not_one`
enumerates the prelude and asserts that number is one.

The capability is the same mechanism `reveal` uses for `internal[T]`, for the same reason. A
capability is undischargeable on a client tier, so a view that mints a code is a placement error
rather than a review comment; a server that mints one has said so in its published row, so
`beck iface` and the derived policy both see it. §3.5's chokepoint rule does the enforcement, and no
second mechanism was added.

The key is derived into a domain string before use (`blake3::derive_key`), so the same secret used
for two purposes yields two unrelated keys, and a token minted by a program does not verify as one
minted by the runtime's own identity provider.

**Alternatives.**

*Return a `secret[Str]`.* Rejected: it preserves the property and destroys the operation. A MAC that
can only travel through `with_secret_header` cannot be a cookie, a URL parameter or a page — which
is most of what a MAC is for — and the program would be back where §49.4 started, one indirection
further along.

*No capability, just a pure function.* Tempting, and nearly defensible: a `secret[Str]` cannot reach
a browser, so a client-placed function has no key to sign with and the capability looks redundant.
Rejected because redundancy is not the point — the row is. An effect row is how this project makes a
program's privileges *legible* to the deployment (§6.5) and to a reader; a declassification that
leaves no trace in a signature is one nothing downstream can be told about.

*Refuse it, and keep §3.5 exceptionless.* This is the option to be honest about, because it is
coherent: the property is stronger without this primitive, and "Beck cannot issue a signed token"
is a sentence someone could live with. It was rejected because the alternative is not "no token" —
it is a token minted outside the language, in Rust or in another service, where none of §3.5 applies
at all. A declassification that is named, singular and charged is better than one that has moved
somewhere the compiler cannot see.

**Consequences.**

§3.5 gains a clause and does not lose one. The claim is no longer "nothing turns a secret into a
value" — it never was, precisely — it is "exactly one thing does, it is a one-way function of the
key, and it requires a capability". The test that keeps it exactly one is the part that matters:
without it, this ADR is an intention.

The threat model has a new row (§43.2) and a corrected exclusion (§43.3). Constant-time comparison
is not attempted anywhere in Beck **except** `digest_eq`, which exists because comparing a MAC with
`==` returns at the first differing byte. Naming the one place is the honest form of an exclusion
that was previously stated absolutely.

A capability-bearing function cannot be exercised by a `test` block, because a test's own row must be
empty (§21.3) and `cap.*` is deliberately not auto-stubbable. `compiler/lib/crypto.beck` is
therefore written in two layers — a pure one that takes the code it expects as an argument, and two
lines that compute one — and the second is tested from Rust.
[`52`](../52-crypto-and-identifiers-report.md) §52.5 records that as a finding rather than as a
convention.
