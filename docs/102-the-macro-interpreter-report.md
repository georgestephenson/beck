# 102 — The macro interpreter

**Built.** A macro body is ordinary Beck, evaluated at compile time in a capability-restricted
environment, with `quote:` the one form whose value is syntax. This closes the item
[`08`](08-roadmap.md) §8.5.4 put **first** and called the largest fan-out left anywhere in the
plan, and it lands with the gate [`12`](12-standards-and-conformance.md) §12.7 said must land
*with* it rather than after it.

**And `derive` is built on it.** That is a correction to what this report first said: it listed
`derive` beside typed macros as wanting the checker's answers, and it does not, because a model's
fields are in its declaration. What it wanted was four parsing rules made uniform, and
[`lib/json.beck`](../compiler/lib/json.beck)'s `derive_json` is the program.

**And typed macros are built on it too**, which is the other half of the same correction: §2.4's
second flavour receives the AST with inferred types attached, and what that turned out to need was
not a second interpreter but a **caller** — the checker, expanding a call once it has inferred the
call's arguments. §102.9 is that work, and the finding in it is that the interesting problem was
never the types: it was what a *probe* must not leave behind.

What it does **not** establish: a `Node` that a *running* program can hold (a `quote` that survives
expansion is still `B0332`). §102.8 says why that waits — and it also records the constraint that
used to keep every macro out of a library, which neither this report nor [`02`](02-syntax.md) §2.4
had written down until somebody went looking for it.

## 102.1 What was there, and what the sentence in §2.4 actually asked for

[`02`](02-syntax.md) §2.4 has always said macro bodies "run at compile time in the compiler's own
Beck interpreter, with a *capability-restricted* environment". What existed was a **template
expander**: a body of `let`s and a final `return quote:`, where a `let` did not evaluate anything —
it instantiated its right-hand side *as a template*, substituting `$x` and binding the resulting
syntax. Anything else raised `B0205`, whose text said so.

That is enough for six of Felleisen's seven forms ([`63`](63-expressiveness-report.md)) and for
every macro in this repository, which is why it survived three phases. It is not enough for
anything that **computes**: a macro that iterates over a model's fields, that builds a name out of
a string, that emits one definition per element of a list. The premise those forms are evidence for
— [`01`](01-vision-and-premise.md) §1.1's "programs that write programs" — was backed for notation
and unbacked for computation, and §12.10 is where that was written down.

## 102.2 What a macro body is now

Bindings, `if`, `for`, `while`, lambdas with closures, records, lists, indexing, and calls: to a
local, to one of the module's own `def`s, or to the pure part of the prelude. `quote:` evaluates to
a piece of syntax; `$e` inside one is an **ordinary expression** whose value is reflected back into
the template, and `$*xs` splices a list of them.

```python
macro doubled_each(items):
    out = []
    for a in node_args(items):
        out = list_append(out, node_form("*", [a, 2]))
    return quote:
        [$*out]
```

Reflection over syntax is seven functions in the prelude's own naming style — `node_head`,
`node_args`, `node_is_call`, `node_is_lit`, `node_sym`, `node_form`, `node_str` — plus
`splice([…])`, which returns several forms where one was written and is why `expand_module`
flattens a `do` at the top of a module. That is the shape §2.4's `derive` returns, and it works
today even though `derive` does not.

**One semantic change to bodies that already worked**: a `let` computes rather than substituting.
`n = 2 + 3` used to bind the syntax `2 + 3` and `$n` used to expand to it; it now binds `5` and
`$n` is the literal. No macro in this repository was written the other way — the change is visible
only to a body that was relying on a template engine to look like an interpreter.

## 102.3 The finding: a second interpreter is a differential, or it is a divergence

