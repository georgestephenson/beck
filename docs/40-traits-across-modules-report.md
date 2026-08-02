# 40 — Phase 3 report, part 12: a trait that crosses a module

[`39`](39-bounds-report.md) §39.6 said the gap had moved:

> This was a footnote in [`37`](37-traits-report.md) and it is not one now. Before bounds, a trait
> was an internal convenience; with bounds it is how a library says what it needs, and a library
> that cannot publish `def largest[T: Ord_]` cannot publish the interesting half of itself.

It publishes it now. A `.becki` carries the trait, the `impl` headers and the bounds:

```
trait Labelled:
    def label(self) -> Str

impl Labelled for Todo

@on(any)
def labels[T: Labelled](xs: list[T]) -> list[Str]
```

An importing module calls `t.label()` on a `Todo` it imported, calls `labels(...)` and never sees an
implementation; and it may write `impl Labelled for Summary` for a type of its own, because the
orphan rule's "owns the trait **or** the type" is a rule about modules and finally has two of them
to be about. `corpus/project/` does all three, across the three modules it already had.

**What crosses is a header, not a body.** The implementation stays where it was written. What an
importing module receives is that the impl exists and what it promises, which is exactly what a call
site needs to resolve, and the signatures it will call through are *derived* from the trait and the
impl target rather than published as names — `Priced::pence@Item` is a compiler-generated name and
no parser could read it back.

**Found while building it: a diagnostic that pointed at the wrong file.** `Interface::parse` made a
`SourceMap` of its own and dropped it, so every diagnostic about a `.becki` carried a `FileId` the
caller then resolved against a *different* map. An error in `lib.becki` was rendered against
`app.beck`, confidently, at whatever line the offsets happened to land on. §40.5.

553 tests, no failures, no compiler warnings, no clippy warnings — up from 548. One new error code,
`B0316`, for the loose end [`39`](39-bounds-report.md) §39.7 named: a bound on a *declaration* used
to be parsed and silently ignored.

`sicp/refusals/rational.beck` still stands. This report does not touch the operator decision
[`39`](39-bounds-report.md) §39.7 wrote out, and that decision is now the only thing left in the
directory.

## 40.1 What was asked, and what is answered

| asked for | status | where |
|---|---|---|
| A `trait` in a `.becki` | done | §40.2 |
| An `impl` header in a `.becki` | done — new syntax, and only there | §40.2 |
| A bounded `def` published with its bound | done | §40.3 |
| An importing module calling a trait method | done | §40.3 |
| An importing module implementing an imported trait | done — the orphan rule, across modules | §40.4 |
| `--wire-compat` rules for a trait, a method and an impl | done, with the asymmetry that matters | §40.6 |
| A bound on a `model`/`union` refused rather than ignored | done — `B0316` | §40.7 |
| `+` through a trait, so §2.1.1's rationals compile | **not done**, and the decision is still open | §40.7 |

## 40.2 A bodyless impl, which is what a header is

A `def` in a `.becki` has no body, and that is what makes it a signature (§3.6). An `impl` now has
the same two forms for the same reason:

```
impl Priced for Item:          # a module: the bodies
    def pence(self):
        return self.pence

impl Priced for Item           # a .becki: that it exists
```

Each is refused where the other belongs — an impl with methods in an interface, and an impl without
them in a module — with the note saying which file it belongs in. That mirrors `B0335`'s "a bodyless
`def` is a declaration, which is what a `.becki` interface file is made of", and it means the two
shapes cannot drift into meaning the same thing.

A trait crosses **as types, not as syntax**. The checker keeps a trait's methods as `Node`s while it
is desugaring impls and bounds, because splicing is what that pass does; `Interface` carries the
same declaration as a `TraitSig` of `Ty`s. A trait that crossed as syntax would carry spans into a
file that does not own them, which is the defect §40.5 is about, arriving by a second route.

Going back the other way needs the inverse of `ty_from_node`, so that an imported trait can be
spliced exactly as a local one is. That is `ty_to_node`, twenty lines, and it is why an impl of an
imported trait needs no special case anywhere: by the time `expand_impl` sees the trait, it cannot
tell where it came from.

## 40.3 What a bounded definition publishes

Not its dictionaries. `def labels[T: Labelled](xs: list[T]) -> list[Str]` was lowered to a
two-parameter function ([`39`](39-bounds-report.md) §39.3), and what crosses is the **bound**:

```
@on(any)
def labels[T: Labelled](xs: list[T]) -> list[Str]
```

The importing module rebuilds the dictionary parameter from the same bound, by the same rule, in the
same order — `import_bounded` is the mirror of `expand_bounds`, working from types rather than from
syntax because an imported name arrives as a scheme. If the two ever disagreed the call would be
built with the wrong arity, so they are written next to each other and the round-trip test is what
holds them together.

The published row of an imported impl method is the **trait's** declared row rather than the
implementation's inferred one. That is an over-approximation and it is the safe direction: §37.5
already made the trait's row the bound every impl is held to, so a caller in another module may
assume exactly what a caller in the same module may assume, and no more.

## 40.4 The orphan rule, with two modules to be about

[`37`](37-traits-report.md) §37.4 wrote the rule and could not demonstrate it: in a single module,
"the trait or the type is declared here" is always true. `corpus/project/` now has the real shape —
`domain.beck` declares `Labelled` and implements it for its own `Todo`; `app.beck` imports the trait
and implements it for `Summary`, which is `app`'s. Neither module can be the one that disagrees,
because neither could have written the other's impl.

What the rule still refuses, and now refuses across a boundary rather than hypothetically: an impl
where **both** halves are imported. That is the case where two modules could each supply one, and
coherence has no way to choose.

