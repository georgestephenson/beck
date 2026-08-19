# The utility table's oracle

[`docs/104`](../../docs/104-styling-and-the-component-library.md) §104.4 takes Tailwind's design
system and refuses its delivery mechanism, and it is specific about how the accepted names stay
honest: **the oracle is Tailwind itself, not a table somebody typed in.**

This directory is that oracle, held the way [`clbg/`](../clbg/README.md) holds the Benchmarks Game's
published output.

| | |
|---|---|
| [`candidates.txt`](candidates.txt) | The names to ask about: every closed name `beck_core::style::enumerate` produces, a sample of the open ones, and whole families the table does **not** implement — because a list written from the table would make the gate a restatement of what was typed in rather than a measurement of it, and one written *narrower* than the table hides what it accepts. It was narrower for a while, and seventeen unsound acceptances lived there |
| [`generate.sh`](generate.sh) | Runs Tailwind over the candidates and writes down **what** it emits for each: the at-rules, the selector and the declarations, plus the theme tokens and registered properties the sheet needs. Run by a person when the pinned version moves; not a test |
| `expected/tailwind-<version>.txt` | What it said. Committed, so no gate needs a network |

`beck-cli/tests/style.rs::the_utility_table_agrees_with_tailwind` reads the committed answer and
holds `beck_core::style::rule` to it — the whole rule, byte for byte — in three buckets:

- **unsound** — Beck accepts a name Tailwind refuses, or renders one differently. Asserted at zero:
  the first is a class the compiler puts in a stylesheet that the browser finds nothing behind, and
  the second is a page every other page on the web disagrees with.
- **wrongly refused** — Tailwind refuses, Beck accepts. The same error read the other way, also
  zero.
- **a gap** — Tailwind accepts, Beck does not know it. **Not a failure.** The table is a documented
  subset; the count is printed and ratcheted so that adding a family is visible and losing one is a
  failure.

## Why this is a script and not a test

A gate that installs from a package registry fails when somebody else's server does. `clbg/` does not
re-run the Benchmarks Game either — it rebuilds its asserted constants from the Game's published
output files, so a wrong constant fails even against a matching wrong expectation. This is the same
arrangement: the answer is an artefact, and re-obtaining it is a deliberate act with a version in the
filename.

## What asking it caught

All of it recorded in §104.4, and none of it a thing a person writing the table down would have got
right:

1. **Tailwind 4's spacing scale is multiplicative** — `calc(var(--spacing) * n)` — so `p-2.75` and
   `gap-13.5` are rules. A table of steps refuses both.
2. **`1` is not a multiple of one.** `p-1` is `var(--spacing)`, `p-0` is `0px` rather than `0`, and
   `space-x-0` drops the reverse-margin `calc` that every other value in its family has.
3. **`2xl:flex` escapes to `.\32 xl\:flex`** — CSS's hex escape for a leading digit, terminated by a
   *space* that is part of the escape rather than whitespace in the selector.
4. **`auto` is a padding value in no family at all**, and `screen` is not a size for `size-`. The
   table accepted seventeen such names; Tailwind emits nothing for any of them.

And three things about reading an oracle, each of which made an answer look authoritative while
being wrong:

1. **The first `generate.sh` asked `grep -F ".rounded-ful"`** of Tailwind's output and got a hit,
   from `.rounded-full` — so the exact misspelling §104.4 chose to illustrate the point came back as
   a utility. It compares selectors as a set now.
2. **The second read one level of at-rule** and lost `dark:md:flex`, which is nested two deep, and
   stopped at the space inside `\32 `, which lost every `2xl:` rule. It walks the block tree now.
3. **The candidate list was narrower than the table**, so item 4 above was invisible: a name the
   table accepts and the list never mentions is in none of the gate's three buckets.
   `beck_core::style::enumerate` closes that, and `style.rs::every_name_the_table_accepts_was_asked_about`
   is the gate.
