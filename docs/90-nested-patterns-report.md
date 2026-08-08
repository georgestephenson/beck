# 90 — Phase 3 report, part 58: a pattern that nests, and the check that had to be rebuilt for it

**Built.** Nested patterns — `case Some(Circle(r))`, `case [Circle(r), *rest]`,
`case Yes(Circle(0))` — which is the "pattern matching completion" half of
[`08`](08-roadmap.md)'s concurrency-and-errors bullet, and the last **named remainder** on any
Phase 3 bullet. `B0345`, whose message was *"nested patterns are not available in Phase 1"*, has
been in the tree for three phases; it is retired.

The feature itself is small: `Pattern` becomes recursive, the checker recurses with the field's
type, and the evaluator matches a field the way it matches a scrutinee. **The exhaustiveness check
is the work**, and §90.2 is why it could not survive: it was a set of variant *names*, and
`case Some(Circle(r))` names `Some` and covers a fraction of it. A set-based check has to answer
that in one of two wrong ways — accept it as covering `Some`, which lets a program through that
crashes on `Some(Square(…))`, or refuse the whole `match`, which demands a `case _` that can never
run. It is Maranget's usefulness algorithm now, and the same machinery answers a second question
for free: **which arms can never match** (§90.4).

## 90.1 What was refused, and what it cost

