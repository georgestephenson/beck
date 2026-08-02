# 27 — Phase 3 report, part 5: three of the six walls

[`25`](25-benchmarks-and-expressiveness.md) §25.6 measured six walls between Beck and the rest of
SICP, and §25.7 put them in dependency order. This is the first three, built:

| §25.7 | wall | status |
|---|---|---|
| 1 | **A library cannot be run.** Every SICP solution is a library, and so is every domain module a project would most want unit tests for | **built** — §27.4 |
| 2 | **A type cannot mention itself, or anything declared later.** "the single highest-value item on the list for reasons that have nothing to do with SICP" | **built** — §27.3 |
| 3 | **The `B0320` defect** — an `if` over two function values refused when one is a call's result | **built** — §27.2 |
| 4 | Proper tail calls | still standing |
| 5 | The numeric tower | still standing |
| 6 | User-written polymorphism | still standing |

Three things came out of it that were not on anybody's list. `sicp/ch1.beck`'s five-declaration
wrapper — the one §25.6 item 1 left "in view rather than tidied away" — **is gone**, and the chapter
is now what it always should have been. [`sicp/ch2.beck`](../compiler/sicp/ch2.beck) exists, which
needed *both* of the first two fixes and could not have been written for either alone.
And [`corpus/25-thread.beck`](../compiler/corpus/25-thread.beck) is the twenty-eighth corpus program
— a comment thread, which is the first program in this project to describe a tree.

473 tests, no failures, no compiler warnings, no clippy warnings — up from
[`26`](26-arrangement-sharing-report.md)'s 466.

## 27.1 What "the suite found it" is worth, measured twice

[`25`](25-benchmarks-and-expressiveness.md) §25.6 closed with a claim about method:

> Twenty-six corpus programs, a differential harness, a replay harness and 396 tests did not surface
> it, because every one of them is a program shaped like the todo sketch. **Ninety minutes of chapter
> 1 did**, because SICP is relentlessly about the one thing this compiler has never been pointed at:
> building abstractions out of procedures.

That was one data point and it was about finding a defect. This report is a second one, and it is
about a different property: **the walls were in dependency order, and the order was right.** Item 3
took an afternoon and is thirty lines. Item 2 took a pass split into three and unblocked a whole
chapter. Item 1 turned out to be the one that changes what the tool is *for* — a compiler that can
only test applications is a compiler that cannot test the modules an application is made of — and it
was listed first because it was smallest, which is not the same reason.

The suite also did the thing a benchmark is supposed to do and is usually too polite to: two of the
three fixes below are held to account by tests that were **turned round rather than deleted**. A wall
that came down is a capability that can regress, and the shape it would regress in is the shape it
had.

## 27.2 The `if` that was typed as though one branch were the other's expectation

[`25`](25-benchmarks-and-expressiveness.md) §25.6 item 6 diagnosed this from the message alone, and
the diagnosis was exactly right:

> `{}` is the empty effect row, printed where a row variable was expected — the branches' rows appear
> to be unified against each other rather than each against a fresh variable, so a call's polymorphic
> row meets a literal's concrete empty one and the join is reported as a conflict.

The line was `self.unify(&alt.ty, &then.ty, then.span, "the two branches")`, and everything wrong
with it is in the argument order. [`Subst::unify`] is **asymmetric on purpose**: its first argument
is the actual type and its second the expected one, and `subsume_row` leans on that so a function
which does less than its context allows is accepted — which is what lets a pure `lambda t: t.done`
be passed where `(a -> b ! e)` is wanted. That asymmetry is right everywhere it was designed for and
wrong here, because **the branches of an `if` are not actual-and-expected**. Making one of them the
standard the other must meet says that a branch returning `identity` — inferred pure, so its row is
closed — is what a branch returning `compose(f, f)` has to match, and a row variable is not a subset
of the empty row.

`Subst::unify_join` is the replacement, and it is the answer every row-typed language reaches: the
alternatives do not meet each other, they both flow into a **fresh row**, and the result performs
whatever either of them might. Structurally it is `unify` with two changes — a `Fun`'s row becomes a
fresh variable both sides subsume into, and a `Con`'s arguments are joined rather than unified, so a
`list[Int -> Int]` gets the same treatment one level down. Parameters are left to ordinary
unification, because they are contravariant and joining them would be unsound in the other
direction.

Two things about that are worth stating rather than assuming.

**It is sound in the direction §3.2 requires.** The joined row contains both branches' atoms, so an
effect can never be lost — and `joining_two_branches_keeps_the_effects_of_both` asserts that on the
number every downstream pass actually reads: a `pick` whose branches are a pure call and a `nondet`
one has `nondet` in its *inferred* row. Losing it would have bought exercise 1.43 by breaking
placement, `beck iface` and §3.5's proofs, silently.

