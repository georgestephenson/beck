# The utility table's oracle

[`docs/104`](../../docs/104-styling-and-the-component-library.md) §104.4 takes Tailwind's design
system and refuses its delivery mechanism, and it is specific about how the accepted names stay
honest: **the oracle is Tailwind itself, not a table somebody typed in.**

This directory is that oracle, held the way [`clbg/`](../clbg/README.md) holds the Benchmarks Game's
published output.

| | |
|---|---|
| [`candidates.txt`](candidates.txt) | The names to ask about. Deliberately **wider than the table** — it includes whole families `beck_core::style` does not implement — because a list written from the table would make the gate a restatement of what was typed in rather than a measurement of it |
| [`generate.sh`](generate.sh) | Runs Tailwind over the candidates and writes down which ones it emits a rule for. Run by a person when the pinned version moves; not a test |
| `expected/tailwind-<version>.txt` | What it said. Committed, so no gate needs a network |

`beck-cli/tests/style.rs::the_utility_table_agrees_with_tailwind` reads the committed answer and
holds `beck_core::style::is_utility` to it in three buckets:

- **unsound** — Beck accepts, Tailwind refuses. Asserted at zero: it is a class the compiler would
  put in a stylesheet and the browser would find no rule for, which is a page missing a style with
  every gate green.
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

Two things, both of which a person writing the table down would have got wrong, and both recorded in
§104.4:

1. **Tailwind 4's spacing scale is multiplicative** — `calc(var(--spacing) * n)` — so `p-2.75` and
   `gap-13.5` are rules. A table of steps refuses both.
2. **The first version of `generate.sh` read its oracle wrongly.** It asked `grep -F ".rounded-ful"`
   of Tailwind's output and got a hit, from `.rounded-full` — so the exact misspelling §104.4 chose
   to illustrate the point came back as a utility. It compares selectors as a set now. A generator
   that reads its oracle wrongly is worse than no oracle, because the answer looks authoritative.
