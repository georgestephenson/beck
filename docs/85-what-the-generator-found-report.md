# 85 — Phase 3, part 54: what the generator found

**Built.** Grammar-aware fuzzing of the front end — and it found **three** places the recursion
ceiling did not cover, in an afternoon, having been forecast to find exactly that.

[`42`](42-security-assurance.md) §42.9 pinned the method with a trigger: "grammar-aware fuzzing as
the method that finds the rest of this class (trigger: **the bound lands**)". The bound landed in
[`44`](44-wave-0-report.md). §42.11's row asks for "a structure-aware generator over the corpus".

The forecast was §42.2's, quoting the Scriban advisory (GHSA-p6q4-fgr8-vx4p): **a limit added at the
one production somebody thought of is bypassed through a different one.** That sentence has been in
the tree since Wave 0 as a warning. It was also, it turns out, a description.

## 85.1 Why byte mutation could not have found these

§42.1 ran 600 iterations of byte-level mutation over `compiler/corpus/*.beck`, found nothing, and
said why that was not reassuring:

> random mutation cannot *generate structure*, so the one crash class the front end actually has is
> precisely the one this method is blind to.

A mutated file is a *slightly wrong* file. The crash class is a *deeply nested* or *very long* one,
and no amount of flipping bytes in a 40-line program produces 80,000 nested calls. So the generator
here does not mutate: it **builds** programs from the grammar, with the recursive productions
parameterised by depth and the flat ones by length, and the sizes chosen to reach past where the
stack used to give out rather than past where the counters now stop.

That last clause is the whole method, and the first version of this file got it wrong. It generated
sizes up to 3,000 — comfortably past `MAX_NESTING` (256) and `MAX_BLOCK` (2,048), and comfortably
short of the aborts those ceilings replaced, which §42.2 measured at 3,785 nested parens and
[`64`](64-compile-speed-report.md) §64.4 at 12,000 flat bindings. **It passed, and meant nothing.**
A ceiling is cheap to test just past, because the counter stops immediately; the failure worth
finding is the one where no counter stops it. Raising the sizes to 120,000 turned three tests green
into a process abort within a minute.

The property asserted is one line, and it is the only one worth asserting about arbitrary input:

> **The front end answers.** For every generated program, it either accepts it or produces
> diagnostics — never an abort, never a panic, never a failure to terminate.

Not "it compiles". Most of these are nonsense, and a generator that only produced valid programs
would be testing the wrong half.

## 85.2 The three

Each was refused as `refused` for one shape and aborted the process for another, which is what made
them findable: the generator varies the *production* while holding the depth, so a shape that
survives is the control for the one that does not.

**One — the type grammar was not counted at all.** `Parser::type_expr` recurses in four places and
never called `enter`. `list[list[list[…]]]` 80,000 deep aborted; `((((…))))` 80,000 deep was refused
with a span, because parens recurse through `primary`, where the counter is. A whole production with
no counter, which is the Scriban shape in its plainest form.

**Two — the counter was released before the recursion happened.** `Parser::primary` enters, reads a
leaf, and *leaves*. The recursion that makes `g(g(g(…)))` deep happens afterwards, in `postfix`'s
loop, through `call_args` → `expr` → `postfix`. So the depth returned to **zero at every level** and
80,000 nested calls aborted while 80,000 nested parens did not. This is the subtler one: the counter
was present, on a function on the path, and still saw nothing — because it measured the wrong
interval.

**Three — an iterative parser builds a deep tree without recursing.** `1 + 1 + 1 + …` is
left-associative, so the Pratt loop `expr_bp_from` reads it **without recursion at all** and builds a
left-leaning tree of the same depth. No recursion counter can see that, because there is no
recursion; the depth is real in the tree and shows up later, in whatever walks or drops it. At
120,000 terms the macro expander's own ceiling happened to catch it (`B0213`, "the form nests too
deep to expand") — the wrong counter, for the wrong reason, and only by luck of ordering. At 300,000
the process died before anything could report.

The third is the one that changes how to think about this. **A recursion counter counts recursion
done; what the stack cares about is tree depth built**, and those are the same number only when the
parser is recursive. Where a production is a loop, they come apart.

## 85.3 The fixes, and the fourth thing

| | |
|---|---|
| `type_expr` | enters and leaves, like every other production |
| `postfix` | enters and leaves around the **whole chain**, not just the leaf. A nested expression now spends two levels of the ceiling rather than one, which is affordable at 256 against a corpus whose deepest expression is 11 |
| `expr_bp_from` | counts its **iterations** against `MAX_BLOCK`, and refuses with `B0122`. Not a recursion counter: a bound on how long a flat run may be, which is the same thing `MAX_BLOCK` already means for a block of sequential bindings |

The fourth thing was found before the generator existed, by reading
[`64`](64-compile-speed-report.md) §64.4, and is fixed here because a generator that reaches 120,000
would have found it too: **a flat block of sequential bindings** recursed once per statement with
nothing counting, so a debug build aborted at 12,000 and a release build at 100,000 — "which
programs compile depended on how the compiler was built", which is precisely the property
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) says a ceiling must never have.
`MAX_BLOCK` is 2,048, refused with `B0389`, and `the_block_ceiling_fits_the_declared_stack` measures
the checker at **6.8 KiB a statement** in an unoptimised build — so the ceiling costs 14 MiB, and
28 MiB with the doubled margin the parser's and the evaluator's tests also apply, against a declared
64 MiB.

