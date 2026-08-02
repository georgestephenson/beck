# SICP in Beck

The expressiveness suite proposed in
[`docs/25-benchmarks-and-expressiveness.md`](../../docs/25-benchmarks-and-expressiveness.md),
started. [`docs/00-original-idea.md`](../../docs/00-original-idea.md) opens with "I read SICP and
had this idea"; this directory is that sentence turned into something that can fail.

The claim under test is not performance. It is that Beck kept Scheme's means of combination and
means of abstraction while gaining a Python surface and a type system — and that it is no more
verbose for it. §25.5 sets out the protocol; nothing here is a comparison yet.

## What is here

| | |
|---|---|
| [`ch1.beck`](ch1.beck) | Chapter 1: §1.1.7, §1.2 and §1.3. Twenty-one `test` blocks, all but one asserting an answer the book states — four of them a printed double, digit for digit. The exception asserts the *property* §1.2.1 states instead: that an iterative process runs in constant space |
| [`ch2.beck`](ch2.beck) | Chapter 2's §2.2 — the sequence abstractions the reader is asked to build (`map`, `filter`, `accumulate`, `append`, `length`, `reverse`), the closure property, and §2.2.3's conventional interfaces. §2.1's rationals need *exact* rationals, which reals are not |
| [`refusals/`](refusals/) | One file per wall still standing between here and the rest of the book. Each is the smallest program that hits it, with the diagnostic in its header comment |

The harness is [`../crates/beck-cli/tests/sicp.rs`](../crates/beck-cli/tests/sicp.rs). It runs both
chapters, and asserts that every refusal is **still refused** — so a wall coming down shows up as
a test that starts failing rather than as something somebody notices.

```console
$ cargo test --release --test sicp
$ ./target/release/beck test sicp/ch1.beck
$ ./target/release/beck test sicp/ch2.beck
```

## Six walls down, one discovered

§25.7 put the six in dependency order and all six are built: running a module with no merge point,
recursive and forward-referencing types and the `B0320` defect
([`docs/27`](../../docs/27-walls-report.md)); proper tail calls
([`docs/28`](../../docs/28-tail-calls-report.md)); reals and user-written polymorphism
([`docs/29`](../../docs/29-numeric-tower-and-polymorphism-report.md)).

Each wall that came down left a test pointing the other way rather than no test at all. The clearest
example is `refusals/tail.beck`, which asserted that a tail call eight thousand deep aborts the
process and is now `ch1.beck`'s §1.2.1 exercise asserting that one a quarter of a million deep does
not.

What is in `refusals/` now was written by the removals rather than by §25.6, which is the suite
working rather than the suite running out:
[`rational.beck`](refusals/rational.beck) — §2.1.1 needs *exact* arithmetic, and a new numeric type
cannot join the resolution `+` goes through — and
[`generic-type.beck`](refusals/generic-type.beck) — a `def` may take a type parameter and a `union`
may not. Both headers end at the same place: **traits**.

## Why this is not in `corpus/`

[`../corpus/`](../corpus/) has one measured job: placement inferred with no annotations, the
Phase 2 exit criterion. Hundreds of pure-function files would drown that measurement. These are
different questions and they get different directories.

## The register an exercise lands in

Per §25.5, and it matters more than the pass rate: **translated** (same algorithm, book's answer
asserted, counts toward the line comparison), **re-expressed** (same problem in Beck's idiom,
because a transliteration would be a caricature — shown both ways, argued, counted in neither), or
**refused** (Beck cannot, and the report says whether that is a gap or a decision). An exercise
skipped without a register is a bug in the suite.