Untyped macros expand **before** the checker runs, which §2.4 states as a design decision and which
here is a constraint with teeth: there is no `Core` IR at expansion time and no type for a macro
body to be evaluated against — only `Node`. `beck-eval` evaluates `Core`, and `beck-core` (which
lowers to it) *depends on* `beck-macro`, so the dependency could not be turned round even if the IR
existed. The interpreter is therefore a **second implementation of the pure part of the language**,
and this project has a name for what those do: they agree until they do not.

So the centre of the work is not the evaluator. It is `macro_interp.rs`'s differential, which is
[`04`](04-compiler-architecture.md) §4.8's instrument pointed at the two evaluators instead of at
the backends: twenty-four pure expressions, each written **twice** — once in a macro body, where
the interpreter evaluates it and `$v` lands the answer in the program as a literal, and once in a
`def`, where `beck-eval` evaluates it while the program runs — and compared inside the program with
the language's own `==`, so the oracle is not two Rust renderings of two values.

Writing the expression twice is the point, and getting it wrong is instructive: the obvious design
is one macro taking the expression as an argument, and that macro **cannot work**. A macro
parameter is bound to *syntax*, so `$e` puts the argument's syntax back and the program contains
the expression rather than its value. Both halves would then be `beck-eval`, and the differential
would pass against itself forever. The gate has its own control —
`the_differential_notices_when_the_two_halves_disagree` — because a comparison that has never been
different is not evidence that two things are the same
([`82`](82-the-edge-report.md) §82.10, for the fifth time).

The expressions are chosen for where two implementations drift rather than for coverage: string
operations whose unit is characters and not bytes, integer division and remainder around negatives,
and the higher-order list operations, which are the ones with an evaluation order to get wrong.
Where an operation is somebody else's table — case mapping, substring replacement — the interpreter
calls `beck-prim`, the crate the evaluator and a compiled program already call
([`93`](93-the-native-backends-report.md) §93.12), so agreement there is not two implementations
being careful. There is one implementation.

## 102.4 The sandbox stopped being free

§2.4 calls phase separation non-negotiable, and until now the project got it for nothing: expansion
was a pure `Node -> Node` function over a template, so there was no environment, no name a macro
body could use, and nothing to check. `security.rs` said exactly that, and §12.7 marked the row
*Verified, vacuously, and the vacuity is the point to watch*.

The interpreter is what it was watching for. A macro body now has an environment, so the property
is a claim, and `macro_sandbox.rs` is the gate:

- **The environment is a whitelist.** A name resolves to a local, to one of the module's own
  `def`s, or to one of the pure builtins, and to nothing else. There is no `read_file`, no
  `getenv`, no `spawn`, because nothing defines one — five of them are asserted to resolve to
  nothing.
- **The prelude's effectful primitives are refused by name.** `now()` is a name the language
  *has*, so "cannot find `now`" would be a lie about the reason; `B0207` names the atom it
  performs and says where to move the computation. That list is a copy of a table in a crate this
  one cannot depend on, so it is **enumerated**: the gate walks `beck_core::prelude` and fails if a
  primitive carrying an atom is missing from it, or is on the compile-time whitelist. Adding an
  effectful primitive without telling the interpreter is a red test rather than a quiet hole
  ([`93`](93-the-native-backends-report.md) §93.9 is the same lesson: a refusal is a claim, and
  nothing was checking it).
- **The corpus still compiles**, which is the direction a sandbox most easily passes by refusing
  everything.

`Raises` is deliberately not an atom for this purpose. A primitive that can fail — `json_parse`,
`time_parse` — is pure computation with a failure case, and refusing it at compile time would be
refusing arithmetic because it can divide by zero.

## 102.5 Three bounds, and what each one is a bound on

The front end already had three limits on expansion and every one of them bounds a *shape of the
program*: `B0201` re-expansion depth, `B0213` structural nesting, `B0214` what expansion produces
([`14`](14-review-findings.md) F17). None of them bounds what a macro body **does**, and `while
true:` in one is a compiler that does not finish.

