# 90 — Pattern matching

**Built, and the list is empty.** Nested patterns — `case Some(Circle(r))`, `case [Circle(r),
*rest]` — or-patterns, guards and `@` bindings. `B0345`, whose message was *"nested patterns are not
available in Phase 1"*, had been in the tree for three phases; it now refuses one thing rather than a
feature.

**The features are small and the exhaustiveness check is the work.** It was a set of variant *names*,
and `case Yes(Circle(r))` names `Yes` and covers a fraction of it — so a set-based check has to
answer that in one of two wrong ways. It is Maranget's usefulness algorithm now, and the same
machinery answers two more questions for free: which arms can never match, and what an or-pattern
covers.

Two findings outlast the features. **§90.5 is a field fourteen passes would have walked straight
past** — adding one to a struct is a compile error at every site that *builds* one and silence at
every site that *reads* one, which is where the passes are. And **§90.3's better error message
arrived by accident**, which is the ordinary way a rewritten check pays for itself.

---

## 90.1 What was refused, and what it cost

[`27`](27-the-walls-come-down-report.md) §27.3, which built list patterns:

> **Nested patterns are still refused**, as they are for a constructor: patterns in Beck are one
> level deep, which is what §3.1's exhaustiveness check needs and no more.

**That sentence is exactly right about *why*, and it is the thing this inverts**: what the old
exhaustiveness check needed is what kept patterns shallow, so the pattern language could not grow
until the check did. Four reports carried "patterns are still one level deep" in their closing lists.

What it cost a program is a name for a value that already had one — an inner `match` on a binding
whose only purpose was to be matched again, against:

```python
match found:
    case Yes(Circle(r)):
        return 3 * r * r
    case Yes(Square(side)):
        return side * side
    case No:
        return 0
```

The second is the one the book, the tutorial and every other language write. **Nothing about the
first is *wrong*** — and that is the point of saying so rather than asserting that one is better:
this is a notation, and by [`63`](63-felleisen-report.md)'s criterion the rewrite is local, so
nothing was inexpressible. **What is recovered is the reading.**

## 90.2 Why exhaustiveness had to be rebuilt rather than extended

The old check was three lines of bookkeeping and a lookup: each arm `insert`ed the name of the
variant it matched into a `covered` set, and a list was two magic strings because a list's shapes are
not declared anywhere. It was the right check for a pattern language one level deep, **and it has one
assumption in it: naming a constructor covers it.**

A nested pattern breaks that assumption **in both directions at once**:

- `case Yes(Circle(r))` **names `Yes` and does not cover it**. Keep the old rule and the program
  compiles, then fails at run time on `Yes(Square(…))` — which is the one thing §3.1 says this check
  exists to prevent, in the sentence about a missed migration being "a compile error rather than a
  3 a.m. page".
- `case Yes(Circle(r))` *and* `case Yes(Square(side))` **together do cover `Yes`**, and no arm names
  it. A check that refuses this is worse than useless: the only way to satisfy it is a `case _` that
  can never run, **which is dead code the compiler asked for.**

So the answer stops being a lookup and becomes Maranget's question (*Warnings for pattern matching*,
JFP 2007): given the arms already written, is there a value none of them matches? The implementation
is the textbook one — specialise the pattern matrix by each constructor of the first column, recurse,
and report the first constructor no row wrote.

Two things about it are Beck's rather than the paper's.

**A list is `nil` and `cons` here and nowhere else.** `Pattern::List` is flat — some fixed elements
and an optional tail — which is the right shape for an evaluator that can check a length and index,
and **the wrong shape for this, because "exactly two" and "at least one" do not partition anything.**
So a list pattern is *viewed* as `nil`/`cons` for the length of the check. Two constructors that
partition a list is what lets one algorithm answer for a list and for a union, and it replaced the two
magic strings.

**The top level is asked one constructor at a time.** The algorithm naturally yields a single
counterexample, and a `match` on a union that misses three variants should say all three — that is
what it has said since Phase 1 and what a developer acts on. So the top level runs the check once per
constructor and collects; below the top level there is one witness and it is a whole value.

