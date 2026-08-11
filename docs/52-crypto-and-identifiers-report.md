# 52 — Phase 3, part 22: digests, encodings and identifiers

**Built.** The rest of Wave 2 that is cryptography, and the identifier half nobody had written a
reader for. Nine primitives and one file in [`compiler/lib/`](../compiler/lib/crypto.beck).

The interesting part is not the hash. It is that a message authentication code is the first
operation this project has met that **has to** turn a `secret[Str]` into a `Str` — not as a
convenience, but as the definition of the operation — and §3.5's claim is that nothing does. What
follows is the decision that took, the test that keeps it singular, and the two findings writing it
produced, neither of which is about cryptography.

## 52.1 The items, and what each of them is

[`08`](08-roadmap.md) §8.5.4's Wave 2 has been carrying "crypto, UUID parsing, arbitrary-precision
decimal, bignums and numeric coercion" as untouched since [`46`](46-standard-library-report.md).
This is the first two.

| | Where it went | Why there |
|---|---|---|
| `digest`, `digest_keyed`, `digest_eq` | primitives | A hash function is somebody else's table. [`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md) is which one and why no new dependency |
| `hex_encode`/`hex_decode`, `base64_encode`/`base64_decode` | primitives | An alphabet is a grammar. Written here rather than taken as a dependency, against RFC 4648 §10's own vectors |
| `uuid_parse`, `uuid_version` | primitives | A canonical form is a grammar too — and this one **normalises**, which is the reason it is not a `str_len` check in Beck |
| a fingerprint, a digest of several values, a signed token, `is_uuid` | [`lib/crypto.beck`](../compiler/lib/crypto.beck) | Composition, per [`lib/README.md`](../compiler/lib/README.md)'s division |

`EncodingError` and `UuidError` are declared in the prelude beside `JsonError` and `TimeError`, and
the decoders **raise** rather than returning a `Result`, which is [`27`](27-the-walls-come-down-report.md)'s
shape and [`46`](46-standard-library-report.md) §46.2's rule applied without a new argument.

**A digest is pure**, and that is the line this group is drawn on rather than a detail of it. The
other things a crypto library is usually asked for — random bytes, a nonce, a clock — are
nondeterministic, and Beck already has `uuid()` and `now()` for those, both charged `nondet` and
both refused inside a fold by §3.7. `digest` performs nothing, so a fingerprint may be computed
inside a fold and a replay recomputes the same one. `security.rs::a_digest_is_pure_and_a_keyed_one_is_not`
asserts both halves against `Prim::effects` directly.

## 52.2 The one function that spends a secret

§3.5 says a `secret[T]` cannot reach a browser, and three phases have held that without exception.
[`49`](49-http-client-report.md) §49.4 met the first program that needed to *spend* one — a
credential in a header — and closed it by moving **when** the secret is unwrapped: the request
carries its secrets apart and the runtime merges them at the edge. Nothing was weakened.

That trick does not reach a MAC. A message authentication code's output is *meant* for the party
that must not learn the key — a session cookie, a signed link, a webhook signature — so there is no
edge to defer to. The value being produced is the thing the program then puts in a page. Either the
language computes one, or [`48`](48-identity-report.md)'s `SignedIdentity`, which is exactly this
computation, stays in Rust for ever and a Beck program cannot issue its own tokens.

So:

```
digest_keyed : (secret[Str], Str) -> Str ! {cap.sign}
```

Three things make that a decision rather than a hole, and
[`adr/0014`](adr/0014-a-keyed-digest-is-the-one-declassifier.md) is the record of taking it.

**It is one function, and a test says so.** Not a rule about a family of operations, not a
`declassify` escape hatch — one primitive, and
`security.rs::exactly_one_primitive_turns_a_secret_into_something_that_is_not_one` enumerates the
whole prelude, filters for a parameter that is a `secret[T]` and a result that is not, and asserts
the answer is the single name `digest_keyed`. A second declassifier added without a second argument
fails there, which is the only place it would fail.

