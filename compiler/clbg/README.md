# `clbg/` — The Computer Language Benchmarks Game, in Beck

[`docs/25`](../../docs/25-benchmarks-and-expressiveness.md) §25.2 adopts the **Computer Language
Benchmarks Game** for "the language core, popularly", and is blunt about what it is worth:

> Widely quoted and widely misused: entries are hand-tuned, so it measures effort as much as
> compilers. Worth running for the trend line; not worth citing.

§25.9 schedules the harness for Phase 3 and the number for later. This directory is the harness.
[`docs/68`](../../docs/68-clbg-report.md) is the report: what it establishes, what it refuses to
claim, and the findings porting it produced.

## The oracle

This is the one thing about this directory that is different from
[`awfy/`](../awfy/README.md), and it is why the harness did not exist until now.

[`docs/64`](../../docs/64-compile-speed-report.md) §64.7.1 recorded the blocker precisely: the
Game's expected-output files were unreachable from this environment, so "the ten benchmarks could
be *written* here and not one of them could be *verified*, which would produce a suite that
measures Beck against numbers this repository made up." The harness was therefore owed **with its
sources to hand** rather than owed generally.

They are to hand now, so they are checked in verbatim under [`expected/`](expected/), and the
directory is arranged so that **the published file is the only oracle**:

| | how it is asserted | what stops it drifting |
|---|---|---|
| Five small outputs | the port's `test` block holds the exact text | `clbg.rs` rebuilds that literal from `expected/` and fails if the source does not contain it |
| `fasta`, `revcomp` (10,245 characters each) | the port asserts `digest(…) == "<hex>"` | `clbg.rs` recomputes the hex from `expected/` with the same BLAKE3 the `digest` primitive is |

So a constant cannot be invented here: a wrong one fails the Beck test, and a wrong *pair* — a port
and a matching made-up expectation — fails `clbg.rs`. Nothing in this directory chose a number.

## What is here

Eight of the Game's ten, each a Beck **library** whose `test` block is its published output.

| File | What it measures | The Game's own oracle |
|---|---|---|
| [`spectralnorm.beck`](spectralnorm.beck) | Eigenvalue by the power method — float loops | `1.274219991` at `N = 100` |
| [`nbody.beck`](nbody.beck) | The Jovian planets under a symplectic integrator | `-0.169075164` and `-0.169087605` at `N = 1000` |
| [`fannkuchredux.beck`](fannkuchredux.beck) | Indexed access to a tiny integer sequence | `228` and `Pfannkuchen(7) = 16` |
| [`binarytrees.beck`](binarytrees.beck) | Allocating and walking many perfect trees | six lines of counts at `N = 10` |
| [`fasta.beck`](fasta.beck) | A specified LCG and weighted alphabets | 10,245 characters at `N = 1000` |
| [`revcomp.beck`](revcomp.beck) | Reverse-complementing DNA | 10,245 characters, from the input the Game publishes |
| [`knucleotide.beck`](knucleotide.beck) | Hashtable update and k-nucleotide strings | 26 lines, from the same input |
| [`pidigits.beck`](pidigits.beck) | Gibbons' streaming spigot over arbitrary-precision integers | 30 digits, ten to a line, at `N = 30` |

Every file here is a benchmark. The fixed-decimal formatting three of the ports need lives in
[`lib/format.beck`](../lib/README.md), where `docs/68` §68.4 said it belonged and could not go:
until [`docs/69`](../../docs/69-standard-library-imports-report.md) nothing outside `lib/` could
import the standard library, which is also what stopped `pidigits`.

**`pidigits` measures [`lib/bignum.beck`](../lib/README.md)**, which is worth stating beside the
benchmark rather than in a report: every other entry in the Game's table for it measures GMP or a
runtime's built-in big integer, and this one measures schoolbook arithmetic over base-10,000 limbs
written in Beck, on a tree-walker. It was also the first benchmark here to want more than the
evaluator's default step budget — the only size the Game publishes an oracle for is `N = 30`, so
unlike `awfy/`'s three there is no reduced configuration to gate at — and what that bought was a
faster long division rather than a `--fuel` flag in the harness
([`docs/69`](../../docs/69-standard-library-imports-report.md) §69.6). Every file here runs under
the default budget.

`beck-cli/tests/clbg.rs` gates the directory: a file added here is run by being here, the eight
names and the two absent ones are enumerated in one place, and the oracle cross-check above is a
test. `measure_clbg.rs` prints wall-clock and gates on nothing.

## What the port changes

The Game's programs assume stdin, threads, mutable arrays and bitwise operators. Beck has none of
the four. Rather than let each file improvise, the whole directory follows five rules — the first
four are [`awfy/README.md`](../awfy/README.md)'s, unchanged, because the same language is missing
the same things:

1. **A mutable array is a `Map[Int, T]`.** An absent key reads as the Java's zero-initialised
   element (`unwrap_or`), and a write is `map_insert`.
2. **A mutable object is a record, rebuilt.** Where the original mutates two things at once, the
   port carries both in one record.
