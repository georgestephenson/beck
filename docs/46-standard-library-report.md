# 46 — The standard library

**Built.** [`08`](08-roadmap.md) Phase 3's standard-library bullet: strings, collections, JSON,
time, sets, dates and durations, money, an HTTP client, and the directory that holds the half of the
library written in Beck — importable by name from any program since
[`10`](10-decisions.md) D23. Beside it, the two benchmark suites §8.4 stands up against the
evaluator: [`53`](53-are-we-fast-yet-report.md)'s Are We Fast Yet and this chapter's Benchmarks
Game, eight of ten, every constant derived from a file somebody else published.

The chapter's subject is not the primitive count. It is **the division** — which half of a library is
the host's and which is the language's (§46.1) — and what testing that division against real callers
found, which is four walls and two costs:

- A trait's declared row is a **bound**, so `Money` could not have `+` (§46.6). One feature, and the
  test that says so was written to go red the day it lands.
- **A credential could not be sent** (§46.10), invisible for three phases because no program had ever
  tried to *spend* a secret.
- **`lib/` was a standard library nothing outside `lib/` could import** (§46.12), invisible because
  nothing had ever reached across a directory boundary.
- **A record orders by field name, not by declaration** (§46.6), which silently inverts every
  two-key sort written the obvious way.
- The **accumulator idiom is quadratic** and the evaluator's own step budget cannot see it (§46.14).
- And one thing went the other way: a benchmark demanding enough to make a cost visible made
  **every caller of arbitrary-precision division 2.3× faster** (§46.15).

---

## 46.1 The division, which is the only interesting decision here

A standard library has to answer a question about itself: which of it is the *host's* and which is
the *language's*. Getting that wrong in either direction is expensive. Too far one way and the
library is a thin wrapper over Rust that proves nothing about Beck; too far the other and
`str_upper` is a hand-rolled Unicode table that is slower and wrong.

The line this project draws is **not** "what is fast". It is **what has a definition in the
language**:

| Kind | Where | Why |
|---|---|---|
| A host's table or grammar | a primitive | `str_upper` is a Unicode table; `json_parse` is somebody else's grammar; RFC 3339 is somebody else's spelling of an instant. Writing any of them over a `list[Str]` in Beck would be a slower, less correct copy of what the host already has |
| Composition | `compiler/lib/*.beck` | lines, words, padding, an amount of money, a split that adds back up, a civil calendar, a set. There is nothing to ask the host for, so asking would be an admission |

`compiler/lib/` is the second half of the deliverable. **If Beck cannot express its own library,
[`01`](01-vision-and-premise.md) §1.1's argument about means of abstraction is not one this project
is entitled to make** — and four phases of writing a *compiler* had never tested it, because every
corpus program is shaped like the todo sketch.

The calendar is the first item where that line was genuinely hard to place, because the host already
has one: `time_format` exists, it *is* the civil calendar, and adding `time_add_days` next to it
would have been half a day's work. It is a file, and the argument is the division's own. **RFC 3339
is a grammar** — somebody else's spelling, with a `T` in the middle and a `Z` on the end — and
grammars are the host's. "Which day is 2024-02-29 plus one" is not a grammar. It is integer
arithmetic with a leap rule, and integers are something the language has. So the calendar is written
in Beck, and what the host keeps is the notation.

## 46.2 What is built

**The primitives**: thirteen for strings, seventeen for collections, two for JSON, two for time, one
exchange for HTTP, and nine for digests, encodings and identifiers
(§46.7). The higher-order ones are row-polymorphic in their
argument's effects, so a pure caller of `list_fold` stays pure however another caller uses it —
§3.2's `map : (list[a], (a -> b ! e)) -> list[b] ! e`, which nothing about this had to re-establish.

**`compiler/lib/`**, ten files, each with its own `test` and `property` blocks:

| | |
|---|---|
| `text.beck` | lines, words, padding, case, truncation, a tolerant pair reader |
| `money.beck` | an exact amount in one currency as minor units, with a `split` whose parts sum back to what was split |
| `documents.beck` | a JSON document read as data with `match`, and RFC 3339 instants |
| `collections.beck` | `Set[T]`, sorting, grouping, deduplication — 26 definitions, and the only four primitives this directory has ever asked for: `list_min`, `list_max`, `list_sum` and `list_unique` (§46.16). The last of those *replaced* a definition here rather than being added beside one — `unique` is now a call to it, for the reason the row below gives |
| `dates.beck` | the civil calendar and durations — 49 definitions, no primitives added |
| `http.beck` | builders, header lookup, status predicates, a JSON body |
| `crypto.beck` | over the nine primitives (§46.7) |
| `bignum.beck`, `decimal.beck` | arbitrary precision, in Beck (§46.8) |
| `format.beck` | fixed-decimal rendering, which three benchmark ports needed |

`stdlib.rs` gates the **directory** rather than a list of names, so a library added and not
registered is not a thing that can happen. It also asserts each file is still a *library* — `beck
check` says "a library" — because "a library runs its own tests" and "a library is an application"
are different claims, and [`27`](27-the-walls-come-down-report.md) removed the wall between them
without merging them.

## 46.3 The shape Wave 2 was waiting for

`json_parse : Str -> Json ! { raises(JsonError) }` and
`time_parse : Str -> Int ! { raises(TimeError) }`. Both **raise**; neither returns a `Result`.

That is the whole of [`08`](08-roadmap.md) §8.5.3's trap 2, cashed. A caller who wants a `Result`
writes `try:`; a caller already inside something fallible writes nothing at all. **Had these been
written a wave earlier, every one of their signatures would now be wrong**, and so would every
signature that mentions them. `lib/documents.beck` is where the difference shows: `field_of` has a
`Result` in its type because it is the boundary, and nothing inside it mentions failure.

The same property runs through `lib/money.beck`. `same_currency` raises; `plus`, `minus`, `total`
and `split` inherit `raises(MoneyError)` by inference and never mention it in their bodies. **"You
cannot add pounds to euros" is one line, not a rule four functions each remember.**

## 46.4 Three decisions that are not obvious

**Indices are characters, everywhere or nowhere.** `str_len`, `str_slice` and `str_index_of` are one
unit. A library that counts in bytes and slices in characters is a trap with a test suite that
passes on ASCII, so the gate asserts the three agree on `"héllo"`.

**Out-of-range is clamped, not raised.** `str_slice("abc", 10, 5)` is `""` and `list_take(xs, 99)`
is `xs`. This is a decision rather than an oversight: **a slice is not a parse**, and `raises` is for
a *program's* vocabulary rather than for the library's arithmetic. Where a result genuinely may not
exist, the answer is `Option` — `list_get` and `str_index_of` both return one.

**Time is arithmetic, not a dependency.** `time_format` and `time_parse` are Hinnant's
`days_from_civil` and its inverse: exact, well known, and — the property that decides it here —
**pure arithmetic with no table behind it**, so `beck replay` cannot disagree with the run it is
replaying because a time-zone database was updated in between. RFC 3339 in UTC and nothing else. An
offset is *refused* rather than silently shifted, because accepting `+01:00` would mean accepting
that two spellings of one instant are two values, **and a log is not where you want to find that
out.**

The gate tests the two cases a round-trip would not catch — a wrong formatter and a wrong parser
agree with each other — so it checks 2000-02-29, 2100-02-28 (divisible by four, not a leap year) and
instants before the epoch, where floor division is the bug every hand-rolled formatter has.

