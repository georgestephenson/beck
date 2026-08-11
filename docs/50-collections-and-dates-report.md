# 50 — Phase 3 report, part 20: collections and dates

> **What this is**: the rest of [`08`](08-roadmap.md) §8.5.4's Wave 2 that is a library rather than
> a language feature — the four collection operations [`46`](46-standard-library-report.md) §46.6
> listed as missing, and the date arithmetic it listed in the same table. Both are **built**, both
> are written in Beck, and between them they added **no primitives at all**: `prelude.rs` is
> untouched. What they did add is two findings, and neither is a wall — §50.5.

## 50.1 The items, and the division they were tested against

[`46`](46-standard-library-report.md) §46.6, on what the standard library's first half left out:

> **Collections** — built for lists and maps. No `Set` operations, no sorting beyond `sort_by`, no
> grouping, no deduplication.
>
> **Time** — built for instants: RFC 3339, UTC, milliseconds. No durations, no arithmetic on dates,
> no zones, no locale.

Those are two files: [`compiler/lib/collections.beck`](../compiler/lib/collections.beck) (236
lines, 26 definitions) and [`compiler/lib/dates.beck`](../compiler/lib/dates.beck) (441 lines, 49
definitions), carrying fourteen and thirteen `test` and `property` blocks. `compiler/lib/` is now
six files and 1,212 lines.

The interesting part was not writing them. It was that `lib/README.md` states a division — a host's
table or grammar is a primitive in `prelude.rs`, composition is a file in `lib/`, and

> The line is not "what is fast" — it is **what has a definition in the language**.

— and the calendar is the first item where that line was genuinely hard to place, because the
host already has one. `time_format` exists, it is the civil calendar, and adding
`time_add_days` next to it would have been half a day's work.

It is a file, and the argument is the division's own. RFC 3339 is a *grammar* — somebody else's
spelling of an instant, with a `T` in the middle and a `Z` on the end — and grammars are the host's.
"Which day is 2024-02-29 plus one" is not a grammar. It is integer arithmetic with a leap rule, and
integers are something the language has. So the calendar is written in Beck, and what the host
keeps is the notation.

## 50.2 A set is a map's keys, and the order is the values'

```beck
type Set[T] = newtype[Map[T, Bool]]
```

The value is `Bool`, always `true`, and unread. What a set stores is the *key*, and `Map` is the
only ordered keyed collection the host provides — so membership is the map's lookup rather than a
scan over a `list[T]`, which is the entire reason to have the type.

Two properties hold across the file and are stated once in it rather than per function:

- **Every function is total.** A collection has no failure of its own. The only way to get an error
  out of this file is to pass in a function that has one, and then the row is that function's and
  travels by inference ([`27`](27-the-walls-come-down-report.md),
  [`27`](27-the-walls-come-down-report.md)). Nothing here `raise`s — which is a different shape from
  `money.beck` and `dates.beck`, and the difference is the subject matter rather than a
  disagreement about style.
- **Every result is ordered, and the order is a function of the values.** `elements` is sorted
  because a `Map` is, not because anything sorts it. A replay that disagreed with the run it was
  replaying about the order of a set would be a replay of a different program, so "in whatever order
  the hash landed" is not available here and is not wanted. `stdlib.rs::a_maps_keys_come_back_in_the_values_own_order`
  pins the host behaviour that promise rests on.

The one design question was sorting. `sort_by` takes a **key**, not a comparator, and a library
that wanted "by score descending, then by name" could have grown a comparator-taking twin. It did
not need one: a compound value compares structurally, so the two-key sort is one key function
returning a `Pair`, and `sorted` is `sort_by(xs, lambda x: x)`.
`stdlib.rs::sorting_is_stable_and_a_compound_value_orders_structurally` pins stability and the
union ordering, because neither is visible in the signature and the file relies on both.

Writing that sentence down is what found the second half of §50.5, because the sentence as first
written was wrong.

`unique` and `elements(set_of(xs))` are both duplicate-free and are **different functions** — the
first keeps first-seen order, the second sorts. Both are wanted, neither is the other, and a
library that shipped only one would have shipped the wrong one for half its callers. The test says
so side by side.

## 50.3 The calendar is arithmetic, and the cross-check is against the host