**`match` never had the defect**, and finding out why was the useful part. It types its arms against
a `result` type taken from the expected type when there is one — so the arms meet the *declared
return type*, whose row `ty_from_node` made a variable, and the variable absorbs both. The `if` had
the same `expected` in hand and ignored it in favour of the other branch. So this was not a missing
theory; it was one expression form not using the machinery the one beside it already did.

The second half is the diagnostic. `Mismatch::Effects("")` rendered as `may not perform {}`, which
[`04`](04-compiler-architecture.md) §4.5 fails on its own terms — no user can act on it. Naming what
is missing is only possible when something is, so the case where the *actual* side's extra is a row
variable is now its own variant with its own sentence: "may perform effects this context does not
allow: one side's effects are not decided here, and the other's are fixed and empty."

Exercise 1.43 is back in [`sicp/ch1.beck`](../compiler/sicp/ch1.beck), asserted against the book's
answers, and `sicp/refusals/higher-order.beck` is deleted.

## 27.3 A type may now mention itself, and anything declared later

[`25`](25-benchmarks-and-expressiveness.md) §25.6 item 2:

> `collect_types` resolves each declaration's field types as it walks the file in source order, so
> `union Tree: Node(left: Tree, …)` is `B0310: cannot find type 'Tree'`, and so is a plain forward
> reference between two models.

The fix is the one `collect_signatures` already made one layer up — its own comment says "register
every top-level `def`'s signature before checking any body, so definitions may refer to each other in
any order" — applied to declarations. Three passes where there was one:

1. **`declare_type_names`** registers every model, union and newtype under a placeholder, so
   `ty_from_node`'s existence check passes while the real declaration is still being built. A
   placeholder is never observed: pass 3 overwrites every one of them.
2. **`collect_aliases`** resolves `type` aliases, on demand and in dependency order.
3. **`collect_types`** resolves every declaration's field types against the complete set of names.

Aliases are the reason this is three passes and not two, and the reason is worth writing down because
it is the one place the design has a genuine asymmetry in it. An alias is **transparent** —
`ty_from_node` replaces it with its target the moment it sees one — so a placeholder alias would
expand every mention of it to the placeholder's target. It has to be resolved before anything that
mentions it, and "before" cannot mean source order or the pass has achieved nothing. So each alias
resolves the aliases it names first, recursively.

### "Three passes" is a count of stages, not a budget

Worth saying plainly, because the sentence invites the other reading: three is how many times the
compiler sweeps the declaration list, not how far a chain or a ring may reach. Nothing in the design
bounds depth, and the pass that could plausibly have bounded it is the one that recurses — a
forty-link alias chain declared so that every link is a forward reference resolves, and so do six
mutually recursive declarations in none of the orders a single source-order pass could have taken.
`the_three_passes_are_stages_and_not_a_depth_limit` is both.

What *is* bounded is a recursive **value**, and by the evaluator rather than by the checker. A `Tree`
spine 500 deep is an ordinary value in a release build and 50,000 aborts the process — which is wall
4 (§25.6 item 5) reached from a different direction than §1.2.1's iterative process, and reached by a
program a user could plausibly write. `what_bounds_a_recursive_types_depth_is_the_evaluator_and_not_the_checker`
asserts both ends, at 100 rather than 500, because the depth that fits moves with the build profile —
and *that* it moves with the build profile is the finding, not the number.

That makes cycles reachable, and cycles are where a union and an alias part company: **a union may be
recursive and an alias may not.** A variant is a finite tag plus fields, so `union Chain: End,
Link(next: Chain)` is a perfectly ordinary type of unbounded depth; an alias has no such boundary, so
`type Chain = list[Chain]` is an infinitely large type and `type A = B; type B = A` is no type at
all. Both are refused by name, with `B0312`, and
`an_alias_that_is_defined_in_terms_of_itself_is_still_refused` is what stops the fix turning a
source-order limitation into a hang.

### Compiling one is the easy half

The checker was, as §25.6 predicted, "the only wall" — `cost.rs`, `gen.rs` and `secure.rs` already
cut recursive-type cycles and have tests named for it. But *predicted* is not *measured*, and a
corpus program is carried through placement, effect inference, the slicer, the plan, the incremental
engine against its recompute oracle, replay determinism, `Repr`, the value generator behind
`property` blocks, the cost model, `Sendable` and `beck iface`. Every one of those walks a type.

