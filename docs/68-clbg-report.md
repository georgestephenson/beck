# 68 — Phase 3, part 37: the Benchmarks Game harness, and the standard library nothing can import

**Built.** [`compiler/clbg/`](../compiler/clbg/README.md) — seven of the Computer Language
Benchmarks Game's ten benchmarks, each verified against the Game's own published output file — and
[`clbg.rs`](../compiler/crates/beck-cli/tests/clbg.rs), the gate, with
[`measure_clbg.rs`](../compiler/crates/beck-cli/tests/measure_clbg.rs) beside it.

This is the **last item on [`25`](25-benchmarks-and-expressiveness.md) §25.9's Phase 3 row**.
[`64`](64-compile-speed-report.md) §64.7 said so — "it is now the *only* thing left" — and §64.7.1
said why it had not been attempted:

> The Computer Language Benchmarks Game publishes its expected output per benchmark and per `N`,
> and in this environment both its site and its repository are refused by the network policy. […]
> So the ten benchmarks could be *written* here and not one of them could be *verified*, which
> would produce a suite that measures Beck against numbers this repository made up. […] The harness
> is therefore owed **with its sources to hand** rather than owed generally.

The sources are to hand. The rest of this report is what they bought and what they did not.

## 68.1 What was ported, and what it is verified against

Seven of ten. Each is a Beck library whose `test` block asserts the Game's published output for the
`N` the Game publishes one at.

| Benchmark | `N` | The Game's own oracle | Ported from |
|---|---|---|---|
| `spectralnorm` | 100 | `1.274219991` | Jarkko Miettinen, modified by Isaac Gouy |
| `nbody` | 1000 | `-0.169075164`, then `-0.169087605` | Mark C. Lewis |
| `fannkuchredux` | 7 | `228`, `Pfannkuchen(7) = 16` | Oleg Mazurov |
| `binarytrees` | 10 | six lines of tree counts | Jarkko Miettinen |
| `fasta` | 1000 | 10,245 characters | Mehmet D. AKIN |
| `revcomp` | — | 10,245 characters, from the Game's published input | Leonhard Holz |
| `knucleotide` | — | 26 lines, from the same input | James McIlree, Tagir Valeev |

Not ported: `mandelbrot`, `pidigits`, `regexredux`. §68.6 is each reason, and each is a fact about
the language rather than about effort — which is why `clbg.rs` asserts all three are *still* facts,
so that the change removing one turns a test red.

## 68.2 The oracle, which is the whole point of the exercise

§64.7.1's objection was not that a harness would be hard. It was that a harness verified against
invented numbers is **worse than no harness**, and it named the precedent:
[`59`](59-havlak-report.md) §59.5, where a benchmark whose workload was discarded had no oracle for
its workload, so a loop that ran once instead of fifty times passed every verification the suite
published.

So the answer here is not "we were careful". The Game's expected-output files are checked in
verbatim under [`clbg/expected/`](../compiler/clbg/expected/), and `clbg.rs` makes them the only
oracle by construction:

- **Five outputs are small**, so the port asserts the exact text and `clbg.rs` *rebuilds that
  literal* from the published file — escaping it the way Beck's lexer would — and fails if the
  source does not contain it. `each_ports_asserted_output_is_the_published_file_byte_for_byte`.
- **Two are 10,245 characters.** A literal that long asserts the same thing and is no more
  checkable by a reader, so the port asserts `digest(…) == "<hex>"`, and `clbg.rs` recomputes the
  hex from the published file with the same BLAKE3 the `digest` primitive is.

The property that buys is worth stating plainly: **a wrong constant fails the Beck test, and a
wrong constant with a matching wrong expectation fails the Rust one.** Nothing in the directory
chose a number. That is a stronger guarantee than [`awfy/`](../compiler/awfy/README.md) has, where
the constants are transcribed from the original's `verifyResult` by hand and trusted — and it is
stronger because it could be, not because Are We Fast Yet was done carelessly. Are We Fast Yet
publishes constants inside source files; the Benchmarks Game publishes *files*.

