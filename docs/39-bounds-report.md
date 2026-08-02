# 39 — Phase 3 report, part 11: bounds, and a dictionary that is a parameter

[`37`](37-traits-report.md) §37.8 item 2 said this:

> **Bounds are now the single item everything else is waiting behind.** `rational.beck`, the numeric
> tower, `@derive`, a `sort` anybody can call, and a trait that crosses a `.becki` all need the same
> thing: a type parameter that carries a constraint, and a call site that supplies the
> implementation. That is one feature, and it is the next one.

It is built.

```
trait Ord_:
    def before(self, other: Self) -> Bool

def largest[T: Ord_](xs: list[T]) -> Option[T]:
    match xs:
        case []:
            return None
        case [first, *rest]:
            return bigger(first, largest(rest))
```

`largest([3, 1, 7, 2])` is `Some(7)` and `largest(["a", "c", "b"])` is `Some("c")`, from one
definition whose body knows nothing about either type beyond the fact that it can order them.

**The implementation is a parameter, not a data structure.** A bounded definition gains one ordinary
parameter per method of each bound, named exactly as an impl method is named but with the *type
parameter* as the target:

```
def largest[T: Ord_](xs: list[T]) -> Option[T]
  ⇒ def largest[T](xs: list[T], Ord_::before@T: (T, T) -> Bool) -> Option[T]

largest([3, 1])   ⇒   largest([3, 1], Ord_::before@Int)
```

So bounds add **no IR node and no evaluator case**, for the same reason [`37`](37-traits-report.md)
did not: a dictionary is a function argument, and Beck already had those. Between the two reports,
the whole trait system has cost the runtime nothing.

[`38`](38-literature-survey.md) §38.1 surveyed this feature before it was built and returned
**adopt**: "a bound is a Wadler–Blott qualified type … elaborates to an extra dictionary parameter,
and the call site resolves the unique impl and passes it." That is what is here, arrived at from the
shape of the existing lowering rather than from the paper — which is a agreement worth recording
either way, and §39.8 says what the survey named that this does *not* do.

548 tests, no failures, no compiler warnings, no clippy warnings — up from 541. No new error code:
`B0386` covered "no implementation can be chosen here" already, and what changed is that it now
distinguishes the three ways that happens.

**`sicp/refusals/rational.beck` still stands, and this report does not empty the directory.** §2.1.1
needs `x + y` on a user's numeric type, which needs the *operator* to go through a trait. Bounds
were the missing machinery; where the trait that `+` resolves through should **live** is a decision
that has not been taken, and §39.7 sets out the options rather than picking one under time pressure —
which is what that refusal file asks for in its own header.

## 39.1 What was asked, and what is answered

| asked for | status | where |
|---|---|---|
| `[T: Trait]` on a definition's type parameter | done | §39.2 |
| Several bounds on one parameter (`[T: A + B]`) | done | §39.2 |
| A generic body calling a trait method | done | §39.3 |
| A bounded definition calling another, passing its own dictionary | done, tested | §39.4 |
| The implementation chosen when only the *context* fixes the type | done | §39.5 |
| A diagnostic that says which of the three failures happened | done | §39.5 |
| Bounds published across a `.becki` | **not done** — dropped, not half-published | §39.6 |
| `+` through a trait, so §2.1.1's rationals compile | **not done**, and the decision is stated | §39.7 |
| A bounded definition passed as a value | **refused**, by name | §39.7 |

## 39.2 The notation, and where it goes

`[T: Show + Eq, U]`. The bound follows the parameter, several traits are joined by `+`, and an
unbounded parameter stays a bare name so that the four declaration forms
[`36`](36-parameterised-types-report.md) gave a type-parameter list are unchanged.

In the AST a bounded parameter is `(: T Show Eq)` — the annotation form, which already existed for
every other place a name meets a type. That means one printer change and no new node kind, and
`every_program_round_trips_through_the_formatter` over the corpus is what says the two surfaces
agree.

Bounds are **only on definitions**. A `model`, a `union`, a `newtype` and a `type` may take
parameters and may not constrain them: a declaration has no body, so there is nothing inside it that
could use a method, and a bound there would be a promise with no reader. Nothing refuses it loudly
today — the parser accepts the syntax and the checker ignores it, which is the one loose end this
report leaves in the surface rather than in the semantics.

## 39.3 The dictionary is a parameter

The lowering is the whole implementation, and it is four lines of idea:

1. For each bounded parameter, for each trait, for each method of that trait, append a parameter
   named `Trait::method@T` whose type is the trait's signature with `Self := T`.
2. Inside the body, `x.before(y)` where `x : T` resolves the name `Ord_::before@T` and finds a
   **local**.