3. **A bitwise operator is arithmetic.** `1 << k` is `k` doublings (`binarytrees.beck`).
4. **A number is the Game's.** No file invents a verification value, and where a size had to
   change the file says which of the Game's published sizes it is at.
5. **A redirected file is the program that generated it.** `revcomp` and `knucleotide` read a
   FASTA file on stdin; Beck has no stdin and a `test` block has no file to open, so both call
   `fasta.beck` instead. This is exact rather than approximate — the Game's published
   `revcomp-input.txt` and `knucleotide-input.txt` *are* its published `fasta-output.txt`, and
   `clbg.rs` asserts that rather than the ports assuming it.

Two more differences are worth naming separately because they are not mechanical:

- **Nothing here is parallel.** `fannkuchredux` and `knucleotide` are contributed in
  thread-pool form, and both pools exist to divide a space that is then summed before anything is
  printed. Each port runs the whole space in one pass. The answers are identical and the work is
  the same work; what is not measured is the parallel decomposition, which Beck has no threads to
  express.
- **Every entry point is named after its own benchmark** — `fasta_output`, `revcomp_output` — and
  not `benchmark`. Beck links modules into one flat namespace with no qualified reference
  (`B0601`), so two files that both defined `benchmark` could not be imported together, and two of
  these import a third. `clbg.rs` gates the convention.

## What is not here

**Two of the ten are not ported, and each is a fact about the language rather than about effort.**
`clbg.rs` asserts both are still true, so the change that removes one turns a test red.

| | Why not |
|---|---|
| `mandelbrot` | The published output is a binary PBM — `P4\n200 200\n` and then packed bits, NUL bytes included. Beck's `Str` is UTF-8 and there is no byte string, so the answer cannot be represented, let alone compared. (Are We Fast Yet's variant of this benchmark, which returns a checksum instead of an image, *is* ported: [`awfy/mandelbrot.beck`](../awfy/mandelbrot.beck)) |
| `regexredux` | The Game requires its nine specific regex patterns — "use the same simple regex patterns" — and Beck has no regex. Writing one in Beck would make the benchmark measure our regex engine, which is the one thing the instruction is there to prevent |

There were three. `pidigits` was the third, and its reason was never a fact about the language:
Beck had the arbitrary-precision integers it needs and this directory could not reach them
(`docs/68` §68.4). That was a repository limitation, it is fixed, and the benchmark is ported.

**Every port runs the Game's format-checking size, not its measuring size.** Each description page
gives two: an `N` it publishes an expected output for, and a much larger one "to check program
performance" — `N = 100` against 5,500 for `spectralnorm`, 7 against 12 for `fannkuchredux`, 1,000
against 50,000,000 for `nbody`. The oracle only exists at the first, and this directory is built
around the oracle. `docs/68` §68.5 says what that costs.

**There is no comparative number, and there will not be one here yet.**
[`docs/25`](../../docs/25-benchmarks-and-expressiveness.md) §25.9 holds every comparative claim
until a second backend exists, because the tree-walker is a placeholder and a number about it is a
number about scaffolding. What `measure_clbg.rs` prints is wall-clock of this binary on this
machine, and it says so.

## Provenance

These are ports. The originals are the Java programs published by the Computer Language Benchmarks
Game at <https://salsa.debian.org/benchmarksgame-team/benchmarksgame/> — contributed by Jarkko
Miettinen (`spectralnorm`, modified by Isaac Gouy, and `binarytrees`), Mark C. Lewis (`nbody`),
Oleg Mazurov (`fannkuchredux`), Mehmet D. AKIN (`fasta`), Leonhard Holz (`revcomp`), James
McIlree with Tagir Valeev (`knucleotide`) and Isaac Gouy (`pidigits`) — and each file names its own. The expected-output files
in [`expected/`](expected/) are the Game's own, downloaded from `public/download/` in that
repository and unmodified.

The Game's programs and data are licensed under the **BSD 3-clause licence**, whose first condition
is that redistributed source retains the notice. It is reproduced here in full rather than linked,
because a link is not a retained notice, and `clbg.rs` fails if it goes missing:

> Copyright © 2004-2008 Brent Fulgham, 2005-2024 Isaac Gouy
> All rights reserved.
>
> Redistribution and use in source and binary forms, with or without modification, are permitted
> provided that the following conditions are met:
>
> 1. Redistributions of source code must retain the above copyright notice, this list of conditions
>    and the following disclaimer.
>
> 2. Redistributions in binary form must reproduce the above copyright notice, this list of
>    conditions and the following disclaimer in the documentation and/or other materials provided
>    with the distribution.
>
> 3. Neither the name "The Computer Language Benchmarks Game" nor the name "The Benchmarks Game"
>    nor the name "The Computer Language Shootout Benchmarks" nor the names of its contributors may
>    be used to endorse or promote products derived from this software without specific prior
>    written permission.
>
> THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR
> IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND
> FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR
> CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
> DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
> DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER
> IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT
> OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Per condition 3, nothing here is endorsed by the Benchmarks Game, and no result in this repository
has been submitted to it or should be read as one of its measurements.
