# 29 — Phase 3 report, part 7: the last two walls

[`25`](25-benchmarks-and-expressiveness.md) §25.6 measured six walls between Beck and the rest of
SICP and §25.7 put them in dependency order. [`27`](27-walls-report.md) took the first three,
[`28`](28-tail-calls-report.md) the fourth. This is the last two:

| §25.7 | wall | status |
|---|---|---|
| 5 | **The numeric tower** — "reals first, then rationals and bignums" | **reals built** — §29.1–§29.5. Rationals and bignums are not, and §29.9 says so |
| 6 | **User-written polymorphism** — "the largest of the six" | **built** — §29.7–§29.8 |

**The strongest oracle in the suite came out of the first one, and it is an equality.** SICP prints
the value of `(sqrt 9)` as `3.00009155413138`. `beck test sicp/ch1.beck` now prints
`3.00009155413138` — not to a tolerance, to the digit, because both sides are IEEE 754 doubles
running the same sequence of operations. Three more of the book's printed reals go with it, `(sqrt
(+ 100 37))` and `(square (sqrt 1000))` and exercise 1.35's golden ratio, and all four are `expect
… == …` on a string.

**The second one gave `map` back to the reader.** `sicp/refusals/generic.beck` held SICP §2.2.1's
`map` and asserted that it did not parse. It is now in `ch2.beck`, written once, used at four
element types, and `filter`, `append` and `length` are beside it.

Along the way the reals found a defect nothing in SICP would have caught — `<` answered backwards
for every negative real, because the fold's total order was the order of `f64::to_bits` (§29.2) —
and removing the last wall made a **new** one visible, which now has a refusal file of its own: a
`list[T]` cannot be taken apart, so §2.2.1's `accumulate` still cannot be written, for a reason that
has nothing to do with polymorphism (§29.10).

482 tests, no failures, no compiler warnings, no clippy warnings — up from
[`28`](28-tail-calls-report.md)'s 478. Chapter 1 is 21 tests (was 15), chapter 2 is 10 (was 6).

## 29.1 What was asked, and what is answered

| asked for | status | where |
|---|---|---|
| Reals: `+ - * /`, comparison, `abs` | done, resolved from the operands rather than from a type class | §29.3 |
| §1.1.7 Newton's method, §1.3.3 fixed points, §1.3.4 derivatives | done — nine new tests in `ch1.beck` | §29.5 |
| The book's own printed doubles | done, **as equalities** rather than tolerances | §29.5 |
| A real in a durable fold, on the wire, through `Repr` and replay | done — `corpus/26-sensors.beck` | §29.6 |
| Exact rationals, bignums | **not done**. §2.1.1 still cannot be written, and reals are not a substitute for it | §29.9 |
| `def map[T, U](…)` — a user-written polymorphic definition | done, with the parameter **rigid** inside the body | §29.7 |
| Instantiated afresh at every call | done, and measured across a `.becki` boundary rather than believed | §29.8 |
| Type parameters on a `model` or `union` | **not done** — `union Tree[T]` is not writable | §29.9 |
| Effect polymorphism for a user's higher-order definition | **not done, and now conspicuous** — a pure caller of a generic `map` inherits an effectful caller's row | §29.9 |

## 29.2 The ordering defect, which no SICP exercise would have found

`Value::Float` is a `u64` and always has been, with a comment saying why: a fold's accumulator is a
map key and a component of the state digest, so it needs a total order, and `f64` has none. What it
held was `f64::to_bits`.

`to_bits` is a total order. It is the wrong one. The sign bit is the *top* bit, so `(-1.0).to_bits()`
is larger than `(1.0).to_bits()`, and the derived `Ord` that every comparison primitive and
`sort_by` go through answered:

```
-1.0 < 1.0   →  false
sort_by([1.5, -2.5, 0.0], id)  →  [0.0, 1.5, -2.5]
```

Nothing in SICP chapter 1 would have caught it. Every number in `ch1.beck` is positive, and
`good_enough` compares an absolute value against a tolerance. The test that caught it was written
because the *representation* looked suspicious, not because a program failed — which is worth
recording, because "the suite found it" has been this project's argument for the SICP work twice
now and this one it did not find.

The fix is the standard monotone transform: flip the sign bit for a non-negative float, invert every
bit for a negative one. `-inf` gets the smallest key, `+inf` the largest, and the integer order and
the numeric order become one order. Two IEEE values are canonicalised on the way in, because `Value`
is `Eq` and `Ord` and neither survives them: `-0.0` becomes `0.0`, and every NaN becomes one NaN.
**Beck's `==` on reals is therefore structural, and `NaN == NaN`** — a deviation from IEEE 754 that
somebody porting numeric code has to know about, stated here rather than discovered.

## 29.3 Reals without a numeric type class

Phase 1 already had one ad-hoc operator, and said why:

> Ad-hoc, bidirectional: `+` is Int addition unless one side is already known to be a `Str`, in
> which case it concatenates. Phase 1 has no numeric type class, and a `(a, a) -> a` scheme would
> let `Bool + Bool` typecheck.

The tower needs the same answer with one more tier. `+`, `-`, `*`, `/`, unary `-` and `abs` are
resolved from their operands: whichever of the two operands and the expectation *first* resolves to
`Int` or `Float` decides, and an expression with nothing known about it defaults to `Int` — so every
program written before reals existed still means what it meant. Comparison needed nothing: `<` and
friends have been `(a, a) -> Bool` since Phase 1, and §29.2 is what made them right.

Three things this deliberately does **not** do.

**It does not coerce.** `n + x` with `n: Int` and `x: Float` is an error, not a promotion. C's
implicit widening is where a whole category of numeric bug lives, and the cost of refusing it is one
`float(n)` at the call site, which is visible.

**It is not a type class, and does not pretend to be one.** `ch1.beck` carries `square: Int -> Int`
and `square_real: Float -> Float`, two definitions of the same function, because no signature can
say "either tier". That is the price, it is paid in the most-read file in the suite, and the comment
above them says so. Note that **user-written polymorphism does not fix it either** — `def square[T](x:
T) -> T: return x * x` needs `*` at `T`, which is a *constraint* on `T` and not a quantification over
it. Traits are what fix it, and Phase 1's report has been carrying "traits are parsed but not yet
checked" since the beginning.

**An `Int` operation is checked and a real one is not.** `i64` overflow has no representable answer,
so it is an error; IEEE 754 defines an answer for every real operation including division by zero,
so `1.0 / 0.0` is an infinity rather than a diagnostic. Making it an error would be inventing a rule
the format already has. `%` stays `Int`-only.

The prelude gains three names: `abs` (resolved like the operators), `sqrt` and `float`. `sqrt` is
there even though SICP *builds* it, and `ch1.beck` shadows it with the book's own definition —
which a `def` may do, and which is worth one line of the chapter's header because it looks like a
mistake and is not.

## 29.4 What the reals cost the rest of the compiler

Almost nothing, and the almost is the interesting part. Placement already sized a `Float` at 8
bytes; the value generator already produced and shrank one; the wire encoder already had a case for
one; `Repr` already round-tripped one. Four passes were ready for a type no program could construct.

One was not. `decode_field` — the runtime's decoder for a command arriving from a browser — knew
`Str`, `Int` and `Bool` and answered `Phase 1 cannot decode \`Float\` from the wire` for anything
else. A `Command` variant with a real field would have compiled, placed, sliced, passed its tests
and failed at the first click. It accepts a JSON number now, integral or not, because `1` and `1.0`
are the same JSON token.

