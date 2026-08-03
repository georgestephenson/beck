# `lib/` — the standard library, written in Beck

Wave 2 of [`docs/08`](../../docs/08-roadmap.md) §8.5.4. This directory holds the half of the
standard library that is **written in the language**, and its existence is a claim: if Beck cannot
express its own library, [`01`](../../docs/01-vision-and-premise.md) §1.1's argument about means of
abstraction is not one this project is entitled to make.

## The division

| Kind | Where it lives | Why |
|---|---|---|
| A host's table or grammar | a primitive in `beck-core/src/prelude.rs` | `str_upper` is a Unicode table, `json_parse` is somebody else's grammar, `time_format` is the civil calendar. Writing any of them over a `list[Str]` in Beck would be a slower, less correct copy of what the host already has |
| Composition | a file here | lines, words, padding, an amount of money, a split that adds back up. There is nothing to ask the host for, so asking would be an admission |

The line is not "what is fast" — it is **what has a definition in the language**. `money.beck` is
integer arithmetic with a scale and a rounding rule; every part of that is expressible, so it is
expressed.

## What is here

| File | What it is |
|---|---|
| [`money.beck`](money.beck) | An exact amount in one currency, as minor units. Addition that refuses to mix currencies, and a `split` whose parts sum back to what was split |
| [`text.beck`](text.beck) | Lines, words, padding, case, truncation, a tolerant pair reader — over the string primitives |
| [`documents.beck`](documents.beck) | JSON and time: a document read as data with `match`, and RFC 3339 in UTC |
| [`http.beck`](http.beck) | A request built up and a response read back — over `http_fetch`, which is the *call*. There is no `get(host, path)` here and there cannot be: the host is written at the call site so the egress policy is derivable ([`adr/0013`](../../docs/adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md)) |
| [`collections.beck`](collections.beck) | A `Set[T]` as a map's keys, the three set operations and the two questions; sorting by a value rather than by a comparator; grouping, indexing, counting, deduplication and a partition. Every function total, and every result in an order that is a function of the values |
| [`crypto.beck`](crypto.beck) | A fingerprint, a digest of several values that is not the digest of their concatenation, and a signed token in two layers — a pure one that takes the code it expects as an argument, and the two lines that compute one. The seam is where the key is, because a `test` block's row must be empty and `cap.sign` is not auto-stubbable ([`52`](../../docs/52-crypto-and-identifiers-report.md) §52.5) |
| [`bignum.beck`](bignum.beck) | An integer of any size, as a sign and base-10,000 limbs: schoolbook multiplication, long division, `impl Num`, and every coercion to and from it named rather than implicit. The last floor of the numeric tower ([`55`](../../docs/55-bignums-report.md)) |
| [`decimal.beck`](decimal.beck) | An exact number of the form `units × 10^-scale`, over `bignum.beck`: canonical so `1.50` and `1.5` are one value, `/` exact or refusing, and three rounding rules rather than one ([`56`](../../docs/56-decimal-report.md)) |
| [`dates.beck`](dates.beck) | The civil calendar as arithmetic — Hinnant's two functions in Beck, checked against the same two in Rust rather than against themselves — plus `Date`, a `Duration` with its own `impl Num`, clamped month arithmetic, and `YYYY-MM-DD` read and written |

Each file carries its own `test` and `property` blocks and runs under `beck test`, which is what
[`27`](../../docs/27-walls-report.md) made possible for a library with no application around it.
`beck-cli/tests/stdlib.rs` runs all of them, so a change to a primitive that breaks a caller is a
failing build.

**One file imports another.** `decimal.beck` is written over `bignum.beck`, which is the first time
anything in this directory has done that — and every one of the three findings in
[`56`](../../docs/56-decimal-report.md) §56.5 came from it. A module importing another was a shape
the *compiler* supported and the *tools* had never been run against: `beck check`, `beck test` and
`beck iface` were right about it, and `beck doc` was wrong three ways.