- **`B0215`, a step budget** — one million steps for the whole module, per module because that is
  what a compile is and because a per-call budget can be spent once per call site
  ([`82`](82-the-edge-report.md) §82.5 is the same arithmetic one subsystem over). It is a bound on
  how long a compile takes rather than on how big a program is. The number is measured from both
  ends: the most expensive macro body in this repository spends **84 steps**, which its own test
  prints rather than merely asserting, and exhausting the whole budget costs under a second of
  `beck check` in an unoptimised build.
- **`B0216`, the nesting ceiling**, for a compile-time call chain with no base case. It is
  [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md)'s counter rather than a reading
  of the stack, and `beck-macro` now carries the `the_ceiling_fits_the_declared_stack` test that
  `beck-syntax` and `beck-core` have: reaching the ceiling costs **1.9 MB** of the declared 64 MiB,
  measured at the ceiling rather than extrapolated from a per-level cost, because a recursion with
  no base case stops at exactly the limit and what it spent *is* the worst case.
- **Nothing for a module with no macros in it.** Collecting the `def`s a macro body could call
  copies a body each, which is proportional to the whole module — so it happens only when the
  module has a `macro` in it, which almost none do.

## 102.6 What it costs to be honest about a small environment

Two lines are drawn where the interpreter would otherwise have had to invent semantics, and both
are written down rather than discovered later:

- **No unions, so no `Option`.** `str_to_int`, `str_index_of` and `list_get` return one, so they
  are not compile-time builtins; indexing (`xs[i]`) is, and refuses out of range rather than
  answering `None`. `match` is refused for the same reason — its patterns are about variants.
- **No transcendentals.** `sqrt`, `sin` and `cos` are absent from the macro environment. The
  reason they were refused — that what the compiler *produced* would depend on the host's libm —
  no longer holds, since these are computed rather than asked for
  ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)); what is
  left is that nothing has asked for them at compile time. A macro that needs one emits the call
  instead of performing it.

Both are refusals with a reason, which is the shape this project asks for; neither is a limitation
a caller has to discover by hitting it, because the diagnostic names the environment.

## 102.7 The lane was wrong, and the reasoning was plausible

[`08`](08-roadmap.md) §8.5.5 files this item under **Lane A**, the serial one, on the reasoning
that it "changes what reaches the checker, so it occupies this lane's hands". The reasoning is
true and the conclusion was wrong: the change touched `beck-macro/`, one table in `beck-diag/`,
`security.rs`'s comment, and two new test suites — **not one line of `check/mod.rs` or `ty.rs`**.
What reaches the checker is a `Node` either way.

That is the second time an item has been in the wrong lane, and the first —`Set` and dates, filed
under Lane A because a standard-library item sounded like a language item — had a weaker argument
behind it than this one did. The lane rule is about *which files two branches would both rewrite*,
and an argument about consequences is not an answer to it. §8.5.5 now says so with both examples.

## 102.8 What this does not establish

- **Typed macros are built** — §102.9 — and are struck from this list rather than left in it with a
  correction filed three sections later. What they do *not* yet retire is the compiler-provided
  `ui:` block (D22): a user-written `ui:` needs the pattern half of the `$` rules §102.9 ends on.
- **`derive` is built, and it did not need them** — which is the correction this list owes, because
  it said the two were one item. §2.4's sketch reads a `model`'s fields and emits code per field,
  and a model's fields are *in the declaration*: `(model Point (typarams) (field x Int) …)` is
  syntax, so `node_args` answers what `.as_model()` was going to. What it needed instead was four
  rules made uniform — a block passed to a macro **in item position** holds declarations, a
  `quote:` holds them too, `$` unquotes where a **type** and where a **field name** go, and a `do`
  at module level flattens all the way down. [`lib/json.beck`](../compiler/lib/json.beck)'s
  `derive_json` is the program and it generates a JSON encoder, closing the row
  [`46`](46-standard-library-report.md) §46.16 and `prelude.rs` both carried. What is still owed
  is the *spelling*: `.as_model()`, and the `*traits` a parameter list has no rest form for.