## 90.3 The better message, which arrived by accident

```console
error[B0341]: match is not exhaustive
   |     ^^^^^^^^^ missing: the empty list — `case []`, [_]
```

**That is a better message than the one it replaces, and by accident rather than by design.** The old
check answered `case [a, b]` with "the empty list and a list with elements", which is true and
unhelpful — the author covered a list with elements. The new one names a length that escapes, and
`[_]` is an input to write a test with.

**What it cannot prove, said before somebody finds it.** An `Int`, a `Str` and a `Float` have
constructors nobody can enumerate, so a column of literal patterns is never complete: `case 0` and
`case 1` over an `Int` need a wildcard, and always will. **This is not a limitation of the
implementation but of the question** — a check that claimed otherwise would need a range analysis,
and nothing in this language has asked for one.

## 90.4 The three questions the same machinery answers

**Unreachable arms.** An arm is *reachable* exactly when it is useful against the arms above it —
when some value it matches escapes all of them. That is the same function with a pattern where the
wildcard was, so it cost one more entry point and no new algorithm. It is a **warning** and not an
error, and the reason is a habit rather than a principle: `case _:` written after every variant is
something people write on purpose, and turning it into a build failure is a change to what compiles.
Nothing in the tree triggers it, which was established **by running rather than by believing** —
every `.beck` file in the repository was checked.

Nested patterns are what make an unreachable arm easy to write by accident, **which is why the two
arrived together**: `case Yes(Circle(r))`, then `case Yes(_)`, then `case Yes(Square(side))` is a
program whose third arm is dead, and nothing above the pattern language can see that.

**An or-pattern is several rows.** `|` is an infix operator in the expression grammar at the lowest
precedence, producing an ordinary `Node` — which is [`02`](02-syntax.md) §2.6's rule ("patterns *are*
expressions … nothing new to represent") and the same division `*rest` has always had: the grammar
accepts it anywhere and the **checker** refuses it where a pattern is not wanted. The token was free
because [`53`](53-are-we-fast-yet-report.md) established that Beck has no bitwise operators.

The rule that makes an or-pattern a *pattern* rather than two arms sharing a body is that **every
alternative binds the same names at the same types.** That is checked, and then the alternatives'
variables are unified onto the first alternative's, so the body reads one `r` and neither the
evaluator nor any later pass has to know which alternative matched. For exhaustiveness the
alternatives are split into one row each — **lazily, at the column being inspected**, rather than by
distributing every nested alternative up front, because the second is exponential in the number of
them and nothing reads the difference.

**That lazy split is where the one real bug was, and a test written for it is what found it.** The
matrix functions expand column zero on entry, so the view never meets an unexpanded alternative —
except that the top-level coverage pass specialises the matrix *itself* before calling them, and
there an or-pattern fell through the "not a constructor" case and **was read as a wildcard**. A
`match` covering two of three variants was called exhaustive. The failing test exists because **"the
check says yes" and "the check is right" are different claims**: the test that an or-pattern *covers*
what its alternatives cover passes just as well when the alternatives are ignored entirely.

**A guarded arm is no row.** `case x if x < 0:` reads its condition in the scope of what the pattern
bound, and a false one **falls through to the next arm** — which is the whole of what makes it a
guard rather than an `if` in the body. What it costs the check is a rule, and it is the rule that is
easy to get backwards:

- **a guarded arm covers nothing.** Whether it matches depends on a value rather than on a shape, so
  counting it would call a `match` exhaustive on the strength of a condition that can be false.
  `case _ if ok(x):` is not a `case _`.
- **a guarded arm cannot make a later arm unreachable**, for the same reason from the other side. An
  arm above that only sometimes matches does not swallow the one below it, and reporting it dead
  would be **a warning about a correct program, which is worse than no warning at all.**

Both directions are gated, and the second is the one that would otherwise have been found by a user.

