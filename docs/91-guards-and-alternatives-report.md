# 91 — Phase 3 report, part 59: a guard, an alternative, a name, and a field nobody would have walked

**Built.** Everything [`90`](90-nested-patterns-report.md) §90.6 left on the pattern-matching list:
**or-patterns** — `case Circle(r) | Square(r):` — **guards** — `case x if x < 0:` — and **`@`
bindings** — `case whole @ Circle(r):`. The list is empty, [`08`](08-roadmap.md)'s
concurrency-and-errors bullet has no named remainder except structured concurrency's missing
backend, and **no bullet of Phase 3's fourteen has a named remainder at all**.

Neither feature needed a new algorithm. §90.2's exhaustiveness check already answers both: an
or-pattern is *several rows of the same matrix*, and a guarded arm is *no row*. What each of them
cost is written out below, and the interesting cost is neither of those — it is §91.3, where a
struct gained a field that fourteen passes would have walked straight past.

## 91.1 An or-pattern is several rows

`|` is an infix operator in the expression grammar, at the lowest precedence, producing an ordinary
`Node` — which is [`02`](02-syntax.md) §2.6's rule ("patterns *are* expressions … nothing new to
represent") and the same division `*rest` has had since
[`33`](33-effect-polymorphism-and-list-patterns-report.md): the grammar accepts it anywhere and the
**checker** is what refuses it where a pattern is not wanted (`B0357`). The token was free because
[`53`](53-are-we-fast-yet-report.md) §53.6 established that Beck has no bitwise operators.

The rule that makes an or-pattern a *pattern* rather than two arms sharing a body is that **every
alternative binds the same names at the same types**. That is checked here, and then the
alternatives' variables are unified onto the first alternative's, so the body reads one `r` and
neither the evaluator nor any later pass has to know which alternative matched:

```console
error[B0356]: the alternatives of an or-pattern bind different names
  |         case Circle(r) | Point:
  |                          ^^^^^ this alternative does not bind r
```

For exhaustiveness the alternatives are split into one row each — lazily, at the column being
inspected, rather than by distributing every nested alternative up front, because the second is
exponential in the number of them and nothing reads the difference.

**That lazy split is where the one real bug was, and a test written for it is what found it.**
The matrix functions expand column zero on entry, so `view` never meets an unexpanded alternative —
except that `coverage` specialises the *top-level* matrix itself before calling them, and there an
or-pattern fell through `view`'s "not a constructor" case and was read as a **wildcard**. A `match`
covering two of three variants was called exhaustive. The failing test is
`an_or_pattern_that_does_not_cover_a_variant_still_says_so`, and it exists because "the check says
yes" and "the check is right" are different claims: the test that an or-pattern *covers* what its
alternatives cover passes just as well when the alternatives are ignored entirely.

## 91.2 A guarded arm is no row

`case x if x < 0:` reads its condition in the scope of what the pattern bound, and a false one
**falls through to the next arm** — which is the whole of what makes it a guard rather than an `if`
in the body.

The parser change is one line and worth stating, because it is the only place the two features
interact with the grammar rather than with each other: a `case` pattern is read at binding power 1
instead of 0. The postfix conditional `a if c else b` is offered only at 0, so at 1 the `if` after a
pattern is unambiguously the guard's — and `|`, which binds at 1, is still read as part of the
pattern. No lookahead, no pattern grammar, one number.

What a guard costs the exhaustiveness check is a rule, and it is the rule that is easy to get
backwards:

- **a guarded arm covers nothing.** Whether it matches depends on a value rather than on a shape,
  so counting it would call a `match` exhaustive on the strength of a condition that can be false.
  `case _ if ok(x):` is not a `case _`;
- **a guarded arm cannot make a later arm unreachable**, for the same reason from the other side.
  An arm above that only sometimes matches does not swallow the one below it, and reporting it dead
  would be a warning about a correct program — which is worse than no warning at all.

Both directions are gated, and the second is the one that would otherwise have been found by a user.

## 91.3 The field fourteen passes would not have walked

`Arm` gained a `guard: Option<Core>`. Adding a field to a struct is a compile error at every place
that *builds* one — and silence at every place that *reads* one, which is where the passes are:

```rust
arms.iter().map(|a| &a.body)
```

Fourteen sites across nine files walk an arm: liveness, the frame-slot pass, the plan's
free-variable analysis, placement, the effect walk, the splitter's variable-supply high-water mark,
the signal graph, the fusion pass, `beck explain incremental`, and the evaluator. A guard those
walks do not see is not a compile error. It is a `Core` that
[`70`](70-last-use-moves-report.md)'s liveness never marks — so a variable read only in a guard may
be moved out from under it — and a `Core` that [`77`](77-a-let-is-a-slot-report.md)'s frame pass
never counts a slot for, and a `Core` whose variables the plan reports as *free*.

