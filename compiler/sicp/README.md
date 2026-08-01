# SICP in Beck

The expressiveness suite proposed in
[`docs/24-benchmarks-and-expressiveness.md`](../../docs/24-benchmarks-and-expressiveness.md),
started. [`docs/00-original-idea.md`](../../docs/00-original-idea.md) opens with "I read SICP and
had this idea"; this directory is that sentence turned into something that can fail.

The claim under test is not performance. It is that Beck kept Scheme's means of combination and
means of abstraction while gaining a Python surface and a type system — and that it is no more
verbose for it. §24.5 sets out the protocol; nothing here is a comparison yet.

## What is here

| | |
|---|---|
| [`ch1.beck`](ch1.beck) | The part of SICP chapter 1 that runs today. Thirteen `test` blocks, each asserting an answer the book states |
| [`refusals/`](refusals/) | One file per wall between chapter 1 and the rest of the book. Each is the smallest program that hits it, with the diagnostic in its header comment |

The harness is [`../crates/beck-cli/tests/sicp.rs`](../crates/beck-cli/tests/sicp.rs). It runs
chapter 1, and asserts that every refusal is **still refused** — so a wall coming down shows up as
a test that starts failing rather than as something somebody notices.

```console
$ cargo test --release --test sicp
$ ./target/release/beck test sicp/ch1.beck
```

## Read `ch1.beck`'s header before anything else

Every SICP solution is a library: pure procedures, no merge point. Beck cannot run a library, so
`ch1.beck` carries a five-declaration application that nothing in the chapter uses, purely to be
executable. It is left visible on purpose — deleting it is item 1 of §24.7.

## Why this is not in `corpus/`

[`../corpus/`](../corpus/) has one measured job: placement inferred with no annotations, the
Phase 2 exit criterion. Hundreds of pure-function files would drown that measurement. These are
different questions and they get different directories.

## The register an exercise lands in

Per §24.5, and it matters more than the pass rate: **translated** (same algorithm, book's answer
asserted, counts toward the line comparison), **re-expressed** (same problem in Beck's idiom,
because a transliteration would be a caricature — shown both ways, argued, counted in neither), or
**refused** (Beck cannot, and the report says whether that is a gap or a decision). An exercise
skipped without a register is a bug in the suite.
