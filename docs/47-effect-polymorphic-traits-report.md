# 47 — Phase 3 report, part 17: effect-polymorphic traits

> **What this is**: [`08`](08-roadmap.md) §8.5.4's Wave 2b, built — the wall Wave 2 wrote, taken
> down. A trait's declared effect row was a ceiling every impl was held to; it is now a floor, an
> impl's row is inferred and published, and a caller inherits what the impl actually performs.

## 47.1 The finding, and where it came from

[`46`](46-standard-library-report.md) §46.5 recorded it as a wall the day it was found, which is the
whole reason this report exists a day later. `lib/money.beck` was meant to be an `impl Num for
Money` so that `+` would work on an amount the way it works on SICP §2.5.1's rationals
([`41`](41-generic-arithmetic-report.md) built exactly that mechanism). It could not be:

```
error[B0370]: `Num::add@Money` performs more than its signature declares
   |     def add(self, other):
   |     ^^^^^^^^^^^^^^^^^^^^^ undeclared: raises(MoneyError)
```

[`37`](37-traits-report.md) §37.5 made a trait's declared row "the bound every impl is held to", by
analogy with B0370's rule for a hand-written `uses` clause — which is a good analogy for a
*signature the author wrote* and a bad one for a signature the author was **handed**. The
consequence, stated in one line: **an operator was unavailable to every type whose operation can
fail.** A `Decimal` that can overflow, a `Matrix` whose dimensions must agree, a saturating
integer, money in two currencies: none of them could have `+`.

It is worth being precise about why four phases did not find this. Traits were exercised by the
compiler's own corpus and by SICP, and every impl in both is *pure*. A wall you cannot see is one
nobody has walked into, and the thing that walked into this one was writing a **library** — which
is [`25`](25-benchmarks-and-expressiveness.md) §25.6's lesson recurring: "every corpus program is
shaped like the todo sketch, which is why none of these had surfaced".

## 47.2 What changed, which is less than it sounds

**A trait's row is a floor, not a ceiling.** An impl's row is inferred, exactly as an ordinary
definition's is when no `uses` clause is written. That is one line of the checker
(`row_is_declared` stops being true for an impl method) and it is the whole of the language change.

**Bounded generics were already right, and nothing had noticed.** `expand_bounds` lowers a bound to
a dictionary parameter, and the parameter's type is a *written function type*, whose row
`ty_from_node` mints as a **variable** — the comment saying so has been there since
[`39`](39-bounds-report.md), citing [`33`](33-effect-polymorphism-and-list-patterns-report.md)
§33.2. So a bounded definition has been effect-polymorphic in its bounds all along, and only the
ceiling stood between that and being useful:

```
def label[T: Show](x: T) -> Str:
    return x.show()
```

is **pure** when given a type whose `show` is pure and performs `nondet` when given one whose
`show` mints a uuid. Same definition, two rows, no annotation. That is
[`33`](33-effect-polymorphism-and-list-patterns-report.md)'s property, arriving for traits by
having been built generally rather than specially — which is the third time in this project that a
feature cost nothing because the mechanism underneath it was general
([`37`](37-traits-report.md) §37.8 names the pattern).

The corresponding fix for an **imported** bound was one line: `import_bounded` rebuilt the
dictionary parameter with the trait's row rather than a fresh variable, so a bound that crossed a
module lost its polymorphism. Nothing had noticed that either, for the same reason.

## 47.3 The module boundary, which is where this could have gone quietly wrong

If a trait's row no longer describes its impls, an importing module cannot read the row off the
trait — and it was reading it off the trait. Left alone, `impl Num for Money` in module A would
have arrived in module B looking **pure**, and B's `a + b` would have performed nothing while the
runtime raised. That is a soundness hole, and it is the kind that a test inside one module cannot
see.

So `ImplSig` carries what each method performs, and `.becki` publishes it:

```
impl Num for Money:
    def add() uses raises(MoneyError)
```

A bodyless `def` with an empty parameter list — a shape the reader already had, and one that does
not repeat the parameter types, which is the second copy [`37`](37-traits-report.md)'s impl rule
exists to refuse. **Pure methods say nothing**, so an impl of a pure trait renders exactly as it
did before this existed, and the three-module corpus project's interfaces are byte-identical.