`days_from_civil` and `civil_from_days` — Howard Hinnant's two functions, in Beck. They are the
same two `time_format` is built from in `beck-eval/src/interp.rs`, written a second time in a
different language. The first two tests in the file walk **5,000 days** and demand that the two
name the same date, in both directions:

- 2,000 consecutive days from 1900-01-01, which crosses the February of a year divisible by four
  that is **not** a leap year, and five ordinary ones;
- 3,000 samples at a stride of 97 days from 1583-01-01 — 291,000 days, to 2379-09-25 — which
  crosses every century boundary in between, leap and not. A wrong leap rule shifts every later
  day, so a stride that steps over the 29th itself still catches it on the next sample.

**Be precise about what that buys.** It is not two independent algorithms, and calling it a
differential without saying so would be the more flattering lie. It is one algorithm on two
evaluators, so what it actually checks is the **language**: that `/` truncates where Hinnant's
algorithm needs it to, that the conditional expression and the operator precedence read the way
they are written, and that nothing was mistyped on the way across. That the algorithm is itself
right is somebody else's proof, plus the leap-year cases `stdlib.rs` already pinned on the host
side. The file's header says this before it says anything else.

It is load-bearing either way. Changing `719468` to `719467` in the Beck copy turns six of the
file's thirteen tests red, both of these among them.

The division semantics turned out to be the real content. Hinnant's algorithms are written for a
division that **truncates towards zero** — the `if` on a negative operand in each is what makes
that exact rather than nearly exact — while splitting an instant into a day has to **floor**, or an
instant before 1970 lands in the day after the one it is in. Both are needed, in one file, and they
are not interchangeable. `floor_div` is four lines of Beck; the truncating one is the operator.
`stdlib.rs::division_truncates_towards_zero_and_the_remainder_follows_the_dividend` now pins the
operator, because a whole library rests on it and nothing had said so.

Two decisions in the file are choices rather than facts, and both are written down where they are
made:

- **`add_months` clamps.** 2024-01-31 plus one month is 2024-02-29, not 2024-03-02. There is no
  answer that is right for every caller — a billing date and a "30 days later" are different
  questions — so this is the one that keeps the *month*, and a caller who wants the other writes
  `add_days`. It also makes the operation lossy: adding a month and subtracting one does not always
  return where it started, and the test asserts that rather than leaving it to be found.
- **The arithmetic is total and the parsing is not.** `days_from_civil(2023, 2, 30)` is the day
  after the 28th, because the arithmetic has no opinion. `parse_date` is where a date is
  *validated*, and keeping them apart is what lets the arithmetic be used to write the validation.

## 50.4 A duration is elapsed, and `Num` is per method again

`type Duration = newtype[Int]` — milliseconds. Elapsed, not calendar: a `Duration` of 24 hours
added to an instant is 24 hours later, which is not the same thing as "the next day" wherever a day
is not 24 hours long. `add_days` is the calendar operation and takes a `Date`; `after` is the
elapsed one and takes an instant. Keeping them apart is the point of having two types.

`impl Num for Duration` is the second one in `lib/` after `money.beck`'s, and it lands in the same
place for the same reason: `add` and `sub` mean something, `mul` and `div` do not — an hour times
an hour is an hour-squared, and an hour divided by an hour is the number two. Both raise
`NotADuration`.

What that made visible, and what the test now says out loud, is that
[`27`](27-the-walls-come-down-report.md)'s inference is **per method and not per impl**. The
first version of the test wrapped all four in `try:` by symmetry with `money.beck`, and the checker
refused two of them with B0392, "nothing here can fail". So:

```beck
expect of_hours(1) + of_minutes(30) == of_minutes(90)          # no handler: `add` is pure
expect (try: of_hours(1) * of_hours(1)) == Err(error=NotADuration(op="*"))
```

`money.beck` could not show this — every one of its methods can fail, because `same_currency` is in
all of them. It took a type where half the operations are total to make the granularity visible,
and the granularity is the useful part: a caller of `+` on a `Duration` inherits nothing.

`render_duration` writes ISO 8601, and stops at hours. `P1D` in ISO 8601 is a *calendar* day and a
`Duration` is elapsed milliseconds, so emitting it would claim something this type does not know.
`of_days(1)` renders `PT24H`, and the test says why next to the assertion.

