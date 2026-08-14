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
| [`ch2.beck`](ch2.beck) | Chapter 2's §2.1.1 and §2.2 — exact rationals, with `+` reaching a user's type through the prelude's `Num` and the book's printed `5/6`, `1/6` and `2/3` as the oracle; and the sequence abstractions the reader is asked to build (`map`, `filter`, `accumulate`, `append`, `length`, `reverse`), the closure property at the book's own generality (`Tree[T]`, used at three element types including `Tree[Tree[Int]]`), and §2.2.3's conventional interfaces |
| [`ch3.beck`](ch3.beck) | Chapter 3, "Modularity, Objects, and State" — the chapter the language disagrees with, and then agrees with. §3.1's objects with the state they hide written down, §3.3.2's queue, §3.3.3's tables, §3.3.4's digital-circuit simulator at the times the book prints (`sum 8`, `carry 11`, `sum 16`), §3.5's streams with three tables of doubles digit for digit, and §3.5.5's account with no assignment — which the file asserts agrees with §3.1.1's fold, because that is [`docs/01`](../../docs/01-vision-and-premise.md) §1.1's premise in the book's own words. Twenty-six `test` blocks and a `property` ([`docs/63`](../../docs/63-expressiveness-report.md)) |
| [`felleisen.beck`](felleisen.beck) | §25.9's formal half: one section per special form SICP introduces, each carrying the code that recovers it or the reorganisation that concedes it ([`docs/63`](../../docs/63-expressiveness-report.md)) |
| [`refusals/`](refusals/) | One file per wall still standing between here and the rest of the book. **Empty**, and its README says what that means and what would put a file back |

The harness is [`../crates/beck-cli/tests/sicp.rs`](../crates/beck-cli/tests/sicp.rs). It runs all
three chapters, and asserts that every refusal is **still refused** — so a wall coming down shows up
as a test that starts failing rather than as something somebody notices. Two of chapter 3's belong
there rather than in the file, because a refused program cannot live in one that has to compile:
§3.4's interleaving is `B0399` and exercise 3.8's is `B0398`.

```console
$ cargo test --release --test sicp
$ ./target/release/beck test sicp/ch1.beck
$ ./target/release/beck test sicp/ch2.beck
$ ./target/release/beck test sicp/ch3.beck
```

## Nine walls down, none standing

§25.7 put the six in dependency order and all six are built: running a module with no merge point,
recursive and forward-referencing types and the `B0320` defect
([`docs/27`](../../docs/27-the-walls-come-down-report.md)); proper tail calls
([`docs/27`](../../docs/27-the-walls-come-down-report.md)); reals and user-written polymorphism
([`docs/27`](../../docs/27-the-walls-come-down-report.md)).

Each wall that came down left a test pointing the other way rather than no test at all. The clearest
example is `refusals/tail.beck`, which asserted that a tail call eight thousand deep aborts the
process and is now `ch1.beck`'s §1.2.1 exercise asserting that one a quarter of a million deep does
not.

Removing the six wrote three more, and those came down too: a `list[T]` that could not be taken
apart ([`docs/27`](../../docs/27-the-walls-come-down-report.md)), a type that
could not take a parameter ([`docs/27`](../../docs/27-the-walls-come-down-report.md)), and exact
rationals — which needed `+` to reach a type the compiler does not know about, and got it when
`Num` joined the prelude ([`docs/27`](../../docs/27-the-walls-come-down-report.md)).

So [`refusals/`](refusals/) is **empty**, and its own README says what that does and does not claim:
every wall this project has *found* has been removed, which is not the same as expressing all of
SICP. Chapter 3 has been attempted since ([`docs/63`](../../docs/63-expressiveness-report.md))
and produced no wall of that kind: every section of it is expressible, and the reorganisation is one
rule applied twelve times. What it produced instead was a **cost** the book names explicitly — §3.5.1
says `delay` should memoise and implements that with two `set!`s, so §3.5.3's tableau is ×5.2 per term
here — a diagnostic that did not exist, and a quadratic in the most ordinary form in the language.
Chapters 4 and 5 are still unattempted.

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
