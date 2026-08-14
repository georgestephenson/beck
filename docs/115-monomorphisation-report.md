# 115 — A generic definition compiles, once per type it is used at

**Built.** A definition with type parameters compiles, to **both** code generators, as one function
per instantiation. It was the largest single refusal class left — 63 of 223 — and the mechanism
turned out to be a **pass over the program** rather than anything in either emitter.

The refusal was true as stated: *"generic over `T` — a type parameter has no machine representation
here."* There is no representation for `T`, because this backend fixes a layout at emit time and `T`
is exactly the thing that is not fixed. What the refusal did not say is that the *program* does not
need one: every call site says what `T` was, and by the time a backend sees the program it has been
saying so all along.

**Across the tree, 850 → 870 definitions compile and refusals go 223 → 208.** The refusals that
blame a type parameter go **63 → 38**, and §115.6 is honest about why the second number is not zero.

---

## 115.1 The thing that was already recorded

There is no type-argument list in `Core`, no instantiation table on `Program`, and no substitution
that survives checking. Looking for one is what makes this sound like a compiler project.

**Every `Core` node carries its solved type.** A call to a generic definition is
`App { func: Global(name), … }`; inference instantiates the callee's scheme with fresh variables and
writes the result onto that `Global` node; unification then decides them against the arguments; and
`resolve_types` grounds every node in the module at the end of checking. So the `Global` node's type
is the *instantiated* function type, while the definition's own `params` and `ret` still name the
rigid `Con("T", [])` that checking minted for the parameter.

| | `firstly`'s declaration | the node at `of_ints`'s call |
|---|---|---|
| parameters | `list[T]`, `T` | `list[Int]`, `Int` |
| result | `T` | `Int` |

Walking those two together, one structure at a time, reads `T := Int` straight off. That is
`mono::recover`, it is thirty lines, and it is the whole mechanism. The checker had been carrying
the answer since [`27`](27-the-walls-come-down-report.md) gave the language generics, and nothing had
read it.

## 115.2 Why it is a backend pass, and why that was not this report's decision to make

[`38`](38-literature-survey.md) §38.1 had already settled it: **dictionaries are the semantics and
monomorphisation is a backend choice**, on the grounds that whole-program specialisation fights
incrementality — the property [`24`](24-incremental-views-report.md) built the engine for. So this
runs inside the native backends, on a `Program` clone. `beck-core` does not change, the checker does
not change, the evaluator still executes one generic definition uniformly, and `beck test`,
`beck run` and `beck up` cannot tell the pass exists.

It is **shared** between the two emitters, for the reason [`101`](101-the-heap-report.md)'s heap is:
it is not a code generator, it is the program both of them are handed.
[`97`](97-cranelift-report.md) §97.3's argument for writing the emitters twice is an argument about
*emitters*; writing this twice would mean the two backends compiled two different subsets, which is
the opposite of what the differential is for.

## 115.3 What one instantiation is

The template's body is cloned and its types are substituted; nothing else changes. `last_use`,
`order` and `locals` — the answers [`70`](70-the-evaluator-gets-fast-report.md)'s liveness,
`fields` and `frames` computed — are facts about the tree's *shape*, and specialising changes no
shape at all, so none of those passes runs again.

The name is `firstly@Int`, on the separator `Trait::method@Target` already uses and that no source
name can contain. Two things about it are load-bearing:

- **It is keyed on the whole type, not on the head constructor.** `dictionary` in
  [`traits.rs`](../compiler/crates/beck-core/src/check/traits.rs) deliberately keys impls on the head
  so `Tree[Int]` and `Tree[Str]` share one — right for a dictionary, wrong here, where they are two
  layouts.
- **It is keyed on the type and not on the representation.** `Int` and `Bool` are both one immediate
  word; a backend that merged them would answer `1` where the evaluator answers `true`, and
  `genfix`'s `of_ints` beside `of_bools` is that case as a program.

Two type parameters are read off in **use** order, not declaration order, which is what makes
`swapped[Str, Int]` — whose body is `paired(b, a)` — call `paired@Int,Str`: *the same instantiation*
`int_then_text` asks for directly. The differential asserts that sharing rather than the count,
because a positional recovery that read the arguments off the declaration would mint a second
function and still compute the right answers.

## 115.4 What it refuses, and why each refusal is a real one

- **Polymorphic recursion.** `def f[T](x: T)` may call `f` at `list[T]`, whose instantiation calls it
  at `list[list[T]]`. The program is finite and the set of instantiations is not. `MAX_INSTANTIATIONS`
  is 64; across the corpus, both benchmark suites, both SICP chapters, the examples and the standard
  library there are **65 templates and 28 instantiations**, and the most any one definition has is
  **three** — so the budget is about twenty-one times the largest real one and is a bound rather than
  a policy.
- **A call where nothing decides the type.** `list_len(anything())` pins `T` against `list_len`'s own
  parameter and no further, so inference finishes with a *variable*. The program is legal and the
  evaluator runs it happily. Minting `anything@?3` would have compiled — and would have made a symbol
  a function of an inference counter rather than of the program, which is a determinism defect
  wearing a feature's clothes. It refuses instead.
- **A bounded definition** is not a template here at all. `expand_bounds` already turned its bounds
  into value parameters holding function values, so specialising one would leave a closure in a
  signature — [`108`](108-closures-arrive-report.md)'s refusal, with [`108`](108-closures-arrive-report.md)'s
  fix, and not something this pass should appear to have answered.

