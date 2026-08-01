# 24 — Benchmarks and expressiveness

> **Two questions.** Are there standard tests we can measure Beck's performance against
> alternatives with? And: what if the solutions to every SICP exercise were written in Beck — not
> as a performance test but as an *expressiveness* one, on the claim that Beck should have Scheme's
> full means of combination and means of abstraction while being no more verbose, and preferably
> less?
>
> Both answers are yes, and they answer different halves of the same claim. This document names the
> third-party suites layer by layer, says which of them would be honest to run today and which
> would measure a placeholder, and then designs the SICP suite — with the part of it that runs
> today already checked in, measured, and gated.

## 24.1 The rule, and what is already committed

Nothing here is a new policy. [`12`](12-standards-and-conformance.md) §12.1 fixes the rule — **a
claim is a test or it is marketing** — and §12.9 already commits to "a public benchmark
methodology: versus a named baseline stack, reproducible from a repo, with the harness published."
[`08`](08-roadmap.md) Phase 5 already names the baseline: "a published benchmark suite versus a
hand-written React+FastAPI+Postgres+Helm baseline — latency, payload size, image size, build time,
*and* lines of code. That last number is the one that travels."
[`01`](01-vision-and-premise.md) §1.5 already fixes what may be claimed: win where it is
structural, concede where it is artisanal, and benchmark *the shipped system* rather than any one
layer.

So the open question was never *whether* to measure. It is **whose rules**. A suite we design
ourselves and win is worth nothing; the value of a standard suite is that somebody else chose the
workload, and that the alternatives already have published numbers on it. §24.2 is that list.
[`13`](13-testing.md) §13.7 is where the resulting harnesses live, as budgets rather than as
press releases.

## 24.2 The standard suites, layer by layer

Beck is five tiers in one program, so there is no single suite — there is one per tier, and the
composite claim (§1.5 item 3) needs its own. Ordered by how load-bearing they are for what Beck
actually asserts.

