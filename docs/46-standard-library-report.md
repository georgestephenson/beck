# 46 — Phase 3 report, part 16: the standard library, first half

> **What this is**: [`08`](08-roadmap.md) §8.5.4's Wave 2, begun — strings, collections, JSON, time,
> and the directory that holds the half of the library written in Beck. It is the bullet
> §8.5.3's trap 2 said must not be started before [`45`](45-error-rows-report.md), and the first
> thing it added is the reason that was right.

## 46.1 The division, which is the only interesting decision here

A standard library has to answer a question about itself: which of it is the *host's* and which is
the *language's*. Getting that wrong in either direction is expensive. Too far one way and the
library is a thin wrapper over Rust that proves nothing about Beck; too far the other and
`str_upper` is a hand-rolled Unicode table that is slower and wrong.

The line this project draws is **not** "what is fast". It is **what has a definition in the
language**:

| Kind | Where | Why |
|---|---|---|
| A host's table or grammar | a primitive | `str_upper` is a Unicode table; `json_parse` is somebody else's grammar; `time_format` is the civil calendar. Writing any of them over a `list[Str]` in Beck would be a slower, less correct copy of what the host already has |
| Composition | `compiler/lib/*.beck` | lines, words, padding, an amount of money, a split that adds back up. There is nothing to ask the host for, so asking would be an admission |

`compiler/lib/` is new and is the second half of the deliverable. If Beck cannot express its own
library, [`01`](01-vision-and-premise.md) §1.1's argument about means of abstraction is not one this
project is entitled to make — and four phases of writing a *compiler* had never tested it, because
every corpus program is shaped like the todo sketch ([`25`](25-benchmarks-and-expressiveness.md)
§25.6 found the same thing about the language's own abstractions).

## 46.2 What was built

**Thirty-one primitives.** Thirteen strings (`str_len`, `str_slice`, `str_split`, `str_join`,
`str_contains`, `str_starts_with`, `str_ends_with`, `str_upper`, `str_lower`, `str_replace`,
`str_index_of`, `str_repeat`, `str_chars`), fifteen collections (`list_get`, `list_slice`,
`list_reverse`, `list_take`, `list_drop`, `list_contains`, `list_index_of`, `list_append`,
`list_fold`, `list_all`, `list_any`, `list_flat_map`, `list_zip_with`, `map_keys`, `map_merge`),
two for JSON and two for time. The higher-order ones are row-polymorphic in their argument's
effects, so a pure caller of `list_fold` stays pure however another caller uses it — §3.2's
`map : (list[a], (a -> b ! e)) -> list[b] ! e`, which nothing about this had to re-establish.

**Three libraries in Beck**, each with its own `test` and `property` blocks:

- `lib/text.beck` — lines, words, padding, case, truncation, a tolerant pair reader.
- `lib/money.beck` — an exact amount in one currency as minor units, with a `split` whose parts
  sum back to what was split.
- `lib/documents.beck` — a JSON document read as data with `match`, and RFC 3339 instants.

`beck-cli/tests/stdlib.rs` gates the directory rather than a list of names, so a library added and
not registered is not a thing that can happen. It also asserts each file is still a *library* —
`beck check` says "a library" — because "a library runs its own tests" and "a library is an
application" are different claims and [`27`](27-walls-report.md) removed the wall between them
without merging them.

## 46.3 The shape the wave was waiting for

`json_parse : Str -> Json ! { raises(JsonError) }` and
`time_parse : Str -> Int ! { raises(TimeError) }`. Both **raise**; neither returns a `Result`.

That is the whole of §8.5.3's trap 2, cashed. A caller who wants a `Result` writes `try:`; a caller
already inside something fallible writes nothing at all. Had these been written a wave earlier,
every one of their signatures would now be wrong, and so would every signature that mentions them.
`lib/documents.beck` is where the difference shows: `field_of` has a `Result` in its type because it
is the boundary, and nothing inside it mentions failure.

The same property runs through `lib/money.beck`. `same_currency` raises; `plus`, `minus`, `total`
and `split` inherit `raises(MoneyError)` by inference and never mention it in their bodies. "You
cannot add pounds to euros" is one line, not a rule four functions each remember.

## 46.4 Three decisions that are not obvious

**Indices are characters, everywhere or nowhere.** `str_len`, `str_slice` and `str_index_of` are
one unit. A library that counts in bytes and slices in characters is a trap with a test suite that
passes on ASCII, so `stdlib.rs` asserts the three agree on `"héllo"`.

**Out-of-range is clamped, not raised.** `str_slice("abc", 10, 5)` is `""` and `list_take(xs, 99)`
is `xs`. This is a decision and not an oversight: a slice is not a parse, and `raises` is for a
*program's* vocabulary rather than for the library's arithmetic. Where a result genuinely may not
exist, the answer is `Option` — `list_get` and `str_index_of` both return one.

**Time is arithmetic, not a dependency.** `time_format` and `time_parse` are Hinnant's
`days_from_civil` and its inverse: exact, well known, and — the property that decides it here —
**pure arithmetic with no table behind it**, so `beck replay` cannot disagree with the run it is
replaying because a time-zone database was updated in between. RFC 3339 in UTC and nothing else. An
offset is *refused* rather than silently shifted, because accepting `+01:00` would mean accepting
that two spellings of one instant are two values, and a log is not where you want to find that out.

`stdlib.rs` tests the two cases a round-trip would not catch — a wrong formatter and a wrong parser
agree with each other — so it checks 2000-02-29 (a leap day), 2100-02-28 (divisible by four, not a
leap year) and instants before the epoch, where floor division is the bug every hand-rolled
formatter has.

## 46.5 The wall this found, which is what writing a library is for

`lib/money.beck` was meant to be an `impl Num for Money`, so that `+` and `-` would work on an
amount the way they work on SICP §2.5.1's rationals in `sicp/ch2.beck`
([`41`](41-generic-arithmetic-report.md) built exactly that mechanism). **It cannot be.**

A trait's declared effect row is a bound every impl is held to ([`37`](37-traits-report.md) §37.5 —
and that check found a hole in what an *empty* declaration meant, which is why it is thorough). The
prelude's `Num` declares a pure row. Adding two amounts in different currencies has to fail. So:

```
error[B0370]: `Num::add@Money` performs more than its signature declares
   |     def add(self, other):
   |     ^^^^^^^^^^^^^^^^^^^^^ undeclared: raises(MoneyError)
```

The compiler is right and the design is incomplete. **An operator is unavailable to any type whose
operation can fail** — which is most of the interesting ones. A `Decimal` that can overflow, a
`Matrix` whose dimensions must agree, a saturating integer: none of them can have `+`.

The fix is known and is one feature: a trait whose method signatures carry a row *variable*, so an
impl may be as effectful as its own type requires and a caller inherits exactly that.
[`33`](33-effect-polymorphism-and-list-patterns-report.md) did precisely this for a user's
higher-order definitions — `map`'s `e` — and nothing has done it for traits.

It is recorded the way this project records walls: as a test that starts failing.
`stdlib.rs::a_trait_method_may_not_be_more_effectful_than_its_trait` asserts the refusal, and its
failure message says what to do — delete the test, give `money.beck` its `impl Num`, and correct
this section. That is `sicp/refusals/`'s pattern, applied to the first wall found since that
directory was emptied.

**Two smaller findings**, recorded rather than fixed:

- `{}` in an argument position with no expectation is `B0346`, because a record literal is checked
  against an expectation and `list_fold`'s second argument has none to offer — its type is decided
  by the *third* argument. `lib/text.beck` writes `no_pairs()` and says why. Checking arguments in
  dependency order rather than left to right would fix it; that is a change to inference and not to
  a library.
- `expect try: f() == x` parses as `try: (f() == x)`, which is the block rule doing exactly what
  §2.3 says and is still a surprise. `(try: f()) == x` is the fix. Worth a diagnostic when the
  handler's block is a comparison, which does not exist.

## 46.6 What is **not** built

The bullet says "collections, strings, time, money/decimal, HTTP client, JSON, UUID, crypto
primitives". Against that list:

| | Status |
|---|---|
| Strings | **built**, thirteen primitives plus `lib/text.beck` |
| Collections | **built for lists and maps**. No `Set` operations, no sorting beyond `sort_by`, no grouping, no deduplication |
| JSON | **built** as data plus a parser and a renderer. **No `@derive`**: turning a `model` into a `Json` is a function somebody writes, and reflection is not being added for it |
| Time | **built** for instants: RFC 3339, UTC, milliseconds. No durations, no arithmetic on dates, no zones, no locale |
| Money/decimal | **`lib/money.beck` is money, not decimal.** Fixed at two minor digits, which is wrong for JPY and KWD and says so; no arbitrary precision |
| UUID | `uuid()` has existed since Phase 1. Nothing parses or formats one |
| HTTP client | **untouched**. It is the one item on the list with an effect row nobody has designed — `net.out(host)` per call site, with the host in the type |
| Crypto | **untouched**, and [`07`](07-dependencies.md) says it is delegated to `ring`/`aws-lc-rs` rather than hand-rolled |
| Bignums and numeric coercion | **untouched**, and §8.5.4 puts them here |

Nothing here has a **number**. There is no benchmark of `str_split` against anything, and there
should not be one yet: the tree-walker is 33× CPython on `fib(30)`
([`25`](25-benchmarks-and-expressiveness.md) §25.3), so a measurement of a primitive would measure
the interpreter. §8.4 stands the Are We Fast Yet and CLBG harnesses up alongside this bullet for
exactly that reason — the harness first, the number when there is a backend worth pointing it at.

And the honest limit on the claim in §46.1: three files is a demonstration, not a standard library.
What `lib/` establishes is that the division *has a place to live* and a gate that runs it. The
argument that Beck can express its own library is one file's worth of evidence per file, and there
are three.
