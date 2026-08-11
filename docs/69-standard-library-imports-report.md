# 69 — Phase 3, part 38: the standard library becomes importable, and the benchmark that was waiting on it

**Built.** `import bignum` works from any directory. The Beck half of the standard library is
compiled into the `beck` binary
([`beck_core::stdlib`](../compiler/crates/beck-core/src/stdlib.rs)) and resolved after the caller's
own directory; [`10`](10-decisions.md) D23 is the language rule and
[`adr/0018`](adr/0018-the-standard-library-is-carried-in-the-compiler.md) is the engineering record.

This is [`68`](68-clbg-report.md) §68.4's finding, repaired:

> `import x` resolves against **the directory the root module lives in**, and against nothing else
> […] So the Beck half of the standard library is reachable from `lib/` and from nowhere:
> `decimal.beck` can import `bignum.beck` because they are siblings, and no program a user writes
> can import either.

That report declined to fix it, and said why: "the fix is a **design decision rather than a
repair** […] That belongs in [`10`](10-decisions.md) and an [`adr/`](adr/), taken deliberately, and
not in a benchmark's change." So this change is the decision, the fix, the two things the fix broke
in the library itself, and the benchmark the finding was holding.

## 69.1 The rule, and the one place it lives

`beck_core::project::check_project` resolves a module by asking the caller's `Loader` first — for
the CLI, the directory the root module lives in — and the embedded table second. It is one
`or_else` in the one function every command already goes through, so `beck check`, `beck test`,
`beck run`, `beck doc` and `beck iface` got it at once and none of them mentions it. Nothing else
changed about linking, checking or placement.

The order is the decision and it could have gone the other way. **Directory first** means a program
that already has a `text.beck` keeps working the day the standard library grows a `text`, so adding
a library can never break a program that never asked for it. Library first would reserve every name
in `lib/` for all time and make each addition a breaking change for somebody. The cost is that a
local module silently shadows a library one — Python's cost, taken with Python's eyes open — and it
is visible in the one place it should be: `B0603` now says where it looked.

```console
$ beck check app.beck
error[B0603]: cannot find module `nowhere`
  = note: looked for `nowhere.becki` and `nowhere.beck` beside the root module, and for a standard-library module called `nowhere`
```

**Why the table is in the binary rather than on a path** is
[`adr/0018`](adr/0018-the-standard-library-is-carried-in-the-compiler.md), and the short form is
that the *other* half of the standard library — the primitives in `prelude.rs` — has always been in
the binary, and the Beck half is written against it version for version. A search path splits the
two across a filesystem boundary and buys a flexibility nothing has asked for. Inside `lib/` nothing
changes: a library file importing a sibling gets the file beside it, because the caller's directory
wins, so working on the library does not mean rebuilding the compiler to see the change.

**A library module's `test` blocks do not become the importing program's.** They are checked — a
library whose tests stopped compiling would be broken — and dropped at the link, the way no test
runner runs its dependencies' tests. `beck test` on a program that imports `bignum` reports that
program's tests. `beck-cli/tests/stdlib.rs` is where the library's own run, as before.

## 69.2 What it broke in the standard library, which is the thing worth reading

Beck links modules into one flat namespace with no qualified reference — `B0601`, "defined in more
than one module" — and [`68`](68-clbg-report.md) §68.4 recorded that as a constraint the Benchmarks
Game had already met. Making the library importable pointed it at the library: **every module in
`lib/` now has to link with every other one**, because a program can import two of them.

Two collisions were waiting, and neither had ever been reachable:

| | where | what happened to it |
|---|---|---|
| `is_negative` | `money.beck` and `decimal.beck` | `decimal.beck`'s is now `is_negative_decimal`, which is the suffix its neighbours `is_zero_decimal`, `compare_decimal` and `negate_decimal` already use |
| `pow10` | `decimal.beck` (returning a `Big`) and `format.beck` (returning an `Int`) | `format.beck`'s is now `ten_to_the` |

`stdlib.rs::the_whole_library_links_into_one_program` is the gate: one program importing every file
in the directory, compiled. It is worth more than the two renames — it is the first thing in the
repository that holds the standard library to being *one* library rather than ten files that each
happen to compile.

