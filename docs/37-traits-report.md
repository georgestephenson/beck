# 37 — Phase 3 report, part 10: traits, checked at last

The compiler used to say this, on every `trait` in every program:

```
warning[B0306]: traits are parsed but not yet checked
  = note: Phase 2 built the effect system this was once expected to arrive with, and trait
    resolution did not come with it: it is still unimplemented, and this warning is the only
    thing standing between a `trait` and silence
```

It does not say it any more, because there is nothing left for it to warn about. `B0306` has been
**retired** — the second code this branch has removed, and for the same reason as the first: its
premise stopped being true.

A `trait` declares signatures over an abstract `Self`. An `impl` supplies the bodies and writes no
types at all, because the trait already wrote them:

```
trait Priced:
    def pence(self) -> Int
    def describe(self) -> Str

impl Priced for Item:
    def pence(self):
        return self.pence
    ...

impl[T] Priced for Bundle[T]:          # one impl, every element type
    def pence(self):
        return list_len(self.parts) * self.each
```

That is [`11`](11-language-tour.md) §11.3's notation, unchanged, and it is the oldest unpaid debt in
the project — named by [`19`](19-phase-1-report.md), [`26`](26-arrangement-sharing-report.md) §26.9,
[`32`](32-numeric-tower-and-polymorphism-report.md) §32.9,
[`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.7 and
[`36`](36-parameterised-types-report.md) §36.11.

**This is the ad-hoc half and not the parametric half, and the difference matters more than
anything else in this report.** A call resolves from the *type of its receiver*, at check time, to
exactly one implementation. There are no **bounds**: `def f[T: Priced](x: T)` is not writable, so
generic code cannot call a trait method and a trait method cannot be passed as a value. Both need
dictionary passing. `B0386` says so by name where a program tries, with a test in each direction so
that the day bounds land, the tests that assert the limit start failing.

So **`sicp/refusals/rational.beck` still stands**, and it is worth being exact about why: §2.1.1
needs `x + y` on a user's numeric type, which needs `+` to be a trait method *and* needs the
resolution to survive a generic context. Neither half is built. The refusals directory has one file
in it, and this report has not emptied it.

541 tests, no failures, no compiler warnings, no clippy warnings — up from 528. The error index is
101 codes, up from 94: seven for traits, minus the retired warning.

## 37.1 What was asked, and what is answered

| asked for | status | where |
|---|---|---|
| `trait` with method signatures over `Self` | done | §37.2 |
| `impl Trait for Type`, with bodies checked | done | §37.2 |
| `impl[T] Trait for Type[T]` — one impl per parameterised type | done | §37.3 |
| Coherence: one impl per trait per type | done, tested | §37.4 |
| The orphan rule | done, tested | §37.4 |
| Completeness: every method, exactly once, no others | done, tested | §37.4 |
| The trait's effect row bounds every impl | done — and it found a hole | §37.5 |
| Dispatch, with no IR node and no evaluator case | done | §37.3 |
| Bounds on a type parameter (`[T: Priced]`) | **not done** — the parametric half | §37.7 |
| `+` and the numeric tower through a trait | **not done**, so §2.1.1 still refuses | §37.7 |
| `@derive` | **not done** | §37.7 |
| A trait across a `.becki` | **not done**, and refused rather than half-published | §37.6 |

## 37.2 A trait writes the signature; an impl writes the body

The rule that shapes everything else: **an impl supplies parameter names and a body, and nothing
else.** Writing a parameter type, a return type or a `uses` clause in an impl is `B0382` rather
than something checked for agreement.

```
error[B0382]: `show` may not restate its return type or its effects
  |
  = note: an impl writes the body; the signature is the trait's, and a second copy of it is a
    second place for it to be wrong
```

It is a smaller language and a stronger guarantee at once. The signature exists in exactly one
place, so an impl cannot disagree with its trait about the types — there is nothing to disagree
with. `def pence(self):` binds `self` to the impl's target and returns `Int` because `Priced` said
so.

`self` alone means `self: Self`, which is the notation §11.3 writes. A trait method must mention
`Self` in at least one parameter, because dispatch is by the receiver: a method that mentioned
`Self` only in its return type would need the call site to say which impl it meant, and there is no
notation for that. `B0381` refuses it and the note says exactly that.

A `trait` holds signatures and not bodies. A default method is a separate feature — it would have to
be checked once against an abstract `Self` rather than once per implementing type — and it is
refused by name rather than by a parse error.

## 37.3 Desugared, not dispatched

An impl is turned into **ordinary top-level definitions** before anything is checked. Each method
becomes a `def` whose name is mangled — `Priced::pence@Item` — whose parameter types are the
trait's with `Self` replaced by the target, and whose type parameters are the impl's:

```
impl[T] Priced for Bundle[T]:        ⇒   def Priced::pence@Bundle[T](self: Bundle[T]) -> Int
    def pence(self):                          return list_len(self.parts) * self.each
        return list_len(self.parts) * self.each
```

From there it is a definition like any other. `collect_signatures` gives it a scheme, `check_items`
checks its body, the effect row is inferred, placement places it, the splitter slices it, the plan
decomposes calls to it, and the evaluator calls it through `CoreKind::Global`.

**So this feature adds no IR node and no evaluator case.** Not one line of `beck-eval` changed. That
is the whole argument for doing the ad-hoc half first: static dispatch is a *name*, and Beck already
had names.

`::` and `@` are not identifier characters in either surface, so a mangled name cannot collide with
anything a program declares. It is deliberately legible rather than hashed, and it is deliberately
**visible in `beck explain place`**:

```
Priced::pence@Item        any      definition {}
Priced::describe@Discount any      definition {}
Priced::pence@Bundle      any      definition {}
```

An impl method is a definition that has to be placed, and hiding it would make that table lie about
what the program contains. `beck doc` shows none of them, because a doc page is written for a reader
and there is no `Priced::pence@Item` in the source.

Dispatch keys on the **head constructor** of the receiver, so `impl[T] Priced for Bundle[T]` is one
impl covering every `T`, and a call at `Bundle[Str]` and a call at `Bundle[Int]` both find it. That
is [`36`](36-parameterised-types-report.md)'s feature meeting this one, and neither would have been
worth much alone: without parameterised types every container needs its own impl, and without traits
a parameterised container has no way to say what its elements can do.

## 37.4 Coherence, in three parts

**One impl per trait per type constructor.** Keyed on the head, so `Bundle[Int]` and `Bundle[Str]`
cannot be given different behaviour and no call has to choose between two. A second impl is
`B0384`, labelled with the span of the first, and the note says why the rule exists: *so that what a
call means never depends on which impls happen to be in scope*.

**No blanket impls.** `impl[T] Priced for T` is refused. It would make coherence a search rather
than a lookup, and Beck's orphan rule is written for one impl per trait per type constructor.

**The orphan rule.** An impl needs the trait *or* the type to be declared in this module. That is
not a ban on implementing a trait for a builtin — `impl Show for Int` is fine when `Show` is
yours, and there is a test that says so — it is a ban on supplying an impl that two modules could
both supply and disagree about.

Two more that are not coherence but are the same kind of check: an impl must implement **every**
method of its trait (`B0382` lists the missing ones and labels the trait), exactly once, and no
others. And a method name belongs to **one** trait: two traits declaring `show` would make `x.show()`
ambiguous at every call site, and resolving it by which impls exist would make adding an impl change
what an unrelated call means.

## 37.5 The effect row is the trait's, and that found a hole

A trait declares its methods' rows, and every impl is held to them. `docs/03` §3.6's "effect
widening is a breaking API change" is what makes this matter: a caller of `show()` knows what it
performs because the *trait* said, and one implementation reaching for a clock would make that
false everywhere.

Getting it right exposed a real gap in how declared rows worked. `check_items` had:

```rust
if def.declared_effects.is_empty() {
    continue;               // nothing declared, so nothing to bound
}
```

For a hand-written `def` that is correct — writing no `uses` means "infer it". For a trait method it
is wrong, because the trait *did* declare, and what it declared was the empty row. Under the old
rule, `def show(self) -> Str` bounded nothing at all, and this would have compiled:

```
trait Show:
    def show(self) -> Str          # declares: performs nothing

impl Show for Point:
    def show(self):
        return uuid()              # performs nondet, and every caller inherits it
```

`Def` now carries `row_is_declared` — "the signature *stated* its row, even if the row it stated was
empty" — which is the distinction the old code could not make. A hand-written `def` with no `uses`
has it false; every impl method has it true. The program above is now `B0370`, naming `nondet`, and
there is a test on it.

That is worth stating plainly: **the hole was not in traits.** It was in the meaning of an empty
declaration, and traits are simply the first construct that could produce one.

## 37.6 What does not cross a module boundary

A `.becki` publishes **neither traits nor impls**, and a `.becki` that contains either is refused
(`B0380`) rather than accepted and ignored. `Interface::of` drops the mangled definitions, so a
library that uses traits internally publishes its ordinary contract and nothing else:

```console
$ beck iface corpus/28-catalogue.beck
$ grep -c 'Priced::' corpus/28-catalogue.becki
0
```

The alternative was to publish `def Priced::pence@Item(self: Item) -> Int` — a line no parser could
read back, describing a function nobody can name, without the trait that gives it meaning. Refusing
is the honest option and it is one line in the report rather than a silently broken interface.

This is the same shape as [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.4's limit
on effect polymorphism across a boundary: a real restriction, stated, with the diagnostic that says
so.

## 37.7 What is still not

- **No bounds, so no parametric use.** `def f[T: Priced](x: T)` is not writable. A generic body
  calling `x.pence()` is `B0386` — and the diagnostic distinguishes "this receiver is a type
  parameter, and there are no bounds" from "this type has no impl", because they are different
  problems with different fixes. Dictionary passing is what closes it, and it needs the call site to
  resolve `T` before it can supply the dictionary; building that badly would be worse than not
  building it.
- **A trait method cannot be passed as a value.** `map_list(ps, show)` is refused for the same
  reason: which implementation `show` means is decided by a receiver that a value has not got yet.
- **No `+` through a trait**, so [`32`](32-numeric-tower-and-polymorphism-report.md) §32.3's ad-hoc
  numeric resolution is untouched and `sicp/refusals/rational.beck` still stands. Operators would
  need a `Num`-shaped trait *and* bounds, in that order.
- **No `@derive`.** [`11`](11-language-tour.md) §11.3 writes `@derive(Eq, Hash, Json)`, and a
  decorator that writes an impl from a declaration's AST is a macro job on top of this one. The
  impls it would write are now writable by hand, which is the precondition.
- **No default methods, no supertraits, no associated types or constants.** Each is refused by name
  where a program reaches for it, rather than mis-parsed.
- **A trait cannot be generic in anything but `Self`.** `trait Convert[T]` is not writable; a trait
  method may not take type parameters of its own either.
- **`check/mod.rs` is 3,169 lines**, up from 3,074 — the split [`36`](36-parameterised-types-report.md)
  §36.9 made is holding, and this feature went into `check/traits.rs` (930 lines including its
  tests) rather than into the file everybody complains about. That is the first time in this
  project's history that a type-system feature did not make `check.rs` bigger than the last one
  found it.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9, [`31`](31-tail-calls-report.md) §31.7,
  [`32`](32-numeric-tower-and-polymorphism-report.md) §32.9,
  [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.7 and
  [`36`](36-parameterised-types-report.md) §36.10 list is unchanged: no LLVM backend, no native
  codegen, no Mode B, no client polish, no `test --update`, no structured concurrency, no
  `Result`/error rows, no SQLite substrate, no standard library v1, no identity beyond a dev-mode
  actor, no LSP, no playground, no supply-chain tooling, no SQL read models, no pgwire, no query
  fusion. Patterns are still one level deep and a `list[T]` is still `O(n)` to take apart.

## 37.8 What this changes for the rest of Phase 3

1. **The standard-library bullet has its third and last precondition.**
   [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.8 found the first — a Beck library
   could not be effect-polymorphic — and [`36`](36-parameterised-types-report.md) §36.11 the second
   — a container could not be written at all. The third is that a library needs to say what its
   elements can *do*, and that is a trait. It is not sufficient yet, because a standard library is
   full of `def sort[T: Ord](xs: list[T])` and bounds are exactly what is missing; but the
   declaration half exists, and what remains is one named piece of work rather than a design.
2. **Bounds are now the single item everything else is waiting behind.** `rational.beck`, the
   numeric tower, `@derive`, a `sort` anybody can call, and a trait that crosses a `.becki` all need
   the same thing: a type parameter that carries a constraint, and a call site that supplies the
   implementation. That is one feature, and it is the next one.
3. **A construct that is desugared costs nothing downstream, and that is a repeatable trick.** Not
   one line of the evaluator, the splitter, the plan, the incremental engine or the runtime changed
   for this. `impl` joins the list of forms that exist in the surface and not in the IR. Anything
   that can be expressed as "a definition with a name a user could not write" should be built that
   way first, and the parts that genuinely need a new IR node should have to prove it.
4. **Two error codes have now been retired on this branch** — `B0309` because an alias may be
   parameterised, `B0306` because traits are checked. Both were entries describing a limitation
   rather than a mistake, and both were removed by the gate in `beck-cli/tests/docs.rs` refusing to
   let an index entry outlive its code. A diagnostic that documents an absence is a to-do item with
   a number, and the gate turns finishing the work into a required edit.
