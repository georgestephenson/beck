# Phase 3 report, part 55 — the chapter that argues back

**Built.** [`compiler/sicp/ch3.beck`](../compiler/sicp/ch3.beck) — SICP chapter 3, "Modularity,
Objects, and State", twenty-six `test` blocks and a `property` against the book's own printed
answers.
[`08`](08-roadmap.md) §8.5.4's Wave 4 has listed "more of SICP, chapter 3 being the part closest to
what Beck is for" as free-standing with no predecessors since the wave was written, and
[`sicp/README.md`](../compiler/sicp/README.md) has said since [`27`](27-the-walls-come-down-report.md)
that "chapters 3, 4 and 5 are unattempted, and finding the next wall needs somebody to write more of
the book". This is chapter 3 written out, and it found three things.

The chapter is the one the language disagrees with. §3.1 opens by introducing `set!`; §3.4 spends
thirteen pages on what two processes sharing mutable state do to each other; and §3.5 arrives, forty
pages later, at a stream formulation of the same bank account §3.1.1 opened with and says of it:
*"this implementation is fully functional … yet it embodies changing state."* That sentence is
[`01`](01-vision-and-premise.md) §1.1's premise in the book's own words, which is why the chapter was
worth writing out rather than declaring inexpressible — and why the file ends by asserting that
§3.1.1's fold and §3.5.5's stream produce the same balances.

## 87.1 What it contains

| Section | Register | Oracle |
|---|---|---|
| §3.1.1 local state, `make-withdraw`, `make-account` | re-expressed | the book's three transcripts — `75, 50, "Insufficient funds", 35`; `50, 30, "Insufficient funds", 10`; `50, "Insufficient funds", 90, 30` |
| Exercises 3.1, 3.2, 3.3, 3.4 | re-expressed | `15, 25`; `10` and one call; `60` then `"Incorrect password"`; the eighth wrong password |
| §3.1.2 Monte Carlo, `estimate-pi` | re-expressed | Cesàro's theorem at a fixed seed, to a tolerance — §87.3 |
| §3.1.3 the costs of assignment, §3.2 the environment model | translated | `5, -5` against `5, 15`, plus a `property` block |
| Exercise 3.8 | refused | there is no such procedure — §87.2 |
| §3.3.2 queues | re-expressed | the book's session, and a **shape gate** on the cost — §87.5 |
| §3.3.3 tables, exercise 3.27 `memo-fib` | translated | the book's own figure; `fib(60)` |
| §3.3.4 the digital-circuit simulator | re-expressed | `sum 8`, `carry 11`, `sum 16` — the times the book prints |
| §3.4 concurrency | refused | `B0399`, asserted in `sicp.rs` — §87.2 |
| §3.5.1–§3.5.3 streams | translated | `10009`; `117`; `233`; three tables of doubles, digit for digit |
| Exercise 3.56 Hamming | translated | the book's twenty numbers |
| §3.5.5 the account with no assignment | translated | the same balances as §3.1.1's fold |

The registers are [`sicp/README.md`](../compiler/sicp/README.md)'s, and this is the first chapter
where most of them are **re-expressed** rather than **translated**. That is the point rather than an
apology: §3.1's objects are closures over mutable state, and the reorganisation is always the same
one — a closure over mutable state is a function of the state, which is
[`53`](53-are-we-fast-yet-report.md) §53.3's rule arrived at there by porting a benchmark and here by
reading the book that benchmark's language came from.

Two oracles are worth naming because they are hard to pass by accident.

**§3.3.4's trace.** The half-adder's `sum 8`, `carry 11` and `sum 16` are a consequence of the whole
simulation: a gate computes its output from the inputs it can see *now* and schedules that value for
`now + delay`, and `set-signal!` runs a wire's actions only when the value actually **changed**. Get
either wrong and the times move. The port stores no closure in any data structure — a gate is a
`union` variant and "what runs when this wire changes" is read off the gate list — so §3.3.4's
`make-wire`, which exists to hold a list of procedures to run later, has no counterpart at all. It
matched on the first run.

**§3.5.3's three tables.** `1.4166666666666665`, `2.8952380952380956`, `3.1415926539752927` and the
rest are IEEE 754 doubles on both sides, so the assertion is equality rather than a tolerance —
[`27`](27-the-walls-come-down-report.md) §27.2's property, and the same standard
`ch1.beck` holds §1.1.7's `3.00009155413138` to. Getting an operand order wrong moves the last digit.
`(+ s0 (* -2 s1) s2)` is `(s0 + (-2 × s1)) + s2`, which is what Beck's left-associative `+` gives.

## 87.2 What is refused, and why each is a decision