The same property has a sharper edge pointing outward, and it is a **cost of D23 rather than a
defect**: a program that imports `bignum` cannot define `one`, `zero`, `divide`, `trim` or `expand`,
because `bignum.beck` does. This was found the ordinary way — by writing a probe whose one function
was called `one()` while measuring §69.4's numbers, and getting `B0601`. Qualified imports are the fix and they
are a package-system decision ([`16`](16-packages-and-ecosystem.md) §16.7); until then, importing a
library reserves its helper names in the importing program, and the diagnostic says so precisely.

## 69.3 `clbg/format.beck` becomes `lib/format.beck`

[`68`](68-clbg-report.md) §68.4's third finding was that fixed-decimal formatting is a
standard-library gap, and that the file implementing it "belongs in `lib/` and is not there, for the
reason above: the directory that needs it could not import it back."

It is there now. `fixed(x, places)` and `lines_of(parts)` are what three of the Benchmarks Game's
ports needed and what anything comparing numbers as *text* needs; the eight ports import it from the
standard library and nothing in `clbg/` is a support module any more. Its doc comment now points a
caller whose values can land exactly on a half at [`56`](56-decimal-report.md)'s `render_at`, which
takes the rounding rule as an argument and works in exact arithmetic — the honest referral that a
`Float` formatter written in one afternoon owes.

## 69.4 What an import costs to compile

`beck check`, release build, median of seven, this four-core container. A module is checked from
source on every build; there is no interface cache.

| the program | lines checked | check |
|---|---|---|
| no imports | 2 | **2.8 ms** |
| `import format` | 69 | **3.5 ms** |
| `import bignum` | 568 | **11.5 ms** |
| `import decimal` (which imports `bignum`) | 1,056 | **17.9 ms** |
| all ten library modules | 2,555 | **32.5 ms** |

About 12 µs a line, which is [`64`](64-compile-speed-report.md) §64.6's figure for the front end
(4.7 ms for the worst 914-line file in the tree) arriving at the same place from the other
direction. It is a real cost and it is linear in what was imported: a program that imports the whole
standard library pays 30 ms a build for the privilege. The place that will feel it is the editor —
[`65`](65-lsp-report.md)'s server re-checks the whole file on every change and does not resolve
imports at all (§69.10), so the day it does, this table is what it adds to
[`04`](04-compiler-architecture.md) §4.6's 100 ms budget. There is room at these numbers and there
would not be at ten times them.

**Nothing caches interfaces between runs.** `.becki` exists and is a published contract rather than
a build artefact, and [`04`](04-compiler-architecture.md) §4.6's Salsa query graph is the thing that
would make this incremental. Neither is built; this is the measurement that says when it will
matter.

## 69.5 `pidigits`, the benchmark the finding was holding

[`68`](68-clbg-report.md) §68.6 called `pidigits` "the one of the three that is a repository
limitation rather than a language one, and the only one that is owed a fix", and `clbg.rs` asserted
the limitation so that the change lifting it would turn a test red. It did, and the benchmark is
ported: [`compiler/clbg/pidigits.beck`](../compiler/clbg/pidigits.beck), verified against
`clbg/expected/pidigits-output.txt` — the Game's own published output at `N = 30`, downloaded from
`public/download/` in its repository like the others, and asserted through the same machinery
§68.2 built. **The Benchmarks Game harness is eight of ten.**

The port is the Java program contributed by Isaac Gouy: Gibbons' streaming spigot from *Unbounded
Spigot Algorithms for the Digits of Pi*, as a 2×2 transformation over arbitrary-precision integers,
with the `extract(3)`/`extract(4)` safety check the Game explicitly requires kept rather than
optimised away.

**What it measures here is `lib/bignum.beck`**, and that is the sentence to keep in mind when
reading any number off it. Every other entry in the Game's `pidigits` table measures GMP, or a
runtime's built-in big integer, or a library its language ships in C; this one measures schoolbook
multiplication and long division over base-10,000 limbs, written in Beck ([`55`](55-bignums-report.md)),
running on a tree-walking evaluator. Two placeholders, not one.