[`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.5, which built list patterns:

> **Nested patterns are still refused**, as they are for a constructor: patterns in Beck are one
> level deep, which is what §3.1's exhaustiveness check needs and no more.

That sentence is exactly right about *why*, and it is the thing this report inverts: what the old
exhaustiveness check needed is what kept patterns shallow, so the pattern language could not grow
until the check did. Four reports since have carried "patterns are still one level deep" in their
closing lists ([`36`](36-parameterised-types-report.md), [`37`](37-traits-report.md),
[`39`](39-bounds-report.md), [`41`](41-generic-arithmetic-report.md)).

What it cost a program is a name for a value that already had one:

```python
match found:
    case Yes(shape):
        match shape:
            case Circle(r):
                return 3 * r * r
            case Square(side):
                return side * side
    case No:
        return 0
```

against

```python
match found:
    case Yes(Circle(r)):
        return 3 * r * r
    case Yes(Square(side)):
        return side * side
    case No:
        return 0
```

The second is the one the book, the tutorial and every other language write. Nothing about the
first is *wrong* — and that is the point of writing both out here rather than asserting that one is
better: this is a notation, and by [`63`](63-felleisen-report.md)'s criterion the rewrite is local,
so nothing was inexpressible. What is recovered is the reading.

## 90.2 Why exhaustiveness had to be rebuilt rather than extended

The old check was three lines of bookkeeping and a lookup. Each arm's pattern `insert`ed the name
of the variant it matched into a `covered` set; the check asked which of the union's declared
variants were missing from that set; a list was two magic strings, `"[]"` and `"[…, *rest]"`,
because a list's shapes are not declared anywhere. It was the right check for a pattern language one
level deep, and it has one assumption in it: **naming a constructor covers it.**

A nested pattern breaks that assumption in both directions at once.

- `case Yes(Circle(r))` **names `Yes` and does not cover it**. Keep the old rule and the program
  compiles, then fails at run time on `Yes(Square(…))` — which is the one thing §3.1 says this
  check exists to prevent, in the sentence about a missed migration being "a compile error rather
  than a 3 a.m. page".
- `case Yes(Circle(r))` *and* `case Yes(Square(side))` **together do cover `Yes`**, and no
  arm names it. A check that refuses this is worse than useless: the only way to satisfy it is a
  `case _` that can never run, which is dead code the compiler asked for.

So the answer stops being a lookup and becomes Maranget's question (*Warnings for pattern matching*,
JFP 2007): given the arms already written, is there a value none of them matches?
[`exhaust.rs`](../compiler/crates/beck-core/src/check/exhaust.rs) is the implementation, and it is
the textbook one — specialise the pattern matrix by each constructor of the first column, recurse,
and report the first constructor no row wrote.

Two things about it are Beck's rather than the paper's.

**A list is `nil` and `cons` here and nowhere else.** `Pattern::List` is flat — some fixed elements
and an optional tail — which is the right shape for an evaluator that can check a length and index,
and the wrong shape for this, because "exactly two" and "at least one" do not partition anything.
So a list pattern is *viewed* as `nil`/`cons` for the length of the check: `[a, b]` is
`cons(a, cons(b, nil))`, `[a, *r]` is `cons(a, _)`. Two constructors that partition a list is what
lets one algorithm answer for a list and for a union, and it replaced the two magic strings.

**The top level is asked one constructor at a time.** The algorithm naturally yields a single
counterexample, and a `match` on a union that misses three variants should say all three — that is
what it has said since Phase 1 and what a developer acts on. So the top level runs the check once
per constructor and collects, and below the top level there is one witness and it is a whole value.
The messages that come out:

```console
error[B0341]: match is not exhaustive
   |     ^^^^^^^^ missing: Yes(Square)
```

```console
error[B0341]: match is not exhaustive
   |     ^^^^^^^^^ missing: the empty list — `case []`, [_]
```

That second one is a **better** message than the one it replaces, and by accident rather than by
design. The old check answered `case [a, b]` with "the empty list and a list with elements", which
is true and unhelpful — the author covered a list with elements. The new one names a length that
escapes, and `[_]` is an input to write a test with.

## 90.3 What it cannot prove, said before somebody finds it

An `Int`, a `Str` and a `Float` have constructors nobody can enumerate, so a column of literal
patterns is never complete: `case 0` and `case 1` over an `Int` need a wildcard, and always will.
This is not a limitation of the implementation but of the question — a check that claimed otherwise
would need a range analysis, and nothing in this language has asked for one.

## 90.4 The second question the same machinery answers

An arm is *reachable* exactly when it is useful against the arms above it — when some value it
matches escapes all of them. That is the same function with a pattern where the wildcard was, so
unreachable-arm detection cost one more entry point and no new algorithm:

```console
warning[B0355]: this case can never match
   |         case Yes(Square(side)):
   |         ^^^^^^^^^^^^^^^^^^^^^^^ the arms above it already cover every value this matches
```

It is a **warning** and not an error, and the reason is a habit rather than a principle: `case _:`
written after every variant of a union is something people write on purpose, and turning it into a
build failure is a change to what compiles rather than a diagnostic. Nothing in the tree triggers
it — every `.beck` file in `corpus/`, `lib/`, `sicp/`, `awfy/`, `clbg/` and `examples/` was checked
for it, which is the sort of claim that is worth making by running rather than by believing.

Nested patterns are what make an unreachable arm easy to write by accident, which is why the two
arrived together: `case Yes(Circle(r))`, then `case Yes(_)`, then `case Yes(Square(side))` is a
program whose third arm is dead, and nothing above the pattern language can see that.

## 90.5 A pattern is an expression, so nesting one is a new way to recurse

[`44`](44-wave-0-report.md) bounded the front end's recursion as a count, and
[`85`](85-what-the-generator-found-report.md) then found **three** productions that reached past the
bound — a type grammar that was not counted at all, a counter released before the recursion it was
counting, and a Pratt parser that builds depth with a loop. Its lesson is the Scriban advisory
[`42`](42-security-assurance.md) §42.2 quotes: *a limit added at the one production somebody thought
of is bypassed through a different one.*

A nested pattern is a new recursion in the checker, so it is counted. What actually answers
`W(W(W(…)))` at 384 deep today is the **reader's** counter and not the checker's — a pattern is an
ordinary call form, so the parser meets it first, and both ceilings are the same number. The
checker's guard is therefore not the thing that fires, and it is kept anyway: this project's own
history is three cases of a limit at one production being reached through another, and a counter at
the consumer is what that history argues for. §90.7 says what could not be constructed to make it
fire, so the next person does not have to rediscover it.

The exhaustiveness check recurses too, and its depth is bounded by something other than a counter:
it specialises only into constructors the *patterns* wrote, so a recursive union with a wildcard arm
terminates at the depth of the deepest pattern. `union Wrap: W(inner: Wrap)` with `case _` does not
loop, and the test that says so is a program rather than an argument.

## 90.6 What is not built

| | Status |
|---|---|
| Nested patterns under a constructor, under a list, and under each other | **built** |
| A literal as a nested pattern — `case Yes(Circle(0))` | **built** |
| Nesting through a type parameter — `Option[Shape]` binds a `Shape` | **built** |
| Exhaustiveness over nested patterns, unions and lists | **built** — one algorithm, replacing the set |
| Unreachable arms | **built**, as a warning (§90.4) |
| A pattern in the **tail** of a list — `[a, *[b, c]]` | **refused**, and `B0345` now says so rather than saying "Phase 1". It is `[a, b, c]` written twice over: a second spelling, not a shape |
| **Guards** — `case Circle(r) if r > 0:` | **not built**, and it is the largest thing left in this bullet. A guard makes an arm's coverage undecidable, so every arm carrying one contributes nothing to exhaustiveness — which is a rule to write down before the feature, not after |
| **Or-patterns** — `case Circle(r) \| Square(r):` | **not built**. The check would need no change: an or-pattern is two rows of the same matrix |
| **Binding a whole value while matching it** — `case c @ Circle(r):` | **not built** |
| Exhaustiveness over `Int`, `Str`, `Float` literals | **not built and not buildable** without a range analysis (§90.3) |

## 90.7 What this leaves open

**The checker's pattern counter cannot be made to fire, and it is still right to have.** A pattern
reaches the checker only through the parser, both count to the same ceiling, and a pattern cannot be
built by the loop that [`85`](85-what-the-generator-found-report.md) §85 found — `case 1 + 1` is not
a pattern at all, it is a call to `+` and `B0343` says so. So the guard is unreachable today. Every
one of [`85`](85-what-the-generator-found-report.md)'s three findings was a place somebody had
concluded exactly that.

**An unreachable arm is a warning, and warnings are not gated.** Nothing in CI fails on one. The
`pending_security.rs` pattern — asserting an absence so that building the control turns a test red —
has no equivalent for "a warning nobody reads", and the honest position is that this diagnostic is
worth what its reader makes of it.

**`check/mod.rs` got smaller.** 3,669 lines before the tests, 3,626 after — 43 fewer, while gaining
a recursive pattern checker and losing forty lines of exhaustiveness bookkeeping to
[`exhaust.rs`](../compiler/crates/beck-core/src/check/exhaust.rs)'s 541. That file has grown in
almost every report since Phase 1, and [`22`](22-phase-3-report.md) §22.6 spent eight reports asking
for something to be lifted out of it rather than added to it. This is the second time it has
happened, after [`36`](36-parameterised-types-report.md) lifted the test-checking pass.

## 90.8 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.5 | "Nested patterns are still refused … patterns in Beck are one level deep, which is what §3.1's exhaustiveness check needs and no more." Built, and the second clause was the reason: the check is what had to change |
| [`08`](08-roadmap.md) | The concurrency-and-errors bullet's "the rest of pattern matching is untouched", and the exit-criterion paragraph's "not the rest of pattern matching". Guards, or-patterns and `@` bindings are what remain, and §90.6 lists them |
| [`36`](36-parameterised-types-report.md) §36.8, [`37`](37-traits-report.md) §37.8, [`39`](39-bounds-report.md) §39.8, [`41`](41-generic-arithmetic-report.md) §41.8 | Four closing lists carry "patterns are still one level deep". They are history and keep their sentences; this is where the change is recorded |
| `B0345` | Its message was "nested patterns are not available in Phase 1" and it now refuses one thing rather than a feature: a pattern in a list's tail |
| The error index | `B0355` is new — the first **warning** in the `B03xx` range |

## 90.9 What Phase 3 is still not

The concurrency-and-errors bullet has `Result`, error rows, `parallel:` and pattern matching that
nests; what it does not have is guards and or-patterns (§90.6). **No bullet of the fourteen now has
a named remainder except that one.**

Unchanged: **no LLVM backend and no native codegen**; **no Mode B and no client polish**; **no
playground**; **no supply-chain tooling**; the OIDC relying party, `managed()` provisioning, the
claims mapping and presence ([`48`](48-identity-report.md) §48.5); the page is still assembled and
diffed rather than streamed as deltas ([`24`](24-incremental-views-report.md) §24.6); `parallel:`
still has no backend that runs two children at once
([`80`](80-a-scope-owns-its-children-report.md) §80.5).

The exit criterion is a claim about a person, and no outside developer has read the guide
[`88`](88-read-models-and-pgwire-report.md) published.
