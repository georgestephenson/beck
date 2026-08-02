# 36 — Phase 3 report, part 9: a type that takes a parameter

[`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.8 item 3 said the refusals directory
was the roadmap. It held two files, and this is one of them:

> **`generic-type.beck`** — `def map[T]` is writable and `union Tree[T]` is not, refused by the
> parser exactly as `def map[T, U]` was before [`32`](32-numeric-tower-and-polymorphism-report.md).

It is writable now. A `model`, a `union`, a `newtype` and a `type` alias each take the same
type-parameter list a `def` has had since [`32`](32-numeric-tower-and-polymorphism-report.md), so
one notation quantifies a definition and a declaration alike:

```
union Tree[T]:
    Leaf(value: T)
    Node(kids: list[Tree[T]])

def tree_map[T, U](t: Tree[T], f: T -> U) -> Tree[U]: ...
```

`sicp/ch2.beck`'s tree is the book's tree now — held at `Int`, at `Str`, and at `Tree[Int]`, with
one `count_leaves` over all three. The refusal file is deleted, and what asserts the wall is gone is
15 passing tests where there were 13.

**Most of the machinery was already there, and had been since Phase 1.** `Option[T]`, `Result[T, E]`
and `Envelope[T]` are parameterised declarations; the checker has instantiated them at every
construction, every field access and every pattern for three phases. What did not exist was a way
for a *user* to declare one — a bracket list on four forms, and a scope to bind it in. §36.3 is the
one genuinely new idea, and it is the difference between the two kinds of type parameter this
language now has.

The decision `generic-type.beck` asked to be taken deliberately — "`beck iface`'s published contract
… and `--wire-compat` has to say whether `Tree[Int]` and `Tree[Str]` are the same boundary" — is
taken in §36.5, with a test each way.

528 tests, no failures, no compiler warnings, no clippy warnings — up from 517. One error code was
**retired**: `B0309` said "an alias is transparent and this one is not parameterised", and an alias
may be parameterised now, so nothing raises it (§36.8). The index is 94 codes, down from 95.

## 36.1 What was asked, and what is answered

| asked for | status | where |
|---|---|---|
| A `model`, a `union` and a `newtype` may take a type parameter | done | §36.2 |
| So may a `type` alias, expanded *and* applied | done | §36.2, §36.5 |
| The parameter reaches the constructors, the field types and the exhaustiveness check | done | §36.3 |
| Arity checked at every mention, rather than left to unification | done — `B0311` | §36.4 |
| A parameter no field mentions is still a parameter | done — arity is declared, not counted | §36.4 |
| The wire encoding and `beck iface`'s published contract | done, and rendered back as source | §36.5 |
| `--wire-compat` on `Tree[Int]` versus `Tree[Str]` | **decided**, both directions tested | §36.5 |
| Bounds on a parameter (`[T: Display]`) | **not done** — there are no traits to bound it by | §36.10 |
| A parameter that is itself applied (`F[T]`) | **refused**, by name | §36.10 |

## 36.2 One notation, four more forms

The parser already had `typarams` — `[T, U]`, a bare name each, no bounds, because there is nothing
to bound one by. It now runs after the name of a `model`, a `union` and a `type` as well as after
the name of a `def`, and the node it produces sits in the same position in all five: `args[1]`.

That shifted what came after it, which the S-expression surface would have felt. §2.3 makes that
surface a notation people write by hand for macro debugging, and `(model Todo (field id Id))` still
reads — the reader normalises a missing list into an empty one, which is what it already did for
`(def f …)`. One AST shape downstream, one `Printer::typarams` upstream, and
`every_program_round_trips_through_the_formatter` over the whole corpus is the check that the two
agree.

An **alias** takes one too, and it is the odd case worth naming: an alias is transparent, so
`Trail[T] = list[Stamped[T]]` names no type of its own and `Trail[Str]` has to *be*
`list[Stamped[Str]]` by the time anything else sees it. It is expanded and applied in one step, at
the mention. The test asserts the consequence rather than the mechanism: a mismatch between
`Pairs[Int]` and `Pairs[Str]` reports `Map[Int, Int]` against `Map[Str, Str]` and never says
`Pairs`, because by then there is no `Pairs`.

## 36.3 Rigid or positional: the two kinds of type parameter

[`32`](32-numeric-tower-and-polymorphism-report.md) §32.7 built the first kind. A `def`'s parameter
is **rigid** — `Ty::Con("T", [])`, which unifies with itself and nothing else — and that is what
makes the body of `def first[T](xs: list[T]) -> T` provably work for every `T` rather than for
whichever one the body happened to force.

A declaration's parameter cannot be that, and the reason is not a detail:

- a declaration has **no body** to constrain, so rigidity has nothing to protect;
- and the parameter has to **survive into the stored `TyDecl`**, because every later mention of
  `Tree[Str]` substitutes for it. A rigid `Ty::Con("T")` stored in a field type would be
  indistinguishable from a mention of a type actually called `T`.

So a declaration's parameter is **positional**: the *n*th is `Ty::Var(SCHEME_BASE + n)`, and
instantiation is an index rather than a search. The base is far above any variable the checker will
mint, so one `Ty` carries both kinds without a tag. This is not a new convention — it is the one
`prelude.rs` has used for `Option`, `Result` and `Envelope` since Phase 1, and the one `decl_arity`,
`variant_field_types`, `model_fields`, `make` and the value generator were each substituting
through, in five copies. They are one function now, `ty::instantiate_decl`, and the five call sites
are what proves the user path and the prelude path are the same path.

The two scopes are never open at once, and the comment on the field says why: a declaration has no
body and a definition has no fields.

## 36.4 Arity, declared rather than counted

`decl_arity` used to read a declaration's arity off its *fields* — the maximum positional variable
any of them mentioned, plus one. For `Option` and `Result` that is exactly right and it is why the
function existed.

For a user's declaration it is wrong, and it is wrong in the direction that compiles:

```
model Tag[T]:
    label: Str
```

No field mentions `T`, so the counted arity is 0, so `Tag[Int]` and `Tag[Str]` would have been the
same type and `Tag` on its own would have been accepted. A phantom parameter is the whole point of a
phantom parameter. `TyDecl` carries `params: Vec<Arc<str>>` now — the names, in order — and arity is
`params.len()`. The names are what a `.becki` and a doc page render; the field types still refer to
the parameters positionally, so **a rename is a rename and nothing more**.

Arity is checked at the mention, by `B0311`, rather than left to unification:

```
error[B0311]: `Tree` takes 1 type argument(s), got 0
  --> t.beck:5:10
  |
5 | def f(t: Tree) -> Int:
  |          ^^^^ write `Tree[_]`
```

Left to the unifier, a bare `Tree` would have unified with `Tree[Int]` — because `Ty::Con("Tree",
[])` and `Ty::Con("Tree", [Int])` differ, but the *first* place they differ is wherever the
mismatch finally surfaces, which is not the line that is wrong.

## 36.5 What crosses a `.becki`, and what `--wire-compat` says

`generic-type.beck` asked for this to be decided rather than discovered:

> a `union Tree[T]` crossing a `.becki` publishes a shape rather than a type, and `--wire-compat`
> has to say whether `Tree[Int]` and `Tree[Str]` are the same boundary.

**A declaration is published once, parameterised. A mention carries its arguments.** Those are two
different things and they are compared differently.

`beck iface` renders the declaration as source, with the parameter names put back:

```
model Stamped[T]:
    at: Int
    actor: Str
    what: T

union Verdict[T]:
    Waiting(item: T)
    Passed(item: T)
    Failed(item: T, why: Str)

type Trail[T] = list[Stamped[T]]
```

A `.becki` is source and has to be readable back, so rendering is the one place that undoes the
positional form — `what: ?1000000` is not something the parser can read. `State`'s own field in the
same file is `recent: list[Stamped[Str]]` and not `Trail[Str]`, which is what "an alias is
transparent" looks like from outside the module. The round-trip test asserts the digest survives it
and that no `?` escapes.

For `--wire-compat`:

| change | verdict | why |
|---|---|---|
| `model Todo` → `model Todo[T]` | **breaking** | every mention of the name has to be rewritten, so no old signature that names it still type-checks |
| a signature's `Stamped[Int]` → `Stamped[Str]` | **breaking** | the field comparison sees it, because a field's type carries its arguments |
| `union Tree[T]` → `union Tree[Elem]` | **invisible** | the fields refer to the parameter positionally; nothing anybody can observe changed |

So `Tree[Int]` and `Tree[Str]` are **not** the same boundary, and the place that decides it is the
structural hash rather than a special case: the hash now instantiates a declaration's body with the
arguments the mention carried, so `Envelope[Added]` hashes as a record with a `body: Added`. It
distinguished the two before this change as well, because the applied arguments appear at the head —
what changed is that the hash now describes the *shape* rather than the name plus its arguments.
That is a clarity improvement and not a fix, and it is worth saying so rather than claiming a bug.

## 36.6 Chapter 2's tree, at the book's generality

`sicp/ch2.beck`'s tree was a tree of `Int`. It is `Tree[T]` now, and the chapter's own tests use it
at three element types:

```
def words() -> Tree[Str]
def nested() -> Tree[Tree[Int]]        # the closure property, at the type level
```

`count_leaves` is one definition over all of them, and `tree_map` is finally the book's — exercise
2.31 asks for a procedure, any procedure, and `tree_map[T, U](t: Tree[T], f: T -> U) -> Tree[U]`
takes one that *changes* the element type. `fringe(tree_map(example(), str))` is
`["1", "2", "3", "4"]`.

**The parameter deleted a wrong line.** `fringe` used to be `map_list(leaves(t), leaf_value)`, and
`leaf_value` had to answer for a `Node` as well as a `Leaf`:

```
def leaf_value(t: Tree) -> Int:
    match t:
        case Leaf(value):
            return value
        case Node(kids):
            return 0            # a number no tree contains
```

At `T` there is no `0` to reach for, so that does not typecheck, and `fringe` is written from the
structure instead — which is how SICP writes it. A type parameter refusing a fudge that a
monomorphic type accepted is the same class of finding as [`33`](33-effect-polymorphism-and-list-patterns-report.md)
§33.5's `Pattern::binders`: the shape was always wrong and nothing could see it.

Chapter 2 is **15 tests**, up from 13. `sicp/refusals/` holds one file — `rational.beck` — and its
header still ends at traits.

## 36.7 A corpus program, because the passes are where a shape gets dropped

A `union Tree[T]` in `sicp/` proves the checker and nothing else. Every pass this project has walks
a type, and a shape only a library exercises is a shape those passes never see —
[`23`](23-general-slicer-report.md) §23.2 found a splitter that accepted a shape it could not
handle, and [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.5 found three `_ => {}`
arms that would have miscounted a list pattern. Both were invisible until a program used the shape.

`corpus/27-review.beck` is that program, on `25-thread.beck`'s precedent. It carries a
parameterised **model** in the durable state, a parameterised **union** matched on in the view, a
parameterised **alias**, and the same declaration at **two different arguments** — `Stamped[Note]`
in `notes` and `Stamped[Str]` in `recent`, which is the case a compiler that ignored the arguments
would still compile and get wrong.

It has no placement annotations, like every program there, and it goes through the corpus harness,
the general slicer, the incremental analysis, the incremental engine against its recompute oracle,
placement properties, the security suite and the workflow suite. Seven `test` blocks and one
`property`, which is also how the value generator gets asked to build a `Stamped[Note]`.

The corpus is 28 programs, up from 27.

## 36.8 An error code retired

`B0309` — "takes no type arguments" — existed for one case: an alias with arguments written after
it. Its index entry read "An alias is transparent and **this one is not parameterised**, so
`Name[…]` says nothing." That premise is no longer true of any alias, and the case it covered is now
an arity mismatch like every other, so nothing raises it.

`beck-cli/tests/docs.rs` is what forced the question rather than letting the entry sit there: it
scans every non-test source file for a `"Bnnnn"` literal and fails if the set differs from the index
in *either* direction. An entry cannot outlive its code. The code is not recycled.

## 36.9 The pass eight reports promised

[`22`](22-phase-3-report.md) §22.6 asked for the test-checking pass to be moved out of `check.rs`.
Every report since has repeated the request and the line count, and this one had written it a ninth
time before deciding that a sentence written nine times is not a request, it is a habit. So it is
done, in this change:

| | before | after |
|---|---|---|
| `check.rs` | 3,659 lines in one file | — |
| `check/mod.rs` | — | 3,074 |
| `check/tests_in_beck.rs` | — | 615 |

**It is a child module and not a sibling**, and that is the whole reason it could move at all. The
pass is not a separate checker: it needs the substitution, the scopes and the diagnostics, and a
private field of `Checker` is visible to a descendant module but not to a sibling. So
`check/tests_in_beck.rs` holds one `impl Checker` block with `test_subjects`, `check_test`,
`check_stub`, `test_atom`, `test_tier` and the two free functions only they use, and exactly two of
those methods are `pub(super)`.

**What made it separable was already true of the design.** Checking a `test` block is *deferred*: a
clause is typed against the state and event types, which are only known once every signal has been
checked, because `given` is a `list[Event]` and `Event` is whatever the program's own `decide` node
produces. `check_module` already collected the items and came back to them at the end. The seam was
there; nobody had cut along it.

Moving it also found a doc comment that had been attached to the wrong function — "Walk a `Core`
tree applying the final substitution to every recorded type", sitting above `body_expr_of` and
describing `resolve_types`. It is back where it belongs. That is what a refactor is for and it is
not worth more than a sentence.

No behaviour changed: 528 tests, the same 528.

## 36.10 What is still not

- **A parameter cannot be bounded.** `[T: Display]` is not writable, because there is nothing to
  bound it by. This is the same sentence [`32`](32-numeric-tower-and-polymorphism-report.md) §32.9
  wrote about `def`, and it is traits again.
- **A parameter cannot be applied.** `F[T]` where `F` is itself a parameter is refused by name —
  `B0313`, "`F` is a type parameter, so it takes no type arguments" — because a parameter has no
  structure to apply arguments to. Higher-kinded types are not designed anywhere in `docs/`, and
  this refusal is a placeholder for that absence rather than a decision against them.
- **A `trait` and an `impl` still take no parameters**, because a `trait` and an `impl` are still
  parsed and not checked. That is the oldest unpaid debt in the project, now named by one refusal
  file, three reports and a warning the compiler emits.
- **Renaming a type parameter is invisible to `--wire-compat`** (§36.5). It is invisible because
  nothing observable changed, which is the right answer for the wire and the wrong answer for a
  human reading a diff of a `.becki`. Nothing reports it.
- **Variance is not a concept.** §3.1 has no subtyping beyond effect-row subsumption, so `Tree[T]`
  relates to `Tree[U]` exactly when `T` and `U` unify, and there is nothing to get wrong. Worth
  stating because a reader coming from a language with subtyping will look for it.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9, [`31`](31-tail-calls-report.md) §31.7,
  [`32`](32-numeric-tower-and-polymorphism-report.md) §32.9 and
  [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.7 list is unchanged: no LLVM
  backend, no native codegen, no Mode B, no client polish, no `test --update`, no structured
  concurrency, no `Result`/error rows, no SQLite substrate, no standard library v1, no identity
  beyond a dev-mode actor, no LSP, no playground, no supply-chain tooling, no SQL read models, no
  pgwire, no query fusion. Patterns are still one level deep, effect polymorphism still does not
  cross a module boundary, and a `list[T]` is still `O(n)` to take apart.

## 36.11 What this changes for the rest of Phase 3

1. **The standard-library bullet has its second precondition.**
   [`33`](33-effect-polymorphism-and-list-patterns-report.md) §33.8 item 1 found the first: until a
   Beck library could be effect-polymorphic, every caller inherited every other caller's effects.
   The second is this one — a standard library is mostly containers, and a container whose element
   type is fixed is not a container. `list` and `Map` were builtins with parameters nobody else
   could have; that is no longer a privilege of the prelude.
2. **`refusals/` is down to one file, and it says "traits".** So do the headers of the two files
   that are gone, three of the last four reports, and a warning the compiler emits on every `trait`
   in every program. The directory that [`33`](33-effect-polymorphism-and-list-patterns-report.md)
   §33.8 called the roadmap now has one entry on it, and the argument for building trait resolution
   next is no longer an argument anybody has to make.
3. **A prelude privilege is worth auditing for.** This feature existed for `Option`, `Result` and
   `Envelope` for three phases and could not be written by a user, and nothing in the compiler was
   lying — the machinery was general, and the surface was not. That is a shape worth looking for
   deliberately: `builtin_arity` still names sixteen type constructors a program cannot introduce —
   `list`, `Map`, `Stream`, `Signal`, `secret` and `internal` among them — and `prims()` names a set
   of operations no Beck definition can be written to join. Neither is wrong today. Both are places
   where "the design says X" and "a user can do X" have already come apart once.
