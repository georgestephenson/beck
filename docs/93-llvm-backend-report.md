# 93 — Phase 3 report, part 61: the second backend

**Built.** [`05`](05-tier-lowering.md) §5.2's LLVM codegen, over the part of the language that has a
machine representation without a heap: a definition whose parameters and result are `Int`, `Float`
or `Bool` compiles through textual LLVM IR and the host's `clang` to machine code, and runs. There
are two implementations of `beck_core::backend::Backend` at last, so
[`04`](04-compiler-architecture.md) §4.8's **differential test between backends** — carried since
Phase 1 as a shape with the evaluator on both sides, and said so in the file — has a second
implementation to point at. **13,505 calls** across six programs — four written for the purpose,
plus SICP chapter 1 and the Are We Fast Yet mandelbrot port — every one compared for its value
*and* its failure, and no disagreement.

And the first honest compute number, which [`08`](08-roadmap.md) §8.4 held back until exactly this
moment: **160× on `fib(30)`, 374× on a million-iteration loop, 58× on the mandelbrot inner loop**,
against the tree-walker, on the same machine, wall clock.

The number to read first is a different one. **The sketch compiles nothing.**
`examples/todo.beck` has nine definitions and all nine are refused, because every one of them takes
a record or a list. That is the shape of this backend: it is the compute half of the language and
none of the rest, and §93.3 is the list of what that leaves out.

## 93.1 The decision the threat model made

`unsafe_code = "forbid"` at the workspace root, inherited by every crate, is
[`43`](43-threat-model.md) §43.2's strongest claim — the only one that is *structural* rather than
tested or asserted. Both of the obvious ways to build a native backend take it away:
`llvm-sys`/`inkwell` is `unsafe` at the execution engine, and running compiled code in process is a
pointer transmuted into a function under any name.