The same rule covers the pipeline. `revcomp` and `knucleotide` read a FASTA file on stdin, and Beck
has neither stdin nor a `test` block that can open a file, so both call `fasta.beck` instead. That
substitution is exact only if the Game's published inputs really are its published fasta output —
so `clbg.rs` asserts `revcomp-input.txt` and `knucleotide-input.txt` are byte-for-byte
`fasta-output.txt` rather than the ports assuming it. They are.

## 68.3 What the port changes, and what each change costs

Four of the five rules are [`awfy/README.md`](../compiler/awfy/README.md)'s, unchanged, because the
same language is missing the same things: an array is a `Map[Int, T]`, a mutable object is a record
rebuilt, a bitwise operator is arithmetic, and a number is the suite's. The fifth is new — a
redirected file is the program that generated it — and §68.2 is why it is exact.

Three differences are larger than mechanical, and each is a thing **not** measured here:

**Nothing is parallel.** `fannkuchredux` and `knucleotide` are both contributed in thread-pool
form. In each case the pool divides a space that is summed before anything is printed —
`fannkuchredux` splits `n!` into 150 chunks across an `AtomicInteger` task counter, and
`knucleotide` runs one task per reading-frame offset — so the union of the tasks is exactly what a
single pass computes. The answers are identical and the work is the same work. What is not measured
is the parallel decomposition, which Beck has no threads to express.

**`binarytrees` measures less here than it does anywhere else.** The Game is blunter about this
benchmark than any other — "don't optimize away the work", "leaf nodes must be the same as interior
nodes, the same memory allocation", no arenas and no pools — because the number it wants is the
allocator's. Beck exposes no allocator, no GC configuration, and nothing about how the evaluator
represents a `Node`. The port honours the *work* (every node built, every node walked, the leaf a
variant of the same union rather than an absent field) and cannot honour the memory rules. That is
a thing to know before reading a number off it, and not a reason to leave it out.

**`nbody` is the second port of that program in the tree**, and the duplication is deliberate.
`awfy/nbody.beck` is Are We Fast Yet's, which is itself derived from this one. The two suites verify
different things: seventeen significant figures of an `f64` after one advance, against two printed
decimals after a thousand. A port satisfying one would not have been checked against the other, and
having both means the same physics is held to a bit-level constant by one suite and to a rounded
decimal by the other.

## 68.4 Three findings, and the largest is about the standard library

None of these is about speed, and the first is the most consequential thing in this report.

### `lib/` is a standard library that nothing outside `lib/` can import

`pidigits` is a spigot over arbitrary-precision integers. Beck has those:
[`lib/bignum.beck`](../compiler/lib/README.md) is [`55`](55-bignums-report.md)'s, and §64.7.1 named
it as the thing that would make `pidigits` easy — "`55`'s bignums are what `pidigits` needs and
what would have been the awkward one".

It could not be imported, and the demonstration is one file in two directories:

```console
$ cp probe.beck lib/    && beck test lib/probe.beck
test "reachable?" … ok

$ cp probe.beck clbg/   && beck test clbg/probe.beck
error[B0603]: cannot find module `bignum`
  = note: looked for `bignum.becki` and `bignum.beck`
```

`import x` resolves against **the directory the root module lives in**, and against nothing else
(`beck-cli/src/main.rs`, `struct Dir`). There is no search path, and `beck.lock` carries placement
and not module sources. So the Beck half of the standard library is reachable from `lib/` and from
nowhere: `decimal.beck` can import `bignum.beck` because they are siblings, and no program a user
writes can import either.

**Nothing had noticed because nothing had tried.** `lib/` files import each other,
`corpus/project/` imports within itself, `awfy/` imports nothing, and `sicp/` imports nothing. A
benchmark suite in a new directory that wants the numeric tower is the first thing in three phases
of work to reach across a directory boundary — which is exactly the shape of
[`56`](56-decimal-report.md) §56.5's finding, where the first `lib/` file to import a *sibling*
found three tool bugs in an afternoon.

