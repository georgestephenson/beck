# 30 — Phase 3 report, part 8: two findings, fixed

[`29`](29-numeric-tower-and-polymorphism-report.md) closed §25.6's six walls and ended with two
things it had found rather than fixed. This is both of them.

| finding | where it was named | status |
|---|---|---|
| A generic higher-order definition's row is shared by every call site, so a **pure caller** of it is published as effectful because *another* caller passed an effectful function | §29.9, called "the clearest next item this report produces" | **fixed** — §30.2 |
| A `list[T]` cannot be taken apart, so §2.2.1's `accumulate` and all of §2.2.3 cannot be written | §29.10, the wall removing wall 6 made visible | **fixed** — §30.5 |

**The first is §3.2's own sentence, finally true of a definition a user wrote.** `map : (list[a], (a
-> b ! e)) -> list[b] ! e` has been in the design since the beginning and in `prelude.rs` since
Phase 2, for the *prelude*. Now:

```
def twice[T](f: (T) -> T, x: T) -> T          # published as: (T) -> T, performing nothing
def plain(n: Int) -> Int                       # pure, and stays pure
def stamped(n: Int) -> Int uses nondet         # charged for exactly what it passed
```

**The second gave chapter 2 its second half.** `accumulate` is written, and §2.2.3 — "Sequences as
Conventional Interfaces", the section the rest of the chapter is built out of — went with it, along
with `append`, `length`, `reverse`, `last-pair`, `list-ref` and exercise 2.33's three rebuilds. All
of them are now the book's own definitions rather than renames of a builtin, which is what the
exercise is for.

A `match` on a list is checked for **exhaustiveness**, which lists did not get for free: a union got
one because its variants are declared, and a list's two shapes had to be taught.

`sicp/refusals/` is not empty and that is deliberate. Two walls [`29`](29-numeric-tower-and-polymorphism-report.md)
§29.9 described in prose now have files and tests: exact rationals, and a type that takes a
parameter (§30.7).

486 tests, no failures, no compiler warnings, no clippy warnings — up from
[`29`](29-numeric-tower-and-polymorphism-report.md)'s 482. Chapter 2 is 13 tests, up from 10.

## 30.1 What was asked, and what is answered

| asked for | status | where |
|---|---|---|
| A pure caller of a generic higher-order definition stays pure | done | §30.2 |
| The effect still arrives somewhere — the caller that supplied it | done, tested in both directions | §30.2 |
| Effect polymorphism across a `.becki` | **not done**, and unchanged from Phase 2 | §30.4 |
| A definition that *returns* a function | **not generalised** — the one shape the fix cannot reach, with the test that says so | §30.3 |
| `accumulate`, and §2.2.3's sequence interface | done — `sicp/ch2.beck` | §30.5, §30.6 |
| Exhaustiveness for a `match` on a list | done, with a diagnostic that names the missing shape | §30.5 |
| Nested patterns (`case [Added(id), *rest]`) | **not done** — patterns stay shallow | §30.7 |
| The two walls §29.9 described in prose | given files and tests | §30.7 |

## 30.2 A row per call site

The defect, stated exactly. A definition's signature mints one row variable per written function
type, and that variable lived in the checker's substitution rather than in the definition's scheme.
So every call site of `apply_each` unified its argument's row with *the same* variable, one caller
passing `uuid()` bound it to `{nondet}`, and every other caller — and `apply_each` itself — inherited
it. Sound, in the sense that it never lost an effect. Wrong, in the sense that `pure_use` performs
nothing and said it performed `nondet`, and placement believed it.

The fix has three parts and none of them is large.

**Quantify.** `Scheme` already carried `row_vars`, because the prelude's `map_list` needs them; a
user's definition now populates it too, with the row variables written into its *parameters*.
`Subst::instantiate` already freshened them. That much was one line of plumbing into machinery that
had been waiting since Phase 2.

**Keep the link.** A quantified variable has to appear in the definition's *own* latent row, or the
call site would rename the parameter's copy and have nothing to attach it to. So the latent row is
`{rv} ∪ {quantified tails}`, where `rv` is the variable standing for what the body does by itself.

**Subtract.** This is the part that is easy to get wrong. After the body is checked, `rv` is bound
to what the body performed — and what the body performed *includes* the parameter's row, reached
through `map_list`'s own row variable. Binding `rv` to that would put the **generic** variable back
into every instantiated call, beside the fresh one, and the contamination would return by a longer
route. So the inferred row is resolved first — flattening `map_list`'s variable down to the
parameter's — and the quantified tails are then removed, because the scheme already carries them.

The test that used to assert the old behaviour is in `check.rs` and has been turned round rather
than deleted, with its old comment quoted in the new one. A second test asserts the other direction,
which is the one a mistake here would break silently: the effect has to arrive at the caller that
supplied it, and `stamped` is charged `nondet` while `plain` is not.

## 30.3 What is over-approximated, and the one shape that is not generalised

**Over-approximated:** a definition promises to perform whatever its function-typed parameters may
perform, whether or not it calls them.

```python
def ignore(xs: list[Int], f: (Int) -> Int) -> Int:
    return list_len(xs)          # never calls `f`