This is [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.5's finding, one struct over.
That report added `Pattern::binders` for exactly this reason — "a new pattern kind falling into that
`_` is not a compile error; it is a silent miscount" — and the answer here is the same shape:
`Arm::exprs()` yields every `Core` an arm holds, in the order they run, and all fourteen sites go
through it. A future field holding an expression has one place to be added rather than fourteen
places to be forgotten.

Two of those sites needed more than a mechanical rewrite, and both are about *order*:

- **liveness walks an arm backwards**, because a last use is the latest read: the body is walked
  first and the guard after it, since the guard runs earlier. Walking them the other way would mark
  a read in the guard as a last use when the body reads the same variable afterwards;
- **the frame pass sums rather than maximises.** Arms are alternatives, so their slot counts are
  maximised against each other; a guard and a body are *sequential within one arm*, so theirs are
  summed.

`a_guard_reads_a_binding_and_the_analyses_see_it` is the program that would fail if either were
wrong: a guard that reads the pattern's binders, calls a primitive, and sits above two more arms.

## 91.4 What the evaluator does, and what it does not

An arm whose pattern matches evaluates its guard **in a scope of its own** — a temporary extension
of the environment with the pattern's bindings — so an arm whose guard is false leaves the frame
exactly as it found it and the next arm matches into a clean one. That is a copy of the bindings on
a failed guard, and it is the honest cost: the alternative is undoing a write to the reserved slots
[`77`](77-a-let-is-a-slot-report.md) built, and an undo that is wrong is a value from the wrong arm.

There is **no decision tree**. A `match` still tries its arms in order, testing each pattern in
full — which is what it did before either feature and what
[`24`](24-incremental-views-report.md)'s plan already treats as opaque. Column-based compilation
(Maranget's other paper, and the one this project has now half-implemented for exhaustiveness) would
test each scrutinee field once instead of once per arm. Nothing has measured that it matters here,
and §91.6 is where it would go.

## 91.5 What is not built

| | Status |
|---|---|
| Or-patterns, at any depth, with binders | **built** |
| `@` bindings, including under a constructor and beside a guard | **built** |
| Guards, in the scope of the pattern's binders | **built** |
| Exhaustiveness and unreachability over both | **built**, and gated in both directions |
| A pattern in a list's **tail** — `[a, *[b, c]]` | **refused**, unchanged from [`90`](90-nested-patterns-report.md) |
| **`@` bindings** — `case whole @ Circle(r):` | **built**, and the forecast held: the binder is irrefutable and the pattern under it decides coverage, so `view` reads an `At` as whatever is inside it and nothing else changed. `@` is an infix operator at a precedence tighter than `\|`, and it is unambiguous because `@` is only special at the *start* of a statement, where it opens a decorator |
| A guard on a `stub` arm | **built by construction and untested** — `stub` shares `case_arms` with `match`, so the grammar accepts it and §21.3's stubs go through the same checker. No program writes one |
| Compiling a `match` to a decision tree | **not built** (§91.4) |
| Exhaustiveness over `Int`, `Str`, `Float` literals | **not built and not buildable** ([`90`](90-nested-patterns-report.md) §90.3) |

## 91.6 What this leaves open

**A guarded arm's condition is not checked for purity.** A guard is an ordinary expression, so its
effect row is inferred and inherited exactly as an `if`'s would be — which means a guard *may*
perform an effect, and a `match` inside a `durable` fold with an effectful guard is refused by the
fold's own row rather than by anything here. That is the right layering and it is untested: no
program has an effectful guard.

**The copy on a failed guard is unmeasured.** §91.4 states the cost and does not put a number on it.
The program that would show it is a `match` whose first arms are guarded and whose guards usually
fail, over a pattern binding several large values, and there is no such program in the tree.

**`stub` arms accept a guard and nothing writes one.** It follows from sharing the grammar, which is
[`22`](22-phase-3-report.md)'s design working, and "it should work" is the sentence this project
treats as a bug report. It is listed rather than claimed.

## 91.7 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`90`](90-nested-patterns-report.md) §90.6 | "**Guards** … not built, and it is the largest thing left in this bullet" and "**Or-patterns** … not built. The check would need no change: an or-pattern is two rows of the same matrix". Both built; the second forecast was right about the algorithm and wrong that it cost nothing — §91.1's bug is what it cost |
| [`08`](08-roadmap.md) | The concurrency-and-errors bullet's remainder is now structured concurrency's missing backend alone |
| [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.5 | Its `Pattern::binders` argument, applied to `Arm` (§91.3) |
| The error index | `B0356`, `B0357` and `B0358` are new |

## 91.8 What Phase 3 is still not

**No bullet of the fourteen has a named remainder.** What is left is whole bullets: **no LLVM
backend and no native codegen**; **no Mode B and no client polish**; **no playground**; **no
supply-chain tooling**; and identity's OIDC relying party, `managed()` provisioning, the claims
mapping and presence ([`48`](48-identity-report.md) §48.5).

Unchanged: the page is still assembled and diffed rather than streamed as deltas
([`24`](24-incremental-views-report.md) §24.6); `parallel:` still has no backend that runs two
children at once ([`80`](80-a-scope-owns-its-children-report.md) §80.5); the render lock is still
there ([`51`](51-arrangement-lifecycle-report.md) §51.7).

The exit criterion is a claim about a person, and no outside developer has read the guide
[`88`](88-read-models-and-pgwire-report.md) published.