That gap is why `corpus/26-sensors.beck` exists rather than the SICP chapters being the whole
evidence: every number in chapter 1 lives and dies inside one pure procedure, and a `Float` in the
*accumulator* is a different question. The corpus harnesses carry it through placement, the slicer,
the plan, the incremental engine against its recompute oracle, replay determinism, `Repr`, the value
generator, the cost model, `Sendable` and `beck iface`.

## 29.5 The book's own doubles, as equalities

Nine tests went into `ch1.beck`: §1.1.7's square roots, §1.3.3's fixed points and average damping,
§1.3.4's derivative, Newton transform and `fixed-point-of-transform`, and exercise 1.35's golden
ratio. Four of them assert a number SICP prints:

| SICP | prints | `beck test sicp/ch1.beck` |
|---|---|---|
| `(sqrt 9)` | 3.00009155413138 | 3.00009155413138 |
| `(sqrt (+ 100 37))` | 11.704699917758145 | 11.704699917758145 |
| `(square (sqrt 1000))` | 1000.000369924366 | 1000.000369924366 |
| ex 1.35, the golden ratio | 1.6180327868852458 | 1.6180327868852458 |

This is a better oracle than anything else in the suite and it is worth being precise about *why*,
because it would be easy to overclaim. It is not evidence that Beck's arithmetic is good. It is
evidence that Beck's `Float` is the same `Float`, that its evaluation order is the same order, and
that nothing in the compiler is quietly rounding, reassociating or contracting — a fused
multiply-add in the wrong place would change the last digit and this would fail. An assertion on the
full printed representation of a double is about as sharp a test of an arithmetic implementation as
a chapter of a textbook can supply for free.

Two of the nine are the other kind: `sqrt_damped`, `cube_root`, `sqrt_newton` and
`sqrt_via_transform` are asserted against 3.0 to a tolerance, because the book does not print their
values and inventing a digit string would be asserting the implementation against itself.