So [`corpus/25-thread.beck`](../compiler/corpus/25-thread.beck) is a comment thread, and it is in the
**corpus** rather than in `sicp/` for that reason. It carries four recursive shapes on purpose:

* a union that mentions itself through a list (`Node.replies: list[Comment]`);
* a model and a union that mention each other, so there is a forward reference in one direction
  however the file is ordered — reordering cannot fix it;
* a type used by a declaration above it, which is the plain forward reference;
* recursive functions over the recursive type, in the fold **and in the view**. The view's matters
  separately: `plan.rs` decomposes a view by inlining definitions, and this is the program that makes
  its recursion guard load-bearing rather than theoretical.

It compiles, places itself with no annotations, folds, renders, slices, and its five `test` and
`property` blocks pass — the property one over 100 generated trees, which is `gen.rs` walking a
recursive union a hundred times. Nothing hung.

The one thing this does **not** yet buy is the thing §2.2 is mostly about: SICP asks the reader to
*build* `map`, `filter` and `accumulate` over trees, and a user still cannot write a polymorphic
definition (§25.6 item 4). So `sicp/ch2.beck`'s `tree_map` takes `Int -> Int` where the book's takes
any procedure. That is a real loss and the file says so at the top rather than in a footnote.

## 27.4 A library runs its own tests

This is §25.7's first item and it is the one that changes what the tool is for.

[`22`](22-phase-3-report.md) §22.6 recorded it as "a real gap for exactly the modules that most want
unit tests", and [`25`](25-benchmarks-and-expressiveness.md) §25.6 item 1 made it unignorable by
putting a five-declaration application into chapter 1 that nothing in the chapter used, "left in view
rather than tidied away". The wrapper is the artefact that was the argument, so removing the wrapper
is how the argument is answered.

`beck check` has said "a library" since Phase 2 — `NOT_AN_APPLICATION` is a named constant listing
the three diagnostics that mean "this module is not an application" rather than "this module is
wrong". What was missing was a way to hand that module back to a caller: `slice` returns
`Option<Placed>` and a library got `None`.

`project::slice_or_library` is the new entry point, and it is a *sibling* of `slice` rather than a
replacement, because "this compiled" and "this is an application" are different questions and every
existing caller is asking the second. On the library path the B0500/B0501/B0505 diagnostics are
**dropped** rather than downgraded to warnings — they are answers to a question this caller did not
ask — and every other diagnostic is kept, so a library with a type error is still a broken module.

`Placed::library` synthesises the four roles, and the interesting decision is that they are chosen to
be **inert rather than plausible**: the fold returns its accumulator unchanged, the view renders
nothing, the initial state is unit. Nothing should ever call them, and `Placed::kind` is the flag
that says so. A placeholder role is a lie a caller must not be able to tell by accident.

Which is why the second half of this work is a refusal. A library has no log to fold, no `validate`
to propose through and no page to render, so a test clause that needs one is named and refused:

```
test "page" … FAILED
  `page` is the view of an application, and a library has none. This module has no merge point,
  so it is a library: add `proposals: Stream[Proposal] = merge_clients()` and a `durable` fold to
  make it an application, or write this test over the module's own definitions
```

`given`, `when` and `fold_of` never reach the runner at all — `B0706` already refused them while
checking, with a note that says exactly why — so `page` is the one that needed the new guard.
`a_library_test_that_needs_an_application_is_refused_by_name` asserts that the pure test in the same
file still passes and only the one that needs an application fails, because "everything failed" and
"the right thing failed" are different outcomes and only one of them is this.

One incidental panic was fixed on the way, and it is the sort worth recording. `Plan::compile` found
the page vertex with `graph.by_name.get(…).unwrap_or(0)` — harmless for an application, and an index
into vertex zero of a graph with no vertices for a library. A library's plan is now its two sources
and a unit.

### What the wrapper's removal is evidence of

`sicp/ch1.beck` is 33 lines shorter net — 53 removed against 20 added, and the 20 are exercise 1.43
and its test. What went is the five declarations and the twenty lines of comment explaining why they
were there. `a_library_runs_its_own_tests_with_no_application_anywhere_near_it` asserts the
absence rather than the presence — that the chapter no longer contains `merge_clients` or a `model
State` — because a `beck test` that merely *tolerated* libraries would have left the wrapper in place
and passed a test that only checked the exit code.

It also asserts that `beck check` still calls chapter 1 a library, because it is one. Running a
library's tests is not the same claim as a library being an application, and nothing here should have
blurred the two.

## 27.5 What is measured, and how to reproduce it

