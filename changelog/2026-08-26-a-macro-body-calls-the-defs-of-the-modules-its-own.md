- **2026-08-26 — A macro body calls the `def`s of the modules its own module imports.**
  [`docs/02`](../docs/02-syntax.md) §2.4, [`docs/102`](../docs/102-the-macro-interpreter-report.md)
  §102.10. It could not, and the way it could not is the defect: a macro crosses an import on the
  imported module's *source*, and which sources the compiler kept was decided by asking whether the
  **imported** file declared a macro — the right question for finding a macro to expand, the wrong
  one for finding a `def` to call, because nothing about `dates.beck` says whether the file
  importing it has a macro. So an imported `def` was reachable only when that other module happened
  to declare a macro of its own, and adding an unused one to it was the difference between `B0208`
  and a compile, while `B0208`'s text — "a `def` in this module" — described a rule that was not
  being applied. The question is the importer's now (`project.rs`), and the interpreter asks "are
  there macros here at all" of every module in play rather than of the imports alone
  (`beck-macro/src/lib.rs`). Cost profile unchanged: the same text pre-filter that kept a second
  parse off macro-free builds still does. Gated three ways in `macro_interp.rs` — the plain pair, the
  same pair with a decoy macro in the imported file (the case that used to pass for the wrong
  reason), and a module that is *not* imported still refused. `docs/08` §8.5.4 had this written down
  as a small unbuilt item one commit earlier; it was a defect, and it is corrected there.