A million-term operator chain and a 400,000-deep call are now both diagnostics with spans.

## 85.4 What it cost elsewhere, which is the honest part

Two gates had to move, and neither was wrong before.

**`compile_speed.rs`'s depth axis measured 6,400 sequential bindings**, which is now a program the
front end refuses — so the gate would have been measuring error recovery. Its three axes now name
their own sizes, and the depth one runs 100 → 1,600. That is not a weakening: the gate asserts a
*shape* (cost per declaration must not grow with the count), sixteen times as many is the
measurement whatever the absolute numbers are, and **the ceiling is a safety property that cannot
move to suit a benchmark while the ratio is arbitrary and can.**

**`front_end_bound.rs` asserted that 5,000 sequential bindings *must compile*** — which was the
correct assertion for the world [`64`](64-compile-speed-report.md) §64.4 described, where the stack
was what bounded the axis. It now asserts the property that replaced it: under the ceiling compiles,
over it is `B0389` with a span, and the number does not move with the profile. That test **fired**,
which is the good case and worth saying next to [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md)
§84.5's two that did not.

**And the ceiling's own tests needed the declared stack, which CI found and this machine did not.**
`MAX_BLOCK` is sized against `beck_diag::depth::STACK_BYTES` — 64 MiB — and a default test thread has
2 MiB, so the first version of `a_block_past_the_ceiling_is_refused_with_a_diagnostic` called the
checker directly and aborted the test binary. It is the same lesson [`80`](80-structured-concurrency-report.md)
§80.11 recorded when `roundtrip.rs` overflowed on `awfy/havlak.beck`: **a harness is a caller of the
front end, and a caller has to honour the declaration.** That is now three harnesses that have had
to learn it separately, which is an argument for the entry point making it hard to get wrong rather
than for remembering.

Two smaller notes on how it was found, because both are about verification rather than about the
front end. A stack overflow **aborts the test binary**, so it prints `fatal runtime error` and
`error: test failed` and *not* `test result: FAILED` — a check that greps for the usual failure
strings misses it entirely, and the exit code is the only reliable signal. And the type-grammar
counter moved which pass refuses a deep type: `a_type_past_the_ceiling_is_a_diagnostic_rather_than_an_abort`
expected the *checker's* `B0390` and now meets the *reader's* `B0121` a stage earlier. Its sibling
for expressions already said "whichever pass reaches it first", which is the right shape for both —
the property in the name is a claim about the front end, not about which half of it.

## 85.5 `proptest` rather than `cargo-fuzz`

§42.11's row names `cargo-fuzz`, which needs libFuzzer and therefore nightly; this workspace pins
stable 1.94.1, and taking a nightly toolchain for one harness is a larger decision than one test
should make. `proptest` is already a dev-dependency, already used by `manifest_properties.rs`, and
shrinks failures to a minimal case.

The substitution is honest because **the generator is the contribution, not the driver.** What found
these three was knowing which productions exist and generating each independently at a size past the
stack — not coverage feedback. What `cargo-fuzz` would add is finding the productions nobody thought
to enumerate, which is a real difference and is §85.6's first row.

Two tests, and the split matters. The property test samples shapes and sizes; the enumerated one
walks every shape across every ceiling and both sides of it, because a random `n` in a range of
40,000 may never land on 256 or 2,048 and those are exactly where this class of failure lives.
A third asserts the refusals are the **counted** ones rather than any diagnostic at all — without it,
a parse error from a file so large the lexer gave up would satisfy "the front end answers" and mean
nothing.

## 85.6 What is not built

| | |
|---|---|
| Coverage-guided fuzzing | **not built** (§85.5). It finds the production nobody enumerated, which is the residual risk this harness has by construction: it tests the grammar somebody wrote down |
| The macro expander as a target | **partly.** `Shape::Ui` reaches it, and `B0213` shows its ceiling works, but nothing generates a *user* macro that expands into more of itself. F17's macro fuel is still unbuilt and still asserted absent in `pending_security.rs` |
| A corpus-seeded generator | **not built.** §42.11's row says "over the corpus", and this generates from the grammar instead. Seeding from real programs and mutating *structurally* — swap a subtree, duplicate a branch — is the version that would find semantic failures rather than depth ones |
| The same treatment for the evaluator | **not built.** This bounds what the *front end* will read. `beck test --fuel` bounds what the evaluator will run ([`62`](62-fuel-report.md)), and nothing generates programs to test that boundary |
| A number for how long it runs | The suite is 96 generated programs plus the enumeration, in a few seconds. §42.11 asks for "a bounded budget per pull request" and that is the budget; nothing has measured what a larger one would find |

## 85.7 What this establishes

**That a warning quoted in a document is not a check.** §42.2 has quoted the Scriban advisory since
Wave 0 — bound the recursion site, not one grammar rule — and the tree contained three violations of
it, one of which (`postfix`) was a counter on the right function measuring the wrong interval. The
project knew the lesson well enough to write it down twice and could not apply it by reading.

**And that the sizes are the method.** The generator was the easy half and the first version of it
proved nothing, because it stopped at 3,000 — past every ceiling and short of every abort. A fuzzer
calibrated against the limits you built tests that your limits work. A fuzzer calibrated against the
failures you had tests whether you have found all the places they happen. That is the same mistake
[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5 found in a `pending_security` test
that proposed 200 events against a limit of 600, two reports running, from opposite directions.