So `beck-llvm` **writes LLVM IR as text and hands it to `clang`**, and the compiled program **runs
as a child process** that the compiler talks to over a pipe. [`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)
is the record, with the alternatives and what would reverse it. Three consequences worth having in
front of a reader here:

- **The dependency graph did not change.** `beck-llvm` depends on `beck-core` and `beck-diag`.
  There is no crate to pin, no `build.rs`, and [`92`](92-sbom-report.md)'s bill of materials is the
  same bill.
- **A machine without LLVM builds everything and passes everything.** The toolchain is found at run
  time; its absence is a printed skip. `BECK_REQUIRE_LLVM=1` forbids the skip, and that is what CI
  sets — [`19`](19-phase-1-report.md) §19.4 item 10's rule, that a gate which can be silently
  skipped is not one.
- **The artefact is readable.** `beck native --out <dir>` leaves a `.ll` that `opt` will transform
  and a person can read. A codegen defect is a diff in a text file.

## 93.2 Agreeing with the evaluator is most of the work

A backend that is 160× faster and answers something else is worthless, and the interesting thing
about matching `beck-eval` is that the *obvious* lowering is wrong in four places. Each is a
handful of instructions and each was found by reading the evaluator rather than by a failing test,
which is the wrong order and is why every one of them now has a test that fails without it.

**Integer arithmetic is checked, so it cannot be an instruction.** `beck-eval` uses
`i64::checked_*`: an overflow is a *value* — `` `+` overflowed `` — and not a wrapped result. So
`+`, `-` and `*` are `llvm.s{add,sub,mul}.with.overflow.i64` with the overflow bit branched on, and
`/` and `%` carry an explicit guard for a zero divisor **and** for `i64::MIN / -1`, whose quotient
is not representable and which `sdiv` treats as immediate undefined behaviour. A missing guard
there would not have been a wrong answer; it would have been the optimiser deleting the code
around it.

**Reals do not compare with `fcmp`.** [`32`](32-numeric-tower-and-polymorphism-report.md) §32.2
made Beck's `==` on reals *structural*, because a fold's accumulator needs a total order and IEEE
754 supplies none: `Value::Float` stores a monotone transform of the bits, under which `NaN` is the
maximum and equals itself. `fcmp` says `false` to both. A comparison here bitcasts and compares the
order keys — four integer instructions, and `a_negative_zero_and_a_nan_mean_here_what_they_mean_in
_the_evaluator` is the test that would have caught the obvious version.

**Every real is normalised where the evaluator normalises it.** `Value::float` maps `-0.0` to
`0.0`, so every operation that can produce a negative zero is followed by a two-instruction
`select`. Without it, `1.0 / (0.0 * -1.0)` is `-inf` here and `+inf` there — a divergence that
needs a division by a zero whose sign came from a multiplication, and which no plausible test would
have thought to write.

**`trunc` saturates.** `f as i64` in Rust is saturating and toward zero, which is
`llvm.fptosi.sat.i64.f64` and *not* `fptosi` — plain `fptosi` is poison out of range, so
`trunc(1e300)` would have been whatever the register happened to hold.

**And one thing cost nothing, because somebody had already paid for it.** Short-circuiting `and`
and `or` are lowered to `CoreKind::If` in the *checker*, and `check/mod.rs`'s comment on that
function says why in as many words: "put it in `interp.rs` and the second backend has to remember
it, which is the class of bug the backend seam exists to prevent". There was no second backend when
that was written. There is now, it did not have to remember, and the prediction is the one thing in
this section that was tested rather than discovered.

The one thing deliberately not reproduced is NaN *payloads*: `Value::float` canonicalises every NaN
to one and the emitted code does not, on the argument that every operation here produces the
platform's default quiet NaN, which is that one. `nan_is_the_same_nan_on_both_sides` is that
argument written as a test — `0.0/0.0`, `inf - inf`, `0.0 * inf` and `sqrt(-1.0)` through both
backends — rather than as a sentence in a comment.

### A defect the matching found

`negate` was the one integer operation that did **not** follow the language's own rule. It computed
`-x`, which for `i64::MIN` *panics the compiler process* in a debug build and silently wraps in a
release one:

```console
$ beck test neg.beck                       # debug
thread 'beck-test' panicked at crates/beck-eval/src/interp.rs:1421:52:
attempt to negate with overflow

$ beck test neg.beck                       # release
test "negating the smallest integer" … FAILED
  these are not equal
       is: -9223372036854775808
    wanted: 0
```

Neither is the language's answer, and the pair is worse than either alone: **which programs ran
depended on how the compiler was built**, which is [`64`](64-compile-speed-report.md) §64.4's
defect on the evaluator's axis and the same shape as the one
[`31`](31-tail-calls-report.md)'s `STACK_BYTES` comment records. It is now `checked_neg`, and
`overflow_and_division_by_zero_are_errors_not_panics` covers every integer operation that has an
input without an answer — `i64::MIN / -1` and `i64::MIN % -1` were untested too.

This is the differential's first finding and it was found by *writing* it rather than by running
it: matching a semantics means reading it, and reading it is what turns up the place where it does
not hold.

## 93.3 What is refused, and why the sketch compiles nothing

There is no heap in the emitted code — no allocator, no collector, no representation for anything
that is not eight bytes. So the subset is: constants, variables, `let`, `if`, `match` on scalar
constants (with or-patterns, guards and `@`), direct calls, and the arithmetic, comparison and
logical primitives. Everything else is refused **by name, with the reason**, and the reason is the
command's main output rather than a footnote:

```console
$ beck native awfy/mandelbrot.beck
Ubuntu clang version 18.1.3 (1ubuntu1)

4 compiled to native code:
  shifted_left                 (i64, i64) -> i64
  xor_of                       (i64, i64) -> i64
  xor_from                     (i64, i64, i64, i64) -> i64
  escapes                      (double, double, double, double, double, i64) -> i64

4 left to the evaluator:
  absorb       parameter `packed` is `Packed`, and only Int, Float and Bool have a machine representation here
  benchmark    reads the field `sum` of a record
  columns      parameter `packed` is `Packed`, and only Int, Float and Bool have a machine representation here
  rows         parameter `packed` is `Packed`, and only Int, Float and Bool have a machine representation here
```

Eligibility is a **fixed point** rather than one pass, because a definition can be refused for what
it *calls*: `middle` calls `bottom`, `bottom` takes a record, and a mutually recursive pair has to
be refused together or not at all. `a_refusal_travels_to_whoever_calls_it` is the gate, and it
checks both directions — a sound mutually recursive pair still compiles, so the fixed point is not
just refusing every cycle.

Across the corpus, the benchmarks, the book and the standard library: **59 programs assemble, 136
definitions compile**. `examples/todo.beck` contributes none of them.

**Nothing falls back silently.** `Artifact::call` on a refused definition is an error, not an
evaluator call wearing a native backend's name — because a differential that quietly compared the
evaluator with itself is the failure mode this whole file exists to avoid, and `backend_seam.rs`
spent two phases being honest about being exactly that.

## 93.4 A tail call is a jump, and that is a guarantee rather than a hope

[`31`](31-tail-calls-report.md) §31.2 makes "a call in tail position is free" a property of the
**language**. Until now it was a property of one backend's trampoline, and a native backend that
spent a frame per iteration would have made every Beck loop a stack overflow waiting for a big
enough input.

So a call in tail position is emitted as `musttail`, which LLVM *guarantees*: if it cannot discard
the frame it **refuses the module**, so a build that succeeds is a build in which every tail call
is a jump. That is stronger than `-O2`'s sibling-call heuristic, and the difference matters —
"usually fires" is not what a language guarantee can rest on.

The compiled functions use the **`tailcc`** calling convention rather than C's, and that is not a
detail. `musttail` under the C convention requires the caller and callee prototypes to match, which
is a rule about arity: `def drain(n, acc)` tail-calling `def double(acc)` is an ordinary tail call
in the language and would have been a frame. Under `tailcc` it is a jump, and the test proves it at
a size no stack survives:

| | |
|---|---|
| `sum_to(50_000_000, 0)` — self-recursive | answers `1250000025000000` |
| `drain(20_000_000, 0)` — tail-calls a definition of another arity | answers `40000000` |

Threading tail position through the emitter is why `if`, `let` and `match` each take a destination
rather than always producing a value: the interesting call is almost never the outermost node of a
body.

## 93.5 The numbers

`cargo test --release --test measure_native -- --nocapture`, Ubuntu clang 18.1.3, `-O2`, median of
seven runs at the small size and three at the large one. Two sizes per benchmark, per `AGENTS.md`:
one measurement cannot tell a constant from a slope.

| benchmark | size | evaluator | native | ratio |
|---|---:|---:|---:|---:|
| `fib` | 24 | 55.1 ms | 338.5 µs | 162.7× |
| | 30 | 930.7 ms | 5.82 ms | 159.8× |
| `sum_to` | 100,000 | 36.4 ms | 107.5 µs | 338.3× |
| | 1,000,000 | 361.7 ms | 966.5 µs | 374.3× |
| `image` (mandelbrot) | 24×24 | 20.6 ms | 394.1 µs | 52.4× |
| | 96×96 | 325.2 ms | 5.60 ms | 58.0× |
| `xor_sweep` | 2,000 | 13.5 ms | 42.3 µs | 318.6× |
| | 20,000 | 134.3 ms | 227.9 µs | 589.6× |

Two of the four are somebody else's code: `escapes` is `awfy/mandelbrot.beck`'s inner loop verbatim
and `xor_from` is its hand-written exclusive-or, which exists because
[`53`](53-are-we-fast-yet-report.md) §53.5 found that Beck has no bitwise operators — eight
recursive steps where another language has one instruction, which is why it is the benchmark this
backend helps most.

**A call that computes nothing costs 23.2 µs.** That is the pipe: two writes, two reads and two
context switches, and it is on every row above. It is a *constant*, which is why the ratio rises
with size in three of the four and why the small sizes understate the compiled code. `fib` is the
fourth and is flat — 162.7× then 159.8× — because even its small size runs for 55 ms, so there was
nothing left for the pipe to be a large fraction of. That is what a constant looks like when it has
already been amortised, and it is why the shape gate is a bound on the *fall* rather than a demand
for a rise.

The round trip is also the honest reason this backend is for compute: a program crossing the
boundary a million times to do a nanosecond of work each time would be slower than the tree-walker.

**Compiling costs 132.8 ms** for nine definitions and 761 lines of IR — 190 µs to emit the module
and the rest inside `clang -O2` and the linker. That is fine for `beck build` and it is precisely
the cost [`05`](05-tier-lowering.md) §5.2 buys Cranelift to avoid for `beck dev`. This report
therefore does not settle the dual-codegen question; it builds one half and measures the reason the
other half is in the design.

The gate on all of this is a **shape**, not a rate: the ratio must not *fall* as the problem grows,
because that is the claim a compiling backend makes and it is false for anything whose per-unit
overhead grows. [`13`](13-testing.md) §13.7's rule — a timing gate on a shared runner cannot be
held honestly — is why there is no threshold on the ratios themselves.

## 93.6 What is not built

| | Status |
|---|---|
| `Int`, `Float`, `Bool` — arithmetic, comparison, logic, `if`, `let`, `match`, direct calls, recursion | **built**, and differentially gated |
| Checked integer arithmetic, order-key real comparison, signed-zero normalisation, saturating `trunc` | **built**, each with the test that would fail without it |
| Proper tail calls, at any arity, guaranteed by `musttail` under `tailcc` | **built** |
| A trap carries the evaluator's own message **and a span** | **built** |
| Anything on a heap — a list, a string, a map, a record, a union, a closure | **not built.** No allocator and no collector. This is the whole of why the sketch compiles nothing |
| Any effect — the fold, `validate`, the view, `raise`, `parallel:`, `http_fetch` | **not built.** They cross the seam and land on the evaluator, and `beck run` and `beck up` are unchanged |
| Generic and bounded definitions | **not built.** A dictionary parameter is a function value, and a function value is a closure |
| **Cranelift**, and therefore §5.2's *dual* codegen | **not built.** One half exists |
| A WASM target | **not built** |
| In-process execution | **refused**, deliberately — [`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md) |