| | |
|---|---|
| `beck test clbg/pidigits.beck`, release | **4.4 s** for the file's four tests |
| the same, debug | 15.9 s |
| what `N = 30` alone needs | **under 16,000,000 steps** — it fails at 15,000,000 and passes at 16,000,000, so it is inside the default budget and takes no `--fuel` |

**That row was 100,000,000 when this port was first written**, which would have made it the fourth
benchmark to need [`62`](62-fuel-report.md)'s `--fuel` and the first to need it *in the gate*:
`awfy/`'s three run reduced configurations in `cargo test` and their large sizes are a choice a
person makes at the command line (§62.3), and `pidigits` has no such choice, because the Game
publishes exactly one expected output and it is at `N = 30`. A per-benchmark budget table in
`clbg.rs` was the first answer. §69.6 is why it is not the answer that shipped.

Two smaller things the port records:

- **Its entry point is fallible and the others are not.** `/` on a `Big` raises on a zero divisor;
  `s` is zero in every matrix the algorithm builds and `t` is a product of odd numbers, so the
  divisor never is — and the type system has no way to be told. The row travels to
  `pidigits_output`, the test opens it with `try:` and compares against `Ok(value="…")`, and
  `clbg.rs`'s oracle check accepts that second shape. A swallowed error with a made-up default would
  have kept the shape uniform and would have been a lie.
- **The padding case is asserted at `N = 17`, not `N = 27`.** A last line shorter than ten columns is
  padded with spaces, and the Game's file cannot show that because 30 is a multiple of ten. The
  digits in that assertion are the first 17 of the published thirty, so it is not a second oracle —
  the thing under test is the three spaces.

## 69.6 The long division underneath it, which is what the two minutes actually were

A gate that costs two minutes is a gate somebody stops running, and "the benchmark is inherently
expensive" is the kind of answer that should be checked before it is accepted. It was wrong.

[`55`](55-bignums-report.md) §55.6 had already named the suspect, in the row that says what a first
implementation deliberately did not do:

> **Knuth's algorithm D** — **not built**, and this is the thing to replace first if any of it is
> ever a bottleneck. The binary search for a trial digit is fourteen comparisons where the
> estimate-and-correct is one multiply and a rare fixup; it is here because it is *obviously* right,
> which for a first division is the trade to make.

`pidigits` is that bottleneck arriving. Every digit of pi costs several long divisions of
hundred-digit numbers, every long division costs one trial digit per limb of the dividend, and every
trial digit was a fourteen-step binary search over `0..9999` — each step multiplying the **whole
divisor** by a candidate. Most of those limbs divide to zero, and the search paid full price to
discover it.

**What changed is four lines and it is not algorithm D.** Knuth's *estimate* now brackets the
search instead of replacing it:

- the divisor is at least its top limb times `base^(n-1)`, so the digit is at most `(H + 1) / v`;
- it is less than one more than its top limb times `base^(n-1)`, so the digit is at least
  `H / (v + 1)`;

where `v` is the divisor's top limb and `H` is the dividend's limbs above the divisor's length — two
integer divisions, against a multiplication over every limb per search step. The search still runs
between those bounds, so **the digit is still the one the search would have found and the search is
still what proves it**; when the top limb is large the bracket is one digit wide and the search
confirms it in a single comparison, and when the top limb is 1 the bracket is wide and the cost is
what it always was. Never worse, because the bounds hold either way. Algorithm D's other half — the
normalisation that makes the bracket narrow *always* — is still not built, and now has a measured
price rather than an assumed one.

| | before | after |
|---|---|---|
| `pidigits`, steps for `N = 30` | just over 100,000,000 | **under 16,000,000** |
| `beck test clbg/pidigits.beck`, debug | 89 s, and only with `--fuel 200000000` | **15.9 s**, default budget |
| `cargo test -p beck-cli --test clbg`, debug | 127 s | **52 s** |
| `beck test lib/decimal.beck`, debug | 8.0 s | **3.5 s** |

The last row is the one that matters most, and it is not a benchmark: `decimal.beck` divides on
every rounded quotient it computes, and nothing about it changed. **Every caller of the standard
library's arbitrary-precision division got 2.3× faster**, which is an argument for the
[`46`](46-standard-library-report.md) division that put this in Beck rather than in Rust — the fix
is in a file, in the language, with the library's own property tests as the check.

