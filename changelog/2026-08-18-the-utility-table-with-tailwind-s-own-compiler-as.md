- **2026-08-18 — The utility table, with Tailwind's own compiler as its oracle.**
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.4's half of styling cluster item
  4: take Tailwind's design system, refuse its delivery mechanism, and hold the accepted names
  against Tailwind itself rather than against a table somebody typed in.
  `beck_core::style::is_utility` knows **631 of 782** candidate names — layout, spacing, colour,
  type, borders, flex and grid, with the variants in front of them — with **no unsound acceptance**
  and every name Tailwind refuses refused here. The gate has three buckets and only two are
  failures: a name Beck accepts that Tailwind refuses is a page missing a rule with every gate
  green, a name Tailwind refuses that Beck accepts is the same error read the other way, and a
  *gap* is counted and printed so a documented subset cannot quietly become the claim. The candidate
  list is deliberately wider than the table, because a list written from the table would make the
  row a restatement rather than a measurement.
  **The oracle is committed, not run.** `compiler/style/generate.sh` asks Tailwind 4.3.3 about every
  candidate and `compiler/style/expected/` holds the answer — [`clbg/`](../compiler/clbg/README.md)'s
  pattern, and for its reason: a gate that installs from a package registry fails when somebody
  else's server does.
  **Two things asking it caught that writing it down would not.** Tailwind 4's spacing scale is
  multiplicative — `calc(var(--spacing) * n)` — so `p-2.75` and `gap-13.5` are rules and a table of
  steps would have refused both; §104.4 named `p-[13px]` as the arbitrary case and did not say the
  numeric scale is open too. And the **first generator was wrong**: it asked `grep -F ".rounded-ful"`
  of Tailwind's output and got a hit from `.rounded-full`, so the exact misspelling §104.4 chose to
  illustrate the point came back as a utility. It compares selectors as a set now. A generator that
  reads its oracle wrongly is worse than no oracle, because the answer looks authoritative.
  `beck explain style` now says of every class a page carries whether it is a utility or the
  program's own, and a gate holds that this tree's semantic names — `done`, `mine`, `column` — are
  **not** read as utilities, because a program's own names are the program's own. What is still not
  built is the emitter: nothing writes CSS, `styles = none` does not exist, and `beck-rt/src/css.rs`
  still serves its eight hard-coded rules, so the exit-table row this cluster exists to move is
  unmoved.