Every iterative procedure in the new sections — `sqrt_iter`, `fixed_point` — is a tail call, so
[`28`](28-tail-calls-report.md) is load-bearing here rather than incidental: `fixed_point` iterates
until it converges, and how many times that is is not a number the author of the chapter knows.

## 29.6 A real in the accumulator

[`corpus/26-sensors.beck`](../compiler/corpus/26-sensors.beck) is the twenty-sixth single-file
corpus program: readings folded into a `Map[Str, Float]` with a running total, a mean, and a
`sort_by` over the reals themselves.

Three claims no SICP exercise could make. A `Float` round-trips through `Repr` and the log, so
`beck replay` reproduces it. A *command* carrying one decodes from the wire — §29.4's gap, with a
test on it. And `sort_by` over a list with negatives in it produces the order a person expects,
which is §29.2 asserted in the one place where getting it wrong would corrupt a rendered page rather
than an expression.

## 29.7 A type parameter that is rigid where it should be and fresh where it should be

`def map[T, U](xs: list[T], f: T -> U) -> list[U]` is four things: syntax, a scope, a
representation, and a scheme.

**The syntax** is a bracketed name list after the definition's name. There are no bounds, because
there are no traits to bound by. `def` gains a `typarams` node that is *always present* and usually
empty, so the form has one shape and every pass downstream indexes into the same positions; the
S-expression reader inserts the empty one for a hand-written `(def f (params …) …)`, because that
surface is a notation people write by hand and a tax on every definition for a feature most do not
use would be the wrong trade.

**The scope** is open exactly while the definition's signature and body are being read, and closed
everywhere else — which is why a monomorphic program cannot see one. Two things are refused rather
than resolved inside it: a parameter that repeats, and one that shadows an existing type. Both are
far more likely to be a mistake than an intention, and neither has any syntax to disambiguate it
afterwards.

**The representation** is the decision that matters. Inside its own body, a type parameter is a
*rigid* nullary type constructor — `Ty::Con("T", [])` — an opaque type that unifies with itself and
nothing else. An ordinary inference variable would have been easier and would have been wrong:

```python
def f[T](x: T) -> Int:
    return x + 1
```

With `T` as an inference variable this typechecks, because `T` unifies with `Int` — and then every
call site typechecks too, and a definition that claims to work for every type works for one. With
`T` rigid it is an error, and the error names what the programmer wrote:

```
error[B0320]: operand of `+` mismatch: expected `Int`, found `T`
```

That message is the second half of the argument for rigid cons over skolem variables. A skolem
prints as a number.

**The scheme** quantifies over the names. `Subst::instantiate` replaces each with a fresh inference
variable per use, which is what makes two calls of the same `map` at two element types two types
rather than one over-constrained one. The prelude's schemes still quantify over numbered variables,
because nobody reads their source; a user's quantifies over names, because the name is what the body
is checked against, what a diagnostic prints, and what `beck iface` publishes.

## 29.8 Across a `.becki`, which is where it would have failed silently

[`27`](27-walls-report.md) §27.7 named the shape of this gap for its own feature:

> No recursive type appears in a `.becki`-crossing position under test … so separate compilation
> over recursive types is compiled-and-believed rather than measured.

Polymorphism would have had exactly the same gap, so it does not: `corpus/project/domain.beck`
publishes `only[T]` and `count_where[T]`, and `app.beck` imports them and uses them at `Todo` and at
`Str`. The published contract carries the parameters —

```
def only[T](xs: list[T], keep: (T) -> Bool) -> list[T]
```

— and an importer instantiates the scheme afresh per call. Had `Interface::exports` kept building
`Scheme::mono`, the first use would have fixed the second's element type and the project would have
failed to link, while every single-module test in the suite went on passing. `beck check
--wire-compat` treats a change to the type-parameter list as a breaking change, because `map[T, U]`
and `map[T]` are different contracts.

## 29.9 What is still not

- **The tower is one tier of three.** Reals are built; **exact rationals and bignums are not**.
  §2.1.1 of SICP is *about* exact rationals — its point is that 1/3 + 1/3 is 2/3 — and reals are not
  a substitute for it, they are the thing it is written against. `factorial` and `fast_expt`
  overflow `i64` exactly as before, with an error rather than a wrap.
- **No numeric type class.** `square` and `square_real` are two definitions of one function in the
  most-read file in the suite (§29.3). Traits are the fix and traits are still parsed-and-not-checked,
  which is now the oldest unpaid debt in the compiler: Phase 1 named it, Phase 2 shipped the effect
  system it was expected to arrive with, and four reports since have not touched it.