**Exercise 3.8** asks for a procedure `f` such that `(+ (f 0) (f 1))` is 0 left-to-right and 1
right-to-left. No such `f` exists in Beck, because `f` would have to be a function of something other
than its argument. That is the exercise's own point — assignment makes evaluation order observable —
and `ch3.beck` carries the property that stands in its place instead.

**§3.4 in full.** §3.4.1's timing diagram is two people withdrawing from one account and the balance
ending up wrong; §3.4.2 is the serializers that stop it. `ch3.beck` cannot contain the first, because
a chapter file has to compile, so `sicp.rs` holds it: the `parallel:` scope whose two children both
touch a `durable` fold is `B0399`, and the version where one child names another is `B0398`
([`80`](80-a-scope-owns-its-children-report.md)). Thirteen pages of the book are two compile errors —
which is a claim about the *problem* being absent, not about Beck having a better lock. What Beck has
instead of §3.4.2's answer to the question serializers are actually for is the merge point (§3.7),
one place rather than a lock per object, and a library file has no way to demonstrate it.

**§3.2, the environment model**, is the one section this file can honestly say is *not needed* rather
than not done. It exists because §3.1.3 shows that with `set!` the substitution model stops working.
Beck's is still valid at the end of the chapter.

## 87.3 A bad generator is reproducible when the generator is a value