Those tests are the reason this is a safe change to make in an afternoon.
`bignum.beck`'s `property` blocks check every result against `Int` arithmetic over 100 generated
pairs, and `stdlib.rs` checks 400 more against `i128` and `decimal.beck`'s rounding against exact
rational arithmetic. Two tests were added for what a bracket specifically can get wrong: a divisor
whose top limb is `1` (the widest bracket), one whose top limb is `9999` (the narrowest), a quotient
limb of zero and one of 9,999 — and a property that divides `divisor · d + r` back by `divisor` for
every digit `d` a limb can hold, because a bound that is wrong one time in ten thousand is not
something a hundred random pairs will find.

## 69.7 A larger one underneath it, **not fixed**: the accumulator idiom is quadratic

§69.6 is a library being slow. This is the language being slow, it is bigger, and it is recorded
here rather than repaired because the repair is a compiler feature rather than an edit.

**`list_append` copies the whole list.** `beck-eval/src/interp.rs`, `Prim::ListAppend`, is
`as_list(&xs)?.to_vec()` and a push. Beck has no mutable sequence, so every loop that builds one is
written as a tail-recursive accumulator — `return go(i + 1, list_append(done, x))` — which is how
`lib/` accumulates limbs, how `awfy/` and `clbg/` build their arrays, how the corpus builds lists
and how both SICP chapters do. Every one of them is therefore **O(n²) in time**.

Measured, release build, over that exact loop:

| n | wall clock | ratio | evaluator steps | steps per element |
|---|---|---|---|---|
| 1,000 | 13 ms | | 14,014 | 14.0 |
| 2,000 | 33 ms | 2.5× | 28,014 | 14.0 |
| 4,000 | 105 ms | 3.2× | 56,014 | 14.0 |
| 8,000 | 385 ms | 3.7× | 112,014 | 14.0 |

**The two halves of that table are the finding.** Time approaches 4× per doubling — quadratic — and
the step count is exactly 2× per doubling — linear, 14 steps an element, flat. So the cost is real
and **the evaluator's own budget cannot see it**: a step is a node evaluated, and a primitive that
copies ten thousand values is one step. [`62`](62-fuel-report.md) built `--fuel` as the backstop
that "bounds one evaluation"; it bounds one evaluation's *nodes*, and a program can do unbounded
work inside a bounded number of them. That is a second finding and it is about the backstop rather
than about lists.

**How much it costs today, on real programs rather than that loop.** Counting primitive calls
through the evaluator:

| | `list_get` | `list_len` | `list_append` | elements copied by the appends |
|---|---|---|---|---|
| `clbg/pidigits` | 593,981 | 407,801 | 326,723 | **3,374,778** — 10.3 per append |
| `awfy/havlak` | 138,602 | 283,673 | 134,100 | 116,889 — 0.9 per append |

Two things follow, and the second is a correction to how the defect reads above. **Reads outnumber
appends about three to one**, which is what decides between the two fixes below. And the copying is
*not* what makes these two programs slow: the lists in them are limb vectors and small collections,
ten to twenty-five elements long, so ten copied `Int`s per append is real waste and is nowhere near
the 16,000,000 evaluator steps beside it. The quadratic bites in proportion to how long a list gets,
which means it is invisible in a tree of programs that never build a long one and unbounded in the
first program that does — a 100,000-element list costs five billion element copies to build. It is a
defect waiting for its caller, in exactly the way division was waiting for `pidigits`.

[`19`](19-phase-1-report.md) §19.4 item 3 found this exact shape in the *fold* — "the accumulator was
being copied on every insert, making a fold over a log `O(events × rows)`" — and
`beck-cli/tests/scaling.rs` exists to keep it fixed, opening with the sentence that settles what
this is: **"That is a semantic defect, not a backend one: it would survive into Cranelift
unchanged."** By the repository's own standard this is a defect, not a cost.

**The obvious cheap fix does not work, and it is worth writing down why.** Making `Prim::ListAppend`
push in place when it holds the only reference — `Arc::try_unwrap` on the `Arc<Vec<Value>>` — was
tried and measured: **no change at any size**. At the moment `list_append(done, x)` runs, the
caller's frame still binds `done`, so the reference count is two and the copy happens anyway. The
value's owner is the environment, and the environment does not know the binding is dead.

