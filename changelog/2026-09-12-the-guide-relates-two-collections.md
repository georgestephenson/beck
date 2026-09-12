- **2026-09-12 — The guide relates two collections, and it needed no new program to do it.**
  The fourth of the five absences [`86`](../docs/86-getting-started.md) §86.12 listed. §86.9's club
  page now shows who nominated each book — `nominator(s, b.isbn)` inside
  `for b in map_values(s.books)` — and `beck explain query` reports a `join` over two arrangements,
  *ordered by the left input's key, left-order-major, as the loop was*. Nothing in the program says
  `join` and there is no query language in the guide, which is
  [`99`](../docs/99-the-data-tier-means-of-combination.md) §99.6's position: a `for` whose body asks
  another collection for a key **is** an equi-join, so you write the loop you would have written
  anyway.
  **The finding is that the club already had one.** `read_count(r, who)` inside
  `for who in map_keys(r.finished)` is the same shape and has compiled to a join since §86.8;
  `beck explain query` had been reporting it to nobody. So the section says that too — the
  recogniser is not looking for a pattern a reader was taught to write, it is looking at what the
  loop reads, and the way to find out is to ask.
  **The transcript gate is now general.** Three `beck explain` transcripts in the guide were two
  gates and a hole, so the second one dispatches on the subcommand instead of naming what the guide
  shows today: a transcript added later is covered by having been written, and an `explain` it
  cannot produce fails loudly rather than passing quietly.
  `every_explain_transcript_in_the_guide_is_what_the_compiler_prints` holds `incremental` and
  `query` by line-containment, because both are quoted in part; `place` keeps its own test and its
  equality, because it is quoted whole. Drifting either of the two was checked to turn it red.
  One mistake worth recording: the first edit changed §86.8's single-file club rather than §86.9's
  split one, because both contain the same `apply_book` and the replacement took the first. The
  guide's own gate caught it on the next run, which is what it is for.