**It is a capability, and the client does not hold one.** `cap.sign`, by the same mechanism `reveal`
uses for `internal[T]` and for the same reason: no client tier discharges a capability, so a view
that mints a code is a *placement error* rather than a review comment, and a server that mints one
has said so in its published row. `minting_a_code_is_a_capability_and_the_client_does_not_hold_it`
takes the secret-handling app from §3.5's own suite, adds one `@on(client)` function that calls it,
and asserts the refusal by code. Its pair,
`signing_inside_the_chokepoint_is_exactly_what_it_is_for`, puts the same call inside `validate` and
asserts it compiles — because a capability nothing may hold is not a capability, it is a ban, and
[`docs/20`](20-phase-2-report.md)'s `internal[T]` pair established that both directions have to be
tested.

**The key is derived, not used.** `blake3::derive_key` under a context string that is not the
runtime's, so one secret used for two purposes gives two unrelated keys and a token minted by a
program does not verify as one minted by `SignedIdentity`.

The alternative worth naming is the one that was rejected: making the result a `secret[Str]` keeps
§3.5 exceptionless and destroys the operation, because a code that can only travel through
`with_secret_header` cannot be a cookie or a URL — which is most of what a code is for. The ADR has
that argument in full, along with the third option, which is to refuse the whole thing and accept
that tokens get minted outside the language where none of §3.5 applies at all.

## 52.3 A comparison that does not stop early

`digest_eq` is constant-time, and it is in the prelude rather than in `lib/` because it is the one
part of the token check that cannot be written in Beck — `==` on two strings returns at the first
byte that differs, and a verifier that does that tells whoever is guessing how much of the guess was
right.

This is a **correction to [`43`](43-threat-model.md) §43.3**, which says "nothing in Beck's design
attempts constant-time anything". That sentence was true when it was written and is now true with
one exception, and the honest form of an absolute exclusion with an exception is the exception
named. §43.3 now names it; the general claim is unchanged, because one comparison is not a side
channel programme.

Length is compared first and in the clear. Padding two strings to a common length to hide it would
be answering a question nobody asked: the length of a digest is not a secret.

## 52.4 What the primitives are checked against

Nothing here is checked against itself.

| | The oracle |
|---|---|
| `digest` | BLAKE3's own published vectors for `""` and `"abc"`, quoted from the reference implementation rather than produced by this code and pasted back |
| `base64` | RFC 4648 §10's seven vectors, in §5's alphabet, both directions — plus every prefix of a 43-character string, so no length class goes untested |
| `hex` | Round-trip over ASCII and non-ASCII, plus the two ways it can fail |
| `uuid_parse` | Six spellings of one identifier normalising to one string, and five near-misses that are not identifiers |
| the token | A real key, in `stdlib.rs::a_token_opens_only_under_the_key_that_minted_it` (§52.5) |

The decoders read what other encoders write: base64 accepts padding it does not emit and the
standard alphabet's `+`/`/` alongside §5's `-`/`_`, because a decoder that refuses what other
encoders produce is a decoder that fails in production. The two alphabets do not overlap, so
accepting both is unambiguous rather than lenient.

`lib/crypto.beck` carries eleven of its own `test` and `property` blocks and is gated by being in
the directory, which is what [`46`](46-standard-library-report.md) built `stdlib.rs` for.

## 52.5 Two findings, and neither is about cryptography

### A `test` block cannot exercise a capability

The first draft of `lib/crypto.beck` had a test that minted a token and opened it. It does not
compile, and the diagnostic is right:

```
error[B0700]: `test a token opens under the key that minted it` performs cap.sign
  = note: an expectation is a pure question about a state, a log and a page; effects belong to the
    *subject*, and §21.3 stubs those
```