So the fix is one of two real changes, and both are somebody's next piece of work rather than this
one's:

| | what it is | what it costs the 3:1 majority |
|---|---|---|
| **Last-use moves** | Compute, per function body, which occurrence of each local is its last, and have the evaluator *take* the binding from the frame there instead of cloning it. Then the reference count at the append is one and the push is in place — the `try_unwrap` above becomes the other half of the fix rather than dead weight. A liveness pass over `Core` plus a change to variable lookup; its correctness rests on closures, since a frame captured by one cannot be emptied | **nothing.** A list stays a contiguous `Vec`, so `list_get` stays one indexed load and `list_len` stays a field read |
| **A persistent sequence** | Replace `Arc<Vec<Value>>` with an RRB or similar, making append `O(log n)` with sharing and no analysis at all. A new dependency ([`07`](07-dependencies.md), [`adr/0004`](adr/0004-full-cargo-deny-gate.md)'s allowlist) and a change at all fifty `Value::List` sites, twenty-six of which take a contiguous slice the structure cannot hand out | **a pointer chase per read.** `list_get` becomes `O(log n)` with a cache miss per level, on the operation the table above says happens three times as often as the one being fixed |

**The first, and the measured mix is why**: the second makes the common operation slower to make
the rarer one asymptotically better, which is the wrong trade for this workload however good the
library is. It is also the one that keeps paying — uniqueness information is what lets a *compiled*
backend turn a functional update into an in-place write, which is how Koka's Perceus and Roc reach
the performance a persistent-vector language like Clojure or Scala does not aim at. `Arc<Vec>` with
last-use moves is the same strategy at interpreter scale.

Either way it wants its own change, its own measurement over `awfy/` and `clbg/`, and a scaling gate
in `scaling.rs` alongside the fold's — and that gate must count **work rather than steps**, because
the table above is the proof that steps do not see it.

## 69.8 How it is tested

| | |
|---|---|
| `beck_core::project` | a module resolves from the library with no file beside the root; a file beside the root shadows the library's module of the same name; the library's tests do not become the program's |
| `stdlib.rs` | every file in `lib/` is importable from a directory that is not `lib/` — the strong form, `beck test` on a probe that imports it, so a file added to the directory and left out of `MODULES` fails here; the whole library links into one program; a local module shadows |
| `clbg.rs` | `pidigits` runs and verifies against the published file, on the default fuel budget like every other file there; `import bignum` works from `clbg/`; the two remaining unported benchmarks are still unportable, and neither has an oracle file sitting unused |
| `bignum.beck` | the trial-digit bracket, at the divisor's widest and narrowest leading limb, at a quotient limb of zero and of 9,999, and over every digit a limb can hold — plus the property blocks that already checked division against `Int` arithmetic |
| `beck-core::stdlib` | the table is a sorted set and nothing in it is empty |

The gate that matters most is the first `stdlib.rs` one, because it is the one that would have
failed before this change. `docs/68` §68.4's probe — one file, two directories — is now a test in
two places rather than a paragraph in a report.

## 69.9 What this corrects

- **[`68`](68-clbg-report.md) §68.4 is fixed rather than recorded.** Its three findings: the import
  limitation is gone, `format.beck` is in `lib/`, and the flat namespace's cost is unchanged and now
  reaches the library (§69.2).
- **[`68`](68-clbg-report.md) §68.6's table is out of date in two rows.** `pidigits` is ported, and
  "a `lib/` import path — **not built**" is built. `mandelbrot` and `regexredux` stand, for the
  reasons that report gives.
- **[`68`](68-clbg-report.md) §68.7's "the standard library has never had a consumer, and could not
  have had one" is answered.** It has eight — the Benchmarks Game's ports, every one of which
  imports `format`, and one of which, `pidigits`, is written on `bignum`.
- **[`55`](55-bignums-report.md) §55.6's "Knuth's algorithm D — **not built**" is half wrong now.**
  His *estimate* is built, as a bracket around the search rather than a replacement for it; his
  normalisation is not, and §69.6 says what it would buy and when. That row's own condition — "the
  thing to replace first if any of it is ever a bottleneck" — is what made this a change rather than
  a discussion.
- **[`46`](46-standard-library-report.md)'s directory is a standard library in the ordinary sense**
  for the first time — reachable by name from any program, versioned with the compiler that
  understands it.
- **[`16`](16-packages-and-ecosystem.md) §16.7 is unaffected.** Nothing here decides how `@beck/std`
  will be spelled; D23 says so explicitly, and `adr/0018` names the namespaced import as what
  supersedes it.

## 69.10 What is not built

| | |
|---|---|
| Qualified or namespaced imports | **not built.** `import bignum` brings every name in it into one flat namespace, so importing a library reserves its helper names. This is the largest thing D23 leaves open and it is a package-system decision ([`16`](16-packages-and-ecosystem.md) §16.7) |
| Selective import (`from x import y`) | **not built**, and it is the cheap half of the above — worth considering before the package system rather than with it |
| A third-party path | **not built.** There is one implicit source and it is the compiler's own library. `beck add` is [`16`](16-packages-and-ecosystem.md)'s |
| An interface cache | **not built**, per §69.4. Every import is checked from source on every build |
| The LSP resolving imports | **not built**, and unchanged: [`65`](65-lsp-report.md) §65.4 already records that a file is analysed alone, so a name imported from *anywhere* — sibling or library — is unresolved in the editor. This change makes that gap easier to hit, and does not widen it |
| `mandelbrot`, `regexredux` | **not ported**, per [`68`](68-clbg-report.md) §68.6. Both reasons are facts about the language and `clbg.rs` still asserts them |
| A number for `pidigits` worth comparing | **not published**, per [`25`](25-benchmarks-and-expressiveness.md) §25.9 and §69.5. It measures our bignum library on our placeholder evaluator |
| Knuth's normalisation | **not built**, per §69.6. Scaling both operands so the divisor's top limb is at least half the base would make the bracket one digit wide *always*, rather than usually; what is built is the estimate alone, and the case it does not help — a divisor whose leading limb is 1 — is the case a test now pins |
| Sub-quadratic multiplication | **still not built**, per [`55`](55-bignums-report.md) §55.6. Division got cheaper; multiplication is the schoolbook it was, and `pidigits` is now the caller that would notice if it changed |
| A linear `list_append` | **not built**, per §69.7, and it is the largest performance item this change found. Every accumulator loop in the language is quadratic in time, and the evaluator's step budget cannot see it |
| A work-counting budget | **not built.** `--fuel` counts nodes evaluated; §69.7's table is one program whose steps are linear and whose cost is quadratic, which is what a node count cannot distinguish |

## 69.11 What this establishes, and what it does not

**It establishes that a program outside `lib/` can use the standard library**, which is a sentence
three reports have implied and none could have made. The evidence is not the compiler's own tests:
it is that eight benchmark ports in another directory now import `format` from it, one of them
computes 30 digits of pi through `lib/bignum.beck`, and the answer is character-for-character the
file the Benchmarks Game published.

**It establishes nothing about the package system.** One implicit source, no versions, no
namespaces, no third-party anything. What [`16`](16-packages-and-ecosystem.md) describes remains
entirely ahead, and the one decision taken here that touches it — the precedence rule — is written
down as the thing a namespaced import would supersede.

**And it is the first time a benchmark in this repository has made the language faster rather than
just measured it.** [`53`](53-are-we-fast-yet-report.md), [`53`](53-are-we-fast-yet-report.md)
and [`63`](63-felleisen-report.md) each found something *missing* — short-circuiting `and`, `sin`
and `cos`, a function type with no arguments. §69.6 is a different kind of finding: nothing was
missing, and a cost nobody could see was being paid by every program that divided a big number. The
suite's value is not the numbers it prints, which §25.9 will not let us publish anyway. It is that
it is the first caller demanding enough to make a cost visible.

**And §69.7 is the same lesson learned the hard way.** The division fix came from asking why a
number was large; the quadratic underneath it came from asking the same question one level further
down, and only after somebody said the first answer was not good enough. A benchmark suite that
prints numbers nobody interrogates is a suite that measures nothing, and the two findings above are
what interrogating one produces. §69.7 is the one still owed.
