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

Each file carries its own `test` and `property` blocks and runs under `beck test`, which is what
[`27`](../../docs/27-walls-report.md) made possible for a library with no application around it.
`beck-cli/tests/stdlib.rs` runs all of them, so a change to a primitive that breaks a caller is a
failing build.

## What is not here yet

Collections beyond the primitives, an HTTP client, UUID beyond `uuid()`, crypto, bignums and
numeric coercion. [`46`](../../docs/46-standard-library-report.md) §46.6 says which of those are
waiting on a language feature and which are simply unwritten.

**One wall is recorded rather than worked around silently.** `money.beck` was meant to be an
`impl Num for Money` so that `+` would work on it, the way `sicp/ch2.beck`'s rationals do. It
cannot be: a trait's declared effect row is a bound on every impl, the prelude's `Num` is pure, and
adding two amounts in different currencies has to fail. The refusal is asserted in
`stdlib.rs::a_trait_method_may_not_be_more_effectful_than_its_trait`, in the pattern
`sicp/refusals/` used — so the day effect-polymorphic traits land, that test goes red and this
directory gains an operator.