The parser change for guards is **one line**, and worth stating because it is the only place the
features interact with the grammar rather than with each other: a `case` pattern is read at binding
power 1 instead of 0. The postfix conditional `a if c else b` is offered only at 0, so at 1 the `if`
after a pattern is unambiguously the guard's — and `|`, which binds at 1, is still read as part of
the pattern. **No lookahead, no pattern grammar, one number.** `@` is an infix operator at a
precedence tighter than `|`, unambiguous because `@` is only special at the *start* of a statement,
where it opens a decorator.

## 90.5 The field fourteen passes would not have walked

`Arm` gained a `guard`. **Adding a field to a struct is a compile error at every place that *builds*
one — and silence at every place that *reads* one, which is where the passes are:**

```rust
arms.iter().map(|a| &a.body)
```

Fourteen sites across nine files walk an arm: liveness, the frame-slot pass, the plan's free-variable
analysis, placement, the effect walk, the splitter's variable-supply high-water mark, the signal
graph, the fusion pass, `beck explain incremental`, and the evaluator. **A guard those walks do not
see is not a compile error.** It is a `Core` that [`70`](70-the-evaluator-gets-fast-report.md)'s
liveness never marks — so a variable read only in a guard may be moved out from under it — and a
`Core` that the frame pass never counts a slot for, and a `Core` whose variables the plan reports as
*free*.

This is [`27`](27-the-walls-come-down-report.md) §27.3's finding one struct over. That report added
`Pattern::binders` for exactly this reason — "a new pattern kind falling into that `_` is not a
compile error; it is a silent miscount" — and the answer here is the same shape: `Arm::exprs()`
yields every `Core` an arm holds, **in the order they run**, and all fourteen sites go through it.
**A future field holding an expression has one place to be added rather than fourteen places to be
forgotten.**

Two of those sites needed more than a mechanical rewrite, and both are about *order*:

- **liveness walks an arm backwards**, because a last use is the latest read: the body is walked
  first and the guard after it, since the guard runs earlier. Walking them the other way would mark a
  read in the guard as a last use when the body reads the same variable afterwards.
- **the frame pass sums rather than maximises.** Arms are alternatives, so their slot counts are
  maximised against each other; a guard and a body are *sequential within one arm*, so theirs are
  summed.

The program that would fail if either were wrong is a guard that reads the pattern's binders, calls a
primitive, and sits above two more arms.

*The fifteenth site turned up later.* `resolve_types` walks an arm's body and not its expressions, so
every node inside a guard kept whatever type variable it had when it was lowered — invisible until
[`93`](93-the-native-backends-report.md) §93.6 became the first consumer to read a node's type from
inside one.

## 90.6 What the evaluator does, and what it does not

An arm whose pattern matches evaluates its guard **in a scope of its own** — a temporary extension of
the environment with the pattern's bindings — so an arm whose guard is false leaves the frame exactly
as it found it and the next arm matches into a clean one. **That is a copy of the bindings on a
failed guard, and it is the honest cost**: the alternative is undoing a write to the reserved slots
[`70`](70-the-evaluator-gets-fast-report.md) built, **and an undo that is wrong is a value from the
wrong arm.**

**There is no decision tree.** A `match` still tries its arms in order, testing each pattern in full —
which is what it did before either feature and what [`23`](23-incremental-views-report.md)'s plan
already treats as opaque. Column-based compilation (Maranget's other paper, and the one this project
has now half-implemented for exhaustiveness) would test each scrutinee field once instead of once per
arm. **Nothing has measured that it matters here.**

## 90.7 A pattern is an expression, so nesting one is a new way to recurse

[`44`](44-wave-0-report.md) bounded the front end's recursion as a count, and
[`85`](85-what-the-generator-found-report.md) then found **three** productions that reached past the
bound. Its lesson is the advisory [`42`](42-security-assurance.md) §42.2 quotes: *a limit added at the
one production somebody thought of is bypassed through a different one.*