- **A *type* still cannot take a parameter.** `def map[T]` is built; `union Tree[T]` and `model
  Box[T]` are not, so `sicp/ch2.beck`'s tree is a tree of `Int` where the book's is a tree of
  anything. Every remaining chapter of SICP that is about data abstraction needs the second half.
- **Effect polymorphism does not survive a user's higher-order definition, and user-written
  polymorphism has made that conspicuous.** A generic `map`'s function parameter has one row
  variable, shared by every call site, so:

  ```
  def apply_each[T, U](xs: list[T], f: T -> U) -> list[U]      # published as f: (T) -> U ! {nondet}
  def pure_use(xs: list[Int]) -> list[Int]                     # published as `uses nondet`
  ```

  `pure_use` performs nothing and says it performs `nondet`, because *another* caller of
  `apply_each` passed an effectful function. This is not new — it has been true of every user-written
  higher-order function since Phase 2, and [`iface.rs`](../compiler/crates/beck-core/src/iface.rs)
  already records the module-boundary half of it — but nobody wrote generic higher-order functions
  before, and now they will. The fix is to generalise a definition's scheme over the row variables
  that appear only in its own signature; the machinery is already there (`Scheme::row_vars`, which
  the prelude uses), and what is missing is resolving the latent row before generalising it. **This
  is the clearest next item this report produces.**
- **`abs` is resolved, `sqrt` and `float` are not.** A reference to `abs` *as a value* — passed to
  `map_list`, say — gets the `Int` form, because there is no operand to resolve from. That is a
  sharp edge with no diagnostic on it.
- **`check.rs` is 3,170 lines**, up from 3,012. §22.6's request to move the test-checking pass out of
  it is still not done, for the sixth report running, and this added two resolutions and a scope
  rather than moving anything.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9 and [`28`](28-tail-calls-report.md) §28.7
  list is unchanged: no LLVM backend, no native codegen, no Mode B, no client polish, no `test
  --update`, no structured concurrency, no `Result`/error rows, no SQLite substrate, no standard
  library v1 beyond these three primitives, no identity beyond a dev-mode actor, no LSP, no
  playground, no supply-chain tooling. SQL read models, pgwire and query fusion are still nothing.

## 29.10 The wall that removing the last wall made visible

§2.2.1 asks the reader to build `map`, then `filter`, then `accumulate`. Two of those three now
work. `accumulate` does not, and the reason is not polymorphism:

**a `list[T]` cannot be taken apart.** The prelude offers `map_list`, `filter_list`, `concat_lists`,
`sort_by`, `list_len` and `list_is_empty`. There is no head, no tail, no indexing, no list pattern,
and `fold` is a *signal* primitive over a `Stream` rather than a fold over a list. So the structural
recursion the book writes — "the empty list, or the first element combined with the accumulation of
the rest" — has nothing to recurse on, and all of §2.2.3 ("Sequences as Conventional Interfaces")
goes with it.

[`sicp/refusals/list-destructuring.beck`](../compiler/sicp/refusals/list-destructuring.beck) is that
wall, written the day the last of §25.6's six fell, with its two diagnostics and the decision it
poses: `head`/`tail` primitives returning `Option[T]` and `list[T]` would let all of it be written,
but the shape that matches the rest of the language is **list patterns in `match`**, because that is
how every other constructor in Beck is taken apart.

That the refusals directory is not empty is the suite working, not the suite running out. Six walls
measured, six removed, one discovered — and the discovered one was invisible from where §25.6 was
standing, because the wall in front of it was taller.

## 29.11 What this changes for the rest of Phase 3

1. **The expressiveness bullet is done and the standard-library bullet has started.** §25.6's list
   was "the language's own means of abstraction, which four phases had never been pointed at". All
   six are answered. What replaces it on the roadmap is not a seventh wall of the same kind — it is
   the standard library, and §29.10 has named its first item.
2. **Traits are now the thing everything is waiting for.** §29.3's two `square`s, §29.9's numeric
   tower, and §2.4–§2.5 of SICP ("Systems with Generic Operations") are one missing feature wearing
   three hats. Parametric polymorphism was the larger *implementation* and it turned out to be the
   smaller *unlock*: it quantifies, and what these need is to constrain.
3. **A defect can hide behind a benchmark as well as behind a test.** §29.2's ordering bug survived
   every SICP number because the book's numbers are positive. "The suite found it" is an argument
   this project has made twice and it is not an argument that a suite finds everything — the
   representation review found this one, and the test was written afterwards.
4. **Two of the walls left tests that assert an equality against an external authority.** SICP's
   printed doubles are not our numbers and cannot be adjusted to fit; neither is `count_change(100)
   == 292`. That is the most valuable property an expressiveness benchmark has, it is rarer than the
   pass rate, and it argues for choosing future benchmarks by whether they *state answers* rather
   than by what they cover.