## 40.5 A diagnostic that pointed at the wrong file

Found by breaking a `.becki` on purpose while testing the import path:

```
error[B0310]: cannot find type `NoSuchType`
  --> app.beck:23:20
   |
23 | test "a trait method resolves through an imported impl":
   |                    ^^^^^^^^^^
```

The error is real and it is in `lib.becki`. The span is not: `Interface::parse` built a `SourceMap`,
added the interface to it, pushed diagnostics carrying that map's `FileId`s into the caller's
`Diagnostics`, and dropped the map. The caller then rendered them against its own map, where the
same `FileId` is a different file. The result is a caret under whatever text happened to be at that
offset in an unrelated module — worse than no span, because it is confident.

`Interface::parse` takes the caller's `SourceMap` now, which is the discipline `project.rs` already
had a comment about: *"the caller's `SourceMap` is what every module is added to, because a
diagnostic about the third module has to point into the third module's source."* One function was
not doing it. The same error now reads:

```
error[B0310]: cannot find type `NoSuchType`
  --> lib.becki:26:15
```

It is a pre-existing defect and it predates traits by two phases. It surfaced now because a `.becki`
has more in it that can be wrong.

## 40.6 What `--wire-compat` says

A trait is not something either side writes onto a wire — it is what a call site resolves against —
so the command/event asymmetry this file exists for does not apply. Removing anything an importer
might have named is breaking; adding a trait or an impl is not.

With one exception, and it is the interesting one:

| change | verdict | why |
|---|---|---|
| trait removed | **breaking** | an importing module's calls resolve against it, and a bound naming it stops type-checking |
| method removed, or its signature changed | **breaking** | the caller and the implementation agree on that signature and nothing else |
| **method added** | **breaking** | every impl of the trait is now incomplete — including impls in modules this release cannot see |
| impl removed | **breaking** | a call resolved to it, and coherence means there is no second one to fall back on |
| trait or impl added | compatible | nothing in the previous release could name it |

"Method added is breaking" is the same shape as "event variant added is breaking", and for the same
kind of reason: the thing that breaks is not a caller but an *exhaustive* obligation somewhere else.
§3.1's exhaustiveness does it for a `union`; §37.4's completeness check does it for a trait.

## 40.7 What is still not

- **`+` does not go through a trait.** [`39`](39-bounds-report.md) §39.7 wrote out the three options
  and this report deliberately does not pick one. It is now the *only* thing in
  `sicp/refusals/`, and the only thing standing between the suite and §2.1.1.
- **A trait may not be generic in anything but `Self`**, a method may not take type parameters of
  its own, and there are no supertraits, no default methods, no associated types. Unchanged from
  [`37`](37-traits-report.md) §37.7, each refused by name.
- **No `@derive`.** Every precondition it has is now met — the impls it would write are writable, the
  bound it would satisfy is expressible, and both cross a boundary — so what is left is a macro that
  reads a declaration's AST and writes an impl. It is the most tractable remaining item in this
  family.
- **A trait's methods are registered as global names**, so a trait method and a top-level definition
  cannot share a name across an import either. Refused, not silent, but it is a flat namespace and a
  large enough program will notice.
- **The choice is still not deferred** ([`39`](39-bounds-report.md) §39.5) and a bounded definition
  still cannot be passed as a value.
- **`check/mod.rs` is 3,260 lines** and `check/traits.rs` is 1,668 including its tests. Three
  type-system features in a row have gone next door; `mod.rs` has grown by 91 lines across all three,
  against 1,668 beside it.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9, [`31`](31-tail-calls-report.md) §31.7,
  [`32`](32-numeric-tower-and-polymorphism-report.md) §32.9,
  [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.7,
  [`36`](36-parameterised-types-report.md) §36.10, [`37`](37-traits-report.md) §37.7 and
  [`39`](39-bounds-report.md) §39.7 list is unchanged: no LLVM backend, no native codegen, no Mode B,
  no client polish, no `test --update`, no structured concurrency, no `Result`/error rows, no SQLite
  substrate, no standard library v1, no identity beyond a dev-mode actor, no LSP, no playground, no
  supply-chain tooling, no SQL read models, no pgwire, no query fusion. Patterns are still one level
  deep and a `list[T]` is still `O(n)` to take apart.

## 40.8 What this changes for the rest of Phase 3

1. **The standard-library bullet has no remaining blocker.** Four were named across four reports —
   effect polymorphism for a user's definition, a container that can be declared, a way to say what
   its elements can do, and a boundary all three can cross. `def sort[T: Ord](xs: list[T])` is
   writable, publishable, and callable from another module. What is left is *writing* one, which is
   work rather than design, and it is the first time that has been true.
2. **`@derive` is now the cheapest large win.** [`11`](11-language-tour.md) §11.3 has written
   `@derive(Eq, Hash, Json)` since before any of this existed. Everything under it is built; the
   decorator is a macro over a declaration's AST, and the impls it writes are ordinary ones.
3. **A `.becki` is now big enough to be worth diffing by hand.** It carries types, traits, impl
   headers, bounds, effect rows and placement. `--wire-compat` classifies changes to all of it, and
   §40.5 is the reminder that the file has to be able to *report* on itself properly — an interface
   nobody can debug is an interface people regenerate blindly.
4. **The desugaring rule has now paid three times.** Traits, bounds and the boundary crossing added
   no IR node, no evaluator case and no runtime change between them. What made the third one cheap
   was the second one's naming choice, and what made the second one cheap was the first's. That is
   worth noticing as a property of the approach rather than three separate strokes of luck.