The measured end-to-end: `money.becki` publishes the row above, and a module that imports it and
writes `a + b` publishes `def sum2(a: Money, b: Money) -> Money uses raises(MoneyError)`. The
failure crossed two module boundaries without anybody writing it down twice.

`--wire-compat` gets this for free, and the sentence it already had is the right one: an impl method
that starts being able to fail is a widened row, and a widened row is breaking.

## 47.4 Two corrections to Wave 1, both making `try:` more precise

Building this found two things wrong with [`45`](45-error-rows-report.md)'s handler. Neither is a
consequence of effect-polymorphic traits; both were latent and this is what made them visible.

**A handler read the row before it was solved.** A row is decided as its definition is checked, so
a call to something declared *later* in the file contributes a row **variable** — and
`try_expr` inspected the raw atoms. It therefore reported "nothing in this block can fail" for a
handler around a forward reference, which is most of them. The row is now resolved through the
substitution first, and the error type is taken from the **expectation** where there is one:

```
def checked(text: Str) -> Result[Int, Refusal]:
    return try: parse(text)          # `Refusal` comes from the signature, not from a solved row
```

That is not a convenience. A `try:` almost always flows into something whose type says
`Result[T, E]`, and reading `E` from there is what makes the handler independent of the order the
checker happens to visit definitions in.

**A handler now catches one failure and lets the others travel.** [`45`](45-error-rows-report.md)
refused a block that could fail two ways (`B0393`) and asked for a union. That was the wrong answer:
precision was available. The handler catches the type its signature names, everything else stays in
the enclosing row, and the caller's signature says so:

```
def checked(text: Str) -> Result[Int, Parse]:
    return try: both(text)           # `checked` performs raises(Budget)
```

`B0393` survives for the case where nothing names anything — no expectation and two raised types —
which is a genuine ambiguity rather than a limitation. This makes `try:` *composable*: a handler is
a filter on one label rather than a barrier across the row.

**And one ordering decision.** Impl methods are now checked before the module's own definitions. A
trait method is what every operator call in a module goes through, so its row has to be decided
before a handler in a caller can discharge it. The ordering is load-bearing rather than tidy, and
the comment in `check_module_with` says so.

## 47.5 What is **not** built

- **A trait's declared row now means very little.** It is a floor nothing enforces: a bounded
  caller gets a row variable that the call site instantiates with the impl's real row, so the
  declared atoms never decide anything. `uses` in a trait method is documentation. Either giving it
  teeth (union it into the dictionary's row) or refusing it outright would be more honest than
  leaving it as prose, and neither was done.
- **No `where`-style row bounds.** There is no way to say "this bound's methods must be pure" —
  which is exactly what a `durable` fold would want of a trait method it calls, since §3.7's
  determinism rule is about the row. Today a fold calling a trait method is checked by the ordinary
  rule on the *resolved* row, which is correct but gives a diagnostic at the fold rather than at
  the bound.
- **Nothing about resumption or general handlers.** `try:` is still one handler for one label with
  one behaviour, as [`45`](45-error-rows-report.md) §45.6 said.
- **A handler still cannot discharge a failure hidden in a row variable.** If a block's row is an
  unresolved tail — a mutually recursive pair, say — the atoms behind it are not visible and the
  handler keeps them. The result is conservative in the safe direction: the enclosing signature
  says it *may* fail when it cannot, never the reverse. Checking bodies in dependency order would
  narrow it further; only the impl-method case is ordered today.
- **No measurement.** As with [`45`](45-error-rows-report.md), a cost number from the tree-walker
  would measure the tree-walker.

## 47.6 What this corrects

- **[`37`](37-traits-report.md) §37.5's "the trait's effect row is the bound every impl is held
  to"** is no longer true, and its argument — B0370's rule for a hand-written `uses` — was the
  wrong analogy: that rule is about a signature its author wrote, and a trait's is one the author
  was handed.
- **[`45`](45-error-rows-report.md) §45.2's account of `try:`** described a handler that refuses a
  block failing two ways. It catches one and passes the rest, and §47.4 says why that is better
  rather than merely different.
- **[`46`](46-standard-library-report.md) §46.5's wall** is down. `lib/money.beck` has its `+`,
  and its `mul` and `div` *raise* — because money times money is not money, and a `raises` row is
  how a method says that instead of inventing an answer. The refusal test in `stdlib.rs` is
  replaced by the property it was waiting for.