## 115.5 The finding: a partial answer was worse than none

The first version of this pass gave up **part way through**. On the polymorphically recursive
program it built sixty-four instantiations, discovered it could not finish, and stopped — leaving
`growing@Int` refusing because it calls `growing@list[Int]`, which refuses because it calls
`growing@list[list[Int]]`, sixty-four times. Every one of those refusals was true. Together they
said nothing: a reader looking for which definition to fix had sixty-four names, none of which they
had written.

So a round that keeps a template it had been specialising is **thrown away** and re-run with that
template forbidden from the start, which leaves exactly one refusal naming exactly one definition —
the one in the source. `kept` only grows, so it terminates in at most one round per template, and it
takes one round for every program in this tree.

That is a shape worth naming, because it is not about generics: *a bounded search that gives up
should give up on the whole thing*. The intermediate results of an abandoned search are individually
correct and collectively misleading, and a refusal a reader cannot act on is a refusal that has
failed at its one job ([`106`](106-lists-arrive-read-only-report.md) §106.7).

## 115.6 What the numbers are, and what they are not

**850 → 870 definitions compile and refusals go 223 → 208** across the corpus, both benchmark
suites, both SICP chapters, the examples and the standard library. Refusals blaming a type parameter
go **63 → 38**, and the 38 need explaining rather than defending:

- **25 of them are `lib/collections.beck`**, measured the way every number in this series is measured
  — each file compiled **alone**. A library's generic definitions have no caller in their own file,
  so nothing asks for an instantiation and the template is kept and refused. That is the right answer
  for the question asked and the wrong impression of the feature.
- **Linked, they specialise.** A program importing `collections` and calling `size(set_of(xs))`
  compiles `set_of@Int`, `set_of@Str`, `size@Int`, `contains@Str`, `insert@Int`, `insert@Str`,
  `empty_set@Int` and `empty_set@Str` — eight instantiations from five templates — and what stays
  refused is the definitions that program does not call.
- A template that produced **nothing** is kept deliberately rather than dropped. It is dead code for
  this backend, and reporting nothing at all about a definition somebody wrote would be worse than
  reporting that this backend was never asked for it at a type.

## 115.7 The gates

- **`native.rs::the_two_backends_agree_on_generics`** and **its Cranelift twin** — 103 and 100 calls
  over `genfix`, which is written around the ways of picking the **wrong instantiation** rather
  than around whether generics work at all: `Int` beside `Bool`, a type argument that is itself a list and
  one nested inside that, two parameters in both orders, a parameter that appears only in the result,
  a parameter that appears twice, a generic calling a generic, ordinary recursion inside a template,
  a generic over a record and over a union, a generic that builds the list it answers with, and a
  generic bound to a name before it is called.
- **The control is by name.** The differential asserts that `firstly@Int`, `firstly@Bool`,
  `firstly@list[list[Int]]`, `paired@Int,Str` and eleven others are among the compiled functions, that
  no template survives, and that `paired` has **one** instantiation and not two — because a run that
  compiled `firstly` once and called it three times would answer correctly on every case and be
  wrong.
- **`a_polymorphically_recursive_definition_is_refused_rather_than_compiled_forever`** asserts the
  refusal *and that there is one of it*, which is §115.5's finding as a test.
- **`a_generic_whose_type_nothing_decides_is_refused_rather_than_guessed`** asserts that no symbol in
  the emitted module is named after an inference variable.
- The Cranelift twin exists separately for a reason worth stating: the two emitters are given the
  same specialised program and are still two independent readings of it, so a specialisation they
  disagree about shows there and nowhere else.

## 115.8 What this corrects

- [`93`](93-llvm-backend-report.md) §93.6's table has a row reading *"Generic and bounded definitions
  | **not built.** A dictionary parameter is a function value, and a function value is a closure."*
  The second half stands and is still the reason a **bounded** definition is refused. The first half
  is now built, and the two were never one item: a generic definition needs no dictionary at all.
- **`beck_llvm`'s module documentation** said a generic definition is still the tree-walker's. It is
  not, unless it is bounded, polymorphically recursive, or called where nothing decides its type.

## 115.9 What this does not establish

- **Nothing about code size.** One function per instantiation is the trade monomorphisation always
  is, and nothing here measures it. Twenty-eight instantiations from sixty-five templates is small
  enough that the question has not arisen; a program that instantiated one definition at forty types
  would make it a real one, and `MAX_INSTANTIATIONS` would stop it at sixty-four for a reason that is
  about termination rather than about size.
- **Nothing about speed.** A specialised call is a direct call where a generic one would have been —
  but there was no generic one, because the whole definition was refused. The comparison this
  replaces is against the *evaluator*, and it is the comparison every report in this series makes.
- **Nothing about bounded generics**, which is the next item and is a different feature: a dictionary
  is a function value, and the fix is at [`108`](108-closures-arrive-report.md)'s boundary rather than
  here.
- **Nothing about incrementality.** [`38`](38-literature-survey.md) §38.1's objection to
  monomorphisation is that it fights incremental compilation, and this pass does not answer it — it
  sidesteps it, by being a whole-program transform in a backend that already recompiles whole
  programs. A native backend that wanted incremental rebuilds would meet that objection intact.