3. At a call site where `T := Int`, the caller resolves the same name at the concrete type,
   `Ord_::before@Int`, and finds the **global** that [`37`](37-traits-report.md)'s impl desugaring
   produced.
4. Nothing else changes.

The row of a dictionary parameter is left to the ordinary rule for a written function type — a fresh
variable — so a caller that supplies a pure implementation stays pure, and one that supplies an
effectful one is charged for it. That is [`33`](33-effect-polymorphism-and-list-patterns-report.md)
§33.2's effect polymorphism arriving for free, because a dictionary is a function-typed parameter and
that is exactly the shape §33.2 generalises.

What the compiler produces is visible rather than hidden. `Def::params` for `label[T: Show]` has two
entries, and the second is called `Show::show@T`; there is a test that asserts both, because a
lowering nobody can see is a lowering nobody can debug.

## 39.4 One rule, two kinds of binding

The part worth calling a design rather than a mechanism: **finding an implementation is one lookup,
and it does not care which kind of answer it gets.**

```
Trait::method@<head of the receiver's type>
```

- the head is a **concrete type** → the name is a global, put there by an `impl`;
- the head is a **type parameter with the bound** → the name is a local, put there by this
  definition's own signature.

So a bounded definition calling another bounded definition needs no special case at all: `outer[U:
Show]` calling `inner[T: Show]` resolves `Show::show@U`, finds its own parameter, and passes it
along. The recursion terminates because a parameter is a value, and there is a test that says
`outer(Point(x=1))` works through two levels.

It is also why [`37`](37-traits-report.md)'s naming choice mattered more than it looked. Mangling
`Trait::method@Target` was introduced so that an impl method could not collide with a user's name;
using the *type parameter* as the target is what makes the two halves of dispatch the same lookup.

## 39.5 When the type is not known yet

An implementation is chosen where the call is written, which means the call's `T` has to be
determined by then. Two things determine it, and both are consulted before anything is reported:

- the **arguments**, checked first;
- the **expectation**, unified next — so `def nothing() -> Option[Point]: return none_of([])` works,
  where the element type is in the return type and not in the argument at all.

If neither says, it is an error at the call rather than a guess. Three failures, three messages:

| what happened | what it says |
|---|---|
| the receiver is a type parameter with no bound | ``B0386: `T` is not known to implement `Show` `` — with the fix, `bound it: [T: Show]` |
| the type is still a variable | ``B0386: cannot tell which type `show` dispatches on here`` — "the type is not determined at this call" |
| the type is concrete and has no impl | ``B0387: `Other` does not implement `Show` `` — labelled with the trait's declaration |

The first is the one that matters most, and it is the one that was wrong before this change: an
unbounded `T` used to be reported as a type with no implementation, which is the right question with
the wrong answer. What is missing is a bound, not an impl, and the diagnostic now says so and offers
the bound.

**The residual limit, stated:** the choice is made at the call and not after the whole body has been
read, so a type that is only pinned by a *later* statement is refused rather than deferred. `x =
none_of([])` followed by a use of `x` that fixes the element type is the shape that fails. Deferring
would mean carrying obligations through to the end of the definition and resolving them against the
final substitution — a real pass, worth building when something needs it, and not before.

## 39.6 What is not published, and why that is now a bigger gap

A bounded definition is dropped from `beck iface`, next to the mangled impl methods
[`37`](37-traits-report.md) §37.6 already dropped:

```console
$ beck iface library.beck
$ grep -c 'largest\|Ord_::' library.becki
0
```

The alternative was to publish `def largest(xs: list[T], Ord_::before@T: (T, T) -> Bool)` — a
signature naming a parameter no source could write, for a trait the importing module cannot see.

This was a footnote in [`37`](37-traits-report.md) and it is not one now. Before bounds, a trait was
an internal convenience; with bounds it is how a library says what it needs, and a library that
cannot publish `def largest[T: Ord_]` cannot publish the interesting half of itself. **Traits
crossing a module boundary is the next piece of work this makes urgent**, and it is a well-shaped
one: publish the trait declarations and the impl headers in the `.becki`, render and re-read them,
and give `--wire-compat` a rule for a removed impl.

## 39.7 What is still not