A test block's own row must be empty. `cap.*` is deliberately **not** auto-stubbable —
[`21`](21-tests-in-beck-and-proof.md) §21.3's reason is that "stubbing a capability would bypass
it", and §21.2's whole claim for `when` is that it goes through the real `validate`. Both are right.
Together they mean the layer of a library that *holds a key* is the layer Beck cannot test, and
writing `stub cap.sign:` would make the test worse than absent: the stub is a constant, so a
tampered token would verify against it and the test would pass on a forgery.

This is not a wall in [`46`](46-standard-library-report.md) §46.5's sense — nothing is
inexpressible, and the language is not wrong. It is a **shape the library has to take**, and taking
it deliberately turned out to improve the library: `crypto.beck` is now two layers with the key at
the seam. Everything about the token's *format* — the two halves, the comparison, the decoding — is
pure and takes the code it expects as an argument, so every way a forgery can arrive is reachable
from an ordinary `test` block. `sign` and `open_token` are the two lines that compute a code, and
what is left to check about them is that a real key produces a code only that key reproduces. That
is one Rust test driving the evaluator, and it is named in the Beck file so a reader looking for the
missing case finds it.

The general statement is worth having, because it is not about this library: **a Beck library whose
functions require a capability has a Rust-tested edge, and the smaller that edge the better.** The
alternative — an auto-stubbable capability — would trade a testable library for a test suite that
cannot tell authorisation from its absence, which is the wrong trade in exactly the way §21.3 says.

### Nine match arms cost a thousand levels of recursion

`sicp.rs::what_bounds_a_recursive_types_depth_is_the_evaluator_and_not_the_checker` builds a tree
1,000 deep and asserts the process survives "in either profile". Adding the nine primitives to the
evaluator's `match op` broke it: `beck test` overflowed its stack in a debug build.

