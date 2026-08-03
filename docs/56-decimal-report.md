# 56 — Phase 3, part 25: arbitrary-precision decimal, and Wave 2 closed

**Built.** [`compiler/lib/decimal.beck`](../compiler/lib/decimal.beck), written over
[`55`](55-bignums-report.md)'s bignums. It is the last item of [`08`](08-roadmap.md) §8.5.4's
**Wave 2**, which is now finished.

[`46`](46-standard-library-report.md) §46.6 said what `money.beck` is and is not — "money, not
decimal … no arbitrary precision" — and [`55`](55-bignums-report.md) §55.6 named this as the thing
the bignums made possible and did not contain, with its one real design question written down:
**what `/` does when the quotient does not terminate.** §56.3 is that decision.

It is also the first module in `lib/` to **import** another, and that turned out to be where the
findings were.

## 56.1 What a `Decimal` is

`units × 10^-scale`, with `units` a `Big` and `scale` at least zero. Exact, and therefore not a
real: `docs/32`'s `Float` is IEEE 754 and `0.1` is not one of the numbers it holds, which is why
`0.1 + 0.2 != 0.3` there and is `0.3` here. Not a rational either — a third has no decimal
expansion, and this type says so rather than rounding.

**The form is canonical: trailing zeros are stripped, so `1.50` and `1.5` are the same value and
`==` is numeric equality.** That is the opposite of `BigDecimal`, whose `equals` is scale-sensitive,
and the reason is [`54`](54-ordering.md): the value order here is a `Map`'s iteration, the state
digest and the patch stream a replay must reproduce bit for bit. Two representations of one number
are two keys and two digests for one fact, and that is a worse bug than any surprise about trailing
zeros.

What it costs is that **a scale is not a significance**. This type cannot hold "2.50" as distinct
from "2.5", so it does not carry two-decimal-places as data. `render_at` carries it as a
*presentation* decision, which is where it belongs, and `money.beck` remains the type whose scale is
fixed by its currency.

`impl Num for Decimal` is the **fourth** floor of the tower added from outside the compiler, after
`sicp/ch2.beck`'s rationals, `money.beck`'s amounts and [`55`](55-bignums-report.md)'s integers —
and the first built on another one. Addition and multiplication are exact and total: a sum's scale
is the wider of the two, a product's is their sum, so nothing is dropped. `Big`'s `+` being pure is
what lets this one's be pure too, which is [`47`](47-effect-polymorphic-traits-report.md)'s per-impl
row composing one library on top of another.

## 56.2 Rounding, as three rules rather than one

`Rounding` is `HalfEven`, `HalfUp` or `Down`, because picking one would be picking a jurisdiction.
`HalfEven` is IEEE 754-2008's default for decimal and is the rule that does not bias a sum of many
roundings, which is what a ledger wants; `HalfUp` is what most tax rules and most people mean by
"round"; `Down` truncates, and is what a quotient's integer part is.

The comparison is `2 × remainder` against the divisor, on `Big`s, so "exactly half" is `==` on
integers and never a question about a fraction.

## 56.3 The decision `55` §55.6 left: `/` is exact, or it refuses

`a / b` produces digits until the remainder reaches zero. A quotient that never gets there — a
third, a seventh — **raises**. `divide_to(a, b, scale, rule)` is the division that always answers,
and it takes both the scale and the rule.

The argument is that a `/` which silently picked a scale and a rounding rule would be *guessing*,
and this library set already refuses to guess in the two places it costs most: `money.beck`'s
`split` exists because rounding each part independently loses money silently, and
[`46`](46-standard-library-report.md) §46.2's rule is that a reader raises rather than inventing a
value. A decimal division is the same shape — the caller knows how many digits it wants and this
type does not.

