- **2026-08-16 · #66 — Macro bodies run Beck at compile time.** The template expander becomes an
  interpreter ([`docs/102`](../docs/102-the-macro-interpreter-report.md)): bindings, `if`, `for`,
  `while`, lambdas, calls to the module's own `def`s and to the pure prelude, `node_*` reflection
  over syntax, and `splice([…])`. A `let` computes where it used to substitute. The gate is a
  **differential** — 24 pure expressions computed by the interpreter and by `beck-eval` and
  compared inside the program (`macro_interp.rs`) — and the sandbox stops being satisfied by
  construction, so `macro_sandbox.rs` enumerates the prelude and fails when an effectful primitive
  is reachable at compile time ([`docs/12`](../docs/12-standards-and-conformance.md) §12.7's G-class
  companion). Three bounds, measured: 84 steps for the largest real macro body against a budget of
  a million (`B0215`), 1.9 MB of the declared 64 MiB at the recursion ceiling (`B0216`), and
  nothing at all for a module with no macros in it. `docs/02` §2.4 and `docs/12` §12.10 corrected
  in place; `docs/08` §8.5.4's first item becomes the list of what it unblocked.