| Layer | The standard | What it measures | Fit |
|---|---|---|---|
| **The whole shipped system** | **TechEmpower Framework Benchmarks** — plaintext, JSON serialisation, single query, multiple queries, fortunes, data updates, caching | The industry's reference for "how fast is your web stack", with hundreds of published entries | **The one that matters.** It is the third-party form of Phase 5's baseline-stack commitment: instead of a baseline we assembled, one everybody else has already run |
| **The browser** | **js-framework-benchmark** (krausest) — create 1k rows, replace all, partial update of every 10th of 10k, select, swap, remove, create 10k, append 1k; plus startup metrics and memory | The reference for DOM update cost, with every mainstream framework in it | Exactly the right test of the patch-interpreter client, and the one where Mode A will be *interestingly* penalised — see the methodology note below |
| **Page quality** | **Lighthouse / Core Web Vitals** (LCP, INP, CLS) | User-perceived performance | Already committed, [`12`](12-standards-and-conformance.md) §12.4, as CI gates on the example apps |
| **The language core** | **Are We Fast Yet** (Marr, Daloze, Mössenböck, DLS '16) — 9 micro- and 5 macro-benchmarks written against a *common subset* of language features | Compiler effectiveness across languages, deliberately constructed so that the comparison is of implementations rather than of standard libraries | The methodologically strongest choice for Beck's core, and much harder to game than the alternative below |
| **The language core, popularly** | **Computer Language Benchmarks Game** — binary-trees, fannkuch-redux, fasta, k-nucleotide, mandelbrot, n-body, pidigits, regex-redux, reverse-complement, spectral-norm | Raw compute, per language, as tuned by whoever cared most | Widely quoted and widely misused: entries are hand-tuned, so it measures effort as much as compilers. Worth running for the trend line; not worth citing |
| **Functional compilers** | **nofib** (Haskell), the **R7RS/Larceny Scheme benchmark set** | Allocation-heavy, closure-heavy, recursion-heavy workloads | Closer to Beck's shape than CLBG is, and the Scheme set pairs directly with §24.5 |
| **The log** | **YCSB** workloads A–F | Append and point-read throughput, latency distributions | Maps cleanly onto the durable log. `beck bench log` already exists as the seed ([`08`](08-roadmap.md) Phase 3) |
| **Read models** | **TPC-H**, **ClickBench** | Analytical query latency over a fixed dataset | For the read-model tier once [`05`](05-tier-lowering.md) §5.3 exists. **TPC-C is not a fit** and should not be attempted: it assumes update-in-place OLTP, which is not Beck's data model |
| **Incremental views** | *No standard exists* | — | The nearest published sets are DBToaster's and Noria's query workloads, and TPC-H run under a stream of updates. This is a **gap**, and one Beck is unusually well placed to define a yardstick for rather than borrow one |
| **WASM** | **Sightglass** (Bytecode Alliance), PolyBenchC | Runtime and compile-time cost of WASM modules | For Mode B and the server-side WASM target ([`07`](07-dependencies.md) §7's Wasmtime choice) |
| **Compile speed** | *No standard exists*; **rustc-perf** is the model | Clean build, incremental build, keystroke-to-diagnostic | Copy the instrument, not a suite. [`13`](13-testing.md) §13.7 already lists these budgets |
| **Conciseness** | **TodoMVC**; **Rosetta Code** as a corpus; Prechelt (2000) as the methodological precedent | Lines of code for a fixed task, across implementations | [`00`](00-original-idea.md) already names the calibration pointers: "Electric Clojure's TodoMVC and Lamdera's, read side by side with the sketch" |

Three methodology notes, because each is a place where a number could be published dishonestly.

**js-framework-benchmark measures a browser; Mode A puts a network in the loop.** Every other
entrant computes its patch in the tab. Beck's default computes it on the server and ships a diff,
so a naive entry posts a number that is mostly RTT and says nothing about the architecture. The
honest form is to publish three columns — Mode A at a stated RTT, Mode A at RTT 0 (server and
client in one process, which is `beck run`), and Mode B — and to let the reader see the tier
crossing rather than hide it in an average. If we cannot publish it that way we should not enter.

**TechEmpower's "single query" and "data updates" assume a mutable relational database.** Beck's
data model is a durable fold; the equivalent work is a read model. Running those two tests means
saying, in the entry, what was actually executed. The other five (plaintext, JSON, fortunes,
multiple queries, caching) map without argument, and *caching* is the one to watch: Beck's answer
is an incrementally maintained view rather than a cache, which is the structural win of §1.5 item 3
in a form somebody else's harness measures.

**Lines of code is a real metric and an easy lie.** It only means something with the counting rule
fixed in advance, the same algorithm on both sides, and the comparison stated in more than one
unit. §24.5 sets that protocol out, because the SICP suite is where LOC does the most work.

## 24.3 What a number from Beck would mean today

It would mean almost nothing, and that is worth stating precisely rather than discovering later.

[`19`](19-phase-1-report.md) records that native codegen was not built and that a `Core` evaluator
stands in for Cranelift behind the `Backend` seam; [`20`](20-phase-2-report.md) §20.5 and
[`22`](22-phase-3-report.md) §22.6 both record it as still unbuilt. Every execution number Beck can
produce today is therefore a number about a tree-walking interpreter that is a placeholder by
design. To put a size on it — `fib(30)`, naive, as a `test` block:

```console
$ beck test fib.beck          # best of three, this 4-core container
4.120s                        # 0.004s of which is compile and harness
$ python3 -c 'def fib(n): return n if n<2 else fib(n-1)+fib(n-2)
fib(30)'
0.125s
```

**About 33× slower than CPython**, on the workload most favourable to an interpreter (integer
arithmetic and calls, no allocation). That is not a criticism of the evaluator, which exists to
make the seam load-bearing and does; it is the reason a CLBG entry today would be a measurement of
scaffolding.

The recommendation follows from that, and it is a sequencing recommendation rather than a
deferral:

1. **Adopt the harnesses now, publish the numbers now, and make them unflattering.** A benchmark
   suite acquired *after* the thing it measures gets good has no history and therefore no
   regression-detecting power. The Phase 0 report already takes this position
   ([`18`](18-phase-0-report.md): "treat these as baselines to regress against"), and Phase 0's
   measurements are worth more today than they were the day they were taken for exactly that
   reason.
2. **Publish no comparative claim until the second backend exists.** [`13`](13-testing.md) §13.1's
   oracle table wants Cranelift-vs-LLVM-vs-interpreter differentials anyway; the first honest
   performance claim and the first correctness differential arrive on the same day.
3. **Order the suites by what Beck actually claims.** §1.5 says the claim is about the shipped
   system, not any one layer — so TechEmpower and js-framework-benchmark and Lighthouse come
   *before* CLBG, even though CLBG is the one people ask about. The suite that measures the claim
   we make outranks the suite that measures the claim we explicitly decline to make.

## 24.4 What none of those suites measures

Every suite in §24.2 measures a shipped artefact. Not one of them measures the property the project
was founded on.

[`01`](01-vision-and-premise.md) §1.1 does not say Beck is fast. It says Beck is SICP's three moves
made into a language, and [`02`](02-syntax.md) plus [`10`](10-decisions.md) D9 add a second claim on
top: that a Python-shaped surface can carry Lisp's power without losing any of it — "homoiconicity
is a property of the core AST, not the notation." Those two claims together are the whole premise,
and there is currently **no artefact that could falsify either.** The corpus measures placement
inference. The differential measures splitting. `beck test` measures programs. Nothing measures
whether the means of combination and the means of abstraction survived the trip from Scheme.

The proposal in the brief is the missing instrument, and it is better than it first looks for three
reasons.

**It is the project's own origin, made falsifiable.** [`00`](00-original-idea.md)'s first line is
"I read SICP and had this idea." A suite drawn from any other book would be an arbitrary yardstick;
this one is the premise turned into a test that can fail.

**It has an oracle.** [`13`](13-testing.md) §13.1 organises the whole testing strategy around free
oracles — pairs of independent paths that must agree — because "the hardest problem in testing is
knowing the right answer." SICP states the right answer: exercises have values, and the book prints
them. So the suite is executable and mechanically checked, not a matter of taste. The taste part —
is this more readable? — sits *on top of* a green test rather than instead of one.

**The verbosity claim already has a rigorous form.** "As expressive as Scheme, with the full means
of abstraction and combination, but not more verbose or more steps" is, almost word for word,
Felleisen's criterion (*On the Expressive Power of Programming Languages*, 1991): a language is
more expressive than another if translating a program requires a **global** reorganisation rather
than a local rewrite. Beck has macros ([`11`](11-language-tour.md) §11.7, hygienic, over the same
`Node` AST), so the criterion is directly checkable rather than rhetorical: **if every special form
SICP introduces can be recovered as a Beck macro, Beck is at least as expressive in the formal
sense, and the line count is then a question about ergonomics rather than about power.** That
separation is what stops the exercise collapsing into an argument about braces.

## 24.5 The SICP suite, designed

### The shape

The second edition has 356 exercises — conventionally 46 in chapter 1, 97 in chapter 2, 82 in
chapter 3, 79 in chapter 4 and 52 in chapter 5. **The target is not 356 green checkmarks**, and a
suite that treated it that way would produce a worse result than one that did not: chapters 3 and 5
are substantially about mutable state and machine models that Beck refuses on purpose, and
transliterating them would measure how well Beck imitates a design it rejects.

So every exercise lands in exactly one of three registers, and **the counts in each register are
the headline** — not the pass rate:

| Register | Meaning | Counts toward |
|---|---|---|
| **translated** | The same algorithm, expressed in Beck. The book's stated answer is asserted | The line-count comparison |
| **re-expressed** | The same *problem*, in Beck's idiom, because a transliteration would be a caricature — mutable-queue exercises done as folds, `set!` exercises done as events | Nothing numerical. Both versions are shown, and the report argues the substitution or withdraws it |
| **refused** | Beck cannot express it, and either should (a gap — file it) or should not (a design decision — cite the number in [`10`](10-decisions.md)) | The gap list |

A silent omission is the failure mode this structure exists to prevent. An exercise that is skipped
without a register is a bug in the suite.

### The protocol, pre-registered

Fixed before any solution is written, because every one of these is a place to cheat:

1. **The baseline is third-party and pinned by commit.** A published Scheme solution set, chosen
   and named before we start — [`mk12/sicp`](https://github.com/mk12/sicp) is the strongest
   candidate found, because its solutions are executable with assertions across Chez, Guile and
   Racket, which makes it an oracle rather than a text. Verify its coverage before pinning it. We
   never write the Scheme side ourselves.
2. **Same algorithm, or it is not `translated`.** A Beck solution that wins on lines by using a
   better algorithm, or by calling a prelude primitive where SICP builds the thing from scratch, is
   `re-expressed` and is excluded from the count. §2.2.1 defines `map` as an exercise; using
   `map_list` there is not a shorter solution, it is a different one.
3. **Three counts, always published together**: non-blank non-comment lines; tokens; and lines
   *excluding* type signatures. Beck is statically typed and Scheme is not. Charging Beck for
   annotations Scheme never writes is unfair in one direction, and omitting them is unfair in the
   other, so the report publishes both and lets the reader pick. A single headline number is the
   tell that someone chose.
4. **`beck fmt` decides the formatting**, on both surfaces, so line counts are the formatter's
   opinion and not the author's.
5. **The refusals are published as prominently as the wins.** §24.6 is written that way already.

### Where it lives, and the gate

Not in [`compiler/corpus/`](../compiler/corpus/). That directory has one measured job — placement
inferred with no annotations, the Phase 2 exit criterion — and adding hundreds of pure-function
files would drown the measurement it exists to make.

[`compiler/sicp/`](../compiler/sicp/) is the sibling, with
[`beck-cli/tests/sicp.rs`](../compiler/crates/beck-cli/tests/sicp.rs) as its harness. Each solution
carries the book's answer as its own `test` block, so `beck test` is the gate and the suite is
checked by the same instrument [`22`](22-phase-3-report.md) built. The refusals are checked too:
each is the smallest program that hits its wall, and the harness asserts the wall is **still
standing** — so a wall coming down is a test that starts failing rather than a fact somebody
notices later.

### The chapters, forecast honestly

The forecast is part of the pre-registration: writing down what we expect to lose, before
measuring, is what makes the result worth reading.

| Chapter | What it needs | Forecast |
|---|---|---|
| **1** — procedures | Recursion, HOFs, `lambda`, closures, reals, **proper tail calls** | Beck should be **level with Scheme, or slightly longer** — type signatures cost a line each and buy exhaustiveness. Runs today except for reals and tail calls (§24.6) |
| **2.1–2.3** — data | Recursive types, symbols and quotation, user-written polymorphism, rationals | The chapter that decides whether §2's dual-surface claim survives. **Blocked** on recursive types today |
| **2.4–2.5** — generic operations | Data-directed dispatch, type tags, coercion | **Beck should lose on lines and win on safety, and should say so first.** Scheme's `put`/`get` table is three lines and unchecked; a trait with impls is longer and total. If the report does not concede this row it is not being run honestly |
| **3.1–3.4** — state | `set!`, `set-car!`, local state, the environment model, serialisers | Mostly **refused** and **re-expressed** — this is the design Beck exists to replace, and D-numbered decisions say so. A representative dozen, argued in prose; not 60 files |
| **3.5** — streams | `cons-stream`, delayed evaluation, infinite streams, the merge problem | **The most valuable chapter in the book for this project**, and the one with a trap: Beck already has a type called `Stream` and it means something else — a tier-level construct with a declared merge point, not a lazy list. §3.5.5 is the exact wound [`01`](01-vision-and-premise.md) §1.1 move 3 claims to answer, so translating it forces that distinction to be stated precisely for the first time |
| **4** — the metacircular evaluator | Recursive ADTs, symbols, environments, and macros for the derived expressions | **The Felleisen test proper.** SICP derives `cond`, `let`, `and`, `or` inside its evaluator; Beck can derive them as macros, which is the stronger claim. Expect the evaluator itself to be **longer** in Beck and impossible to get wrong in the ways the Scheme one can. `amb` (§4.3) needs backtracking and is an open question |
| **5** — register machines | Vectors, mutation, symbols, an assembler | Least aligned; expect mostly **re-expressed** and **refused** |

### Staging

Four stages, each gated on the one before, each producing a publishable number rather than a
promise:

1. **Chapter 1** (46). Needs: running a library, reals, proper tail calls. Buys: the first honest
   LOC number, and a real workload for the evaluator.
2. **Chapter 2.1–2.3** (~60). Needs: recursive types, symbols and quotation, user-written
   polymorphism. Buys: the data-abstraction claim.
3. **Chapters 2.4–2.5 and 3.5.** The two that argue *with* Beck — traits against dispatch tables,
   and SICP's streams against Beck's. Buys: the two rows where the report is most likely to have to
   concede, which is what makes the rest credible.
4. **Chapter 4.** The expressiveness claim at full strength.

Chapters 3.1–3.4 and 5 are the `refused`/`re-expressed` register throughout: prose plus a
representative handful, not 130 files.

## 24.6 What runs today, measured

[`compiler/sicp/`](../compiler/sicp/) is stage 1, started, and the results are the useful part of
this document because they were produced rather than predicted. Reproduce with:

```console
$ cd compiler && cargo test --release --test sicp
$ ./target/release/beck test sicp/ch1.beck
```

**Chapter 1 runs.** [`sicp/ch1.beck`](../compiler/sicp/ch1.beck) carries §1.2.1's factorial both
ways, §1.2.2's Fibonacci and the 292 ways to make a dollar, exercises 1.11, 1.12 and 1.16, §1.2.5's
GCD, §1.2.6's smallest divisor and primality, §1.3.1's `sum`, exercises 1.31 and 1.32 (`product`
and `accumulate`), §1.3.2's anonymous procedures and exercise 1.42's `compose` — **13 tests, all
passing, against the book's own stated answers**. Higher-order procedures, procedures returning
procedures, and recursion all work, and they work in the Python surface with type signatures on
every definition. That is a real result: the part of chapter 1 that is about abstraction rather
than about arithmetic or about cost translates without argument. It also round-trips: `beck fmt
--surface sexpr` prints the chapter as S-expressions and `beck test` runs the same thirteen tests
off that file — which is [`02`](02-syntax.md) §2.2's dual-surface claim, checked for the first time
against code that is *about* abstraction rather than about a signal graph.

Six walls stand between that and the rest of the book. Each has a file in
[`sicp/refusals/`](../compiler/sicp/refusals/) and a test that asserts it is still there.

1. **A library cannot be run.** Every SICP solution is a library — pure procedures, no merge point.
   `beck check` accepts one and prints "a library"; `beck test` reports `B0500` and exits non-zero,
   because the runner is built on `Placed` and there is no `Roles` to drive. So `ch1.beck` carries
   a five-declaration todo-shaped application that **nothing in the chapter uses**, purely to be
   executable. It is left in view rather than tidied away. This is already a known gap —
   [`22`](22-phase-3-report.md) §22.6: "a real gap for exactly the modules that most want unit
   tests" — and the suite's first contribution is to make it unignorable.
2. **A type cannot mention itself, or anything declared later.** `collect_types` resolves each
   declaration's field types as it walks the file in source order, so `union Tree: Node(left: Tree,
   …)` is `B0310: cannot find type 'Tree'`, and so is a plain forward reference between two models.
   §2.2 *is* "Hierarchical Data and the Closure Property", so this ends chapter 2 at §2.2 and takes
   chapters 4 and 5 with it. The encouraging half: `cost.rs`, `gen.rs` and `secure.rs` all already
   cut recursive-type cycles and have tests named for it, so the checker's collection order is the
   only wall.
3. **There is no real arithmetic.** `Float` is a type the checker knows; no primitive operates on
   one. §1.1.7 — square roots by Newton's method, the first substantial program in the book, and
   the thing §1.3's whole argument is built out of — does not typecheck. Rationals (§2.1.1) and
   bignums (factorial overflows quickly) are the same question. This is Phase 3's "standard library
   v1" bullet, dated rather than discovered.
4. **A user cannot write a polymorphic definition.** The prelude is polymorphic in both types and
   effect rows — `map_list : (list[a], (a -> b ! e)) -> list[b] ! e`, exactly as §3.2 writes it —
   but `def map[T, U](…)` is a parse error, so every user abstraction is monomorphic. §2.2.1 asks
   the reader to *build* `map`; Beck can only offer the builtin.
5. **A tail call consumes stack.** §1.2.1 is the first idea in SICP that is about cost rather than
   meaning: two procedures compute the same function and one runs in constant space. Beck's
   evaluator is not tail-recursive, so the distinction the chapter is built around **is not
   observable** — both processes are linear in space. Measured on this container: **3,000 frames
   succeed, 4,000 abort**, and it aborts rather than diagnosing:

   ```console
   thread 'main' has overflowed its stack
   fatal runtime error: stack overflow, aborting
   ```

   Two findings, not one. Every Scheme has required proper tail calls since R5RS, so this is a
   conformance gap against the language Beck is being compared to — and a Beck program can kill its
   process with a recursion a user cannot bound, which is a runtime robustness question that has
   nothing to do with SICP.
6. **An `if` over two function values is refused when one branch is a call's result** — and this
   one is a **defect**, not a missing feature. It is what exercise 1.43 (`repeated`) costs, and it
   is the first thing the suite found:

   ```console
   error[B0320]: the two branches may not perform {} here
   ```

   Returning two named definitions from two branches is fine. Returning a named definition and a
   lambda is fine. Returning *anything* alongside the result of a call that returns a function is
   not. The message is the tell: `{}` is the empty effect row, printed where a row variable was
   expected — the branches' rows appear to be unified against each other rather than each against a
   fresh variable, so a call's polymorphic row meets a literal's concrete empty one and the join is
   reported as a conflict. The rendering also fails [`04`](04-compiler-architecture.md) §4.5 on its
   own terms: no user can act on "may not perform `{}`".

That sixth item is the argument for the whole proposal in miniature. Twenty-six corpus programs, a
differential harness, a replay harness and 396 tests did not surface it, because every one of them
is a program shaped like the todo sketch. **Ninety minutes of chapter 1 did**, because SICP is
relentlessly about the one thing this compiler has never been pointed at: building abstractions out
of procedures.

## 24.7 The order of work this implies

The suite is a prioritised list before it is a benchmark. In dependency order:

1. **Run a module with no merge point.** [`22`](22-phase-3-report.md) §22.6 already names it;
   stage 1 cannot honestly start until the wrapper in `ch1.beck` can be deleted. It is also the
   smallest of the six.
2. **Recursive and forward-referencing types.** Two passes over declarations instead of one. Every
   pass downstream already expects them. This unblocks chapter 2, and it is the single highest-value
   item on the list for reasons that have nothing to do with SICP.
3. **The `B0320` defect in §24.6 item 6.** A row-unification bug in `check.rs` that any program
   returning functions from branches will hit.
4. **Proper tail calls in the evaluator**, or an explicit bounded-depth diagnostic in the interim.
   A process abort is not an acceptable outcome for user code, independently of chapter 1.
5. **The numeric tower** — reals first, then rationals and bignums. Phase 3's standard-library
   bullet.
6. **User-written polymorphism.** The largest of the six, and the one most entangled with
   [`03`](03-type-and-effect-system.md).

For §24.2, the order is: adopt Lighthouse and the compile-time budgets now (they measure things
that exist); stand up TechEmpower and js-framework-benchmark harnesses with published, unflattering
numbers as soon as Mode A is complete enough to enter honestly; hold CLBG and Are We Fast Yet until
there is a backend for them to be about.

## 24.8 What this document does not claim

- **No comparison has been run.** No Scheme baseline has been pinned, no line has been counted, and
  no number in §24.6 is a comparative claim. §24.5 is a design; §24.6 is a measurement of Beck
  alone.
- **Chapter 1 is not complete.** Thirteen tests over roughly a dozen exercises of 46, chosen for
  being expressible, which is a biased sample by construction — the arithmetic-heavy exercises are
  absent because they cannot compile, not because they were finished.
- **The exercise counts are conventional**, taken from the second edition's numbering, and should be
  checked against the pinned baseline when one is chosen.
- **The forecasts in §24.5 are forecasts.** They are recorded so that being wrong about them is
  visible.
- **`sicp/` is not on the roadmap.** Nothing in [`08`](08-roadmap.md) has been changed by this
  document, and adopting the suite is a decision that has not been taken.
