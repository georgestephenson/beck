# 63 — Phase 3, part 32: Felleisen's table, six recovered and one conceded

**Built.** [`compiler/sicp/felleisen.beck`](../compiler/sicp/felleisen.beck) — one section per
special form SICP introduces, each carrying the code that recovers it or the reorganisation that
concedes it, and [`25`](25-benchmarks-and-expressiveness.md) §25.9's forecast answered row by row.

This is the *formal* half of the SICP work rather than the running half. Chapters 1 and 2 ask
whether Beck can express what the book computes ([`27`](27-the-walls-come-down-report.md),
[`27`](27-the-walls-come-down-report.md)); this asks the narrower and older question Felleisen posed
in 1991 — whether a language that lacks a form has lost anything by lacking it.

## 63.1 The criterion, and why it is not the obvious one

Felleisen's definition is not "can you get the same effect". It is **local**: a form is recoverable
if it can be expanded where it is written, without reorganising the program around it. A language
that can simulate a form by rewriting every function on the path is not equally expressive — it is
less expressive, and the simulation is the evidence rather than the refutation.

That distinction is what makes the exercise worth doing, because it is the one that produces a
concession. Every form in SICP is *achievable* in Beck; §25.9 asked which are achievable **in
place**, and the answer is six of seven.

## 63.2 The table

§25.9 recorded a forecast so that being wrong about it would be visible. It was wrong about nothing,
and closer to right than a forecast usually is:

| SICP form | Forecast | Verdict | |
|---|---|---|---|
| `cond`, `and`, `or`, `not` (§1.1, §4.1.2) | Recovered | **Recovered** | ✓ |
| `let`, `let*` (§1.3.2, §4.1.6) | Recovered | **Recovered — by the language, not by a macro** | ✓, with §63.4's qualification |
| `delay` / `force`, `cons-stream` (§3.5.1) | "The interesting one" | **Recovered**, after a one-line compiler fix | ✓ |
| `quote`, quasiquote (§2.3.1) | "Blocked, not global" | **Recovered** — the block was the recursive types, and they came down in [`27`](27-the-walls-come-down-report.md) | ✓, and better |
| `set!`, `begin` (§3.1) | "Refused by design — D1" | **Refused by design**, cited rather than measured | ✓ |
| `amb` (§4.3) | "Expected `global`" | **GLOBAL** | ✓ |
| `define-syntax`, derived expressions (§4.1.2) | "Recovered, and stronger" | **Recovered, and stronger** | ✓ |

**Six recovered, one conceded.** §25.9 said one concession out of seven is a result worth
publishing and seven out of seven would be a result worth double-checking; the count came out where
the forecast put it, and the one row that was conceded is the one the forecast named.

Ten `test` blocks run in the file, and every row that says "recovered" says it because code below
that heading executes. `sicp.rs::felleisens_table_is_seven_forms_with_the_code_behind_each_verdict`
runs them and asserts the count of concessions, so a row quietly upgraded is a test that fails
rather than a report that drifts.

## 63.3 `delay` was the interesting row, and it was blocked by an off-by-one

§25.9 was right about which row mattered:

> It is a special form in Scheme *precisely because* it must not evaluate its argument, which is the
> textbook case for a macro. … If this one fails, the expressiveness claim is in real trouble.

It did not fail. It also did not work, until a one-line change to the checker.

`macro delay(e): return quote: lambda: $e` expands correctly. What could not be *written* was the
type of what it expands to. `() -> T` — a thunk — parsed into a `fn-type` node with one argument,
and `Checker::ty_from_node_inner` required `n.args.len() >= 2` before treating a node as a function
type. So `() -> Int` parsed cleanly and then reported `cannot find type 'fn-type'`, which is a
diagnostic about the compiler's own internal head name reaching a user.

The bound is now `!n.args.is_empty()`. That is the whole fix, and the finding is not its size:

**Nothing had ever written a function type taking no arguments.** Not the prelude, not the
thirty-program corpus, not either SICP chapter, not the standard library, not fourteen Are We Fast
Yet benchmarks. A parameterless callback is a shape a language is expected to have, and this one
did not have it — reachable only by a macro that had to expand into a promise. `sicp.rs::a_function_type_may_take_no_arguments`
is where that stays fixed.

This is the same class of finding as [`53`](53-are-we-fast-yet-report.md) §53.5's short-circuiting
defect and [`46`](46-standard-library-report.md) §46.6's diagnostics: found by writing a
program with a purpose rather than by testing the compiler, in a corner nothing had had a reason to
reach.

## 63.4 Two things a macro cannot do, and one of them is sharper

Neither is a wall in §25.6's sense — nothing here is blocked — and both are worth naming.

**A macro cannot introduce a binder from a parameter.** `lambda $name: $body` is `B0120: expected a
parameter name, found '$'`. So SICP's *derivation* of `let` — an immediately-applied lambda whose
parameter comes from the form's first operand — is not writable, and `let` is recovered because Beck
has local bindings rather than because a user could define it. The verdict stands, because Felleisen
asks whether the form is available locally and it is; what is not available is a **user-defined
binding form**, which is a narrower loss and a real one.