- **`+` does not go through a trait**, so [`32`](32-numeric-tower-and-polymorphism-report.md) §32.3's
  ad-hoc numeric resolution is untouched and `sicp/refusals/rational.beck` still refuses. The
  machinery it needed is now here; what is missing is a **decision**, and the refusal file asks for
  it to be taken deliberately. The options, plainly:
  1. **A prelude trait** — the compiler ships `trait Num` with `add`/`sub`/`mul`/`div`, and `+` on a
     non-builtin resolves through it. Costs: the prelude has to *declare* a trait, which means
     parsing prelude source and giving its declarations spans that no source file owns, and every
     diagnostic that labels "the trait is declared here" has to cope with that.
  2. **A name the compiler knows and the program declares** — `+` looks for a trait called `Num`
     among the module's own. Cheap, and it makes a program that declares an unrelated `Num` behave
     strangely for reasons nothing explains.
  3. **Operators stay closed** and exact rationals are refused permanently, with §2.1.1 written as
     `add_rat(x, y)` and the register recorded as *re-expressed* rather than *translated*
     ([`25`](25-benchmarks-and-expressiveness.md) §25.5).

  (1) is almost certainly right and (2) is almost certainly wrong; the cost in (1) is real and small
  and it is a span-plumbing problem rather than a design problem. It is named here so that whoever
  takes it is taking a decision rather than reaching for the nearest thing.
- **A bound on a declaration is accepted and ignored** (§39.2). `model Box[T: Show]` parses and means
  nothing. It should be refused by name.
- **A bounded definition cannot be passed as a value**, and neither can a trait method: the
  implementation arrives at the call site and a reference has no call site. `B0386` says so, with a
  test in each direction.
- **The choice is not deferred** (§39.5).
- **No `@derive`, no default methods, no supertraits, no associated types**, unchanged from
  [`37`](37-traits-report.md) §37.7. `@derive` is now the most tractable of these: the impls it would
  write are writable by hand, and the bound it would satisfy is expressible.
- **`check/mod.rs` is 3,210 lines**, up from 3,169, and `check/traits.rs` is 1,349 including its
  tests. The split [`36`](36-parameterised-types-report.md) §36.9 made is still holding: the second
  type-system feature in a row went next door rather than into the file everybody complains about.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9, [`31`](31-tail-calls-report.md) §31.7,
  [`32`](32-numeric-tower-and-polymorphism-report.md) §32.9,
  [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.7,
  [`36`](36-parameterised-types-report.md) §36.10 and [`37`](37-traits-report.md) §37.7 list is
  unchanged: no LLVM backend, no native codegen, no Mode B, no client polish, no `test --update`, no
  structured concurrency, no `Result`/error rows, no SQLite substrate, no standard library v1, no
  identity beyond a dev-mode actor, no LSP, no playground, no supply-chain tooling, no SQL read
  models, no pgwire, no query fusion. Patterns are still one level deep and a `list[T]` is still
  `O(n)` to take apart.

## 39.8 What this changes for the rest of Phase 3

1. **The standard-library bullet is unblocked, and that is now a statement about work rather than
   about design.** Three preconditions were named across three reports — effect polymorphism for a
   user's definition ([`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.8), a container
   that can be declared ([`36`](36-parameterised-types-report.md) §36.11), and a way to say what its
   elements can do ([`37`](37-traits-report.md) §37.8). All three exist. `def sort[T: Ord](xs:
   list[T]) -> list[T]` is writable today; what stops a standard library shipping is that a trait
   cannot cross a `.becki` (§39.6), and that is a fourth item rather than a fourth design.
2. **The urgent gap moved from the type system to the module system.** Every remaining trait-shaped
   item — a published `sort`, `@derive` on an imported type, a `Json` anybody can implement — is now
   blocked on the boundary rather than on inference. That is a smaller and better-understood problem
   than the one this report solved.
3. **The survey's deferred list is now this feature's to-do list.**
   [`38`](38-literature-survey.md) §38.1 names three things bounds do not do and one it should be
   checked for. Associated types before multi-parameter traits (**watch**, trigger: the first trait
   that wants two types); tabled resolution before supertraits (**watch**, trigger: supertraits);
   `@derive`'s two published shapes, deferred to the macro work. And the one that is not deferred —
   *which tier holds a dictionary* has to be a checked question rather than an accident, because a
   dictionary resolved on one tier and used on another is cross-stage persistence. Nothing here
   checks it as such; what stands in its place is that a dictionary is an ordinary argument and an
   impl method is an ordinary definition, so both go through placement like everything else.
   Whether that is sufficient is a question this report raises and does not answer.
4. **Two features in a row have cost the runtime nothing.** Traits and bounds together added no IR
   node, no evaluator case and no runtime change: an impl is a definition, and a dictionary is an
   argument. [`37`](37-traits-report.md) §37.8 item 3 proposed that as a rule to reach for
   deliberately, and this is the second time it has paid. The rule is holding, and the next feature
   that claims to need a new IR node should have to say why this trick does not apply to it.