## 93.7 What this leaves open

**There is no fuel in compiled code.** [`62`](62-fuel-report.md)'s per-call step budget is a
property of walking a tree; machine code has no step to count without paying for the counter on
every one. What is here instead is coarser and says so: an optional **wall-clock limit** on one
call, after which the worker is killed and the call is an error naming the limit. It bounds a
program that will not stop; it does not bound one that is merely slow, and it is not a quota. Every
harness in this workspace sets one; nothing else does.

**There is no depth ceiling either, and that is a regression against
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md).** A recursion that is *not*
in tail position spends a real frame, and nothing counts them: the tree-walker's diagnostic —
"evaluation nested 4000 deep" — has no native counterpart, and what happens instead is the process
dying. The host at least *explains* it rather than passing on "failed to fill whole buffer", and
`a_native_recursion_without_a_ceiling_says_what_happened` is the test. Closing it properly costs a
depth parameter threaded through every non-tail call — one increment and one compare per call —
and that number has not been measured, so it has not been spent.

`pascal(0, 1)` in `sicp/ch1.beck` is the concrete case: it is outside the function's domain and
recurses without bottoming out. The evaluator answers with a diagnostic; the worker dies. Both are
failures and only one of them is a message.

**One process, one pipe, one lock.** Two threads calling at once serialise. The runtime calls the
fold from a sequencer task and a view from a connection task, so this is a real constraint the day
anything but a benchmark uses this backend — and the first thing a second version changes.