```

A caller passing an effectful `f` is charged for it. That is because the row is quantified from the
*signature*, before any body is read — which is what lets any definition call any other in any order
without a dependency sort, and is the same property the row variable existed for in the first place.
The direction is the safe one: an effect too many forces a stricter placement, an effect too few
would let a fold read a clock. It is also the rule `uses` has always followed — "a declared effect
is part of the signature whether or not the body reaches it" — so this is that rule applied to a
parameter rather than a new rule. There is a test on it.

**Not generalised:** a definition whose *return type* carries a row of its own — which in practice
means one that returns a function.

```python
def hold(f: (Int) -> Int) -> (Int) -> Int:
    return f
```

`instantiate` renames a quantified variable wherever it appears **syntactically** in the scheme. The
return type's row is bound to the parameter's *through the substitution*, not syntactically, so
renaming would give the call site a fresh variable on one side and the generic one on the other —
and a fresh unconstrained row on a returned function is an effect silently lost, which is the
direction that must never happen. So the whole definition keeps the older monomorphic row, and a
test asserts that it does, phrased so that it starts failing the day this is lifted.

Lifting it needs the resolved signature at generalisation time, which needs bodies checked in
dependency order — SCCs of the call graph, generalising after each. That is a real change to the
checker's shape and it would also change the order diagnostics are emitted in, which the snapshot
suite would notice. It is the next thing here, and it is a different size of job from this one.

## 30.4 The module boundary, unchanged

Effect polymorphism still does not cross a `.becki`. `iface.rs` has said so since Phase 2 and the
reason has not changed: a published row variable's *number* comes from the order the checker minted
it in, so two compilations of an unchanged module would publish different-looking contracts and
§3.6's firewall would never hold. `close_rows` therefore publishes a function parameter's row as
closed, and an importer passing an effectful argument where the contract says pure is refused.

What is new is that this is now the *only* place the old behaviour survives, and that there is a
precedent for fixing it: [`29`](29-numeric-tower-and-polymorphism-report.md) §29.7 published *named*
type parameters, and a canonically-named row parameter — `f: (T) -> Bool ! e0`, numbered by first
appearance — is the same idea one dimension over. It needs surface syntax for a row inside a type,
which [`03`](03-type-and-effect-system.md) §3.2 already writes and the parser does not read.

## 30.5 A list, taken apart

`match` gained two shapes, and stopped there on purpose:

```python
match xs:
    case []:
        return seed
    case [first, *rest]:
        return combine(first, accumulate(rest, seed, combine))
```

Fixed-length patterns come out of the same grammar for free — `[only]`, `[a, b]` — and `_` is a
binder that binds nothing. **Nested patterns are still refused**, as they are for a constructor:
patterns in Beck are one level deep, which is what §3.1's exhaustiveness check needs and no more.

Four decisions worth recording.

**The syntax is `*rest`, parsed inside a list literal rather than in a pattern grammar of its own,**
because §2.6 already says patterns *are* expressions — "`Added(id, text)` is the form `(Added id
text)` … Nothing new to represent". `*` outside a list is still multiplication; the checker is what
refuses `*name` where a pattern is not wanted.

**Exhaustiveness had to be taught.** A union's variants are declared, so the check could enumerate
them; a list's shapes are not. A `match` on a list is exhaustive when it covers the empty list and a
list with elements, and the diagnostic names which one is missing:

```
error[B0341]: match is not exhaustive
   | missing: the empty list — `case []`
   = note: a list is empty or it is not, and a fold that handles only one of those is a fold that
     fails on the input nobody tested