A nested pattern is a new recursion in the checker, so it is counted. **What actually answers
`W(W(W(…)))` at 384 deep today is the *reader's* counter and not the checker's** — a pattern is an
ordinary call form, so the parser meets it first, and both ceilings are the same number. The
checker's guard is therefore not the thing that fires, **and it is kept anyway**: this project's own
history is three cases of a limit at one production being reached through another, and a counter at
the consumer is what that history argues for. §90.8 says what could not be constructed to make it
fire, so the next person does not have to rediscover it.

The exhaustiveness check recurses too, and its depth is bounded by something other than a counter: it
specialises only into constructors the *patterns* wrote, so a recursive union with a wildcard arm
terminates at the depth of the deepest pattern. `union Wrap: W(inner: Wrap)` with `case _` does not
loop, **and the test that says so is a program rather than an argument.**

## 90.8 What is not built

| | Status |
|---|---|
| A pattern in the **tail** of a list — `[a, *[b, c]]` | **Refused**, and `B0345` says so rather than saying "Phase 1". It is `[a, b, c]` written twice over: a second spelling, not a shape |
| Exhaustiveness over `Int`, `Str`, `Float` literals | **Not built and not buildable** without a range analysis (§90.3) |
| Compiling a `match` to a decision tree | **Not built** (§90.6), and nothing has measured that it would matter |
| **The checker's pattern counter cannot be made to fire, and it is still right to have** | A pattern reaches the checker only through the parser, both count to the same ceiling, and a pattern cannot be built by the loop [`85`](85-what-the-generator-found-report.md) found — `case 1 + 1` is not a pattern at all. **So the guard is unreachable today**, and every one of that report's three findings was a place somebody had concluded exactly that |
| **An unreachable arm is a warning, and warnings are not gated** | Nothing in CI fails on one. The `pending_security.rs` pattern — asserting an absence so that building the control turns a test red — has no equivalent for "a warning nobody reads", and the honest position is that **this diagnostic is worth what its reader makes of it** |
| A guarded arm's condition is not checked for purity | A guard is an ordinary expression, so its row is inferred and inherited exactly as an `if`'s would be — which means a guard *may* perform an effect, and a `match` inside a `durable` fold with an effectful guard is refused by the fold's own row rather than by anything here. **That is the right layering and it is untested**: no program has an effectful guard |
| The copy on a failed guard is unmeasured | §90.6 states the cost and does not put a number on it. The program that would show it is a `match` whose first arms are guarded and whose guards usually fail, over a pattern binding several large values, and **there is no such program in the tree** |
| A guard on a `stub` arm | **Built by construction and untested** — `stub` shares its arm grammar with `match`, so §21.3's stubs go through the same checker. It follows from sharing the grammar, which is [`22`](22-phase-3-report.md)'s design working, and **"it should work" is the sentence this project treats as a bug report** |

### What this corrects, elsewhere

| Where | What |
|---|---|
| [`27`](27-the-walls-come-down-report.md) §27.3 | "Nested patterns are still refused … patterns in Beck are one level deep, which is what §3.1's exhaustiveness check needs and no more." Built — **and the second clause was the reason**: the check is what had to change. Its `Pattern::binders` argument is applied to `Arm` in §90.5 |
| [`08`](08-roadmap.md) | Pattern matching has no remainder, so the concurrency-and-errors bullet's is [`80`](80-structured-concurrency-report.md)'s alone |
| `B0345` | Its message was "nested patterns are not available in Phase 1" and it now refuses one thing rather than a feature |
| The error index | `B0355` is the first **warning** in the `B03xx` range; `B0356`, `B0357` and `B0358` are new |
| `check/mod.rs` **got smaller** | 3,669 lines to 3,626, while *gaining* a recursive pattern checker and losing forty lines of bookkeeping to a module of its own. That file has grown in almost every report since Phase 1, and [`22`](22-phase-3-report.md) §22.6 spent eight reports asking for something to be lifted out of it rather than added to it |