**The differential's arguments are chosen, not swept.** Because there is no fuel, a generated
`factorial(i64::MAX)` would be a differential that hangs. Every argument in `native.rs` is either a
boundary value or small, and the file says which of its definitions are total and which are bounded
by their input. Property-based generation over the scalar subset — with a terminating-by-
construction generator — is the obvious next gate and is not written.

**The span a trap carries is not compared.** Both backends carry one and both point into the same
file, but the evaluator's is the `Core` node it was walking and the native one is what the emitter
recorded for the trapping instruction. The *message* is compared, word for word; the span is
checked to exist.

**Nothing here has run on a machine that is not x86-64 Linux with clang 18.** The IR names no target
triple, so it should be host-neutral, and "should be" is the phrase this project treats as a bug
report.

## 93.8 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`05`](05-tier-lowering.md) §5.2 | "compiles to native binaries" was a design. Half of it is built: LLVM, over the scalar subset, out of process. Cranelift is not, and the `beck dev` argument for it is now backed by a measured 132.8 ms compile |
| [`08`](08-roadmap.md) §8.4 | "The interpreter-vs-Cranelift-vs-LLVM differential and the first honest compute number arrive together, and not before." They have arrived, without the Cranelift term |
| [`08`](08-roadmap.md) §8.5.4, §8.7 | Wave 4's "LLVM backend and native codegen" and Lane E are no longer untouched |
| [`04`](04-compiler-architecture.md) §4.8 | The differential between backends exists and is `beck-cli/tests/native.rs`. `backend_seam.rs`'s `two_backends_over_one_program_agree` still runs the evaluator against itself and still says so — it is asserting the *runtime* seam, which is a different claim |
| [`19`](19-phase-1-report.md) §19.6 | "native codegen is not done" — half done, and the half is named |
| [`31`](31-tail-calls-report.md) §31.2 | The tail-call guarantee is now a property of two backends, and on the compiled one it is enforced by the assembler refusing the module rather than by a trampoline |
| [`42`](42-security-assurance.md), [`43`](43-threat-model.md) §43.2, `SECURITY.md` | "all nine crates" is ten. The claim is unchanged and `beck-llvm` inherits the lint like every other member |
| [`62`](62-fuel-report.md) | Fuel bounds the evaluator and nothing else. §93.7 |
| [`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) | The counted ceiling is the evaluator's. Compiled code has none, and `Backend::stack_bytes` answering zero for a compiling backend — the default that ADR set up — is correct about the compiled half and says nothing about the fallback behind it, which is why `Native` forwards the fallback's number |
| [`07`](07-dependencies.md) §7.2 | LLVM is a **run-time** dependency of `beck native` and not a crate. The `inkwell` row is a plan that was not taken; [`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md) says why |
| The evaluator | `negate` on `i64::MIN` is an error rather than a panic or a wrap (§93.2) |

## 93.9 What Phase 3 is still not

Of the four bullets [`91`](91-guards-and-alternatives-report.md) §91.8 named as untouched — the
LLVM backend, Mode B and client polish, the playground, and supply-chain tooling — the first is now
half built and named as half, and the supply-chain one moved in [`92`](92-sbom-report.md).

Unchanged: the page is still assembled and diffed rather than streamed as deltas
([`24`](24-incremental-views-report.md) §24.6); `parallel:` still has no backend that runs two
children at once ([`80`](80-a-scope-owns-its-children-report.md) §80.5) — and this backend does not
change that, because a `parallel:` scope performs an effect and no effect compiles; the render lock
is still there ([`51`](51-arrangement-lifecycle-report.md) §51.7); identity is still not an OIDC
relying party ([`48`](48-identity-report.md) §48.5).

The exit criterion is still a claim about a person, and no outside developer has read the guide
[`88`](88-read-models-and-pgwire-report.md) published.
