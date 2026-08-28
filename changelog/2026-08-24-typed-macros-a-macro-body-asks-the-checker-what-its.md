- **2026-08-24 — Typed macros: a macro body asks the checker what its arguments are.**
  [`docs/02`](../docs/02-syntax.md) §2.4, [`docs/102`](../docs/102-the-macro-interpreter-report.md)
  §102.9, [`docs/08`](../docs/08-roadmap.md) §8.5.4. `typed macro f(x):` is expanded by the **checker**
  rather than by `beck-macro`, at the call, once the arguments have been inferred — so `node_ty(e)`
  answers with the type the checker gave that expression, and the value answers `.name`, `.kind`,
  `.args`, `.result`, `.fields`, `.variants` and `.inner` through the ordinary record notation.
  Declarations are looked into **on access**, because a model whose field mentions itself would
  otherwise not be a finite value, and a mention's type arguments are substituted in, so `Box[Int]`'s
  field is an `Int` and not a `T`. Recursion goes through the expander — a typed macro emits a call
  to itself on something smaller — because a compile-time helper is an ordinary `def` and a function
  over a *type* has no Beck type; `B0201` bounds it. `refuse("…")` (`B0224`) is how a macro that
  writes code from a type says it has no rule for one, at the call site and in the macro author's
  own words. `lib/json.beck` gains `json_of(e)`, which writes a JSON encoder for a model **nobody
  decorated**, reached through a list, a `newtype` and a field, with no reflection at run time.
  Refusals: `node_ty` in an untyped `macro` body says which word to write (`B0209`), and a typed
  macro over a declaration is `B0223`. The finding is in the **probe** rather than in the types:
  inferring an argument to tell the macro what it is must leave behind neither the diagnostics nor
  the effect row, since a macro may *discard* an argument and then nothing performs its effects —
  gated in `macro_interp.rs` with the control beside it, where the same argument through a macro
  that keeps it does charge `nondet`. **One report must survive that rollback**: an argument that is
  itself a typed macro call expands inside the probe, and the production budget is spent once and
  refused once — so the first version deleted the only refusal there would ever be, every later
  expansion produced nothing, and a doubling macro checked clean as `unit`. `macro_bomb.rs` gains
  the typed twin of its own fixture, one word apart, and it went red on that. **Exhaustiveness-aware
  codegen came with it and needed nothing added**: a macro that has read a union's variants builds
  `(match subject (case pat body)…)` with `node_form`, and a constructor it computed is written
  `case n(at):` inside a `quote:` — a template head naming bound syntax already *is* that syntax.
  This change claimed a `$` rule was missing, on the strength of `case $n(at):` failing, and
  withdrew it in the same change once the gate was written; `$n(…)` now refuses with the spelling
  that works rather than with a report about a name the template owns. Twelve gates in
  `macro_interp.rs`, one in `macro_bomb.rs`, two in `check/mod.rs`; `ty.rs` is
  untouched, which is the fourth time §8.5.5's lane rule has been got wrong and the first time the
  argument for the wrong lane was a correct one about dependencies. F17's ceiling is now **two**
  budgets rather than one, which [`docs/42`](../docs/42-security-assurance.md) §42.6 and
  [`docs/43`](../docs/43-threat-model.md) §43.4 say; §8.5.6's sweep, run over this change, also found
  [`docs/82`](../docs/82-the-edge-report.md) and `pending_security.rs`'s own header still calling that
  fuel unbuilt, and both are corrected in place.