This report does not fix it, and the reason is that the fix is a **design decision rather than a
repair**. Making `import bignum` work from anywhere is deciding that `lib/` is on an implicit
search path — which is what every language does and what `lib/README.md` already implies it is —
but it changes name resolution for every program in the language, and it interacts with the flat
namespace below. That belongs in [`10`](10-decisions.md) and an
[`adr/`](adr/), taken deliberately, and not in a benchmark's change. `pidigits` is owed on it, and
`clbg.rs` asserts the limitation so that the change lifting it fails a test that names the
benchmark to port.

### Modules link into one namespace, so a benchmark cannot be called `benchmark`

```console
error[B0601]: `benchmark` is defined in more than one module
  = note: Phase 2 links modules into one namespace and has no qualified reference to tell two
          definitions apart, so a clash is an error rather than a shadowing rule
```

`revcomp` and `knucleotide` both import `fasta`. Had each port named its entry `benchmark` — which
is `awfy/`'s convention, and was this directory's until it stopped compiling — the three could
never have been linked together. Every entry point is therefore named after its own benchmark:
`fasta_output`, `revcomp_output`. `clbg.rs` gates the convention rather than leaving it to habit.

The diagnostic is clear and the behaviour is deliberate, so this is a **constraint recorded, not a
defect**. It is worth recording because it is the first time the flat namespace has cost anything:
a directory of independent programs is precisely the case where every file wants the same names,
and this one wanted `benchmark`, `line_length` and `at` three times over.

### A real has no fixed-decimal formatting, and three of the seven need one

`str` on a `Float` is the shortest representation that round-trips — `str(1.0)` is `"1"` — which is
the right default and is not `printf("%.9f")`. `spectralnorm` is a `DecimalFormat("#.000000000")`,
`nbody` pins nine fraction digits top and bottom, and `knucleotide` is `"%.3f"`; none of the three
can be *checked* at all without it, because the Game's oracle is a file and the file has the digits
in it.

[`clbg/format.beck`](../compiler/clbg/format.beck) is that function, written in Beck, and it is the
one thing in the directory that is not a benchmark. It belongs in `lib/` and is not there, for the
reason above: the directory that needs it could not import it back. Recorded here as a
standard-library gap found by porting rather than as a benchmark's private helper, because that is
what it is.

## 68.5 The numbers, and what they are not

`cargo test --release --test measure_clbg -- --nocapture`, this four-core container:

```
benchmark          check ms    test ms   difference
----------------------------------------------------
binarytrees             5.9      597.8        592.0
fannkuchredux           7.5      658.9        651.4
fasta                   7.6     1471.3       1463.6
format                  5.1        5.4          0.3
knucleotide             9.7     2497.9       2488.1
nbody                   7.7      529.2        521.5
revcomp                 9.1     2831.5       2822.4
spectralnorm            7.7     2639.9       2632.2
```

**No comparative claim is made, and the Game's own table is the specific comparison being
declined.** §25.9 holds every comparative claim until a second backend exists; §25.2 calls the Game
"widely quoted and widely misused"; and §25.3 measured the evaluator at about 33× CPython on the
workload most favourable to an interpreter. Entering a number from a placeholder interpreter into
that table would be the misuse, not a contribution to it. Condition 3 of the Game's licence says
the same thing from the other side, and `clbg/README.md` records it: nothing here is endorsed by
the Benchmarks Game and no number here has been submitted to it.

Three things make even the internal numbers weaker than they look, and all three are stated in
`measure_clbg.rs` rather than left for a reader to discover:

- **Every port runs the Game's format-checking size, not its measuring size.** Each description
  page gives two — `N = 100` for `spectralnorm` against 5,500 "to check program performance", 7
  against 12, 1,000 against 50,000,000. The oracle exists only at the first, and this directory is
  built around the oracle. So these are times for programs a hundred to fifty-thousand times
  smaller than the ones the Game's table is about.
- **A file runs its imports' tests too.** `revcomp`'s 2,831 ms includes `fasta`'s three tests.
- **A `test` block has no local bindings** (`B0705`; §21.2 admits no fixture, deliberately), so an
  assertion about five properties of one 10 KB output computes that output five times. `revcomp`
  and `knucleotide` both do, and most of both numbers is that.

