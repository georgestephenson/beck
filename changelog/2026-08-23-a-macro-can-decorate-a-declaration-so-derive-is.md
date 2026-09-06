- **2026-08-23 — A macro can decorate a declaration, so `derive` is written in Beck.**
  [`docs/02`](../docs/02-syntax.md) §2.4's sketch, [`docs/08`](../docs/08-roadmap.md) §8.5.4's first
  successor to the macro interpreter. The sketch takes a `model`, reads what is in it, and emits
  code per field — and the roadmap had it filed as **Lane A**, waiting on `typed macro` and the
  checker's answers. **It was not.** A model's fields are *in its declaration*:
  `(model Point (typarams) (field x Int) …)` is syntax, so `node_args` reads what `.as_model()` was
  going to, and not one line of `check/` or `ty.rs` changed. That is the third time this project's
  lane rule has been got wrong and the first time the *item* was, which §8.5.5 now records.
  **What it needed was four rules made uniform, each a rule rather than a case for `derive`.** A
  block passed to a macro **in item position** holds declarations, because §2.3's block rule passes
  code and a `model` is code — and a block anywhere else still holds statements, which is the half
  the gate asserts, because a `model` inside a function body would turn a mistake into a mystery. A
  `quote:` holds them too, because a `quote` builds syntax and what may be written in one should be
  what may be written in a program. `$` unquotes where a **type** and where a **field name** go,
  not only where an expression does. And a `do` at module level is flattened all the way down,
  because `derive` returns the block it was given beside what it generated, which is a `do` inside
  a `do`.
  **The `$`-in-a-type rule is what hygiene makes necessary rather than convenient**, and it is the
  reason `derive` cannot be written with string concatenation: a type name *written* in a template
  gets a fresh hygiene scope and refers to nothing, so the generated `impl` has to unquote the
  caller's own name node.
  `compiler/examples/derive.beck` is the program. It generates a JSON encoder from a model's
  fields — two models, different fields, one macro — closing the row
  [`docs/46`](../docs/46-standard-library-report.md) §46.16 and `prelude.rs` have both carried since
  the standard library was written ("turning a `model` into a `Json` is a function somebody writes,
  which is what `@derive` is for when it exists"), and closing it with **no reflection in the
  running program**: the macro reads syntax at compile time and what executes is a `to_json` naming
  each field as though somebody had typed it. Gated by `macro_interp.rs`, four tests, one of them
  the negative.
  **What the work found is the constraint neither §2.4 nor [`docs/102`](../docs/102-the-macro-interpreter-report.md)
  had written down: a macro does not cross a module boundary.** `expand_module` takes one parsed
  file and runs before any import is resolved, so a macro is usable where it is declared and
  nowhere else. That is why `derive` is an example rather than a `lib/` function — the trait and its
  base impls could ship there and the `derive` could not, and half a facility is not one — and it is
  why §2.5's `sql"…"` would be an example too. Nothing refuses it; the name is simply not there,
  which is the kind of absence §8.5.6 exists to find. It is now §8.5.4's item in front of the rest
  of the macro interpreter's successors.
  What is still owed of the sketch is its *spelling*: `.as_model()`, and the `*traits` a parameter
  list has no rest form for. `DEFECTS.md::a-bounded-impl-parameter-is-refused-by-the-compiler-that-suggests-it`
  is what the work ran into on the way and did not fix.