There is a **bound**, and it is honest rather than clever. A terminating quotient needs as many
digits as the larger of the twos and the fives in its reduced divisor; deciding *that* needs a `gcd`
and a factorisation this library does not have. So `max_scale()` is 40, and a quotient that has not
terminated by then is refused. Forty is past anything a program asks for — money is two places, an
IEEE double is about seventeen significant digits.

It is also what keeps a refusal **cheap**, and that is worth stating rather than hiding, because it
is the one number here chosen partly for the gate's sake. The expansion costs one long division per
digit, so the bound *is* the work done before `/` gives up. At 100 it was 25 seconds on
`1 / 10^101` — a gate nobody would keep. §56.6 records what the bound gets wrong.

## 56.4 How it is known to be right

Two layers beyond the file's own examples.

**`property` blocks**: a decimal round-trips through its own rendering, a sum of two decimals is the
sum of the numbers, and a rounded quotient is within one unit of the last place — the last checked
as `|q × b − a| ≤ |b| × 10^-scale`, which is what "rounded to `scale` digits" means and needs no
second division to verify.

**A cross-check against exact rational arithmetic in `i128`, over 300 cases and all three rules —
900 assertions.** `stdlib.rs::decimal_rounding_is_what_exact_rational_arithmetic_says` computes the
expected units in Rust as `2 × |remainder|` against `|divisor|` and compares the rendered string.

The part of it worth copying is not the count. **The halfway cases are generated on purpose**: 52 of
the 300 are `(2k+1) / (2 × 10^scale)`, which lands exactly on a half at that scale. A uniform sample
almost never does, and exactly-half is the *only* input where the three rules disagree — so a
randomly-sampled rounding test has almost no power over the thing it is supposed to be testing.

## 56.5 Three findings, and all three are about a module importing another

`lib/decimal.beck` is the first file in the standard library to `import` a sibling. Everything
below was found by that and by nothing else.

### A rounded division was not canonical

`divide_to` built its result with `Decimal(units=…, scale=…)` rather than through the `decimal`
constructor. So `1 / 2` at four places came back as `5000 × 10^-4` — the same *number* as `0.5` and
not the same *value*. `==` said no; it would have been a second `Map` key and a second digest for
one amount, which is precisely what §56.1 chose the canonical form to prevent.

The file's own tests did not catch it, and the reason is worth recording: every rounding test used
scale 0, where canonicalisation has nothing to do. It was found writing the `i128` cross-check, by
asking what `divide_to` should equal rather than what it should render as. A test that compares
rendered strings cannot see a representation defect at all.

`a_rounded_division_is_canonical_like_every_other_way_to_build_one` is the pin.

### `beck doc` could not read a module that imports another

`stdlib.rs::every_library_documents` went red the moment `decimal.beck` existed:

```
error[B0310]: cannot find type `Big`
```

`beck doc module` read a **single file** — `compile_or_library_str` — while `beck check` and `beck
test` go through the project loader. Nothing had noticed because no module anybody documented had
ever imported one. `beck doc` now goes through `checked_project` like the others.

That fix immediately produced the second half of the same finding. The obvious thing to hand the
documenter is the *sliced* program, and the sliced program is **every module merged** — which is
right for slicing and wrong for a page, because `beck doc lib/decimal.beck` then published 93 names,
53 of them `bignum`'s. The page is built from `Project::interface`, the root module's own contract,
which is what `beck iface` had been using correctly all along. `corpus/project/app.beck` was
over-publishing the same way and dropped from 23 names to 10.

### And the merged program dropped every module's doc comments but one

With both of those fixed, `beck doc lib/decimal.beck` reported **0 of 40 documented** for a file
full of `##`.

`project.rs::link` merges definitions, types, signals and tests across modules. It never merged
`docs`. The accumulator is the first module in dependency order — the *deepest import* — so the
merged program carried `bignum`'s doc comments and none of `decimal`'s. One line, `acc.docs.extend`,
and it is 22 of 40.