- **A macro crosses a module boundary**, and it did not when this list was written. `expand_module`
  took one parsed file and ran before any import was resolved, so a macro was usable where it was
  declared and nowhere else — nothing refused it, the name was simply not there, which is why the
  constraint went unwritten. `expand_module_with` takes the **parsed** modules an importer names,
  which is the right thing for the reason §102.2 already gives about a `def`: a macro body is
  compile-time callable as it was *written*, before expansion, so what crosses is source and not an
  interface. That is also the limit — a macro has no signature for a `.becki` to publish — and
  `B0307`'s note is where somebody meets it. `lib/json.beck` is the first library to ship one, which
  is the difference between a mechanism and a facility.
- **A `Node` at run time.** `B0332` still refuses a `quote` that survives expansion, so code-as-data
  is a compile-time property. §12.10 records which half of D9 that leaves open.
- **Typed literal macros** (§2.5's `sql"…"`, `html"…"`, `regex"…"`). The block rule already
  desugars them to a macro call; what is missing is the sigil, and the parse-at-compile-time it
  implies is now something a macro body could do.
- **`inject`/`unsafe_macro`**, and nested quoting's `(quote depth node)`.
- **Memoised expansion.** §2.4 asks for expansion keyed by content hash and cached in Salsa so that
  macro-heavy code does not destroy IDE latency. There is no Salsa in the front end yet; what is
  new is that there is now something worth memoising, and a step budget that says what it would be
  saving.
- **A macro calling a macro.** A `def` is compile-time callable as it was *written*, before
  expansion, so a `def` whose body calls a macro is not callable from a macro body. That is a
  deliberate refusal rather than an oversight: the alternative is an expansion order that depends
  on who calls what.

## 102.9 Typed macros: the interesting problem was not the types

§2.4's second flavour receives the AST *with inferred types attached*. As a feature list that sounds
like a second interpreter; read against the tree it is a **caller**. The body a typed macro runs is
the one this report already built, unchanged — bindings, `for`, `while`, lambdas, calls to
the module's own `def`s, `quote:` and `$`. What was missing was somebody to run it at a point where
the answer exists, and there is exactly one such point:
[`check/mod.rs`](../compiler/crates/beck-core/src/check/mod.rs)'s `call`, before the head is
resolved.

So the phase order is the design. An ordinary `macro` is expanded by `beck-macro` before anything is
checked; a `typed macro` is left exactly as written by that pass and expanded by the checker, which
infers the arguments, hands the body what they are, and then checks the code the body wrote —
**with the caller's own expectation**, so a typed macro is an expression like any other and
inference flows through it.

### What a body sees, and why it is a record rather than a family of builtins

`node_ty(e)` answers with a value, and the value answers `.name`, `.kind`, `.args`, `.result`,
`.fields`, `.variants` and `.inner` — §2.4 has the table. Three things about that shape are
load-bearing rather than stylistic:

- **`fields`, `variants` and `inner` are read on access.** A model whose field mentions itself is an
  ordinary declaration; a value that carried its own fields eagerly would not be a finite value. A
  *type expression* is always finite, so `.name`/`.kind`/`.args` can be, and only looking into a
  declaration recurses.
- **A declaration's type parameters are substituted by the arguments the mention carried**, so
  `Box[Int]`'s field is an `Int`. This is the half a projection gets silently wrong: forgetting it
  answers `T`, which is a name, and a plausible one.
- **An unsolved unification variable answers `unknown`, not a name.** A body that branched on `?7`
  would be generating code from an accident of inference order.

The value is not forgeable: the compile-time interpreter grew one variant for it rather than
reusing a record, so a macro cannot hand `.fields` something that merely looks like a type.

### Recursion goes through the expander, because a compile-time helper is an ordinary `def`

The obvious way to write `json_of` is a helper that recurses over the type. It cannot be written:
§102.2's rule is that a macro body calls the module's own `def`s, and a `def` is *also* ordinary
code that the checker checks — so a function whose parameter is a **type** has no Beck type to
give it. What works instead is that a typed macro emits a call **to itself**, on something smaller,
and the checker expands that in turn. `lib/json.beck`'s `json_of` reaches a field of a model of a
list that way, and `B0201` — the expander's own depth limit, now counted across the checker's loop
as well — is what stops a type that contains itself.

### A macro that writes code from a type needs a way to refuse

`refuse("…")` is the other half of that, and it is not a convenience. A code generator that meets a
type it has no rule for either emits something that fails to check somewhere else — with a message
about the generated code, which the reader never wrote — or says so itself. `B0224` carries the
macro author's own words, positioned at the **call** with a second label on the line in the body
that decided.

### The finding: a probe is defined by what it does not leave behind

Inferring a call's arguments means checking them, and they are checked again inside whatever the
macro wrote. Everything that check accumulates is therefore a duplicate, and two of those duplicates
are wrong rather than merely noisy:

- **Diagnostics.** A mistake in an argument would be reported twice, and the second report is about
  a pass no reader asked for. The mark-and-truncate is over the whole `Diagnostics`, not over the
  checker's own error helper, because "everything this run reported" is the property — including
  whatever pushed without going through it.
- **The effect row.** A macro may *discard* an argument. Then nothing performs that argument's
  effects, and charging them would put `nondet` on a definition that does not read the clock —
  which is not a cosmetic error in a language where the row decides placement and the generated
  NetworkPolicy. The row is saved and restored around the probe, and the gate has the control
  beside it: the same argument through a macro that *keeps* it does charge `nondet`, so an empty
  row is a fact about the macro rather than about the probe never charging anything.

What is deliberately **not** rolled back is inference itself. A unification the arguments force is
one the expansion would force too, and a substitution rolled back would be a second, quieter answer
to the same question.

**And one report must survive the rollback**, which is the half that was got wrong first and is
worth stating as a rule: *a budget is spent once and reported once, so a discarded report is the
only one there will ever be.* An argument that is itself a typed macro call expands inside the
probe, and expansion draws on the module-wide production budget (F17,
[`14`](14-review-findings.md)). The first version of this work charged that budget correctly and
then deleted its refusal with the rest of the probe — after which every later expansion produced
nothing, the definition checked as `unit`, and `beck check` said the program was **fine**. The
doubling macro one word away from `macro_bomb.rs`'s existing fixture is what found it, on its first
run, which is the argument for writing a gate against the *shape of the gap* rather than against the
fix: a second expander is a second place the same hole opens, and it opens silently.

The cost the probe leaves is not correctness but work. An argument that is a typed macro call is
expanded in the probe and expanded again in the real check, so nesting typed calls `d` deep costs
`2^d` expansions — bounded by the same production budget, charged honestly against it, and the
reason `a_typed_macro_may_be_called_on_another_typed_macros_answer` sits beside the bomb rather than
alone. Memoised expansion (§2.4, still unbuilt) is what would make it linear, and this is now the
second caller that would benefit.

### What is left, and it is one rule

A typed macro can read a union's variants and write a `match`; a `quote:` holds one and a generated
arm checks. What it cannot do is write the *patterns* from the variant names it just read, because
`$` unquotes an expression and a pattern's constructor is a **head** — so `case $n(at)` reads as the
compile-time call `n(at)`. That is the same shape as the two rules `derive` needed (`$` where a type
goes, `$` where a field name goes, both heads) and it is the third. Until it exists,
exhaustiveness-aware codegen — one of the three things §2.4 named typed macros for — is written out
by hand, and a user-written `ui:` (D22) is blocked behind the same rule.
