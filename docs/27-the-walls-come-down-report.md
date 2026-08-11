# 27, 31–33, 36–41, 45, 47 — Phase 3, parts 5–17: the walls come down

**Built.** Eleven pieces of work on the type and effect system, over eleven reports, consolidated
here into one. Between them they took down every wall
[`25`](25-benchmarks-and-expressiveness.md) §25.6 measured between Beck and the rest of SICP, emptied
`sicp/refusals/`, and gave the language the features its own design documents had been describing
since Phase 0: proper tail calls, reals, user-written polymorphism, parameterised types, traits with
bounds that cross a module boundary, generic arithmetic, and errors as a row label.

> **This document replaces eleven reports.** They were one per feature, and each one's opening
> paragraph was the previous one's "what is still not". What is worth keeping is the *order* they
> came in and why it was forced (§27.1), what each feature actually is, the defects they found on
> the way (§27.8), and what is still refused (§27.10). Consolidated under
> [`AGENTS.md`](../AGENTS.md)'s rule that a report is for a phase or a subsystem.
>
> | Was | Now |
> |---|---|
> | 27 three of the six walls · 31 tail calls · 32 the last two walls | §27.2 |
> | 33 effect polymorphism and list patterns | §27.3 |
> | 36 a type that takes a parameter | §27.4 |
> | 37 traits · 39 bounds · 40 traits across modules | §27.5 |
> | 41 generic arithmetic | §27.6 |
> | 45 error rows · 47 effect-polymorphic traits | §27.7 |
> | what each one found and refused | §27.8, §27.9, §27.10 |

## 27.1 The method: a wall is a file, and the order was not chosen

Nothing here was scheduled by taste. Three instruments decided what came next, and all three are
still in the tree:

**`sicp/refusals/`** — one file per thing the language cannot express, each holding the program that
does not compile and the reason. A wall is a *file*, so "what is missing" is a directory listing
rather than an opinion, and the directory is now **empty**. Its README says what puts a file back.

**§25.6's six walls, in §25.7's dependency order.** The order turned out to be right and that is
worth recording: item 3 could not have been done before item 2, item 6 exposed a wall nobody had
seen until it fell, and each report's list of what it could not do became the next report's brief.
Four reports in, the pattern was reliable enough that the roadmap started quoting it.

**A book somebody else wrote.** SICP is relentlessly about the one thing this compiler had never
been pointed at, and ninety minutes of chapter 1 found what months of programs shaped like the todo
sketch had not. The oracle is the book's own printed answers, so a wall cannot be declared down by
the person who removed it.

Every fix below is held by tests that were **turned round rather than deleted** — the file that
asserted the refusal becomes the file that asserts the feature.

## 27.2 The six walls

| §25.7 | wall | what it took |
|---|---|---|
| 1 | **A library cannot be run** — every SICP solution is a library, and so is every domain module a project would most want unit tests for | `beck test` runs a library's own `test` blocks. `beck run` still refuses one, correctly: there is nothing to run |
| 2 | **A type cannot mention itself, or anything declared later** — "the single highest-value item on the list for reasons that have nothing to do with SICP" | declaration order stopped being significant |
| 3 | **`B0320`** — an `if` over two function values refused when one branch is a call's result | the `if` had been typed as though one branch were the other's expectation |
| 4 | **Proper tail calls**, *or* a bounded-depth diagnostic — the only wall written as an either/or | **both**, because the *or* was wrong (below) |
| 5 | **The numeric tower** — "reals first, then rationals and bignums" | reals, with the book's own printed doubles as equalities |
| 6 | **User-written polymorphism** — "the largest of the six" | `def map[T, U]`, rigid where it should be and fresh where it should be, across a `.becki` |

Two of them are worth more than a table row.

**Tail calls, and why the either/or was wrong.** Tail calls alone do not remove the abort — a
tree-walker still spends a host frame on recursion that is *not* in tail position, and `build(n)`
down the spine of a tree is exactly that shape — and a depth diagnostic alone does not make an
iterative process iterative. Both were built. **The number that says it works is an equality**: 1,500
tail calls and 60,000 tail calls spend the *same* host stack — 29,088 bytes unoptimised, 2,488
optimised, identical at both depths in both profiles. Not "small": the same. `sicp/ch1.beck` asserts
a quarter of a million levels of it as an exercise, and `sicp/refusals/tail.beck`, which existed to
assert a `SIGABRT`, is gone. It cost **13% of the evaluator's throughput**, and nothing gets that
back short of a compiling backend.

**Reals, and the strongest oracle in the suite.** SICP prints `(sqrt 9)` as `3.00009155413138`.
`beck test sicp/ch1.beck` prints `3.00009155413138` — not to a tolerance, to the digit, because both
sides are IEEE 754 doubles running the same sequence of operations. Three more of the book's printed
reals go with it, and all four are `expect … == …` on a string.

## 27.3 A row per call site, and a list taken apart

Closing wall 6 made two things visible that no exercise had asked for.