## 46.5 A set is a map's keys, and the calendar is arithmetic

```beck
type Set[T] = newtype[Map[T, Bool]]
```

The value is `Bool`, always `true`, and unread. What a set stores is the *key*, and `Map` is the only
ordered keyed collection the host provides — so membership is the map's lookup rather than a scan
over a `list[T]`, which is the entire reason to have the type.

Two properties hold across `collections.beck` and are stated once rather than per function. **Every
function is total**: a collection has no failure of its own, and the only way to get an error out of
the file is to pass in a function that has one, whereupon the row is that function's and travels by
inference. **Every result is ordered, and the order is a function of the values**: `elements` is
sorted because a `Map` is, not because anything sorts it — a replay that disagreed with the run it
was replaying about the order of a set would be a replay of a different program, so "in whatever
order the hash landed" is not available here and is not wanted.

The one design question was sorting. `sort_by` takes a **key**, not a comparator, and a library that
wanted "by score descending, then by name" could have grown a comparator-taking twin. It did not need
one: a compound value compares structurally, so the two-key sort is one key function returning a
pair. That sentence as first written was wrong, which is §46.6's second wall. And `unique` and
`elements(set_of(xs))` are both duplicate-free and are **different functions** — the first keeps
first-seen order, the second sorts — so the test puts them side by side, because a library that
shipped only one would have shipped the wrong one for half its callers.

**The calendar's cross-check is against the host, and it is worth being precise about what it
buys.** `days_from_civil` and `civil_from_days` are written a second time, in Beck, and the first two
tests walk 5,000 days demanding that the two implementations name the same date in both directions —
2,000 consecutive days from 1900-01-01, which crosses the February of a year divisible by four that
is **not** a leap year, and 3,000 samples at a stride of 97 days from 1583-01-01, which crosses every
century boundary in between. **It is not two independent algorithms, and calling it a differential
without saying so would be the more flattering lie.** It is one algorithm on two evaluators, so what
it checks is the *language*: that `/` truncates where Hinnant's algorithm needs it to, that operator
precedence reads the way it is written, and that nothing was mistyped on the way across. It is
load-bearing either way — changing `719468` to `719467` in the Beck copy turns six of thirteen tests
red.

**The division semantics turned out to be the real content.** Hinnant's algorithms are written for a
division that **truncates towards zero**, while splitting an instant into a day has to **floor**, or
an instant before 1970 lands in the day after the one it is in. Both are needed, in one file, and
they are not interchangeable. `floor_div` is four lines of Beck; the truncating one is the operator —
and the gate now pins the operator, because a whole library rests on it and nothing had said so.

Two decisions in the file are choices rather than facts. **`add_months` clamps**: 2024-01-31 plus one
month is 2024-02-29, not 2024-03-02. There is no answer that is right for every caller — a billing
date and a "30 days later" are different questions — so this is the one that keeps the *month*, and
it makes the operation lossy, which the test asserts rather than leaving to be found. And **the
arithmetic is total while the parsing is not**: `days_from_civil(2023, 2, 30)` is the day after the
28th, because the arithmetic has no opinion, and keeping validation separate is what lets the
arithmetic be used to write it.

## 46.6 A duration is elapsed, and the walls this found

`type Duration = newtype[Int]` — milliseconds. **Elapsed, not calendar**: a `Duration` of 24 hours
added to an instant is 24 hours later, which is not the same thing as "the next day" wherever a day
is not 24 hours long. `add_days` is the calendar operation and takes a `Date`; `after` is the elapsed
one and takes an instant. Keeping them apart is the point of having two types. `render_duration`
writes ISO 8601 and stops at hours, because `P1D` is a *calendar* day and emitting it would claim
something this type does not know.

### A trait's declared row is a bound, so `Money` could not have `+`

`lib/money.beck` was meant to be an `impl Num for Money`, so that `+` and `-` would work on an amount
the way they work on SICP §2.5.1's rationals. **It could not be.** A trait's declared effect row is a
bound every impl is held to, the prelude's `Num` declares a pure row, and adding two amounts in
different currencies has to fail:

```
error[B0370]: `Num::add@Money` performs more than its signature declares
   |     def add(self, other):
   |     ^^^^^^^^^^^^^^^^^^^^^ undeclared: raises(MoneyError)
```

The compiler is right and the design is incomplete. **An operator is unavailable to any type whose
operation can fail** — which is most of the interesting ones: a `Decimal` that can overflow, a
`Matrix` whose dimensions must agree, a saturating integer. The fix is one feature — a trait whose
method signatures carry a row *variable* — and it was recorded the way this project records walls:
as a test that starts failing, whose failure message says to delete the test, give `money.beck` its
impl, and correct this section. [`27`](27-the-walls-come-down-report.md) then built it, and it is
built.

**And the granularity is per method, not per impl.** `impl Num for Duration` is the second one in
`lib/`, and it lands in the same place for the same reason: `add` and `sub` mean something, `mul` and
`div` do not — an hour times an hour is an hour-squared. The first version of its test wrapped all
four in `try:` by symmetry with `money.beck`, and the checker refused two of them with "nothing here
can fail". `money.beck` could not have shown this, because every one of its methods can fail; **it
took a type where half the operations are total to make the granularity visible**, and the
granularity is the useful part — a caller of `+` on a `Duration` inherits nothing.

### The suggestion that was not a program

`collections.beck` wanted `Set[T]` to implement a trait. `impl Sized for Set:` says
`` `Set` takes 1 type argument(s), got 0 — write `Set[_]` ``, and `Set[_]` says
`` cannot find type `_` ``. **There is no wildcard type in Beck**, and the diagnostic had been
telling readers to write a program that does not compile since parameterised types landed, because
the label was generated from the *arity* and so had never had a name to offer.

**The feature was there the whole time**: `impl[T] Sized for Set[T]` works and always did, because an
`impl` head binds its own type parameters exactly as a `def` does. So this is not a wall in the
sense above — nothing was inexpressible. It is the sentence pointing at the feature being wrong. The
fix is that the arity check takes the declaration's **parameter names**, and the gate is worth more
than the fix: `the_spelling_a_missing_type_argument_suggests_is_one_that_compiles` provokes the
diagnostic, applies its suggestion, and asserts the result checks clean. **A diagnostic that offers a
spelling should be tested by writing it down**, and this is the first one in the tree that is.

### A record's order is its field names, not its declaration

```beck
model Declared:
    zebra: Int
    alpha: Int

expect Declared(zebra=2, alpha=0) < Declared(zebra=1, alpha=9)   # `alpha` decides
```

The consequence lands on the feature §46.5 just declined to build a comparator for. **A caller
writing a two-key sort key as `Key(score=…, name=…)` gets it sorted by `name`, because `n` comes
before `s`, and nothing anywhere says so.** The value is right, the ordering is total and
deterministic, and the reader's intent is silently inverted.

It is not a defect, and the reason is structural rather than a preference: **a value carries its
fields and not its declaration**, so at the point the comparison happens there is nothing else to
sort them by. The checker *does* know the type and could desugar `<` on a record into a
declaration-order comparison — and that is the option to reject explicitly, because the same records
are also ordered without going through the checker at all: they are `Map` keys, and `elements`
promises an order that is a function of the values. **Two orderings for one type is worse than one
surprising ordering**, and `sorted(xs)` disagreeing with `elements(set_of(xs))` about the same
records is exactly the bug a library that promises determinism must not have.