Related and *not* a limitation: `$p()` in call-head position works. That was expected to be the same
problem and is not — an unquote may be the head of an application, and only a binder position
refuses one.

**A function-valued field cannot be applied where it stands, and the failure is silent.** This is
the sharper of the two:

```beck
model Box:
    make: () -> Int

def use(b: Box) -> Int:
    return b.make()     # error[B0320]: expected `Int`, found `() -> Int`
```

`b.make()` is read as a method call, resolves to the field, and **drops the application** — the
diagnostic that follows is a type mismatch downstream rather than a refusal at the call. The
workaround is one line (`f = b.make` then `f()`) and the file uses it four times, but a syntax that
silently means something other than what it says is worse than one that is refused. It is recorded
here rather than fixed, because the fix is a decision about method-call resolution that this file
has no standing to take.

`cond`'s arity is a third thing and is not a finding: a macro's parameter list is fixed, so `cond2`
and `cond3` are separate macros where Scheme has one variadic form. Every use site expands, so the
rewrite is still local.

## 63.5 The one concession, written out rather than asserted

`amb` is **global**, and the file makes that readable rather than believable. Backtracking needs
continuations; Beck has none; a macro cannot manufacture one. What backtracking *does* have is a
translation — continuation-passing style — and the file contains it:

```beck
def amb(choices: list[Int], succeed: (Int, () -> Bool) -> Bool, fail: () -> Bool) -> Bool:
    return amb_from(choices, 0, succeed, fail)
```

That works. It is also exactly the concession: every function between the choice point and the
failure has to take a success and a failure continuation, which is a change to **every function on
the path** rather than to the expression where `amb` was written. Beside it, §4.3.2's Pythagorean
triples are written the way a program without `amb` actually writes them — three nested walks and an
explicit accumulator, three functions where Scheme has one `let`.

Both are in the file so the concession can be read. Scheme is more expressive than Beck in
Felleisen's strict 1991 sense, on this row, and §25.9 asked for that to be said plainly rather than
argued away.

## 63.6 `quote` came out better than forecast, and the reason is worth separating

§25.9 forecast "blocked, not global", on the grounds that a program's symbolic data needs a symbol
type and recursive types. Both exist now — [`27`](27-the-walls-come-down-report.md) §27.2 removed the recursive
type wall, and a symbol type is a `union` a program declares — so `union Datum: Atom | Number |
Sequence` carries §2.3.1's `memq` and structural equality with nothing missing.

What is lost is **notation**. Where SICP writes `'(a b c)`, Beck writes
`list_of([atom("a"), atom("b"), atom("c")])`. That is a local rewrite — every quotation becomes a
constructor expression in place — so by the criterion nothing is lost, and this report declines to
call a notation an expressive power. Saying so is the honest form of the verdict: the row is
recovered, and the code is longer.

## 63.7 What is **not** built

| | Status |
|---|---|
| SICP chapter 4's metacircular evaluator | **not attempted.** §25.5's table gives it recursive ADTs, symbols, environments and macros — all four of which now exist, which makes it schedulable rather than blocked. This report is the *Felleisen* half of that row and not the evaluator |
| A user-defined binding form | **not possible**, per §63.4, and the language's own `let` is why nothing is blocked by it |
| A fix for `b.make()` | **not built**, per §63.4. It is a defect and it is recorded rather than corrected |
| A variadic macro | **not built.** `cond2`/`cond3` is what a fixed parameter list costs, and the cost is a name rather than an expression |
| Any Scheme baseline | **none**, unchanged from §25.10. Nothing here is a comparative *measurement* — the one comparative *claim* is §63.5's concession, which is a statement about what can be written rather than about how much of it there is |

## 63.8 What this corrects

- **[`25`](25-benchmarks-and-expressiveness.md) §25.9's Felleisen deliverable is built**, and its
  in-progress note needs updating: the Felleisen table is no longer among the things "not built".
  The compile-speed budgets and the CLBG harness still are.
- **§25.9's `quote` forecast is superseded.** "Blocked, not global" was true when written and the
  block came down in [`27`](27-the-walls-come-down-report.md); the verdict is recovered.
- **[`08`](08-roadmap.md) §8.4's Phase 3 row loses an item.** "SICP stage 1; the Felleisen table;
  compile-speed budgets; Are We Fast Yet and CLBG harnesses" — the first is done, the second is this
  report, Are We Fast Yet is complete ([`53`](53-are-we-fast-yet-report.md)), and two remain.
- **The checker gains a function type it did not have**, per §63.3, and a test that keeps it.

## 63.9 What Phase 3 is still not

The expressiveness suite now runs two chapters of SICP and answers the Felleisen question; it does
not express SICP. `sicp/refusals/` stays empty and `refusals/README.md` still holds that
distinction.

Unchanged from [`62`](62-fuel-report.md) §62.8 otherwise. The exit criterion — an outside developer
building a non-trivial app from documentation alone — is not met and is not closer. Seven bullets of
the fourteen remain untouched, identity has its seam and not its relying party, and
[`23`](23-incremental-views-report.md) §23.19 still names them one at a time.
