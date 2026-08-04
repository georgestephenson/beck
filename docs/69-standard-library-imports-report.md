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
imports at all (§69.8), so the day it does, this table is what it adds to
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
| `beck test clbg/pidigits.beck --fuel 200000000`, release | **23.3 s** for the file's four tests |
| the same, debug | 89 s |
| what `N = 30` alone needs | **just over 100,000,000 steps** — it fails at 100,000,000 and passes at 101,000,000 |

That makes it the fourth benchmark to need [`62`](62-fuel-report.md)'s `--fuel`, and the first that
needs it **in the gate**. `awfy/`'s three run reduced configurations in `cargo test` and
their large sizes are a choice a person makes at the command line (§62.3); `pidigits` has no such
choice, because the Game publishes exactly one expected output and it is at `N = 30`. So `clbg.rs`
carries a per-benchmark budget table, and the whole `clbg` suite now takes 127 s in a debug
`cargo test` where it took a few seconds. That is the price of the only oracle there is, paid
knowingly.

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

## 69.6 How it is tested

| | |
|---|---|
| `beck_core::project` | a module resolves from the library with no file beside the root; a file beside the root shadows the library's module of the same name; the library's tests do not become the program's |
| `stdlib.rs` | every file in `lib/` is importable from a directory that is not `lib/` — the strong form, `beck test` on a probe that imports it, so a file added to the directory and left out of `MODULES` fails here; the whole library links into one program; a local module shadows |
| `clbg.rs` | `pidigits` runs and verifies against the published file; `import bignum` works from `clbg/`; the two remaining unported benchmarks are still unportable, and neither has an oracle file sitting unused |
| `beck-core::stdlib` | the table is a sorted set and nothing in it is empty |

The gate that matters most is the first `stdlib.rs` one, because it is the one that would have
failed before this change. `docs/68` §68.4's probe — one file, two directories — is now a test in
two places rather than a paragraph in a report.

## 69.7 What this corrects

- **[`68`](68-clbg-report.md) §68.4 is fixed rather than recorded.** Its three findings: the import
  limitation is gone, `format.beck` is in `lib/`, and the flat namespace's cost is unchanged and now
  reaches the library (§69.2).
- **[`68`](68-clbg-report.md) §68.6's table is out of date in two rows.** `pidigits` is ported, and
  "a `lib/` import path — **not built**" is built. `mandelbrot` and `regexredux` stand, for the
  reasons that report gives.
- **[`68`](68-clbg-report.md) §68.7's "the standard library has never had a consumer, and could not
  have had one" is answered.** It has eight — the Benchmarks Game's ports, every one of which
  imports `format`, and one of which, `pidigits`, is written on `bignum`.
- **[`46`](46-standard-library-report.md)'s directory is a standard library in the ordinary sense**
  for the first time — reachable by name from any program, versioned with the compiler that
  understands it.
- **[`16`](16-packages-and-ecosystem.md) §16.7 is unaffected.** Nothing here decides how `@beck/std`
  will be spelled; D23 says so explicitly, and `adr/0018` names the namespaced import as what
  supersedes it.

## 69.8 What is not built

| | |
|---|---|
| Qualified or namespaced imports | **not built.** `import bignum` brings every name in it into one flat namespace, so importing a library reserves its helper names. This is the largest thing D23 leaves open and it is a package-system decision ([`16`](16-packages-and-ecosystem.md) §16.7) |
| Selective import (`from x import y`) | **not built**, and it is the cheap half of the above — worth considering before the package system rather than with it |
| A third-party path | **not built.** There is one implicit source and it is the compiler's own library. `beck add` is [`16`](16-packages-and-ecosystem.md)'s |
| An interface cache | **not built**, per §69.4. Every import is checked from source on every build |
| The LSP resolving imports | **not built**, and unchanged: [`65`](65-lsp-report.md) §65.4 already records that a file is analysed alone, so a name imported from *anywhere* — sibling or library — is unresolved in the editor. This change makes that gap easier to hit, and does not widen it |
| `mandelbrot`, `regexredux` | **not ported**, per [`68`](68-clbg-report.md) §68.6. Both reasons are facts about the language and `clbg.rs` still asserts them |
| A number for `pidigits` worth comparing | **not published**, per [`25`](25-benchmarks-and-expressiveness.md) §25.9 and §69.5. It measures our bignum library on our placeholder evaluator |

## 69.9 What this establishes, and what it does not

**It establishes that a program outside `lib/` can use the standard library**, which is a sentence
three reports have implied and none could have made. The evidence is not the compiler's own tests:
it is that eight benchmark ports in another directory now import `format` from it, one of them
computes 30 digits of pi through `lib/bignum.beck`, and the answer is character-for-character the
file the Benchmarks Game published.

**It establishes nothing about the package system.** One implicit source, no versions, no
namespaces, no third-party anything. What [`16`](16-packages-and-ecosystem.md) describes remains
entirely ahead, and the one decision taken here that touches it — the precedence rule — is written
down as the thing a namespaced import would supersede.