## 50.5 Two findings, and neither of them is a wall

### The suggestion that was not a program

`collections.beck` wanted a `Set[T]` to implement a trait. The first attempt:

```beck
impl Sized for Set:
```

```
error[B0311]: `Set` takes 1 type argument(s), got 0
  |
6 | impl Sized for Set:
  |                ^^^ write `Set[_]`
```

So write `Set[_]`:

```
error[B0310]: cannot find type `_`
```

**There is no wildcard type in Beck.** The diagnostic had been telling readers to write a program
that does not compile since parameterised types landed in
[`27`](27-the-walls-come-down-report.md), and the label was generated from the arity — `(0..arity)`
mapped to `"_"` — so it had never had a name to offer.

The feature was there the whole time. `impl[T] Sized for Set[T]` works, and worked before this
change: an `impl` head binds its own type parameters, exactly as a `def` does. So this is not a
wall in [`46`](46-standard-library-report.md) §46.5's sense — nothing was inexpressible. It is the
sentence pointing at the feature being wrong, which is the failure mode
[`27`](27-the-walls-come-down-report.md) §27.8 hit from the other direction when a diagnostic
pointed confidently at the wrong file.

The fix is that `check_arity` takes the declaration's **parameter names** rather than its arity, so
the label now says to write `Set[T]`, and a builtin gets the letters the generated reference
already uses — `Map[a, b]`. When arguments are *missing* it also carries a note, because naming the
parameter is only half of it and the reader still has to bind it:

```
= note: each argument is a concrete type, or a parameter bound where this mention is —
  `def f[T]`, `model M[T]`, or an `impl[T]` head
```

The gate is `check/mod.rs::the_spelling_a_missing_type_argument_suggests_is_one_that_compiles`,
which provokes the diagnostic, applies its suggestion, and asserts the result checks clean. That
shape is worth more than the fix: **a diagnostic that offers a spelling should be tested by writing
it down**, and this is the first one in the tree that is. `split.rs` asserts that a fix *exists*;
nothing until now asserted that one *works*.

### A record's order is its field names, not its declaration

The comment above `sorted` originally read "a record compares field by field", which is true and
says nothing about *which* field. It is the field **names**, sorted:

```beck
model Declared:
    zebra: Int
    alpha: Int

expect Declared(zebra=2, alpha=0) < Declared(zebra=1, alpha=9)   # `alpha` decides
```

The consequence lands on the feature this library just declined to build a comparator for. A
caller writing a two-key sort key as `Key(score=…, name=…)` gets it sorted by **name**, because `n`
comes before `s`, and nothing anywhere says so. The value is right, the ordering is total and
deterministic, and the reader's intent is silently inverted.

It is not a defect, and the reason is structural rather than a preference. **A value carries its
fields and not its declaration**, so at the point the comparison happens there is nothing else to
sort them by. The checker *does* know the type and could desugar `<` on a record into a
declaration-order comparison — and that is the option to reject explicitly, because the same
records are also ordered without going through the checker at all: they are `Map` keys, and
`elements` promises an order that is a function of the values. Two orderings for one type is worse
than one surprising ordering, and `sorted(xs)` disagreeing with `elements(set_of(xs))` about the
same records is exactly the bug a library that promises determinism must not have.

So the finding is recorded three ways rather than fixed. `sorted`'s doc comment states the rule and
why the alternative is worse; `collections.beck` has a `model Declared` whose declaration order and
name order disagree, and a test that pins the answer; and
`stdlib.rs::a_record_orders_by_field_name_and_not_by_declaration_order` pins it from the harness
side, so a change to the runtime's value ordering fails there rather than reordering every two-key
sort in every program quietly.

The real answer is further away and worth naming: **`Ord` as a trait**, the way `Num` is one, so a
type says what its own order is instead of inheriting the one its representation happens to have.
[`27`](27-the-walls-come-down-report.md)'s bounds and [`27`](27-the-walls-come-down-report.md)'s per-impl
rows are the machinery; what is missing is the decision, and it is the same decision
[`27`](27-the-walls-come-down-report.md) took for arithmetic. It is not taken here, and this
paragraph is not a proposal — it is where the next person should start.