§3.1.2's argument for assignment is `rand`: "to implement `rand` we must have it produce a new value
each time it is called, and that requires local state." Beck's answer is not an argument — it is that
§3.5.5 is the book itself withdrawing this one ("we can eliminate the assignment by supplying the
random numbers as a stream"), so the generator is threaded like §3.1.1's balance.

What that cost, and it is worth more than the estimate: the first generator written here was a
textbook power-of-two LCG, and it estimates π at **2.718**. Modulo 2³¹ the low bit of an LCG
alternates, so consecutive draws have opposite parity far more often than chance and Cesàro's test —
the probability that two random integers are coprime is 6/π² — sees the wrong distribution. Lehmer's
minimal standard, a multiplicative generator modulo a *prime*, gives 3.1405 at three thousand trials.

The finding is not that one generator is better than another, which is sixty years old. It is that
the failure was a **deterministic property of a fixed input**, caught by a test that runs on every
pull request. Behind `set!` — where the seed is private to the procedure and the sequence depends on
how many times anything has called it — the same defect is a test that fails sometimes.

## 87.4 A declaration could take a builtin type's name, and nothing said so

SICP's central abstraction in §3.5 is called a **stream**, and `Stream` is already a type in Beck's
prelude: `merge_clients() -> Stream[Proposal]`, the event stream every application folds. They are the
same idea, which is why §3.5.5 can end the chapter the way it does.

Writing that sentence meant checking what happens if a program declares one, and the answer was:
nothing.

```beck
model Int:
    label: Str

def mine() -> Int:
    return Int(label="not a number")

def arithmetic(n: Int) -> Int:
    return n + 1
```

That compiled, and both tests pass. `Int` names the record in one definition and the builtin integer
in the next, in one module, silently. The same for `Str`, `Bool`, `Float`, `Unit`, `Html`, `Attr`,
`list`, `Map`, `Stream`, `Signal`, `Envelope`, `secret` and `internal` — fourteen of the sixteen
builtin type constructors. The two that *were* refused, `Option` and `Result`, were covered by
accident: they are prelude **declarations**, so they sit in the checker's type table and a second
declaration hit B0302's "declared twice".

The shape is [`85`](85-what-the-generator-found-report.md) §85.7's, one report later and from the
other direction. The rule exists in the compiler already: `B0314` refuses a **type parameter** that
shadows an existing type, with a message explaining exactly why —

> a type parameter is a name the declaration invents, and one that shadowed an existing type would
> make its fields read as though they mentioned that type

— and it consults `prelude::builtin_arity` to do it. A *declaration* was the production the same rule
did not cover. `B0317` is the fix, in `declare_type_names` where every `model`, `union`, `newtype` and
`type` passes, and the refusal leaves the builtin in place so the rest of the module's diagnostics
stay about the module.

The test enumerates all sixteen names and both declaration forms rather than picking one, because a
test that named one would be testing the shape of the gap rather than the shape of the fix — which is
[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5's pattern, and the reason this one is
written the way it is.

`ch3.beck` calls the book's stream `Seq[T]` and says why in the file.

## 87.5 `with` was quadratic where a constructor was linear

§3.3.2 does not reach for `set-cdr!` for elegance. It gives a cost: "if we represent a queue as an
ordinary list, `insert-queue!` must traverse the list to find the end", Θ(n) per insert against Θ(1)
for the mutable version with a rear pointer. The functional answer is two lists and is Θ(1)
amortised, so the gate for it is a *shape*: steps per operation must not grow with the number of
operations, in the form `scaling.rs` uses, which needs no clock and cannot flake ([`13`](13-testing.md)
§13.7).

It failed. Five hundred insert-and-delete pairs cost 380 evaluator steps each and four thousand cost
**2,661** — eight times the operations for seven times the cost *each*, which is the shape §3.3.2
reaches for mutation to avoid.

Neither the queue nor the language's list operations were at fault. Narrowed with the same probe at
two sizes:

| Written as | 1,000 elements | 8,000 elements | per element |
|---|---|---|---|
| `Box(tag=b.tag, items=list_append(b.items, x))` | 21,076 steps | 168,610 steps | 21.1 → 21.1 |
| `b.with(items=list_append(b.items, x))` | 520,324 steps | 32,226,562 steps | **520 → 4,028** |

A record **constructor** is linear and the `with` form is quadratic, for the same update. So is
`[first, *rest]` destructuring, measured at 21.1 steps per element either side — the pattern every
list recursion in the tree uses, and the first thing suspected.

The reason is one line of the evaluator and it is [`70`](70-the-evaluator-gets-fast-report.md)'s rule meeting a
second reference. `x.with(f = g(x.f))` reads `x` **twice**: once as the base and once inside `g`. The
base is read first, so it is not `x`'s last use and arrives as a clone; `eval_with` then held that
clone — and therefore a second reference to `x.f` — while it evaluated `g(x.f)`. The read inside `g`
*is* a last use, so it moved the record out of the frame and took the field by move, and then found
the field's `Arc` at two. `list_append` copies rather than pushes at two. A record constructor has no
such second reference: `Box(…)` evaluates `b.tag` (cheap) and then `b.items` as the last read, which
moves.

The fix is that the clone arrives with the fields about to be replaced already empty. It is one pass —
the same pass the clone was already making — so the fields the `with` does not name are cloned as
before and the ones it does are `Unit` until their replacements are computed. Nothing observes them:
the copy is local to `eval_with` and the frame's own record is untouched, which is what a field
expression reads. The other branch, where `Arc::try_unwrap` succeeds, needs nothing: if a field
expression mentioned the base then the base's read was not a last use and that branch was not taken.

Measured after, at the same two sizes: **20.0 steps per element at 1,000 and 20.0 at 8,000** — a
**201× reduction at 8,000** and unbounded beyond. The queue is 53 steps per operation at both 500 and
4,000, and `the_functional_queue_costs_the_same_per_operation_however_long_it_gets` is the gate.

What it costs elsewhere, rotated with a control per
[`70`](70-the-evaluator-gets-fast-report.md) §70.7, best of four, release:

| 2,000,000 `with` on an `Int` field | 300,000 `with` on a `Map` field | 40,000 `with` on a `Str` field |
|---|---|---|
| +0.6% (control +1.3%) | −0.2% (control +3.6%) | **−53.1%** |

Within the control's own noise on the first two, and an asymptote on the third. The first version of
the fix — clone, then blank in a second pass — cost a measurable **+2.6%** on the `Int` case, which is
why there is a second version: doing it inside the clone is free and doing it after the clone is not.

### 87.5.1 Why three reports on this did not find it

[`19`](19-phase-1-report.md) §19.4 found a fold that copied its accumulator,
[`69`](69-standard-library-imports-report.md) §69.7 found `list_append` copying,
[`70`](70-the-evaluator-gets-fast-report.md) fixed the recursive spelling and
[`70`](70-the-evaluator-gets-fast-report.md) fixed the fold spelling three reports later. This is the same
defect in a fourth spelling, and the reason it survived is
[`70`](70-the-evaluator-gets-fast-report.md) §70.2's, exactly:

**nothing in this repository accumulates into a `list` or a `Str` field through `with`.** Every
`with` in `corpus/` and `examples/` updates an `Int` or a persistent `Map` — `s.with(n=s.n + 1)`,
`s.with(items=map_insert(s.items, …))` — and a `Map` is structurally shared, so a second reference
costs it nothing. `awfy/` and `clbg/` contain no `with` at all. So the tree's own programs walked
around the defect by habit for a second time, and what found it was a benchmark from outside the tree
whose author had a *reason* to update a compound structure in a loop: SICP §3.3 is forty pages about
exactly that.

Three of the four were found by asking what an operation should cost. This one was found by a gate
that was written to check somebody else's argument for mutation and turned out to be checking ours.

## 87.6 What chapter 3 does not contain

| | |
|---|---|
| §3.2, the environment model | **not needed**, §87.2 — the section exists because assignment invalidates the substitution model |
| §3.3.1, `set-car!` and `set-cdr!` | **refused, and it is the chapter's premise rather than a gap.** Exercises 3.12–3.20 are about what mutation does to shared structure; in Beck there is no shared structure to mutate, so `append!` and `append` cannot differ |
| §3.3.5, the constraint propagator | **not attempted.** It is the one section left that would be *interesting* rather than mechanical — a constraint network is a dataflow, and Beck's view engine is one ([`24`](24-incremental-views-report.md)) — and the honest reason it is absent is time rather than a wall. Its "Contradiction" is a `raise` ([`27`](27-the-walls-come-down-report.md)), and its `forget-value!` is where the analogy would be tested |
| §3.5.4, streams and delayed evaluation | **not attempted.** `integral` with a delayed integrand is the one place §3.5 needs `delay` in an *argument* rather than in `cons-stream`, and it wants its own look |
| §3.5.1's memoised `delay` | **refused, and it is a cost rather than an inexpressibility** — §87.7 |
| A line-count comparison against the Scheme | **not made.** §25.5 asks for one on *translated* exercises, and most of this chapter is re-expressed, where the comparison would measure the reorganisation rather than the notation |

## 87.7 The one thing chapter 3 cannot express, measured

§3.5.1 does not leave `delay` as a bare promise. It says the resulting stream "will be computed more
efficiently if we arrange things so that the promise is forced only once", and implements that with
`memo-proc` — two `set!`s on state private to one promise.

Beck has no assignment, so a promise is a `lambda` and forcing it twice does the work twice. That is
invisible for every stream in §3.5.1 and §3.5.2, because each of those is forced once along a single
traversal: `ch3.beck` runs the sieve of Eratosthenes to the book's fifty-first prime, `no-sevens` to
its hundredth term, and the second prime past ten thousand out of an interval a million long.

It is ruinous for §3.5.3's tableau, where element *j* of a row needs elements *j*, *j+1* and *j+2* of
the row before it. Measured with `beck test --fuel`:

| terms | 4 | 5 | 6 | 7 |
|---|---|---|---|---|
| steps | 148,391 | 814,819 | 4,284,667 | 21,777,343 |

**×5.2 per term.** Nine is past the evaluator's whole 50,000,000-step budget and ten is out of reach
at any budget worth spending. Exercise 3.56's Hamming numbers, whose definition names itself three
times, are the mild case of the same thing: 8,273 steps for twenty terms, 35,859 for forty, 213,623
for eighty and 2,172,851 for a hundred and sixty — ×4.3, ×6.0 then ×10.2 per doubling, so the cost per
term grows without ever becoming exponential the way the tableau does.

`ch3.beck` carries both: the book's tableau asserted as far as it reaches, and the same nine numbers —
bit for bit — from a tableau whose rows are **lists**. Materialising a row is what `memo-proc`
accomplishes with two assignments, and doing it by hand costs one prefix length the stream version did
not have to name. By Felleisen's criterion nothing is lost: it is a local rewrite of one definition
([`63`](63-felleisen-report.md)). What is lost is the asymptote, and that is the honest form of the
claim.

Whether Beck should have a memoised promise is a question this report raises and does not answer. The
two shapes it could take are a language-level `delay` that is a form rather than a macro (so the
compiler owns the cell), and a value-level memo that would need interior mutability inside a value the
`Map` order, the state digest and the patch stream all read — which is
[`70`](70-the-evaluator-gets-fast-report.md) §72's reason for building the character index eagerly, one
level up. Neither is a line in a chapter file.

## 87.8 What this establishes

**That the expressiveness suite is worth running against a chapter that disagrees with the
language.** Chapters 1 and 2 were a language catching up with a book; six walls came down for them
and three more that the removals wrote. Chapter 3 produced no wall of that kind at all — every §3.1
and §3.3 section is expressible, and the reorganisation is one rule applied twelve times. What it
produced instead was a *cost* the book names explicitly (§87.7), a diagnostic that did not exist
(§87.4) and a quadratic in the most ordinary form in the language (§87.5), none of which is about
expressiveness.

**And that the defect was hiding behind an argument, not behind a number.** §87.5's quadratic was
found because §3.3.2 argues for mutation on cost grounds and the argument had to be answered. Writing
"amortised Θ(1)" in a comment would have been the end of it; writing the gate that says so was three
lines and turned red. `AGENTS.md`'s rule is to state the order of growth and measure it at two sizes;
this is the first time in this project that following it caught something in the *compiler* rather
than in the program being written.