**Effect polymorphism did not survive a user's higher-order definition.** A generic definition's row
was shared by every call site, so a *pure* caller was published as effectful because some *other*
caller had passed it an effectful function. §3.2's own sentence — `map : (list[a], (a -> b ! e)) ->
list[b] ! e` — had been true of the prelude since Phase 2 and was not true of anything a user wrote.
It is now: a row per call site, so `twice(plain, x)` is pure and `twice(stamped, x)` is charged for
exactly what it passed.

**A `list[T]` could not be taken apart**, which blocked §2.2.1's `accumulate` and all of §2.2.3.
List patterns are built; patterns remained one level deep for four more reports
([`90`](90-nested-patterns-report.md) is where that ended).

## 27.4 A type takes a parameter

`def map[T]` was writable and `union Tree[T]` was not — refused by the parser exactly as `def
map[T, U]` had been before wall 6. A `model`, a `union`, a `newtype` and a `type` alias now take the
same type-parameter list a `def` takes, so **one notation quantifies a definition and a declaration
alike**. Arity is declared rather than counted; what crosses a `.becki` is checked, and
`--wire-compat` has an opinion about it. An error code was retired, and the annotating pass that
eight reports had promised finally arrived.

## 27.5 Traits: declared, bounded, and across a module

Three reports, one feature, and each was the previous one's named blocker.

**Declared** (37). The compiler used to warn `B0306: traits are parsed but not yet checked` on every
`trait` in every program. It does not, because there is nothing left to warn about, and the code is
**retired**. A trait writes the signature and an `impl` writes the body; the mechanism is
**desugaring rather than dispatch**, so a trait method is an ordinary definition with a
compiler-chosen name. Coherence is checked in three parts.

**Bounded** (39). `def largest[T: Ord_](xs: list[T]) -> T`. The **dictionary is a parameter** —
appended by the lowering, recovered by an importing module from the same bound, and never published,
because its name is one no source could write.

**Across a module** (40). A `.becki` carries the trait, the `impl` headers and the bounds, so a
library can publish `def largest[T: Ord_]` — which is the interesting half of a library. A bodyless
`impl` is what a header is. The orphan rule had to learn to be about two modules rather than one.

## 27.6 Generic arithmetic, and the directory empties

`sicp/refusals/rational.beck` was the last file in it. It said the missing thing was a *type*, not
an operator: `Rational` is writable as a `model`, but unless the arithmetic goes through the same
resolution `+`, `-`, `*` and `/` use, every expression in §2.5.1 reads `add_rat(x, y)` and the
exercise becomes one about function names rather than about data abstraction.

It reads `x + y`. `Num` is a trait like any other, so an operator inside a generic definition
resolves through the bound, and a trait method reachable only through an operator is still an
ordinary method. **The refusals directory is empty**, and has stayed empty.

## 27.7 Errors are a row label, and a trait's row is a floor

**Wave 1** (45) is the literature survey's own prescription, built: *do not add mechanisms — add row
labels and handlers*. An error is a row label; a signature without it provably cannot fail; `try:` is
a handler that converts the row entry into a value; and `Result` is that **reified** form rather than
a parallel channel. Row aliases were taken at the same time, before the rows got long enough to hurt.

**Wave 2b** (47) is the wall Wave 2 wrote, taken down a day later. A trait's declared effect row was
a **ceiling** every impl was held to, which made `impl Num for Money` unwritable the moment money
wanted to do anything. It is a **floor** now: an impl's row is inferred and published, and a caller
inherits what the impl actually performs. The finding came out of writing the standard library, which
is what a standard library is for.

## 27.8 What they found that nobody asked for

Five defects, none of them in the feature being built, all found because the feature was built:

- **An ordering defect no SICP exercise would have found** — surfaced by the reals, in code that had
  nothing to do with them.
- **The orphan rule was too weak**, and a *prelude* trait found it: the coherence check asked
  whether this module owns the trait, which is the wrong question when the trait belongs to the
  language.
- **A trait's effect row hid a hole** in how impls were checked (§27.7).
- **A diagnostic that pointed at the wrong file**, once there were two modules for it to point at.
- **A trait's methods are registered as global names**, so a trait method and a top-level definition
  can collide — recorded, and still true.

## 27.9 The gates

| Gate | What it holds |
|---|---|
| `sicp/` | Two chapters against **the book's own printed answers**, including four IEEE 754 equalities to the digit. `sicp/refusals/` is empty, and its README says what puts a file back |
| `corpus/` | 32 programs with no placement annotations; every feature here had to survive them |
| `patterns.rs`, `records.rs`, `errors.rs` | What a *program* sees when a feature is wrong, rather than what the checker prints |
| `beck iface` round trips | Every published signature must read back — which is what caught the type-parameter cases that would otherwise have failed silently across a `.becki` |
| `--wire-compat` | A breaking change to a published contract is named before a deploy, including the ones these features introduced |
| `docs/reference/errors.md` | Two diagnostic codes were **retired** here, and a retired code is a code the index must stop describing |

## 27.10 What is still not built

The union of what these eleven refused, minus what has since been built:

- **`Num` is one trait, not a tower with coercion.** SICP §2.5.2's "combining data of different
  types" is not written, and there is no `negate`, so unary minus does not reach a user's type.
- **No `@derive`**, though every precondition it named has been met for several reports now.
- **No `Eq`, `Ord`, `Show`, `Json` or `Hash` in the prelude** — [`54`](54-ordering.md) is the
  options paper on the first two and deliberately stops short of a decision.
- **A trait may not be generic in anything but `Self`**, and a method may not take type parameters
  of its own.
- **A bounded definition cannot be passed as a value**, and neither can a trait method.
- **A bound on a *declaration* is accepted and ignored**: `model Box[T: Show]` parses and means
  nothing.
- **A parameter cannot be applied** — `F[T]` where `F` is itself a parameter is refused by name.
- **No general algebraic effect handlers, no resumption, and no propagation sugar.** `try:` is one
  handler for one label with one behaviour, and there is no `?`, which is the point.
- **A trait's declared row means very little** now that it is a floor: there is no `where`-style row
  bound, so "this bound's methods must be pure" is unsayable.
- **Non-tail recursion is bounded at 4,000 nested evaluations**, and the stack guarantee is provided
  at four places and enforced at none.