This one had been wrong since separate compilation landed in Phase 2 and was unreachable: `beck doc`
never resolved imports, so nothing ever asked the merged program for a doc comment. Fixing the first
finding is what made the third one *possible to observe*, which is the argument for fixing a tool
even when nothing is currently broken by it.

**The pattern across all three is one thing.** A module importing another is a shape the compiler
supported and the *tools around it* had never been run against. `beck check`, `beck test` and `beck
iface` were right; `beck doc` was wrong three ways, and one Beck file in `lib/` found all of them.

## 56.6 What is **not** built

| | Status |
|---|---|
| An exact quotient needing more than 40 places | **refused**, and wrongly. `1 / 2^41` terminates and this raises. The fix is a `gcd` and a count of twos and fives in the reduced divisor, which is a `gcd` this library does not have — §56.3 is the trade, stated |
| `gcd`, and a rational over `Big` | **not built**, unchanged from [`55`](55-bignums-report.md) §55.6 |
| An exponent in the reader | **not built.** `decimal_of_str("1e6")` is `NotADecimal`. The lexer takes one for a *float literal* ([`53`](53-are-we-fast-yet-report.md) §53.4) and this reader does not, which is an inconsistency named rather than defended |
| Square root, powers, logarithms | **not built.** None has an exact decimal answer in general, so each needs the scale-and-rule shape `divide_to` has, and none is written |
| A scale that carries significance | **not built**, deliberately — §56.1. `render_at` is the presentation, `money.beck` is the fixed-scale type |
| `Decimal` in `money.beck` | **not done.** `Money` is still `Int` minor units. Rewriting it over `Decimal` is now expressible and is a change to a type other files use, so it is not being done in passing |
| A number for any of it | **none.** `beck test lib/decimal.beck` takes 7.2 s in a debug build, which is a fact about the gate. [`46`](46-standard-library-report.md) §46.6's reason is unchanged: the tree-walker would be what a measurement measured |
| Per-definition module provenance | **not built**, and it is what the second §56.5 finding is really about. A definition does not record which module it came from; the page is right because the *interface* is the root's, not because the program can be filtered |

## 56.7 What this corrects

- **[`08`](08-roadmap.md) §8.5.4's Wave 2 is finished.** "Crypto, UUID parsing, arbitrary-precision
  decimal, bignums and numeric coercion" — the last of the five.
- **[`46`](46-standard-library-report.md) §46.6's money/decimal row moves.** "`lib/money.beck` is
  money, not decimal … no arbitrary precision" stays true of `money.beck`, and the decimal it said
  was missing now exists beside it.
- **[`55`](55-bignums-report.md) §55.6's decimal row moves**, and its "one real design question" is
  answered in §56.3.
- **[`34`](34-generated-documentation-report.md) has a correction it could not have made.** Its
  claim is what `beck doc` generates; §56.5 is three ways `beck doc` was wrong about a module that
  imports another, none of which was reachable when that report was written because no such module
  existed.
- **[`54`](54-ordering.md) gains its second concrete instance.** `Decimal`'s derived order is wrong
  twice over — the first field is a `Big` whose own first field is a sign flag — and the answer
  taken is §54's recommendation again: expose an ordering function, and say why in the code.
- **[`21`](21-tests-in-beck-and-proof.md) §21.3's empty-row rule shaped a test.** The boundary case
  uses `pow10` rather than `big_pow` because `big_pow` raises `BigError`, a `try:` catches one error
  type, and a test block's own row must be empty — so a test reaching for it could not say which
  failure it was asking about. The diagnostic (`B0393`) said exactly that.

## 56.8 What Phase 3 is still not

Unchanged from [`55`](55-bignums-report.md) §55.8 except where this touches it. **The standard
library bullet's library half is done** — Wave 2 has no items left — and what remains on that bullet
is the benchmark harness, which is nine-fourteenths of one suite. The exit criterion — an outside
developer building a non-trivial app from documentation alone — is not met and is not closer.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
