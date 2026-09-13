- **2026-09-12 — The guide shows what the compiler does not recompute, and that transcript is gated
  too.**
  [`86`](../docs/86-getting-started.md) §86.5 is called "What the compiler worked out" and covered
  placement only. It covers maintenance now, which closes the third of the five absences
  §86.12 listed — and the reason it could not before is that the guide's page *counted* the shelf
  without ever listing it, so `beck explain incremental` answered **"nothing in this view is
  maintained by delta: the plan found no collection for a delta to flow through"**. A reading list
  whose page does not show the list was the bug in the example, not in the compiler.
  With the list there, `map_len  ±1 per insert or remove` is what the report prints, and it is the
  answer to the question the exit table in [`08`](../docs/08-roadmap.md) phrases as *will this
  recount a million rows every time somebody clicks* — the count is moved by the change rather than
  recomputed from the map, with no annotation anywhere in the program.
  **The paragraph after it quotes the tool telling the reader where it stops**: the page's children
  are still assembled in full, 14 of the 16 operators have no delta rule, and a command that
  printed the good half would be worth less than one that prints both
  ([`23`](../docs/23-incremental-views-report.md) §23.8 is the measurement and the same caveat).
  `getting_started.rs::the_maintenance_report_in_the_guide_is_what_the_compiler_prints` holds it, as
  the sibling of the placement gate added beside it. This one is quoted in **part** — the full
  report is forty lines — so it asserts that every quoted line appears rather than that the two are
  equal, which is the strongest check an elided quotation admits. Rewriting one line of the table
  turns it red.
