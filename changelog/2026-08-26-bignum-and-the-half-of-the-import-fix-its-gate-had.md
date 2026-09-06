- **2026-08-26 — `bignum"…"`, and the half of the import fix its gate had missed.**
  [`docs/02`](../docs/02-syntax.md) §2.5, [`docs/102`](../docs/102-the-macro-interpreter-report.md)
  §102.10. The third typed literal and the first that is a **capability**: an `Int` literal is
  sixty-four bits, so until now there was no way to write a large integer in Beck at all — every
  value past that reached a program through `big_of_str`, which raises. The macro groups the digit
  run into base-10,000 limbs at compile time and emits `normalise(false, […])`, so
  `bignum"1267650600228229401496703205376"` is the `Big`, in limbs, before the checker sees it.
  The grouping is written twice (the run-time half reads `str_to_int`'s `Option`) and the
  differential in `bignum.beck`'s own tests holds them together, against constants that were
  Python's answers first.
  **Writing it found the previous change to be half a fix.** Yesterday's gate put the macro in the
  *root* module and its `def` in an imported one; the case it did not have is the one a library
  is — a macro declared in `decimal.beck` whose body calls what `decimal.beck` imports, expanded by
  a program two modules away. A macro body resolves in **its own** module's environment, so what
  must be reachable is the closure of what *that* module can see, and it was still `B0208`. The
  environment is that closure now, merged flat, which the flat namespace makes unambiguous
  (`B0601`) and which is loose in one direction — a body can name what its module reaches
  transitively. Gated with three modules and the macro in the middle one. `decimal.beck` is what it
  bought: `is_digit_run` moved to `bignum.beck`, which it already imports, so what counts as a
  digit has one answer for both files and both phases.