## 50.6 What is **not** built

| | Status |
|---|---|
| A set's cost | **a map's.** `Set[T]` is `Map[T, Bool]`, so it is an ordered structure with a comparison at every step and a `Bool` per member that nobody reads. There is no hash set and no bitset, and nothing here is measured — [`46`](46-standard-library-report.md) §46.6's reason stands: the tree-walker is 33× CPython ([`25`](25-benchmarks-and-expressiveness.md) §25.3), so a number would measure the interpreter |
| `intersection` and `difference` | **linear in the left side, via a list.** Both go out to `elements` and back through `set_of`. A map-to-map operation would be a primitive, and the division says a primitive has to be a host's table or grammar rather than a faster copy of something expressible |
| Immutable-collection sharing | **not built and not expressible.** `list_append` copies ([`27`](27-the-walls-come-down-report.md) §27.3), so `unique` and `file_under` are quadratic in the same way `sicp/ch2.beck`'s folds are. The fix is a persistent list, and that is a runtime change rather than a library one |
| Time zones and locale | **not built, deliberately.** `prelude.rs`'s reason for `time_format` covers the whole file: a time zone is a database with a release schedule, and a replay must not disagree with the run it is replaying about what a date is |
| Weeks, ISO week dates, quarters | **not built.** `weekday` is there and ISO 8601's week-numbering rules are not |
| Duration as a rational number of seconds | **not built.** A `Duration` is whole milliseconds. Anything finer is the bignum and arbitrary-precision item, still untouched |
| Parsing a duration | **not built.** `render_duration` writes ISO 8601 and nothing reads it back — reading is where a grammar starts, and a grammar is the host's half of the division |
| `time_parse` for a date alone | **not built.** `parse_date` reads `YYYY-MM-DD` in Beck; the host's parser still wants a whole RFC 3339 instant |
| UUID, crypto, bignums, numeric coercion | **untouched**, unchanged from [`46`](46-standard-library-report.md) §46.6. Those are what is left of Wave 2 |
| `Ord` as a trait | **not built**, and §50.5's second finding is the argument for it. Ordering is the runtime's structural one, so a type cannot say what its own order is — which is exactly the position `+` was in before [`27`](27-the-walls-come-down-report.md) |
| The AWFY and CLBG harnesses | **not stood up.** [`08`](08-roadmap.md) §8.4 asks for them alongside this bullet and §8.5.4 calls them the largest thing still owed here. They still are |

## 50.7 What this corrects

- **[`46`](46-standard-library-report.md) §46.6's table** moves two rows. Collections goes from
  "built for lists and maps" to built, with §50.6's first three rows as the honest remainder; Time
  goes from "no durations, no arithmetic on dates" to built, with zones and locale still refused for
  the reason that table already gave.
- **[`08`](08-roadmap.md) §8.5.4's Wave 2** loses two of its five untouched items. What is left is
  crypto, UUID parsing, arbitrary-precision decimal, bignums and numeric coercion — and the
  benchmark harnesses, which are the item that has been owed longest.
- **[`27`](27-the-walls-come-down-report.md) should be read as per-method.** It says an
  impl's row is inferred and published; §50.4 is the first program where that granularity is
  observable, because it is the first impl with both a pure method and a fallible one. Nothing in
  [`27`](27-the-walls-come-down-report.md) is wrong — the reading that a type "becomes
  fallible" when one of its methods can fail is, and no test refuted it until now.
- **[`27`](27-the-walls-come-down-report.md) gains a footnote it should have had.** An `impl` head
  binds its own type parameters and always has; the report does not show one, and the diagnostic
  that would have told a reader so was pointing at a syntax that does not exist. §50.5 is the
  correction, and the test is what keeps it corrected.
- **[`27`](27-the-walls-come-down-report.md) §27.10's "no `Eq`, `Ord`, `Show`, `Json` or `Hash` in
  the prelude" now has a consequence attached.** That bullet records the absence and says of `==`
  that "nothing refuses that today; it is simply not expressible". The same is true of `<`, and
  §50.5 is what a program gets *instead*: a total, deterministic order over its representation,
  which for a record is its field names. The absence was already written down; what it costs a
  caller was not.