```

**`Pattern::binders` is the quiet half, and the half that would have been a bug.** Three passes read
a pattern's bound variables — the plan's free-variable analysis, the splitter's variable-supply
high-water mark, and the evaluator — and two of them were written as `Bind` / `Ctor` / `_ => {}`. A
new pattern kind falling into that `_` is not a compile error; it is a silent miscount that produces
a false *free* variable in one pass and a colliding variable id in the other, on a program that
typechecks. All three now go through one method, so the next pattern kind is a compile error in
three files instead of a defect in none.

**A list pattern is on the incremental path, not only in a pure expression.**
`corpus/26-sensors.beck` puts one inside a signal the engine maintains, so the corpus harnesses
carry it through the slicer, the plan, the recompute oracle and replay — which is what would have
caught the paragraph above if it had been wrong.

## 30.6 Chapter 2's second half, and what it costs

`sicp/ch2.beck` is 13 tests. §2.2.1's `map`, `filter`, `accumulate`, `append`, `length`,
`last-pair`, `reverse` and `list-ref` are written the way the book writes them — structural
recursion over a list — rather than delegating to `map_list` and `filter_list`, which is the
difference between doing the exercise and citing it. §2.2.3's `enumerate-interval`,
`sum-odd-squares` and `even-fibs` follow, and exercise 2.33 rebuilds three of the first set out of
`accumulate` to show they were accumulations all along.

Five of the new assertions are lists SICP prints: `(list-ref squares 3)` is 16, `(length odds)` is
4, `(append squares odds)` is `(1 4 9 16 25 1 3 5 7)`, `(last-pair (list 23 72 149 34))` is `(34)`,
`(reverse (list 1 4 9 16 25))` is `(25 16 9 4 1)`.

**The cost is quadratic and it is stated rather than discovered.** Beck's `list[T]` is an
`Arc<Vec<T>>`, so it cannot share a suffix: `*rest` copies the tail, and `cons` copies the whole
list. A fold written the book's way is therefore `O(n²)` where Scheme's is `O(n)`. That is fine for
a chapter of exercises and it is not fine for a standard library, and the fix is a representation
with a shared tail — which is a change to `Value`, the wire format and the digest, not a change to
the pattern. `ch2.beck` says so where somebody reading the fold will see it.

## 30.7 What is still not

- **Effect polymorphism does not cross a module boundary** (§30.4) and **is not generalised for a
  definition that returns a function** (§30.3). Both have tests that assert the current behaviour,
  so both become visible the day they are lifted.
- **Patterns are still one level deep.** `case [Added(id), *rest]` is refused, as `case Node(Leaf(v))`
  always has been. Every fold in SICP chapter 2 is writable without it; §2.3.4's Huffman decoder is
  the first thing that wants it.
- **A list is `O(n)` to take apart** (§30.6). Nothing in the compiler warns about it.
- **Two walls named in [`29`](29-numeric-tower-and-polymorphism-report.md) §29.9 now have files
  rather than fixes**, which is the point of a refusal file:
  [`rational.beck`](../compiler/sicp/refusals/rational.beck) — §2.1.1 needs *exact* arithmetic, and
  a new numeric type cannot join the ad-hoc resolution `+` goes through, which is traits again; and
  [`generic-type.beck`](../compiler/sicp/refusals/generic-type.beck) — `def map[T]` is writable and
  `union Tree[T]` is not, refused by the parser exactly as `def map[T, U]` was before
  [`29`](29-numeric-tower-and-polymorphism-report.md).
- **Traits are still parsed and not checked**, which is now named by two refusal files, two reports
  and a warning the compiler emits. It is the oldest unpaid debt in the project.
- **`check.rs` is 3,410 lines**, up from 3,170. §22.6's request to move the test-checking pass out of
  it is unmet for the seventh report running, and this added a pattern kind, an exhaustiveness rule
  and a generalisation pass rather than moving anything. At some point the sentence in these reports
  has to become a change.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9, [`28`](28-tail-calls-report.md) §28.7
  and [`29`](29-numeric-tower-and-polymorphism-report.md) §29.9 list is unchanged: no LLVM backend,
  no native codegen, no Mode B, no client polish, no `test --update`, no structured concurrency, no
  `Result`/error rows, no SQLite substrate, no standard library v1, no identity beyond a dev-mode
  actor, no LSP, no playground, no supply-chain tooling, no SQL read models, no pgwire, no query
  fusion.

## 30.8 What this changes for the rest of Phase 3

1. **§3.2's headline sentence is true of user code now, not just of the prelude.** "Effect
   polymorphism is what keeps one standard library" was written about `map_list`; a standard library
   is going to be mostly *Beck*, and until this report a Beck library could not have been written
   without every caller inheriting every other caller's effects. The standard-library bullet was
   blocked on this and nobody had listed it.
2. **A `_ => {}` in a match over a compiler IR is a latent defect with a date on it.** §30.5's
   `Pattern::binders` is the second time this project has found one: [`23`](23-general-slicer-report.md)
   §23.2 found a splitter that accepted a shape it could not handle rather than refusing it. Both
   were invisible until a program used the shape. Exhaustive matches over IR enums are worth the
   noise.
3. **The refusals directory is the roadmap now.** Six walls were measured, six removed, and what is
   in the directory today was written by the removals rather than by
   [`25`](25-benchmarks-and-expressiveness.md). Both remaining files say "traits" somewhere in their
   header, which is a stronger argument for building them than any of the four reports that have
   mentioned them in passing.
