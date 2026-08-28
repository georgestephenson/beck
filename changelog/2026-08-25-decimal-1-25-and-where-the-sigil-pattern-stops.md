- **2026-08-25 — `decimal"1.25"`, and where the sigil pattern stops sharing.**
  [`docs/02`](../docs/02-syntax.md) §2.5, [`docs/102`](../docs/102-the-macro-interpreter-report.md)
  §102.10. The second typed literal, written in a different library to find out whether the first
  was a coincidence. The pattern held — `decimal"1.25"` **is** `of_units(125, 2)` by the time the
  checker sees it, where `decimal_of_str` raises and `of_units` asks the author to count the
  fractional digits themselves — and the *sharing* did not. A date's validator is integer
  arithmetic that a macro body runs as written; this library decides validity through a `Result`,
  which a macro body has no unions to read, so what is factored out is the **grammar**
  (`is_decimal_text`, asked by both doors) and the *value* is still built twice, out of an `Int` and
  a `Big`. Two bounds the run-time door does not have: eighteen digits, and `max_scale()`. Writing
  a function for both phases also showed the compile-time subset is narrower than "no unions"
  sounds — `list_get` and `str_index_of` both answer one, so the halves of `"1.25"` are recovered
  with `str_starts_with`/`str_replace` rather than by indexing what `str_split` returned. A smaller
  limit is now a roadmap line rather than a surprise: **a macro body resolves its own module's
  `def`s and not an imported module's** (`B0208`), so `dates.beck` and `decimal.beck` carry the same
  three-line digit predicate under two names. Gates in `macro_interp.rs`: the nine texts
  `decimal.beck` refuses at run time are the nine the sigil refuses at compile time, with the
  accepting control beside them, and the two bounds tested from both sides.