So it is recorded three ways rather than fixed — in `sorted`'s doc comment, in a `model Declared`
whose two orders disagree with a test pinning the answer, and in a harness assertion so that a change
to the runtime's value ordering fails there rather than reordering every two-key sort quietly. **The
real answer is `Ord` as a trait**, the way `Num` is one, so a type says what its own order is instead
of inheriting the one its representation happens to have. That is [`54`](54-ordering.md), which
writes it out and explicitly does not recommend it.

## 46.7 Digests, encodings and identifiers — and the one function that spends a secret

Nine primitives and one library file, with no new dependency
([`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md)). A hash function is somebody
else's table and an alphabet is a grammar, so `digest`, `digest_keyed`, `digest_eq`, hex, base64url
and the UUID reader are primitives; a fingerprint, a digest of several values and a signed token are
composition and live in `lib/crypto.beck`. **A canonical form is a grammar too** — which is why
`uuid_parse` is a primitive rather than a `str_len` check in Beck: it *normalises*.

**A digest is pure, and that is the line this group is drawn on rather than a detail of it.** The
other things a crypto library is usually asked for — random bytes, a nonce, a clock — are
nondeterministic, and `uuid()` and `now()` already exist for those, both charged `nondet` and both
refused inside a fold. `digest` performs nothing, so a fingerprint may be computed inside a fold and
a replay recomputes the same one.

### The one function that spends a secret

§3.5 says a `secret[T]` cannot reach a browser, and three phases held that without exception. §46.10
met the first program that needed to *spend* one and closed it by moving **when** the secret is
unwrapped. **That trick does not reach a MAC.** A message authentication code's output is *meant*
for the party that must not learn the key — a session cookie, a signed link, a webhook signature —
so there is no edge to defer to. Either the language computes one, or [`48`](48-identity-report.md)'s
`SignedIdentity` stays in Rust for ever and a Beck program cannot issue its own tokens.

```
digest_keyed : (secret[Str], Str) -> Str ! {cap.sign}
```

Three things make that a decision rather than a hole, and
[`adr/0014`](adr/0014-a-keyed-digest-is-the-one-declassifier.md) is the record.

**It is one function, and a test says so.** Not a rule about a family of operations, not a
`declassify` escape hatch — one primitive, and
`exactly_one_primitive_turns_a_secret_into_something_that_is_not_one` enumerates the whole prelude,
filters for a parameter that is a `secret[T]` and a result that is not, and asserts the answer is
that single name. **A second declassifier added without a second argument fails there, which is the
only place it would fail.**

**It is a capability, and the client does not hold one.** `cap.sign`, so a view that mints a code is
a *placement error* rather than a review comment. Its pair — the same call inside `validate`,
asserted to compile — matters as much, **because a capability nothing may hold is not a capability,
it is a ban.**

**The key is derived, not used.** Under a context string that is not the runtime's, so one secret
used for two purposes gives two unrelated keys and a token minted by a program does not verify as one
minted by the runtime's own provider.

The alternative worth naming is the one rejected: making the result a `secret[Str]` keeps §3.5
exceptionless and **destroys the operation**, because a code that can only travel through a header
cannot be a cookie or a URL — which is most of what a code is for.

**`digest_eq` is constant-time**, and it is a primitive because it is the one part of a token check
that cannot be written in Beck: `==` on two strings returns at the first byte that differs, and a
verifier that does that tells whoever is guessing how much of the guess was right. Length is compared
first and in the clear, because **the length of a digest is not a secret**. This is the named
exception to [`43`](43-threat-model.md) §43.3's "nothing in Beck's design attempts constant-time
anything" — the general claim is unchanged, because one comparison is not a side-channel programme.

Nothing here is checked against itself: BLAKE3's own published vectors, RFC 4648 §10's seven vectors
in §5's alphabet with every prefix of a 43-character string so no length class goes untested, six
spellings of one identifier normalising to one, and five near-misses that are not. The decoders read
what other encoders *write* — base64 accepts padding it does not emit and both alphabets, because **a
decoder that refuses what other encoders produce is a decoder that fails in production**, and the two
alphabets do not overlap so accepting both is unambiguous rather than lenient.

### A test block cannot exercise a capability

The first draft had a test that minted a token and opened it. **It does not compile, and the
diagnostic is right**: a test block's own row must be empty, and `cap.*` is deliberately not
auto-stubbable because stubbing a capability would bypass it. Both rules are right. Together they
mean **the layer of a library that holds a key is the layer Beck cannot test** — and writing
`stub cap.sign:` would make the test *worse than absent*, since the stub is a constant and a tampered
token would verify against it.

Taking that deliberately improved the library. `crypto.beck` is two layers with the key at the seam:
everything about the token's *format* is pure and takes the code it expects as an argument, so every
way a forgery can arrive is reachable from an ordinary `test` block, and what is left is one Rust
test that a real key produces a code only that key reproduces. **The general statement is not about
this library: a Beck library whose functions require a capability has a Rust-tested edge, and the
smaller that edge the better.**

### Nine match arms cost a thousand levels of recursion

Adding the nine primitives to the evaluator's `match op` broke a test that builds a tree 1,000 deep:
`beck test` overflowed its stack in a debug build. **Nothing about digests is recursive.** The
mechanism is that the primitive dispatch is one arm per primitive, its frame is as wide as the widest
arm, and it is reached from the recursive path — so inlining merges the two and **every local a new
arm adds is a local every nested call carries.**

The fix is one attribute. What is worth recording is the **coupling**: the evaluator's declared stack
is a budget shared between the depth a program may reach and the width of a `match` nobody thinks of
as costing anything, and the only reason this was caught is that the depth is *asserted by a test*
rather than discovered by a user.
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)'s argument that a bound must be
declared rather than discovered has a second illustration: **the number it declares moved because of
a change in a different crate, and a test said so within the hour.**

## 46.8 The numeric tower: bignums, coercion, and a decimal that refuses to guess

Arbitrary-precision integers and decimals, **written in Beck**, with no new primitive and no new
dependency. §25.7 ordered the tower — reals first, then rationals and bignums — and these are the
floor underneath both, plus the decimal built on that floor.

**Why a file and not a primitive**, and this is where §46.1's division first cost something real.
Schoolbook long multiplication is not a host's table or grammar: a carry, a borrow and a trial
quotient are arithmetic on `Int`, and `Int` is something the language has. Taking `num-bigint` would
have been faster to run and would have been the admission the directory exists to avoid making. That
argument was easy to write for `money.beck`, where the alternative was silly; **it is worth more here,
because the alternative was *reasonable*.**

A `Big` is a sign and a list of base-10,000 limbs, least significant first. **Base 10,000** so two
things are true by inspection rather than by argument: a product of two limbs plus a carry is nowhere
near an `Int`'s range, and a limb is exactly four decimal digits so rendering and parsing are
*grouping* rather than division. **Canonical form** — no leading zero limbs, zero is the empty list —
because a library whose zero has two values is a library whose tests pass and whose folds do not. And
**a magnitude layer that knows nothing about signs**, because a sign rule applied halfway through a
borrow is the classic way to get subtraction wrong, **and the way not to is to have nowhere to apply
it.**

`impl Num for Big` is the third floor added from outside the compiler, and the first where the type is
a number in the ordinary sense — **which matters, because a tower whose new floors can only ever be
domain types is not a tower.** Only `div` can fail, so the row is inferred from it alone and `a + b`
stays pure; the checker enforces that in the direction that is easy to get wrong, since a `try:` over
an expression that cannot fail is itself a diagnostic.

**Coercion is a decision more than it is code**, and none of it is implicit: `big(n)` is total,
`big_to_int` answers an `Option` because narrowing is a question with a "no" rather than a failure,
and `big_to_real` is lossy and therefore a different function so a reader has to mean it. Adding a
bignum is exactly the moment the refusal of `1 + 1.0` comes under pressure — an `Int` that silently
widened on overflow would be convenient. **It would also make the cost of arbitrary precision
invisible**, and `Int` arithmetic is checked precisely so that overflow is a message rather than a
wrong answer. Silent widening trades a loud, cheap failure for a quiet, expensive success.

The one detail worth reading the code for is **the most negative `Int`**, where a conversion written
the obvious way has exactly one input it cannot take: `abs(n)` of it overflows, so the conversion
peels limbs off the *signed* value and the reverse accumulates negatively. It is also a number with no
literal — its text is not a token — so the test reaches the value by arithmetic on both sides.

### `/` is exact, or it refuses

A `Decimal` is `units × 10^-scale` with `units` a `Big`. **Exact, and therefore not a real**: `0.1` is
not one of the numbers IEEE 754 holds, which is why `0.1 + 0.2 != 0.3` there and is `0.3` here. Not a
rational either — a third has no decimal expansion, and this type says so rather than rounding.

**The form is canonical, so `1.50` and `1.5` are the same value** — the opposite of `BigDecimal`,
whose equality is scale-sensitive — and the reason is [`54`](54-ordering.md): the value order here is
a `Map`'s iteration, the state digest and the patch stream a replay must reproduce bit for bit. **Two
representations of one number are two keys and two digests for one fact, and that is a worse bug than
any surprise about trailing zeros.** What it costs is that a scale is not a significance; `render_at`
carries that as a *presentation* decision, and `money.beck` remains the type whose scale is fixed by
its currency.

`a / b` produces digits until the remainder reaches zero, and a quotient that never gets there
**raises**; `divide_to(a, b, scale, rule)` is the division that always answers and takes both. **A `/`
that silently picked a scale and a rounding rule would be guessing**, and this library set refuses to
guess in the two places it costs most — `money.beck`'s `split` exists because rounding each part
independently loses money silently, and §46.3's rule is that a reader raises rather than inventing a
value. The caller knows how many digits it wants and this type does not.

There is a **bound, and it is honest rather than clever**: deciding whether a quotient terminates
needs a `gcd` and a factorisation this library does not have, so the cap is 40 places. Forty is past
anything a program asks for. It is also what keeps a refusal **cheap** — the expansion costs one long
division per digit, so the bound *is* the work done before `/` gives up, and at 100 it was 25 seconds
on one input. §46.16 records what the bound gets wrong.

`Rounding` is `HalfEven`, `HalfUp` or `Down`, **because picking one would be picking a jurisdiction**.
The comparison is `2 × remainder` against the divisor, on `Big`s, so "exactly half" is `==` on integers
and never a question about a fraction.

### How the tower is known to be right

Constants first — factorials and powers of two computed by an outside arbitrary-precision
implementation and said to be one. Then `property` blocks checking this implementation against the one
the language already has: for every generated pair, `Big` arithmetic and `Int` arithmetic agree, and
division and remainder reassemble the dividend. Then cross-checks against `i128` over 400 pairs built
*past* `Int`, where the property blocks cannot reach, and against exact rational arithmetic over 300
rounding cases and all three rules.

**Be exact about what that establishes, because §46.5 was exact about the same shape for the calendar
and the limit is the same one: it is not two independent algorithms.** It is one claim checked against
a different implementation of it, on a different evaluator, in a different language. What it catches
is a transcription error, a carry dropped at a boundary, and a sign rule wrong in a case nobody wrote
a constant for.

**The part of the rounding cross-check worth copying is not the count.** Fifty-two of its 300 cases
are generated to land *exactly on a half* at their scale. A uniform sample almost never does, and
exactly-half is the **only** input where the three rules disagree — so a randomly-sampled rounding test
has almost no power over the thing it is supposed to be testing. The generator is a fixed xorshift
rather than a random source, **because a cross-check that fails on Tuesdays is a cross-check nobody
keeps.**

### What writing the tower found

**A comment, and the finding is about comments.** `str_slice`'s third argument is a **count, not an
end index** — the signature reads either way and the compiler's own comment beside it did not say.
Nothing had ever exercised the ambiguity: every other call site in the tree passes a start of zero or
a count that clamps, and under both readings those give the same answer. **This was the first caller
with a non-zero start and a real count, and it got a plausible wrong number rather than an error**,
which is the worst available outcome. Worse, the comment that *was* there said indices are byte
offsets; they are characters, which a passing test had asserted since §46.4. No test can check that a
sentence describes the code beside it; what it can have, and now has, is the sentence stating the thing
a signature cannot.

**And three defects in `beck doc`, all found by one file importing a sibling.** `decimal.beck` is the
first module in `lib/` to `import` another, and:

- **A rounded division was not canonical.** It built its result directly rather than through the
  constructor, so `1 / 2` at four places was the same *number* as `0.5` and not the same *value* —
  a second `Map` key and a second digest for one amount. The file's own tests could not catch it,
  because every rounding test used scale 0 where canonicalisation has nothing to do. It was found by
  asking what the division should *equal* rather than what it should render as: **a test that compares
  rendered strings cannot see a representation defect at all.**
- **`beck doc` could not read a module that imports another.** It read a single file while `beck check`
  and `beck test` went through the project loader, and nothing had noticed because no module anybody
  documented had ever imported one. Fixing that immediately produced the second half: the obvious thing
  to hand the documenter is the *sliced* program, which is every module merged — right for slicing and
  wrong for a page, so `beck doc` published 93 names, 53 of them the imported module's. The page is
  built from the root module's own interface now, which is what `beck iface` had been using correctly
  all along.
- **And the merged program dropped every module's doc comments but one.** The linker merged
  definitions, types, signals and tests across modules and never merged `docs`, and the accumulator is
  the *deepest import* — so a documented file reported 0 of 40 documented. **This had been wrong since
  separate compilation landed in Phase 2 and was unreachable**, because `beck doc` never resolved
  imports, so nothing ever asked the merged program for a doc comment.

**The pattern across all three is one thing.** A module importing another is a shape the compiler
supported and the *tools around it* had never been run against. `beck check`, `beck test` and `beck
iface` were right; `beck doc` was wrong three ways, and one Beck file found all of them — **which is
the argument for fixing a tool even when nothing is currently broken by it.**

## 46.9 The outbound call: the host is written where the call is

Every other effect atom in the language is a constant of whatever performs it. `durable` is
`durable`; `env` is `env`; `json_parse` raises `JsonError` and always will. **`net.out(host)` is
parameterised, and the parameter is a *value*** — so a primitive that makes a request has a row that
depends on one of its arguments, and no scheme in `prelude.rs` can say that.

The atom is also the one read by something outside the compiler:
[`06`](06-kubernetes-and-packaging.md) §6.5 derives the deployment's egress NetworkPolicy from a
program's `net.out` atoms and nothing else. **So the design question was not "what type does a
request have". It was: what has to be true of a program for the policy to be complete?**

`http_fetch(host, request)` takes the host as its first argument and that argument has to be a
**literal**. The checker reads it there and charges `net.out(that host)` at the call site;
[`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md) records the
decision, its two alternatives and what it costs. That makes `http_fetch` the second primitive whose
row is a function of an argument rather than a constant — `raise` was the first — and the precedent
is exact: in both cases the atom names the argument so that something downstream can read it, a
handler there and a NetworkPolicy here.

A program writes no `uses` clause and no `@on`, and gets:

```
name                 tier     kind       effects
fresh_rate           server   definition {net.out(rates.example.com), raises(HttpError)}
```

```yaml
# k8s/100-policy.yaml
  annotations:
    beck.dev/egress-hosts: "rates.example.com"
```

The refusals are the same rule from the other side. A computed host is `B0395`, because it is a call
the deployment could not be told about. A literal that is not a name a `uses net.out(…)` clause could
also write is `B0396` — a URL, a host with a port on it, or `origin`, which is refused for its own
reason: it is the one outbound atom a *client* tier discharges, and a client reaches its own server
over the command channel rather than by fetching.

**A status is a reply.** A 500 arrived; a client that turned it into an exception would have thrown
away the sentence explaining why. So `HttpError` has the three cases where *nothing* arrived, and a
fourth the primitive never raises and the library's `require_ok` does. A caller who wants "give me
the body or fail" gets it in one call; a caller who wants to read a 429's `retry-after` still can.
And `json_body` is where two failures inherited from two callees stay distinguishable — a `try:`
catches the one its type names while the other travels, so "the peer said 503" and "the peer said
something that is not JSON" cannot be confused for one another.

## 46.10 A credential could not be sent

`lib/http.beck` was meant to have `with_bearer(req, token: secret[Str])`, and the first attempt was
the obvious one:

```
error[B0320]: argument mismatch: expected `internal[?5]`, found `secret[Str]`
   |     return with_header(req, "authorization", "Bearer " + reveal(token))
   |                                                                 ^^^^^
```

The compiler is right, and for §3.5's best reason: **there is no `reveal` for a `secret[T]`.** That
is the claim that keeps one out of a browser. So a secret cannot become the `Str` a header value
needs — and an authenticated outbound request, which is nearly all of them, was inexpressible.

**Note what the tree did *not* say about this.** `corpus/03-billing.beck` has held an `ApiKey` since
Phase 1 and has never used it. The gap was invisible for three phases because no program had ever
tried to *spend* a secret — which is §46.6's lesson one wave later: writing a library finds what
writing a compiler does not.

The fix is not a `reveal` for secrets, which would delete the property. `HttpRequest` has a
`secrets: Map[Str, secret[Str]]` field, and the runtime merges it into the headers **at the edge**,
past every tier the checker places. The credential becomes bytes exactly where it becomes a request,
and never becomes a value the program could put somewhere else. A request carrying one is not
Sendable, so the type system already refuses to let it near a client with nobody asserting anything.
The two halves are both gated: `"Bearer " + reveal(token)` is still a compile error, and the
credential is still on the wire.

## 46.11 The network, on the seam F11 asked for

[`14`](14-review-findings.md) F11 names three resources that cannot be retrofitted: clock, network
and disk. [`44`](44-wave-0-report.md) §44.3 closed the clock, three phases late.

This closes the network, and closes it **early** — `beck_core::net::Outbound` is a trait with three
implementations *before there is a second caller*. `Refusing` is the default, so a process that has
not decided to make outbound calls refuses them and says so in a sentence. `Canned` is replies
decided in advance and requests kept, which is what a Rust-level test uses when it wants to assert
what a program sent. The real one is over the hyper that has been in this workspace since Phase 1 —
**no new direct dependency**. `beck run` installs it and `beck test` deliberately does not, because
`net.out` is auto-stubbed there and a test that reached a socket would depend on somebody else's
uptime.

Two bounds are in the implementation rather than in the language, and both are stated where they are
set: a 10-second deadline per exchange, and 8 MiB of reply read at most — **a peer that streams for
ever is the cheapest denial of service there is.** Both are elapsed time and bytes, neither enters
the log, and neither can change what a replay produces.

`beck test`'s existing machinery needed **no change at all**, which is the strongest evidence that
the atom is an ordinary atom: `stub net.out(rates.example.com): 42` names the peer, and a test that
says nothing gets the auto-stub and a line in the report saying what it did.

## 46.12 The library becomes importable, and what that broke in it

For three phases `lib/` was a standard library **nothing outside `lib/` could import**, and the
demonstration is one file in two directories:

```console
$ cp probe.beck lib/    && beck test lib/probe.beck
test "reachable?" … ok

$ cp probe.beck clbg/   && beck test clbg/probe.beck
error[B0603]: cannot find module `bignum`
```

`import x` resolved against the directory the root module lives in and against nothing else. There is
no search path, so `decimal.beck` could import `bignum.beck` because they are siblings, and **no
program a user writes could import either**. Nothing had noticed because nothing had tried: `lib/`
files import each other, the corpus project imports within itself, and the two benchmark suites
import nothing. A benchmark suite in a new directory wanting the numeric tower was the first thing in
three phases to reach across a directory boundary.

The fix is [`10`](10-decisions.md) D23 and
[`adr/0018`](adr/0018-the-standard-library-is-carried-in-the-compiler.md): the Beck half of the
library is compiled into the `beck` binary and resolved **after** the caller's own directory. It is
one `or_else` in the one function every command already goes through, so every command got it at once
and none of them mentions it.

**The order is the decision and it could have gone the other way.** Directory first means a program
that already has a `text.beck` keeps working the day the standard library grows a `text`, so adding a
library can never break a program that never asked for it; library first would reserve every name in
`lib/` for all time and make each addition a breaking change for somebody. The cost is that a local
module silently shadows a library one — **Python's cost, taken with Python's eyes open** — and it is
visible in the one place it should be: `B0603` now says where it looked. Why the table is in the
binary rather than on a path is the ADR, and the short form is that the *other* half of the library —
the primitives — has always been in the binary and the Beck half is written against it version for
version.

**What it broke in the library is the thing worth reading.** Beck links modules into one flat
namespace with no qualified reference, and making the library importable pointed that at the library:
**every module in `lib/` now has to link with every other one**, because a program can import two of
them. Two collisions were waiting and neither had ever been reachable — `is_negative` in two files,
and `pow10` in two more, returning different types. `the_whole_library_links_into_one_program` is the
gate, and it is worth more than the two renames: **it is the first thing in the repository that holds
the standard library to being *one* library rather than ten files that each happen to compile.**

The same property has a sharper edge pointing outward, and it is a **cost of D23 rather than a
defect**: a program that imports `bignum` cannot define `one`, `zero`, `divide`, `trim` or `expand`.
Qualified imports are the fix and they are a package-system decision; until then, importing a library
reserves its helper names in the importing program, and the diagnostic says so precisely.

**What an import costs to compile**, release build, median of seven — a module is checked from source
on every build, and there is no interface cache:

| the program | lines checked | check |
|---|---|---|
| no imports | 2 | **2.8 ms** |
| `import format` | 69 | **3.5 ms** |
| `import bignum` | 568 | **11.5 ms** |
| `import decimal` (which imports `bignum`) | 1,056 | **17.9 ms** |
| all ten library modules | 2,555 | **32.5 ms** |

About 12 µs a line, which is [`64`](64-compile-speed-report.md) §64.6's front-end figure arriving at
the same place from the other direction. **The place that will feel it is the editor**:
[`65`](65-the-editor-report.md)'s server re-checks the whole file on every change and does not resolve
imports at all, so the day it does, this table is what it adds to
[`04`](04-compiler-architecture.md) §4.6's 100 ms budget. There is room at these numbers and there
would not be at ten times them.

## 46.13 The Benchmarks Game harness, and an oracle nothing here chose

[`compiler/clbg/`](../compiler/clbg/README.md) — eight of the Computer Language Benchmarks Game's
ten benchmarks, each verified against the Game's own published output file.

The objection that held it up was not that a harness would be hard. It was that **a harness verified
against invented numbers is worse than no harness**, and there was a precedent:
[`53`](53-are-we-fast-yet-report.md) §53.6, where a benchmark whose workload was discarded had no
oracle for its workload, so a loop that ran once instead of fifty times passed every verification the
suite published.

So the answer is not "we were careful". The Game's expected-output files are checked in verbatim, and
the gate makes them the only oracle **by construction**: five outputs are small, so the port asserts
the exact text and the gate *rebuilds that literal* from the published file — escaping it the way
Beck's lexer would — and fails if the source does not contain it; two are 10,245 characters, so the
port asserts a digest and the gate recomputes the hex from the published file with the same BLAKE3
the `digest` primitive is.

**A wrong constant fails the Beck test, and a wrong constant with a matching wrong expectation fails
the Rust one.** Nothing in the directory chose a number. That is a stronger guarantee than
[`awfy/`](../compiler/awfy/README.md) has, where the constants are transcribed from the original's
`verifyResult` by hand and trusted — and it is stronger **because it could be**, not because Are We
Fast Yet was done carelessly: Are We Fast Yet publishes constants inside source files, and the
Benchmarks Game publishes *files*.

The same rule covers the pipeline. Two ports read a FASTA file on stdin, and Beck has neither stdin
nor a `test` block that can open a file, so both call the generator instead — and that substitution
is exact only if the Game's published inputs really are its published output, so the gate asserts
they are byte-for-byte rather than the ports assuming it. They are.

Three differences from the originals are larger than mechanical, and each is a thing **not** measured
here. **Nothing is parallel**: two of the ports are contributed in thread-pool form, and in each case
the pool divides a space that is summed before anything is printed, so the answers are identical and
the work is the same work — what is not measured is the parallel decomposition, which Beck has no
threads to express. **`binarytrees` measures less here than anywhere else**: the Game is blunter
about that benchmark than any other, because the number it wants is the *allocator's*, and Beck
exposes no allocator, no GC configuration and nothing about how the evaluator represents a node — the
port honours the *work* and cannot honour the memory rules. And **`nbody` is the second port of that
program in the tree, deliberately**: the two suites verify seventeen significant figures after one
advance against two printed decimals after a thousand, so a port satisfying one would not have been
checked against the other.

Two of the ten are not ported, and each reason is a fact about the language rather than about effort
— so the gate asserts both are *still* facts, and the change removing one turns a test red.
`mandelbrot`'s published output is a binary PBM with NUL bytes in it, and Beck's `Str` is UTF-8 with
no byte string beside it. `regexredux` requires the Game's nine specific patterns and Beck has no
regex — **writing one in Beck would make the benchmark measure our regex engine, which is the one
thing that instruction exists to prevent.**

**No comparative claim is made, and the Game's own table is the specific comparison being declined.**
[`25`](25-benchmarks-and-expressiveness.md) §25.2 calls the Game "widely quoted and widely misused";
entering a number from a placeholder interpreter into that table would be the misuse rather than a
contribution to it, and condition 3 of the Game's licence says the same thing from the other side.
Three things make even the internal numbers weaker than they look, all stated in the measurement file
rather than left for a reader to discover: **every port runs the Game's format-checking size, not its
measuring size**, because the oracle exists only at the first — so these are times for programs a
hundred to fifty-thousand times smaller than the ones the Game's table is about; a file runs its
imports' tests too; and a `test` block has no local bindings, so an assertion about five properties
of one 10 KB output computes that output five times.

**A flat namespace has a first cost here too.** Two ports both import a third, so had each named its
entry `benchmark` — which was this directory's convention until it stopped compiling — the three
could never have been linked together. Every entry point is named after its own benchmark, and the
gate holds the convention rather than leaving it to habit. It is a **constraint recorded, not a
defect**, and it is worth recording because a directory of independent programs is precisely the case
where every file wants the same names.

## 46.14 The accumulator idiom is quadratic, and the step budget cannot see it

**`list_append` copies the whole list.** Beck has no mutable sequence, so every loop that builds one
is written as a tail-recursive accumulator — `return go(i + 1, list_append(done, x))` — which is how
`lib/` accumulates limbs, how both benchmark suites build their arrays, how the corpus builds lists
and how both SICP chapters do. **Every one of them is `O(n²)` in time.**

| n | wall clock | ratio | evaluator steps | steps per element |
|---|---|---|---|---|
| 1,000 | 13 ms | | 14,014 | 14.0 |
| 2,000 | 33 ms | 2.5× | 28,014 | 14.0 |
| 4,000 | 105 ms | 3.2× | 56,014 | 14.0 |
| 8,000 | 385 ms | 3.7× | 112,014 | 14.0 |

**The two halves of that table are the finding.** Time approaches 4× per doubling — quadratic — and
the step count is exactly 2× per doubling — linear, flat at 14 steps an element. So the cost is real
and **the evaluator's own budget cannot see it**: a step is a node evaluated, and a primitive that
copies ten thousand values is one step. [`53`](53-are-we-fast-yet-report.md) built `--fuel` as the backstop that
"bounds one evaluation"; it bounds one evaluation's *nodes*, and a program can do unbounded work
inside a bounded number of them. That is a second finding, and it is about the backstop rather than
about lists.

Counting primitive calls through the evaluator on real programs rather than that loop, **reads
outnumber appends about three to one** — which is what decides between the two fixes. And the copying
is *not* what makes those programs slow: their lists are limb vectors and small collections, ten to
twenty-five elements long. **The quadratic bites in proportion to how long a list gets**, which means
it is invisible in a tree of programs that never build a long one and unbounded in the first program
that does — a 100,000-element list costs five billion element copies to build. It is a defect waiting
for its caller, in exactly the way division was (§46.15).

[`19`](19-phase-1-report.md) §19.4 found this exact shape in the *fold*, and `scaling.rs` exists to
keep it fixed, opening with the sentence that settles what this is: **"That is a semantic defect, not
a backend one: it would survive into Cranelift unchanged."**

**The obvious cheap fix does not work, and it is worth writing down why.** Pushing in place when the
primitive holds the only reference was tried and measured: **no change at any size**. At the moment
`list_append(done, x)` runs, the caller's frame still binds `done`, so the reference count is two and
the copy happens anyway. The value's owner is the environment, and **the environment does not know
the binding is dead.**

So the fix is one of two real changes:

| | what it is | what it costs the 3:1 majority |
|---|---|---|
| **Last-use moves** | Compute, per function body, which occurrence of each local is its last, and have the evaluator *take* the binding from the frame there instead of cloning it. Its correctness rests on closures, since a frame captured by one cannot be emptied | **nothing.** A list stays a contiguous `Vec`, so a read stays one indexed load |
| **A persistent sequence** | Append becomes `O(log n)` with sharing and no analysis at all. A new dependency, and a change at all fifty `Value::List` sites, twenty-six of which take a contiguous slice the structure cannot hand out | **a pointer chase per read**, on the operation that happens three times as often as the one being fixed |

**The first, and the measured mix is why**: the second makes the common operation slower to make the
rarer one asymptotically better. It is also the one that keeps paying — uniqueness information is
what lets a *compiled* backend turn a functional update into an in-place write, which is how Koka's
Perceus and Roc reach performance a persistent-vector language does not aim at.
[`70`](70-the-evaluator-gets-fast-report.md) built exactly that, and
[`93`](93-the-native-backends-report.md) §93.7 is where the same question came back on an arena that
cannot prove uniqueness — and was answered a third way.

## 46.15 What a caller made faster: the trial-digit bracket

A gate that costs two minutes is a gate somebody stops running, and "the benchmark is inherently
expensive" is the kind of answer that should be checked before it is accepted. **It was wrong.**

§46.16 had already named the suspect, in the row saying what a first
implementation deliberately did not do: Knuth's algorithm D is "the thing to replace first if any of
it is ever a bottleneck. The binary search for a trial digit is fourteen comparisons where the
estimate-and-correct is one multiply and a rare fixup; it is here because it is *obviously* right,
which for a first division is the trade to make."

`pidigits` is that bottleneck arriving. Every digit of pi costs several long divisions of
hundred-digit numbers, every long division costs one trial digit per limb, and every trial digit was
a fourteen-step binary search over `0..9999` — each step multiplying the **whole divisor** by a
candidate. Most of those limbs divide to zero, and the search paid full price to discover it.

**What changed is four lines and it is not algorithm D.** Knuth's *estimate* now brackets the search
instead of replacing it: the divisor is at least its top limb times `base^(n-1)`, so the digit is at
most one bound; it is less than one more than that, so the digit is at least the other. Two integer
divisions, against a multiplication over every limb per search step. **The search still runs between
those bounds, so the digit is still the one the search would have found and the search is still what
proves it** — when the top limb is large the bracket is one digit wide and the search confirms it in
a single comparison, and when the top limb is 1 the bracket is wide and the cost is what it always
was. Never worse, because the bounds hold either way.

| | before | after |
|---|---|---|
| `pidigits`, steps for `N = 30` | just over 100,000,000 | **under 16,000,000** |
| `beck test clbg/pidigits.beck`, debug | 89 s, and only with `--fuel 200000000` | **15.9 s**, default budget |
| `cargo test --test clbg`, debug | 127 s | **52 s** |
| `beck test lib/decimal.beck`, debug | 8.0 s | **3.5 s** |

**The last row matters most, and it is not a benchmark**: `decimal.beck` divides on every rounded
quotient it computes, and nothing about it changed. Every caller of the standard library's
arbitrary-precision division got 2.3× faster — **which is an argument for the §46.1 division that put
this in Beck rather than in Rust**, because the fix is in a file, in the language, with the library's
own property tests as the check.

Those tests are the reason it was a safe change to make in an afternoon: property blocks check every
result against `Int` arithmetic over 100 generated pairs, and the harness checks 400 more against
`i128`. Two tests were added for what a bracket specifically can get wrong — a divisor whose top limb
is `1` (the widest bracket) and one whose top limb is `9999` (the narrowest), a quotient limb of zero
and one of 9,999 — and a property that divides `divisor · d + r` back for every digit a limb can
hold, **because a bound that is wrong one time in ten thousand is not something a hundred random
pairs will find.**

This is the first time a benchmark in this repository has made the language faster rather than just
measured it. The suite's value is not the numbers it prints, which §25.9 will not let us publish
anyway. **It is that it is the first caller demanding enough to make a cost visible** — and §46.14 is
the same lesson learned one level further down, and only after somebody said the first answer was not
good enough.

## 46.16 What is not built

| | Status |
|---|---|
| **A linear `list_append`** | **Not built** (§46.14), and it is the largest performance item in this chapter. Every accumulator loop in the language is quadratic in time, and the evaluator's step budget cannot see it |
| **A work-counting budget** | **Not built.** `--fuel` counts nodes evaluated, and §46.14's table is one program whose steps are linear and whose cost is quadratic |
| **`Ord` as a trait** | **Not built** (§46.6), and [`54`](54-ordering.md) writes it out and does not recommend it. Ordering is the runtime's structural one, so a type cannot say what its own order is |
| **TLS** | **Not built.** `http_fetch` speaks HTTP/1.1 over TCP, so a credential sent with `with_secret_header` is confidential exactly as far as the network under it is. `pending_security.rs::an_outbound_call_has_no_transport_security` asserts the absence. Taking a TLS stack is a dependency decision, and [`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md) is where it was taken for the identity work |
| A host that is a value | **Refused, deliberately** ([`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md)). The cost is one call site per host in an application that talks to several; a host-parameterised client type is the shape that would lift it, and it needs type-level strings |
| Redirects, retries, cookies, connection reuse | **Not built.** A redirect that silently changes which host is reached is the last thing a derived egress rule wants, so following one is an application's decision. Retry is writable in Beck today and is not written |
| Percent-encoding and query building | **Not built.** It needs a code point, and no primitive gives one |
| Repeated headers | **Not representable.** `headers` is a `Map[Str, Str]`, so a second `Set-Cookie` replaces the first. Said in `prelude.rs` where the field is declared |
| HTTP/2, streaming bodies, per-call timeouts | **Not built.** A per-call deadline is a language question — §3.6 would have to give it a place in a signature |
| Qualified or namespaced imports | **Not built**, and it is the largest thing D23 leaves open. Importing a library reserves its helper names ([`16`](16-packages-and-ecosystem.md) §16.7) |
| Selective import, a third-party path, an interface cache | **Not built.** There is one implicit source and it is the compiler's own library; every import is checked from source on every build |
| The LSP resolving imports | **Not built**, unchanged: a file is analysed alone, so a name imported from anywhere is unresolved in the editor. D23 makes that gap easier to hit and does not widen it |
| `@derive` for JSON | **Built, and in `lib/` where the row wanted it.** `import json` and `derive_json:` over a `model` generates its `ToJson` impl — the fields are read out of the declaration at compile time by a macro ([`02`](02-syntax.md) §2.4, [`102`](102-the-macro-interpreter-report.md)), so **reflection is still not being added**: what runs is an `impl` naming each field, which is what somebody would otherwise have written. It could not ship here until a macro crossed an import, which is the other half of what this row was waiting on and was not written down anywhere. The base cases — `Int`, `Float`, `Str`, `Bool` — are written by hand on purpose, because what `Int` means as JSON is a decision rather than drudgery. **And the row has a second half now**: `json_of(e)` is a *typed* macro in the same file ([`102`](102-the-macro-interpreter-report.md) §102.9), so a `model` nobody decorated — somebody else's, three imports away — gets an encoder from the value rather than from the declaration, reached through a list, a `newtype` and a field, still with no reflection at run time |
| A set's cost | **A map's.** `Set[T]` is `Map[T, Bool]`, so it is an ordered structure with a comparison at every step and a `Bool` per member nobody reads. No hash set, no bitset. `intersection` and `difference` are linear in the left side, via a list — a map-to-map operation would be a primitive, and the division admits one only for a host's table or grammar, or for a combining form the view engine has to recognise ([`lib/README.md`](../compiler/lib/README.md)). A set operation is neither |
| Time zones and locale | **Not built, deliberately.** A time zone is a database with a release schedule, and a replay must not disagree with the run it is replaying about what a date is |
| Weeks, ISO week dates, quarters; parsing a duration; a date-only parse in the host | **Not built.** Reading is where a grammar starts, and a grammar is the host's half |
| Money beyond two minor digits | **Not built.** `lib/money.beck` is fixed at two, which is wrong for JPY and KWD and says so. Rewriting it over `Decimal` is now expressible and is a change to a type other files use, so it is not being done in passing |
| **An exact quotient needing more than 40 places** | **Refused, and wrongly** (§46.8). `1 / 2^41` terminates and this raises. The fix is a `gcd` and a count of twos and fives in the reduced divisor, which is a `gcd` this library does not have — the trade is stated rather than hidden |
| `gcd`, a rational over `Big`, square root, powers, logarithms | **Not built.** None of the last three has an exact decimal answer in general, so each needs the scale-and-rule shape `divide_to` has, and none is written |
| An exponent in the decimal reader | **Not built.** The lexer takes one for a *float literal* and this reader does not, which is an inconsistency named rather than defended |
| A scale that carries significance | **Not built, deliberately** (§46.8). `render_at` is the presentation; `money.beck` is the fixed-scale type |
| **Asymmetric signatures, and encryption of any kind** | **Not built.** No Ed25519, no RSA, no JWKS, no JWT verification, no AEAD, no key agreement. **A digest is not encryption**, and a program that needs confidentiality at rest does not get it here. [`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md) says why the symmetric half was taken without the dependency decision, which is still owed — [`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md) then took it for the identity work rather than for the library |
| **A `distinct` that sorts** | **Not built, and a decision rather than an omission** ([`99`](99-the-data-tier-means-of-combination.md) §99.9 item 7). The library has had two duplicate-free lists since it was written — `unique`, which keeps the order the list had, and `elements(set_of(xs))`, which sorts — and the view engine needed *one* of them named as a primitive so it could maintain it. `list_unique` is `unique`'s answer, so no third answer entered the language and `unique`'s body is now a call to it. The sorted spelling stays a composition, and a program that wants it writes what it always wrote |
| **A sum over `Float`** | **Not built, and a decision rather than an omission** ([`99`](99-the-data-tier-means-of-combination.md) §99.9 item 6). `list_sum` is `list[Int] -> Int`: it is the *exact* sum, raising only when the answer does not fit, so it agrees with folding `+` wherever the fold has an answer and also has one where the fold overflows on the way back inside `Int`. Over `Float` that same definition would not extend the fold, it would **disagree** with it — an order-independent float sum is a different number in the last bits, on ordinary inputs — so a program holding one of each would have two answers to one question. A float total stays the recursion a program writes and `beck explain cost` prices it as the recompute it is |
| Random bytes | **Not built, and a decision rather than an omission.** `uuid()` mints an identifier and is `nondet`; a general `random_bytes()` would be a second nondeterministic source at the edge, and the one thing it is usually wanted for — a key — is `secret_env`'s job |
| A binary key, key rotation, a key id, an expiry in a token | **Not built.** A key is a `secret[Str]`, so a binary key is hex or base64 first. An expiry is *deliberately* absent: a time is `now()`, which is `nondet`, and putting one inside `sign` would make a signature nondeterministic — a caller puts an instant in the payload it signs |
| A parsed UUID type; the variant bits | **Not built.** An identifier is a `Str` before and after and `uuid_parse` normalises rather than changing the type; a `Uuid` newtype is expressible in Beck today and is a program's decision. Nothing validates the variant, because **a value whose variant bits say nothing is still an identifier and refusing it would refuse an identifier that works** |
| Per-definition module provenance | **Not built**, and it is what §46.8's second `beck doc` finding is really about. A definition does not record which module it came from; the page is right because the *interface* is the root's, not because the program can be filtered |
| Sub-quadratic multiplication, and Knuth's normalisation | **Not built** (§46.15). Normalising both operands would make the bracket one digit wide *always* rather than usually; what is built is the estimate alone, and the case it does not help is the case a test now pins |
| `mandelbrot`, `regexredux` | **Not ported** (§46.13), and the gate asserts both reasons are still facts |
| The Game's measuring sizes, and a rate gate | **Not run, and not built.** No oracle exists at the measuring sizes, and [`13`](13-testing.md) §13.7's "a gate that flakes gets deleted" covers the rest |
| A benchmark of any primitive | **None.** [`25`](25-benchmarks-and-expressiveness.md) §25.3 measures the evaluator at roughly 33× CPython, so a measurement of `str_split` would measure the interpreter |

