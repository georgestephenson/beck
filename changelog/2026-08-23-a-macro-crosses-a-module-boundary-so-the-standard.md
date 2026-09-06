- **2026-08-23 — A macro crosses a module boundary, so the standard library ships one.**
  [`docs/02`](../docs/02-syntax.md) §2.4, [`docs/08`](../docs/08-roadmap.md) §8.5.4. Expansion ran per
  module, on the parsed file, **before any import was resolved** — so a macro was usable in the file
  that declared it and nowhere else. Nothing refused it; the name was simply not there, which is why
  neither §2.4 nor [`docs/102`](../docs/102-the-macro-interpreter-report.md) had written the constraint
  down until the `derive` work went looking for it. It is what made every one of the macro
  interpreter's successors a mechanism a program could use and a library could not ship.
  **What crosses is the source**, and that follows from a rule §102.2 already stated about a `def`:
  a macro body is compile-time callable as it was *written*, before expansion. So
  `expand_module_with` takes the **parsed** modules an importer names, and `project.rs` — which
  already loaded every source and ordered them topologically — hands over the ones that declare a
  macro. It keeps a parse only for those: the text is searched for the keyword first, so a module
  with no macro pays nothing, which is the same guard `collect_macros` already had one level down
  and which widened by exactly one word — a module with no macros that *imports* one still has to
  hand over its definitions, because the imported macro's body may call them.
  **Two rules were already the right ones and stayed.** The flat namespace decides a collision:
  two macros of one name cannot both be in scope, wherever they came from, and `B0200` — which has
  always refused a module that declared one twice — refuses that too, so the crossing added no
  second rule about names. And a macro is visible where its module is imported *directly*, which is
  what a `def` already does.
  **The limit is an interface.** A macro has no signature, so a `.becki` has nothing to publish and
  an import that resolves to an interface alone does not carry one. `B0307`'s note says so where
  somebody meets it — "if `x` is a macro, this module has to declare it or import one that does" —
  which is now the likeliest cause of that error and was not worth saying while the answer was
  "macros do not cross".
  **`compiler/lib/json.beck` is the first library file to ship a macro.** `import json`, then
  `derive_json:` over a `model`, and its `ToJson` impl is generated from the fields in the
  declaration — closing the row [`docs/46`](../docs/46-standard-library-report.md) §46.16 and
  `prelude.rs` have carried since the standard library was written, and closing it with **no
  reflection in the running program**. The base cases (`Int`, `Float`, `Str`, `Bool`) are written by
  hand on purpose: a macro should generate per-field drudgery and nothing else, and what an `Int`
  means as JSON is a decision. The library derives nothing for *itself* — a `model` declared there
  would be a name every importer could not use, and a test fixture is the last thing worth spending
  the flat namespace on — so what `derive_json` does is asserted from outside, in
  `macro_interp.rs`, beside the crossing it depends on. `examples/derive.beck`, which carried the
  hand-written version for one commit, is deleted: two `ToJson`s in one tree is one too many, and
  the library is the one that is real.
  **And one defect closed on the way, because it was in the way.**
  `DEFECTS.md::the-gesture-measurement-asserts-on-a-clock` — `measure_mode_b.rs`'s
  `what_a_gesture_costs_against_a_command` asserted a wall-clock ratio at 1.0× with 18% of measured
  margin — went red twice in this work's own full-suite runs and green alone, which is
  [`docs/13`](../docs/13-testing.md) §13.7's warning happening. It now asserts on a **counter**:
  `Client::steps` reports what the kernel's backend has executed, and a gesture must charge
  *strictly less* than a command. **6,244 steps against 4,228 at 100 cards and 61,144 against 41,128
  at 1,000** — 1.48× and 1.49×, against the clock's 1.16× and 1.23×, which are still printed because
  what a gesture costs is worth reading. Strictly less rather than "no more" is what makes it able
  to fail: both paths end in the same render, so a gesture that stopped being routed locally would
  charge exactly what a command charges, and `>=` would have accepted that.
