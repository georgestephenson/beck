# 41 — Phase 3 report, part 13: generic arithmetic, and an empty refusals directory

`sicp/refusals/rational.beck` was the last file in that directory. It said:

> What is missing is a *type*, not an operator. `Rational` would be a pair of `Int` in lowest terms
> — which is writable today as a `model` — but the arithmetic has to be part of the numeric
> resolution `+`, `-`, `*` and `/` go through (docs/32 §32.3), or every expression in the section
> reads `add_rat(x, y)` instead of `x + y` and the exercise is about function names rather than
> about data abstraction.

It reads `x + y`:

```
model Rational:
    numer: Int
    denom: Int

impl Num for Rational:
    def add(self, other):
        return make_rat(numer(self) * denom(other) + numer(other) * denom(self),
                        denom(self) * denom(other))
    ...

def two_thirds() -> Rational:
    return one_third() + one_third()
```

`print_rat(one_half() + one_third())` is `"5/6"`, which is the string on the page of SICP. So is
`"1/6"` for the product, and so is `"2/3"` — the number §2.1.1 exists to be exact about, and the one
a real gives as `0.6666666666666666`.

**The decision [`39`](39-bounds-report.md) §39.7 declined to take under time pressure, taken.**
Three options were written out. The answer is the first, and the reason is that SICP already
answered it: §2.5.1 builds *generic arithmetic* by hand, as a set of operations named `add`, `sub`,
`mul` and `div` that each type installs an implementation for. That is a trait. So `Num` ships in the
prelude, with the book's own four method names, and `+` resolves through it when its operands are
neither `Int` nor `Float` nor `Str`.

**`sicp/refusals/` is empty**, and there is a `README.md` in it saying what that means and what would
put a file back. The harness asserts the emptiness rather than stopping asserting anything, because
"nothing measured is refused today" is a claim and claims get tests.

558 tests, no failures, no compiler warnings, no clippy warnings — up from 553. No new error code.
Chapter 2 is 18 tests, up from 15.

## 41.1 What was asked, and what is answered

| asked for | status | where |
|---|---|---|
| `+`, `-`, `*`, `/` on a user's type | done | §41.2 |
| Exact rationals — SICP §2.1.1 | done, against the book's printed answers | §41.4 |
| Where the trait lives, decided rather than defaulted | **prelude**, and SICP is the argument | §41.2 |
| The numeric rule unchanged where it had an answer | done, tested in both directions | §41.3 |
| Operators inside a generic definition (`[T: Num]`) | done, free | §41.5 |
| The orphan rule against a prelude trait | **tightened** — it was too weak | §41.6 |
| `Num` visible in the generated reference | done | §41.6 |
| A refusals directory that says what empty means | done | §41.7 |

## 41.2 The tower is open because the book's is

[`32`](32-numeric-tower-and-polymorphism-report.md) §32.3 resolved `+` from its operands and was
explicit that this was temporary: *"an ad-hoc resolution is the honest thing to build before traits
exist rather than a stand-in for them"*. Traits exist. So:

```
trait Num:
    def add(self, other: Self) -> Self
    def sub(self, other: Self) -> Self
    def mul(self, other: Self) -> Self
    def div(self, other: Self) -> Self
```

It is in `prelude.rs`, built as a `TraitSig` in Rust and handed to the checker through the **same
door an imported trait comes through** — `import_traits`, which [`40`](40-traits-across-modules-report.md)
built for a different reason. So there is no special case for it anywhere: no prelude source to
parse, no spans belonging to a file that does not exist, and every diagnostic about it is the same
diagnostic as for any other trait.

**Why the prelude rather than a name the compiler knows and the program declares.** The second
option would have been cheaper by a day and would have made a program that declared an unrelated
`Num` behave strangely for reasons nothing explains. The third — leaving operators closed and
writing §2.1.1 as `add_rat(x, y)` — is the one the refusal file argued against, and it was right to:
the exercise is about data abstraction, and a language where the third floor of the tower reads
differently from the first two has not abstracted anything.