Two smaller findings, recorded rather than fixed. `{}` in an argument position with no expectation is
`B0346`, because a record literal is checked against an expectation and `list_fold`'s second argument
has none to offer — its type is decided by the *third* argument; checking arguments in dependency
order rather than left to right would fix it, and that is a change to inference rather than to a
library. And `expect try: f() == x` parses as `try: (f() == x)`, which is the block rule doing exactly
what §2.3 says and is still a surprise; `(try: f()) == x` is the fix, and a diagnostic when a
handler's block is a comparison does not exist.

## 46.17 The gates, and what this establishes

| | |
|---|---|
| `stdlib.rs` | Every file in `lib/` is a library and runs its own tests; **every file is importable from a directory that is not `lib/`**, in the strong form — `beck test` on a probe — so a file added to the directory and left out of the table fails here; the whole library links into one program; a local module shadows a library one; and the host behaviours the library rests on are pinned separately — character indices agreeing on `"héllo"`, a map's keys coming back in the values' own order, stable sorting with structural comparison, division truncating towards zero, and a record ordering by field name |
| `clbg.rs` | Every asserted output is rebuilt from the Game's published file; the two substituted inputs are byte-for-byte the published generator's output; the entry-point naming convention; and the two unported benchmarks are still unportable, with neither having an oracle file sitting unused |
| `outbound.rs` | The two halves of §46.10 — `reveal` on a secret is still a compile error, and the credential is still on the wire |
| `bignum.beck` | The trial-digit bracket at the divisor's widest and narrowest leading limb, at a quotient limb of zero and of 9,999, and over every digit a limb can hold |
| `beck_core::project` | A module resolves from the library with no file beside the root; a file beside the root shadows it; the library's tests do not become the program's |
| `check/mod.rs` | `the_spelling_a_missing_type_argument_suggests_is_one_that_compiles` — the first diagnostic in the tree whose suggested spelling is tested by writing it down |

**What the Benchmarks Game harness establishes** is that Beck computes what those eight programs
compute, character for character, against files somebody else published — a correctness result about
reals, integer arithmetic, string handling, maps and sorting, obtained from an oracle with no stake
in Beck being right. **It establishes nothing about speed**, and it does not establish that Beck can
express the Benchmarks Game: eight of ten is what was reached, and the two that were not are named
with the language feature each is waiting on.

**What `lib/` establishes** is that the §46.1 division has a place to live, a gate that runs it, and —
since D23 — a consumer. That last is not the compiler's own tests: it is that eight benchmark ports in
another directory import `format` from it, one of them computes 30 digits of pi through
`lib/bignum.beck`, and the answer is character-for-character the file the Benchmarks Game published.

**It establishes nothing about the package system.** One implicit source, no versions, no namespaces,
no third-party anything, and the one decision taken here that touches it — the precedence rule — is
written down as the thing a namespaced import would supersede.
