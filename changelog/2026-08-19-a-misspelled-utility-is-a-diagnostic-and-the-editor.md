- **2026-08-19 — A misspelled utility is a diagnostic, and the editor answers from the same table.**
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.4b — the second and third of
  §104.4's four consequences, and what closes
  [`docs/08`](../docs/08-roadmap.md) §8.5.4's styling item 4. **`B0222`**: a class that is not a
  utility and is within one edit of one — two, from eight characters up — is a warning naming the
  utility it is one slip from. `rounded-ful` → `rounded-full`, `bg-emerald-550` → `bg-emerald-500`,
  `items-centre` → `items-center`. And the **editor**: `class="fl‸"` completes to `flex` from the
  closed table plus the scale Tailwind documents, and hovering `gap-2` prints
  `.gap-2 { gap: calc(var(--spacing) * 2); }` — the rule the sheet will actually carry, because it
  comes from the function that emits it.
  **A warning rather than a refusal, which is the design and not a hedge.** `B0217` and `B0218` are
  errors because their vocabularies are closed; a class vocabulary is open, `done`, `column` and
  `mine` are names this tree's own programs write, and Beck still has no way to write a rule of your
  own — so refusing them would be refusing an escape hatch that does not exist.
  **The gate asserts the margin, not the outcome.** Every class this tree writes is **three or more**
  edits from anything in the table (`card` to `grid`, `here` to `h-px`, `mine` to `inline`), so the
  threshold sits one edit clear of the population it must not touch.
  `style.rs::a_misspelled_utility_is_a_diagnostic` asserts that distance rather than asserting that
  nothing was said — the second passes on any threshold below it, the first goes red when a family
  added to the table lands near a name somebody chose.
  **And the editor's negative gate took two attempts, which is the entry's other half.** A class is
  a token inside a string, so the only thing between four thousand utility names and every
  completion in the file is the test for where the caret is. The first gate put a caret in a string
  reading `"hello"` and asserted nothing came back — true for the wrong reason, since no utility
  begins with `hel`. Pointed at `placeholder="gap in the diary"`, whose prefix matches seventy, it
  went red at once: `class=` was to the left on the same line and the caret was inside *a* string,
  which was all the context test checked. It now requires the text between `class=` and the caret's
  own string to be the value itself, which is what tells `class=["flex", "ga‸"]` from
  `class="flex", placeholder="ga‸"`.
  The `Class` **type** is not built and is no longer owed: what it would have checked is what
  `B0222` checks, and [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.12 records it
  as absent rather than as scheduled.