One limit of `format.beck` is worth naming because it is the kind of thing that is true until it
suddenly is not: `fixed` rounds **half away from zero** and `printf` rounds **half to even**. They
differ only when the scaled value lands exactly on a half, which none of these does — they are
quotients and square roots — but a benchmark added here whose value *is* a tie would need the other
rule, and would find out by failing.

## 68.6 What is **not** built

| | Status |
|---|---|
| `mandelbrot` | **not ported.** The published output is a binary PBM: `P4\n200 200\n` and then packed bits, NUL bytes included. Beck's `Str` is UTF-8 and there is no byte string, so the answer cannot be represented, let alone compared. Are We Fast Yet's variant — which returns a checksum instead of an image — *is* ported, at `awfy/mandelbrot.beck` |
| `pidigits` | **not ported**, per §68.4. The bignums exist and cannot be imported. This is the one of the three that is a repository limitation rather than a language one, and the only one that is owed a fix |
| `regexredux` | **not ported.** The Game requires its nine specific patterns — "use the same simple regex patterns" — and Beck has no regex. Writing one in Beck would make the benchmark measure our regex engine, which is the one thing that instruction exists to prevent |
| The Game's measuring sizes | **not run**, per §68.5. No oracle exists at them |
| A rate gate | **not built, deliberately.** [`13`](13-testing.md) §13.7's "a gate that flakes gets deleted", the same as `measure_awfy.rs` |
| An entry in the Game's table | **not made, and not to be made**, per §68.5 |
| A `lib/` import path | **not built**, per §68.4. It is a decision for [`10`](10-decisions.md) and an ADR, not for this change |

## 68.7 What this corrects

- **[`25`](25-benchmarks-and-expressiveness.md) §25.9's Phase 3 row is complete.** "SICP stage 1;
  the Felleisen table; compile-speed budgets; Are We Fast Yet and CLBG harnesses against the
  evaluator" — the last of the five. Its "Published" column is kept: chapter 1's line comparison
  and no compute number.
- **[`08`](08-roadmap.md) §8.4's Phase 3 row is met.** "Are We Fast Yet and CLBG harnesses, run
  against the evaluator" — [`61`](61-deltablue-report.md) §61.6 recorded the first half; this is
  the second.
- **[`64`](64-compile-speed-report.md) §64.7.1's blocker is discharged rather than worked around.**
  Its condition was the harness be built "with its sources to hand", and every constant in the
  directory is derived from a checked-in file the Game published — enforced, per §68.2, not
  asserted.
- **[`64`](64-compile-speed-report.md) §64.7's "only thing left" is no longer left.**
- **The standard library has never had a consumer, and could not have had one**, per §68.4. Three
  phases of reports describe `lib/` as the standard library; this is the first measurement of what
  a program outside it can do with it, and the answer is nothing.

## 68.8 What the harness establishes, and what it does not

**It establishes that Beck computes what these seven programs compute** — character for character,
against files somebody else published, including a 10,245-character FASTA output that depends on a
specified generator's exact arithmetic and a reverse-complement of it that depends on all 10,245.
That is a correctness result about reals, integer arithmetic, string handling, maps and sorting,
obtained from an oracle with no stake in Beck being right, and it is the strongest kind of evidence
this project can currently produce for the front end and the evaluator together.

**It establishes nothing about speed.** Not against the Game's table, not against another language,
and not against another Beck backend, because there is not one yet. The three qualifications in
§68.5 mean the numbers are barely comparable to each other.

**It does not establish that Beck can express the Benchmarks Game.** Seven of ten is what was
reached, and the three that were not are named in §68.6 with the language feature each is waiting
on — a byte string, an import that reaches `lib/`, a regex.

## 68.9 What Phase 3 is still not

Unchanged from [`64`](64-compile-speed-report.md) §64.9. The exit criterion — an outside developer
building a non-trivial app from documentation alone — is not met and is not closer; §68.4 is a
small argument that it is further away than it looked, since the standard library a documented
program would import cannot be imported. Seven bullets of the fourteen remain untouched, identity
has its seam and not its relying party, and [`26`](26-arrangement-sharing-report.md) §26.9 still
names them one at a time.