**`Int` and `Float` do not implement it.** They go through the primitives, as they always have. A
tower whose ground floor is a dictionary call would make every existing program slower to prove a
point, and the point is already made by the floor above.

## 41.3 Dispatch only where there is something to dispatch to

The rule, exactly: if neither operand is `Int`, `Float` nor `Str`, and the operand's type has a
`Num` implementation — or is a type parameter bounded by `Num` — the operator becomes a call to it.
Otherwise the numeric rule runs unchanged.

That last clause is doing real work. `1 + true` is still `B0320: operand of '+' mismatch: expected
'Int', found 'Bool'` — a mismatch, not a lecture about traits — and `1 + 1.0` is still refused,
because [`32`](32-numeric-tower-and-polymorphism-report.md) §32.3's decision not to coerce is
untouched. Both have tests, and they are the tests that say this feature is additive.

The one case that *does* change is a declared type with no implementation, and it changes for the
better:

```
error[B0387]: `Money` does not implement `Num`
  |
5 |     return a + b
  |            ^^^^^ `+` resolves through it
  = fix: write `impl Num for Money`
```

Before, that read "expected `Int`, found `Money`", which names the symptom. This names the cure.

An operand whose type is still a unification variable is left alone and defaults to `Int`, which is
what keeps every program written before this compiling.

## 41.4 §2.1.1, against the book's own answers

`sicp/ch2.beck` opens with §2.1.1 now rather than §2.2.1, which is the order the book has:

- `make_rat` normalises through `gcd`, and **exercise 2.1**'s sign handling is there — `make_rat(1,
  -2)` is `-1/2` and `make_rat(-1, -2)` is `1/2`;
- `numer`, `denom` and `print_rat` are the book's selectors, with `print_rat` returning a `Str`
  because the book prints and a `test` compares;
- `add-rat`, `sub-rat`, `mul-rat` and `div-rat` are the book's, installed as the generic operations
  §2.5.1 installs them as — and **nothing below that line ever names one of them**.

Three of the tests assert strings SICP prints. A fourth asserts the thing the section is *about*:

```
expect (1.0 / 3.0) + (1.0 / 3.0) != 0.6666666666666667
```

— that the real is a different number, which is why `Rational` had to exist. That one is the reason
this wall survived [`32`](32-numeric-tower-and-polymorphism-report.md)'s reals: reals were never
this wall, and the refusal file said so in its own second paragraph.

Chapter 2 is 18 tests, up from 15.

## 41.5 What came free

**Operators inside a generic definition.** `Num` is a trait like any other, so
[`39`](39-bounds-report.md)'s bounds apply to it without a line of new code:

```
def twice[T: Num](x: T) -> T:
    return x + x
