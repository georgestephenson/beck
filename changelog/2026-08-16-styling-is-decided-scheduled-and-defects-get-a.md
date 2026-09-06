- **2026-08-16 · #69 — Styling is decided, scheduled, and defects get a register.**
  [`docs/104`](../docs/104-styling-and-the-component-library.md) measures the position: the stylesheet a
  running application serves is eight rules hard-coded in `beck-rt/src/css.rs` and `css:` has no
  parser, so three documents claiming otherwise are corrected in place. Tailwind styles a Beck page
  with no configuration and no language change (63 ms, 35 utilities, 7,047 bytes) and its *scanner*
  cannot be adopted — 71 Beck files that style nothing emit 15 rules extracted from English prose in
  comments, a typo and a computed class vanish at exit 0, and an application whose components are an
  imported module yields 1 utility of 12, which is fatal because a Beck package has no source tree
  to scan. [`docs/10`](../docs/10-decisions.md) **D29** settles it: take the design system, refuse the
  delivery, a name Beck does not know is a diagnostic, and **on by default with `styles = none` to
  turn all of it off**, the switched-off path gated beside the switched-on one per §8.3. The eight
  items are scheduled in [`docs/08`](../docs/08-roadmap.md) §8.5.4 with a class and a lane each, and
  Phase 3's exit table gains the question a developer actually asks. The component half needs no
  language feature — components compose as functions, may be generic over the application's own
  command, and `ui:` already emits SVG with `viewBox` and `aria-label` intact, which answers
  [`docs/09`](../docs/09-risks-and-open-questions.md) §9.6 item 8 in favour of a library. Nothing is
  built. New file [`DEFECTS.md`](../DEFECTS.md) — what is wrong right now, every entry naming the gate
  its fix owes, deleted by the change that fixes it, union-merged like this file — seeded with the
  three defects this audit found and one older one, that `beck fmt` deletes `#` comments.