Nothing about digests is recursive. The mechanism is that `Interp::prim` is one arm per primitive,
its frame is as wide as the widest arm, and it is reached from `eval_prim`, which is on the
recursive path — so inlining merges the two and every local a new arm adds is a local that every
nested call carries. Nine arms with a `Vec` pop and a `String` each were enough to spend the
headroom [`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) declared for
recursion, on a primitive that has nothing to do with recursion.

The fix is one attribute: the arms live in `digest_prim`, marked `#[inline(never)]`, so their frame
is a leaf rather than a term in the recursive one. What is worth recording is the **coupling**, not
the fix. The evaluator's declared stack is a budget shared between the depth a program may reach and
the width of a `match` nobody thinks of as costing anything, and the only reason this was caught is
that the depth is asserted by a test rather than discovered by a user. `adr/0007`'s argument that
the bound must be *declared* rather than *discovered* now has a second illustration: the number it
declares moved because of a change in a different crate, and a test said so within the hour.

## 52.6 What is **not** built

| | Status |
|---|---|
| Asymmetric signatures | **not built.** No Ed25519, no RSA, no JWKS, no JWT verification. [`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md) says why the symmetric half was taken without it and that the dependency decision is unchanged and still owed |
| TLS | **not built**, unchanged from [`49`](49-http-client-report.md) §49.6. Still asserted as absent in `pending_security.rs` |
| Encryption of any kind | **not built.** There is no AEAD, no key agreement, and no primitive that turns a value into an unreadable one. A digest is not encryption, and a program that needs confidentiality at rest does not get it here |
| Random bytes | **not built**, and this is a decision rather than an omission. `uuid()` mints an identifier and is `nondet`; a general `random_bytes()` would be a second nondeterministic source at the edge, and the one thing it is usually wanted for — a key — is `secret_env`'s job |
| A key that is not a `Str` | **not built.** A key is a `secret[Str]`, so a binary key is hex or base64 first. Real, and cheap to live with; a `secret[list[Int]]` would be worse |
| Key rotation, a key id in the token | **not built.** `sign` produces two fields, not three. A deployment rotating a key has to accept both during the overlap and this library gives it no help |
| An expiry in the token | **not built**, and deliberately not: a time is `now()`, which is `nondet`, and putting it inside `sign` would make a signature nondeterministic. A caller puts an instant in the payload it signs |
| `uuid()` returning a parsed type | **not built.** An identifier is a `Str` before and after, and `uuid_parse` normalises rather than changing the type. A `Uuid` newtype is expressible in Beck today and is a program's decision |
| UUID v7's timestamp, the variant bits | **not read.** `uuid_version` reads the version nibble and nothing validates the variant, because a value whose variant bits say nothing is still an identifier and refusing it would refuse an identifier that works |
| Bignums, arbitrary-precision decimal, numeric coercion | **untouched**, unchanged from [`46`](46-standard-library-report.md) §46.6. That is what is left of Wave 2 besides the harnesses |
| The AWFY and CLBG harnesses | **still not stood up**, and [`50`](50-collections-and-dates-report.md) §50.6 said the same. They remain the largest thing owed on this bullet |
| A number for any of this | **none.** [`46`](46-standard-library-report.md) §46.6's reason is unchanged: the tree-walker is 33× CPython, so a measurement of a primitive would measure the interpreter |

And the limit on the claim in §52.2. What is defended is that **exactly one function declassifies,
that it requires a capability, and that a test keeps both true**. What is *not* claimed is that a
program using it is secure: this is a MAC, so it is only as good as the key, the key comes from
`secret_env`, and there is no rotation, no expiry and no revocation anywhere in this change.

## 52.7 What this corrects

- **[`07`](07-dependencies.md)'s crypto row is narrower than it reads.** It says cryptography is
  delegated to `ring`/`aws-lc-rs`; that remains the decision for signatures, key agreement and AEAD,
  and is **not** the decision for the standard library's digests, which are the BLAKE3 already in
  the tree. [`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md) is the record and no
  new dependency was taken.
- **[`43`](43-threat-model.md) §43.3's first exclusion gains a named exception.** "Nothing in Beck's
  design attempts constant-time anything" is now "nothing except `digest_eq`, and here is why". The
  exclusion itself is unchanged: one comparison is not a side-channel programme. §43.2 gains a row
  for the declassifier, because a property that is asserted by a test belongs in the table that
  lists what a test asserts.
- **[`08`](08-roadmap.md) §8.5.4's Wave 2 loses two of its five untouched items.** What is left is
  arbitrary-precision decimal, bignums and numeric coercion — and the benchmark harnesses.
- **[`46`](46-standard-library-report.md) §46.6's table moves two rows.** UUID goes from "`uuid()`
  has existed since Phase 1; nothing parses or formats one" to built; crypto goes from "untouched"
  to the digest half built and the asymmetric half named in §52.6.
- **[`48`](48-identity-report.md) §48.5's list is unchanged and one of its predecessors is closer.**
  `SignedIdentity`'s construction is now expressible in Beck, so a program can issue and check its
  own tokens; the OIDC relying party still needs asymmetric verification and TLS, which
  [`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md) explicitly does not decide.
- **[`21`](21-tests-in-beck-and-proof.md) §21.3 should be read as having a consequence.** Its rule
  that `cap.*` is not auto-stubbable is right and is unchanged; §52.5 is the first library to find
  what it costs — the layer of a library that holds a capability is the layer Beck cannot test — and
  the answer is to make that layer small rather than to change the rule.
- **[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) gains a second
  illustration.** The evaluator's declared stack is a budget shared with the width of its primitive
  `match`, and §52.5 is what spending it looks like from the other end.

## 52.8 What Phase 3 is still not

Unchanged from [`51`](51-arrangement-lifecycle-report.md) §51.7 except where this touches it. The
standard-library bullet is now most of the way rather than "everything but crypto, UUID parsing and
the bignums"; the exit criterion — an outside developer building a non-trivial app from
documentation alone — is not met and is not closer, because none of this is documentation an outside
developer would build from.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
