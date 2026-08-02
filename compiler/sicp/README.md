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
| [`ch1.beck`](ch1.beck) | The part of SICP chapter 1 that runs today. Fifteen `test` blocks — fourteen asserting an answer the book states, and one asserting the *property* §1.2.1 states instead: that an iterative process runs in constant space |
| [`ch2.beck`](ch2.beck) | Chapter 2 as far as §2.2's closure property. §2.1's rationals need the numeric tower; the exercises that ask the reader to *build* `map` and `accumulate` need user-written polymorphism |
| [`refusals/`](refusals/) | One file per wall still standing between here and the rest of the book. Each is the smallest program that hits it, with the diagnostic in its header comment |

The harness is [`../crates/beck-cli/tests/sicp.rs`](../crates/beck-cli/tests/sicp.rs). It runs both
chapters, and asserts that every refusal is **still refused** — so a wall coming down shows up as
a test that starts failing rather than as something somebody notices.

```console
$ cargo test --release --test sicp
$ ./target/release/beck test sicp/ch1.beck
$ ./target/release/beck test sicp/ch2.beck
```

## Four walls down, two standing

§25.7 put the six in dependency order and four are built. Running a module with no merge point,
recursive and forward-referencing types, and the `B0320` defect are
[`docs/27`](../../docs/27-walls-report.md); proper tail calls are
[`docs/28`](../../docs/28-tail-calls-report.md). What is left in `refusals/` is the numeric tower
(`real.beck`) and user-written polymorphism (`generic.beck`).

Each wall that came down left a test pointing the other way rather than no test at all — the clearest
example is `refusals/tail.beck`, which asserted that a tail call eight thousand deep aborts the
process and is now `ch1.beck`'s §1.2.1 exercise asserting that one a quarter of a million deep does
not.

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