```

resolves `+` to the dictionary `twice` was handed. That is three features composing — traits,
bounds, and the operator desugaring — and none of them knows about the others.

**A trait method that is only reachable through an operator is still an ordinary method.**
`r.add(other)` works, because `Num`'s methods are registered like any trait's. That was not designed
for; it is what happens when a feature is built out of the general mechanism rather than beside it.

## 41.6 The orphan rule was too weak, and a prelude trait found it

Building this exposed a real defect in [`37`](37-traits-report.md) §37.4's coherence check. It read:

```rust
let owns_trait = self.traits.contains_key(&trait_name);
```

`self.traits` is every trait *in scope*, which includes imported ones — so "the trait or the type is
declared here" was satisfied by merely being able to see the trait. Any module could implement any
imported trait for any imported type, which is precisely the case the orphan rule exists to refuse.
Nobody could hit it before [`40`](40-traits-across-modules-report.md), because before that no trait
was ever in scope without being local.

It reads `self.own_traits.contains(&trait_name)` now — *declared here*, not *visible here* — and
`impl Num for Int` from an ordinary module is `B0385`, with a test. `impl Num for Rational` is fine,
because `Rational` is yours.

`Num` is also in the generated reference, under a new **Traits** section of the prelude page, with
its methods and the sentence a reader needs: a numeric type is something a program declares rather
than something the compiler has a list of.

## 41.7 What is still not

- **`Num` is one trait, not a tower with coercion.** SICP §2.5.2's "combining data of different
  types" — raising an `Int` to a `Rational` so `1 + one_half()` works — is *not* built.
  `1 + one_half()` is a mismatch. That is deliberate and it is the same decision
  [`32`](32-numeric-tower-and-polymorphism-report.md) §32.3 took about `1 + 1.0`: coercion is a
  design with consequences (which direction? is it transitive? does it apply to comparison?) and
  guessing it here would be exactly the sort of thing this project writes reports to avoid.
- **There is no `negate` in `Num`**, so unary minus does not reach a user's type. The four the
  refusal file named are the four that are there.
- **No `Eq`, `Ord`, `Show`, `Json` or `Hash` in the prelude.** [`03`](03-type-and-effect-system.md)
  §3.1 names them and this report adds one trait, not six. `==` is still structural, which works for
  a `Rational` because `make_rat` normalises — and would not work for a type where equality is not
  structural. Nothing refuses that today; it is simply not expressible.
- **§2.5.1's actual section is not written.** What is built is the *mechanism* §2.5.1 argues for.
  The book's own generic-arithmetic package, with `scheme-number`, `rational` and `complex` packages
  installed side by side, would need `Eq` and a coercion story. §2.3.4's Huffman decoder still wants
  a pattern more than one level deep ([`33`](33-effect-polymorphism-and-list-patterns-report.md)
  §33.7), and chapters 3, 4 and 5 are unattempted.
- **The refusals directory is empty, and that is a weaker claim than it looks.** "Every wall this
  project found has been removed" is not "Beck expresses SICP". Finding the next wall needs somebody
  to write more of the book, and `sicp/refusals/README.md` says exactly that rather than letting an
  empty directory imply otherwise.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9, [`31`](31-tail-calls-report.md) §31.7,
  [`32`](32-numeric-tower-and-polymorphism-report.md) §32.9,
  [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.7,
  [`36`](36-parameterised-types-report.md) §36.10, [`37`](37-traits-report.md) §37.7,
  [`39`](39-bounds-report.md) §39.7 and [`40`](40-traits-across-modules-report.md) §40.7 list is
  unchanged: no LLVM backend, no native codegen, no Mode B, no client polish, no `test --update`, no
  structured concurrency, no `Result`/error rows, no SQLite substrate, no standard library v1, no
  identity beyond a dev-mode actor, no LSP, no playground, no supply-chain tooling, no SQL read
  models, no pgwire, no query fusion.

## 41.8 What this changes for the rest of Phase 3

1. **The expressiveness benchmark has run out of measured walls, which means it needs more book.**
   [`25`](25-benchmarks-and-expressiveness.md) §25.5's protocol has been doing its job for six
   reports; what it needs now is chapter 3, which is state and time and is the part of SICP closest
   to what Beck is actually *for*. That is the next thing the suite should be pointed at, and it
   will produce refusals rather than confirmations, which is the point.
2. **The prelude is a place where things live now, and that needs a rule.** One trait went in on the
   argument that SICP put it there first. `Eq`, `Ord` and `Show` have the same argument available and
   a much weaker case for urgency, and "the prelude is small on purpose" (`docs/reference/prelude.md`)
   is a claim the next addition should have to answer. A short ADR on what earns a place in the
   prelude would be cheaper than the argument it prevents.
3. **A defect found by building the thing that would expose it.** §41.6's orphan-rule hole existed
   for two reports and could not be reached until a trait was in scope without being local. Both
   [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.5 and
   [`23`](23-general-slicer-report.md) §23.2 record the same shape: a check that was written for a
   situation the language could not yet produce. The lesson repeats often enough to be worth a rule
   — when a check's condition can only be true one way today, write down which way, because the day
   the second way exists is the day the check is wrong.
4. **The four-report trait arc is closed.** [`37`](37-traits-report.md) declarations and impls,
   [`39`](39-bounds-report.md) bounds, [`40`](40-traits-across-modules-report.md) the boundary,
   [`41`](41-generic-arithmetic-report.md) the operators. Between them: no IR node, no evaluator
   case, no runtime change. Every part of it is a definition, an argument or a name.