## What is not here yet

Time zones.
[`46`](../../docs/46-standard-library-report.md) §46.6 says which of those are waiting on a
language feature and which are simply unwritten;
[`49`](../../docs/49-http-client-report.md) §49.6 says the same for what `http.beck` does not do —
no TLS, no redirects, no percent-encoding; and
[`50`](../../docs/50-collections-and-dates-report.md) §50.6 for what the two newest files leave —
a set whose cost is a map's, no zones, no locale, and a `Duration` that is milliseconds rather
than a rational number of seconds; and
[`52`](../../docs/52-crypto-and-identifiers-report.md) §52.6 for what `crypto.beck` is not — no
asymmetric signature, no encryption of any kind, no key rotation and no expiry in a token; and
[`55`](../../docs/55-bignums-report.md) §55.6 for `bignum.beck`'s — schoolbook and nothing
sub-quadratic, no Knuth algorithm D, no `gcd` and no modular arithmetic, and the decimal that would
sit on top of it unwritten.

**One wall was found here and removed from here.** `money.beck` was meant to be an
`impl Num for Money` so that `+` would work on it, the way `sicp/ch2.beck`'s rationals do. It could
not be: a trait's declared effect row was a *ceiling* every impl was held to, the prelude's `Num` is
pure, and adding two amounts in different currencies has to fail. That refusal was asserted as a
wall in `sicp/refusals/`'s pattern, and [`47`](../../docs/47-effect-polymorphic-traits-report.md)
took it down a day later — a trait's row is now a floor, an impl's row is inferred and published,
and `money.beck` has its operator. `stdlib.rs` asserts the property from this side, so a regression
reads as "money lost its operator" rather than as a type error three files away.

**A second wall was found here, one wave later, and closed the same way.** `http.beck` was meant to
have a `with_bearer(req, token)` that wrote `"Bearer " + reveal(token)`. It could not: §3.5 gives a
program no way to read a `secret[Str]`, which is the property that keeps one out of a browser — so
a credential could not reach a header, and an authenticated request was inexpressible. The fix was
not to weaken the property but to move the moment: `HttpRequest` carries its secret headers apart,
and the runtime merges them at the edge. `with_secret_header` is that, and
[`49`](../../docs/49-http-client-report.md) §49.4 is why.

**The next two findings were not walls.** `collections.beck` wanted a `Set[T]` in a trait, and the
diagnostic that catches a missing type argument said to write `Set[_]` — which is not a program,
because there is no wildcard type. The feature was there all along (`impl[T] Trait for Set[T]`), so
what was broken was the sentence pointing at it; the label now uses the declaration's own parameter
names, and a test in `check/mod.rs` applies the suggestion and demands the result compiles. The
second is that **a record orders by field name, not by declaration order**, so a two-key sort key
written `Key(score=…, name=…)` sorts by the name. That one is pinned rather than fixed — a value
carries no declaration, and a checker-side rule would disagree with the order the same records come
out of a `Map`. [`50`](../../docs/50-collections-and-dates-report.md) §50.5 is the record for both.

**The third was not a wall either, and it shaped a file rather than a feature.** `crypto.beck`
wanted a test that minted a token and opened it. A `test` block's own row must be empty (§21.3) and
`cap.*` is deliberately not auto-stubbable — stubbing a capability would bypass the thing it exists
to enforce — so the layer of a library that *holds a key* is the layer Beck cannot test, and writing
`stub cap.sign:` would make the test pass on a forgery. Nothing is inexpressible and the rule is
right; what changed is the library. The token's format is a pure layer that takes the code it
expects as an argument, `sign` and `open_token` are the two lines that compute one, and
`stdlib.rs::a_token_opens_only_under_the_key_that_minted_it` is the Rust-side edge.
[`52`](../../docs/52-crypto-and-identifiers-report.md) §52.5 is the record.
