- **2026-08-22 — The corpus-wide counts are gated, so a fortieth program cannot silently falsify
  six documents.**
  `DEFECTS.md::corpus-wide-counts-drift`, closed. Several documents quote a number derived from the
  whole corpus — what the native backends compile and refuse, what the WebAssembly emitter is
  measured against, where the corpus places, how many of its names rename — and every one of them
  moves when a program is added. It had drifted four times, each re-derivation finding the last one
  stale somewhere different, and twice in one day when programs 38 and 39 landed together.
  **The hard part was never the assertion; it was that a count has to be findable in prose.** A
  test cannot grep for `985` and know which document meant which quantity. So a marked number
  carries an HTML comment naming it — `997<!--c:native-compiled-->` — invisible where the markdown
  renders and greppable where it is edited, and **the number is read out of the sentence rather
  than out of the marker**: a marker carrying its own value would agree with itself while the prose
  beside it said something else, which is the failure being gated.
  `docs.rs::every_corpus_wide_count_quoted_in_prose_is_the_one_the_tree_has` derives all **18**
  quantities — no `clang`, no engine, no release build, 5 seconds — and asserts two things: every
  marked number is the one the tree has, and every quantity it derives is quoted *somewhere*, so a
  figure cannot leave the documents and stop being checked. Run with `--nocapture` it prints the
  table, which is now the cheapest way to re-derive them.
  It fails three ways, each checked by doing it: a fortieth corpus program turns **26 marked
  numbers red at once**; changing one document and forgetting the other five that quote the same
  quantity names the file and line; and deleting a marker fails as "derived here and quoted
  nowhere". The first is the one that matters, and the message says to re-read them all rather than
  the one that failed, because they move together.
  Also records, in [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 8 and
  [`docs/08`](../docs/08-roadmap.md) §8.5.4, that **fusion for the new operators has no program**:
  swept across all 45 programs in `corpus/` and `examples/`, nothing in the tree has a `filter_list`
  above a join, because the recogniser consumes the filter into the operator. The e-graph that item
  is named after has no rewrite to arbitrate yet.
  Two prose figures became digits to be checkable — `docs/93`'s "All thirty-nine" and "twenty-six
  of thirty-nine" — and `docs/20`'s tier table now cites the gate rather than the release-only
  suite as the command that derives it.
