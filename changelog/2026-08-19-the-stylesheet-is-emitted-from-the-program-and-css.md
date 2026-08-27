- **2026-08-19 — The stylesheet is emitted from the program, and `css.rs` is deleted.**
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.4a and
  [`docs/08`](../docs/08-roadmap.md) §8.5.4's styling item 4, second half. `beck build` writes
  `styles.css` and a running program serves the same bytes at `/beck.css`, derived at startup from
  the program it is executing: **one rule per class its pages can carry**, the theme tokens those
  rules read and nothing else, in front of a nine-rule preflight that is Beck's own rather than
  Tailwind's. `AppConfig::styles` is the `styles = none` switch and **both settings are run** by a
  gate, per §8.3 item 8. The eight rules hard-coded in `beck-rt/src/css.rs` are gone with the file.
  **A predicate cannot emit a sheet, and that is what the second half changed about the first.**
  `beck_core::style::rule` turns a name into declarations and `is_utility` is now *defined* as
  "there is a rule for it", so a table and an emitter cannot disagree about a page; the oracle
  records what Tailwind **emits** rather than whether it emits; and the gate compares the at-rules,
  the selector and the declarations byte for byte. **3,474 of the 3,625 names Tailwind 4.3.3 emits a
  rule for are rendered identically here**, 35 it refuses are refused, 151 remain as the families
  this table has not taken, and all 293 theme tokens are Tailwind's own values.
  **Asking the bigger question found four details and seventeen defects.** The details are that `1`
  is `var(--spacing)` and not `calc(var(--spacing) * 1)`, `0` is `0px`, `space-x-0` drops the
  reverse-margin `calc`, and `2xl:flex` escapes to `.\32 xl\:flex` — a hex escape terminated by a
  *space* that the previous reader of the oracle stopped at, so every `2xl:` rule had been silently
  missing from its answer. The defects are `size-screen`, `max-w-auto` and fifteen `-auto` paddings:
  names the table accepted, Tailwind emits nothing for, and **`candidates.txt` had never been asked
  about**, so they were in none of the gate's three buckets through every green run
  ([`docs/82`](../docs/82-the-edge-report.md) §82.10 exactly). The table enumerates itself now and
  `style.rs::every_name_the_table_accepts_was_asked_about` fails when it accepts a name the oracle
  never saw.
  **The sketch is the proof and its sheet is 2.3 KB.** `examples/todo.beck` is restyled onto
  utilities — seventeen classes, seventeen rules, six of the theme's 293 tokens — and its `done`
  class, the one name in it that was the program's own and the reason `css.rs` existed, is
  `line-through`. Restyling it found a defect worth its own entry in
  [`DEFECTS.md`](../DEFECTS.md): `class=["a", b]` with one non-literal element lowers to a `str_join`,
  which has no delta rule, so **the shape §104.4 recommends turns a page into a recompute**. No
  program in this tree had ever written one. An all-literal list is folded at lowering time and
  costs nothing; the mixed list is `class-list-recomputes`, and the sketch writes two whole
  alternatives behind an `if` until it is fixed.
