# 102 — The macro interpreter

**Built.** A macro body is ordinary Beck, evaluated at compile time in a capability-restricted
environment, with `quote:` the one form whose value is syntax. This closes the item
[`08`](08-roadmap.md) §8.5.4 put **first** and called the largest fan-out left anywhere in the
plan, and it lands with the gate [`12`](12-standards-and-conformance.md) §12.7 said must land
*with* it rather than after it.

What it does **not** establish: a `Node` that a *running* program can hold (a `quote` that survives
expansion is still `B0332`), typed macros, or `derive`. Those want the checker's answers or a run
time, and §102.8 says why each waits.

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

- **Typed macros and `derive`.** §2.4's second flavour receives the AST *with inferred types
  attached*, which is the checker's output, and this interpreter runs before the checker. That is
  the genuinely Lane A piece, and it is what retires the compiler-provided `ui:` block (D22).
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