```console
$ cd compiler && cargo test --workspace          # 473 tests
$ cargo test --release --test sicp               # the suite and the walls
$ ./target/release/beck test sicp/ch1.beck       # 14 passed  (13, plus exercise 1.43)
$ ./target/release/beck test sicp/ch2.beck       # 6 passed   (new)
$ ./target/release/beck test corpus/25-thread.beck   # 5 passed, one over 100 generated trees
```

CI runs both chapters through the binary, the way a person does, beside the corpus and the sketch —
which is a different claim from `cargo test`, and the reason the workflow already ran chapter 1 that
way.

## 27.6 The corrections this makes to the design documents

| Document | Correction |
|---|---|
| [`25`](25-benchmarks-and-expressiveness.md) §25.6 items 1, 2 and 6 | Three of the six walls are down. `sicp/refusals/` has three files rather than six, and the tests that asserted those three walls were **turned round** into tests that assert the capability, rather than deleted |
| [`25`](25-benchmarks-and-expressiveness.md) §25.6 item 1's wrapper | Gone. The paragraph in `ch1.beck` that explained it is gone with it, and the harness now asserts its absence |
| [`25`](25-benchmarks-and-expressiveness.md) §25.6 item 2 | "the checker's collection order is the only wall" — measured rather than predicted, by putting a recursive type through every corpus harness (§27.3). It held |
| [`22`](22-phase-3-report.md) §22.6 | "a library module … cannot have its tests run" — no longer true (§27.4) |
| [`03`](03-type-and-effect-system.md) §3.2 | The branches of an `if` join their rows rather than one subsuming the other. Row subsumption is still the only subtyping in the language; what changed is that alternatives are not an actual-and-expected pair |
| [`04`](04-compiler-architecture.md) §4.5 | `may not perform {}` was a diagnostic no user could act on, and §4.5's contract is that a diagnostic is actionable. The case that produced it now has a sentence |
| [`08`](08-roadmap.md) | Phase 3's standard-library bullet said "Reals first, because §25.6"; the three items above it in §25.7's order are done, so reals are next on that list rather than fourth |

## 27.7 What is still not

- **Three walls still stand**, and they are the three §25.7 put last because they are the largest:
  proper tail calls (a Beck program can still kill its process with a recursion a user cannot bound,
  which is a runtime robustness question and not only a SICP one), the numeric tower, and
  user-written polymorphism.
- **Chapter 2 stops at §2.2**, and §2.2 is not complete: the exercises that ask the reader to *build*
  `map` and `accumulate` are the ones polymorphism blocks, so what runs is the tree half of the
  section. §2.1's rationals need the numeric tower. Chapters 3–5 are untouched.
- **A library still cannot be `beck run`.** That is correct — there is nothing to run — but the
  asymmetry is now visible in a way it was not: `beck test` and `beck check` accept a module `beck
  run`, `beck build` and `beck up` refuse, and nothing states that contract in one place.
- **`check.rs` grew for the first time in four reports**, from 2,806 lines to 3,012. §22.6's request
  to move the test-checking pass out of it is still not done, and this added two passes rather than
  moving one.
- **No recursive type appears in a `.becki`-crossing position under test.** `beck iface` publishes
  `corpus/25-thread.beck`'s types and the corpus harness compares them, but no *project* test imports
  a module whose published type is recursive, so separate compilation over recursive types is
  compiled-and-believed rather than measured.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9 lists is unchanged: no LLVM backend, no
  Mode B, no client polish, no `test --update`, no structured concurrency, no `Result`/error rows, no
  SQLite substrate, no standard library v1, no identity beyond a dev-mode actor, no LSP, no
  playground, no supply-chain tooling. Nine Phase 3 bullets, untouched.

## 27.8 What this changes for the rest of Phase 3

1. **The unit of testing is now a module, not an application.** Every Phase 3 bullet below this one —
   the standard library, `Result`/error rows, structured concurrency — is a *library* concern, and
   until now none of them could have had a test written in Beck about itself. That is a precondition
   nobody had listed, and it was one item away.
2. **Recursive types are what the remaining walls are measured against.** Chapters 4 and 5 of SICP are
   a metacircular evaluator and a register machine, and both are recursive types over a recursive
   type. They are no longer blocked by the checker; they are blocked by polymorphism and the numeric
   tower, which is a sharper statement than §25.6 could make.
3. **A benchmark that names its walls one at a time gets them removed one at a time.** Three of six
   in one change, each with a test that was turned round rather than deleted, is the strongest
   argument in this report for the discipline [`25`](25-benchmarks-and-expressiveness.md) proposed —
   and the cheapest thing to keep doing.
