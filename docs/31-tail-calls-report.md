# 31 — Phase 3 report, part 6: proper tail calls, and a diagnostic where the abort was

[`25`](25-benchmarks-and-expressiveness.md) §25.7 item 4 is the fourth of the six walls, and it is
the only one on the list written as an either/or:

> **Proper tail calls in the evaluator**, or an explicit bounded-depth diagnostic in the interim.
> A process abort is not an acceptable outcome for user code, independently of chapter 1.

Both are built, because the or was wrong. Tail calls alone do not remove the abort — a tree-walker
still spends a host frame on recursion that is *not* in tail position, and `build(n)` down the spine
of a tree is that shape — and a depth diagnostic alone does not make an iterative process iterative.
The two together are what §1.2.1 asks for and what a language that runs other people's code owes
them.

**The number that says it works is an equality.** 1,500 tail calls and 60,000 tail calls spend the
*same* host stack: 29,088 bytes in an unoptimised build, 2,488 in an optimised one, identical at
both depths in both profiles. Not "small" — the same. `sicp/ch1.beck` now asserts a quarter of a
million levels of it as an exercise, and `sicp/refusals/tail.beck`, which existed to assert the
`SIGABRT`, is gone.

**Wall 4 is down. Two of six stand**: the numeric tower and user-written polymorphism.

It cost throughput, and the number is here rather than in a footnote: **+13% wall clock and +6.7%
instructions** on a tree-recursive benchmark (§31.5). Two structural fixes took that down from what
the first working trampoline cost; nothing took it to zero, and nothing is claimed to have.

478 tests, no failures, no compiler warnings, no clippy warnings — up from
[`27`](27-walls-report.md)'s 473.

## 31.1 What was asked, and what is answered

| asked for | status | where |
|---|---|---|
| A call in tail position costs no host stack | done, and **measured as an equality** at depths forty times apart | §31.2, §31.5 |
| An iterative process observably differs from a recursive one | done — `ch1.beck`'s new §1.2.1 test, a quarter of a million levels | §31.6 |
| No user program aborts the process | done — a counted depth ceiling with a diagnostic, and a declared host stack to make the count reachable | §31.3 |
| The bound is deterministic | done, and it is why the ceiling is a **count** rather than a reading of the stack pointer (§3.7) | §31.3 |
| The runtime provides the stack rather than hoping for it | done — `Backend::stack_bytes`, on the seam, asked by the runtime and by the CLI | §31.3 |
| `sicp.rs`'s 32 MiB wrapper goes away | **half**: gone from `sicp.rs`, moved to where it can be stated once and tested. [`27`](27-walls-report.md)'s prediction is corrected in §31.4 | §31.4 |
| Chapter 1's tail-recursive exercises run in constant space | done — `fact_iter`, `expt_iter`, `ex_1_11_iter`, `gcd` and `find_divisor` are all tail calls | §31.6 |
| Non-tail recursion becomes unbounded | **no**, and it should not: a recursive process needs space linear in its depth, which is SICP's point. It is bounded at 4,000 nested evaluations and says so | §31.3, §31.7 |

## 31.2 What a tail position is, and what the trampoline does with it

`Core` has fourteen kinds and four of them have a tail position in them: the branches of an `If`, the
body of a `Let`, the body of a matched arm, and — the one that matters — the body of the closure an
`App` applies. A fifth, `Global`, has no tail position but is *transparent*: the value of a reference
to a top-level definition is the value of its body, in no environment at all.

Everything else — a constant, a variable, a lambda, a primitive, a record, a list, a map — contains
no tail position anywhere. That is a small enough list to walk.

`Interp::step` walks it. Given a body, it descends through `If`, `Let`, `Match` and `Global` in a
loop, without recursing, until it reaches either a value or an `App`. If it reaches an `App` it
evaluates the arguments, resolves the callee, and **returns the call** rather than making it.
`Interp::eval` is the loop that makes it: bind the parameters, step into the body, repeat. A
thousand tail calls are a thousand iterations of one `while` in one host frame.

The one thing that had to be got right is *which* environment the new frame extends. It extends the
**closure's**, not the caller's — which is ordinary lexical scoping, and is also what keeps the
environment chain as short at the ten-thousandth iteration as at the first. Extending the caller's
would have made a tail call cost no stack and a linear amount of heap instead, which is the same bug
wearing a different hat.

### Why it is two functions

`eval` and `step` could be one loop, and the reason they are not is a borrow rather than a taste.
The body a tail call jumps into lives inside an `Arc<Closure>` that the loop has just taken
ownership of, and a `&Core` pointing into a local that the loop then reassigns is not something safe
Rust will write. Returning the call to `eval` and re-entering `step` costs **one host frame per
call**, which is a constant — not per level of recursion, which is the number that had to be zero.
The measurement in §31.5 is what says the constant is a constant.

## 31.3 The abort, and the three things it took to remove it

The abort was never really about tail calls. It was about a tree-walker whose only bound on host
stack was the host's stack:

```
thread 'main' has overflowed its stack
fatal runtime error: stack overflow, aborting
```

A `SIGSEGV` is not a diagnostic. It carries no span, no message, and no advice; it takes the server
down rather than the request; and it cannot be caught, logged or replayed. Three things replaced it.

### A ceiling that is a count

`beck-eval` refuses to nest more than `DEFAULT_MAX_DEPTH` = **4,000** evaluations, and says so:

```
evaluation nested 4000 deep, which is the evaluator's limit — a call in *tail position* does not
nest and has no limit, so a recursive process this deep has to be written as an iterative one
(SICP §1.2.1)
```

It is a count and not a measurement of remaining stack, and that is the load-bearing choice.
A budget read from the stack pointer would let the same program over the same log succeed in a
release build and refuse in a debug one, or on one machine and not another — and [`03`](03-type-and-effect-system.md)
§3.7 requires a fold's result to be a function of the log alone. `beck replay --verify` has to agree
with the run it is replaying about *everything*, including where the evaluator gave up. Fuel has
always been a count for the same reason; this is its counterpart for space.

The count is evaluator nesting rather than Beck-level calls, because host frames are what it is
protecting and a nested expression spends one too. It is a ceiling on a resource, described in the
units of the resource.

### A host stack that makes the count reachable

A fixed count is only honest if somebody guarantees the stack it needs. `beck-eval` measures what a
level costs rather than assuming it — `the_depth_ceiling_fits_the_smallest_stack_we_run_on` drives a
probe recursion and reads the host stack pointer at the bottom of it:

| profile | host stack per non-tail level | 4,000 levels |
|---|---|---|
| debug | 6,595 bytes | 25.2 MiB |
| release | 1,233 bytes | 4.7 MiB |

`beck_eval::STACK_BYTES` is **64 MiB**, which is the worse of those with a factor of two over it, and
the test fails the build if the ceiling ever outgrows the declaration. It is address space rather
than memory: pages are committed as they are touched, and a program that never recurses touches one.

### A seam that carries the requirement

The runtime is what spawns threads, and [`19`](19-phase-1-report.md) §19.9 forbids it to name a
backend crate. So it asks: `Backend::stack_bytes` is new on the seam, defaulted to zero — "whatever
the caller has", the right answer for a backend that compiles to a loop and never nests host frames
— and the evaluator returns 64 MiB. Four places supply it:

- `beck`'s command dispatch, on a thread of that size, so every command that evaluates anything has
  it;
- the `run`/`up` server runtime, whose tokio worker threads are built with `thread_stack_size` —
  `#[tokio::main]` cannot say that, and folds and views run on those threads;
- `beck_rt::testing::run`, which asks the backend it was handed and spawns accordingly, so `beck
  test` and every in-process harness get it without knowing the number;
- nothing else, which §31.7 states as the limit it is.

`backend_seam.rs` holds the seam to account for it, including the two ways a second backend would
get it wrong: a wrapper that forgets to forward the number, and the intercepting backend `beck test`
swaps in.

## 31.4 The wrapper that did not go away

[`27`](27-walls-report.md) shipped `sicp.rs` with a 32 MiB thread around chapter 1 and twelve lines
explaining it, ending:

> When tail calls land, this wrapper goes away and the `RUST_MIN_STACK`-shaped workaround goes with
> it.

**Half of that came true, and the half that did not is the more useful half.** The wrapper is gone
from `sicp.rs`, because `beck_rt::testing::run` now asks the backend and provides the stack itself.
But it did not go away: it moved. A tree-walker spends a host frame on non-tail recursion whatever
its tail calls do, so *something* has to size a thread, and the prediction was written as though tail
calls would remove the need rather than remove one caller's need to know about it.

What changed is worth more than a deletion would have been. The number is declared once, by the
component that needs it, measured by a test rather than guessed, asked for through the seam by
everything that spawns a thread, and backed by a diagnostic when a program exceeds what it buys.
The old wrapper was a harness working around a runtime; this is a runtime with a stated requirement.

## 31.5 What it cost

A trampoline is not free, and the first working one was not close to free. Two structural things
paid most of it back, and this section is the honest arithmetic.

The benchmark is tree recursion with nothing in tail position, so it measures exactly the case the
trampoline can only slow down — `bench20.beck`:

```python
def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def denom(kinds: Int) -> Int:
    if kinds == 1:
        return 1
    if kinds == 2:
        return 5
    if kinds == 3:
        return 10
    if kinds == 4:
        return 25
    return 50

def cc(amount: Int, kinds: Int) -> Int:
    if amount == 0:
        return 1
    if amount < 0 or kinds == 0:
        return 0
    return cc(amount, kinds - 1) + cc(amount - denom(kinds), kinds)

test "a tree recursion, hard enough to time":
    expect fib(20) == 6765
    expect cc(120, 5) == 494
```

Both binaries built `--release` from the same tree, the "before" one from this report's parent
commit. `null.beck` is the same file with `expect fib(1) == 1` instead, so that the compiler's own
startup subtracts out:

```console
$ git stash && cargo build --release -p beck-cli && cp target/release/beck /tmp/beck-old
$ git stash pop && cargo build --release -p beck-cli && cp target/release/beck /tmp/beck-new
$ for b in /tmp/beck-old /tmp/beck-new; do $b test bench20.beck; done          # best of 12
$ valgrind --tool=callgrind --callgrind-out-file=/dev/null $b test bench20.beck
```

| | before | after | |
|---|---|---|---|
| wall clock, best of 12 | 154 ms | 174 ms | **+13%** |
| instructions, less a null run's | 1.083 × 10⁹ | 1.155 × 10⁹ | **+6.7%** |

The gap between the two is cache and branch behaviour rather than work: the trampoline touches more
distinct code per node than a single recursive `match` did.

**Two things brought it down from considerably worse.**

The first is that not every subexpression needs the trampoline. Routing all of them through `eval`
put a second host frame and a loop under every constant and every variable in a program, and a real
program is mostly those. `Interp::operand` dispatches on the kind: a constant or a variable is
answered on the spot without so much as a depth check, because neither can contain a call and so
neither can nest; the seven kinds with no tail position in them go straight to the leaf evaluator;
only the five that can tail-call go through `eval`.

The second is that the environment is only *replaced* by a `let` or a matched arm. Cloning it on
entry to every node — which is what the first version did — put two atomic refcount operations on
every node that changed nothing. It is held by reference until something extends it.

There is a third thing that is about the stack rather than the clock, and it is worth stating because
it is the sort of change that looks like tidiness. Each arm of the leaf evaluator with a local of its
own is a separate function, `#[inline(never)]` **in debug builds only**. An unoptimised build gives
every arm of a `match` its own slot in the enclosing frame, so one fat `match` on the recursive path
charged every level of a program's recursion for the arms it did not take; splitting it cut the
measured debug cost per level by about a third, and the `cfg_attr` keeps the release build free to
inline it all back. The declared stack is sized from the debug figure because that is the worse one.

## 31.6 What turned round

Three tests changed direction rather than being deleted, which is the discipline
[`27`](27-walls-report.md) §27.1 argued for and the reason a wall coming down is visible.

**`sicp/refusals/tail.beck` became an exercise.** It asserted that `count_to(0, 8000)` aborts. It is
now `ch1.beck`'s second §1.2.1 test, asserting that `count_to(0, 250000)` returns 250,000 — and
alongside it `count_up(500)`, the same count written as a recursive process, so the file states both
halves of the distinction the section is about. It is the one test in chapter 1 that asserts a
property rather than a number SICP prints, because §1.2.1 is where the book states a property.

**`a_tail_call_consumes_stack_…` became `a_tail_call_costs_nothing_…`.** A million tail calls, run
through the binary rather than in-process, because if the trampoline ever regresses the failure is a
dead process rather than a `Result` and only a subprocess can tell the difference.

**The 50,000-deep tree now asserts the diagnostic instead of the abort.** `build(n)` recurses inside
a list literal inside a constructor, so it is not in tail position and never will be; what changed is
that the program fails and the process lives. Its shallow case moved from 100 to 1,000 and lost the
hedge that went with it — the depth that fits is no longer a property of the build profile, because
what stops it is a counted ceiling rather than whatever stack the process happened to have.

Chapter 1 is 15 tests, up from 13. `beck test sicp/ch1.beck` runs them, including the quarter of a
million tail calls, in **0.43 s**.

## 31.7 What is still not

- **Non-tail recursion is bounded at 4,000 nested evaluations**, and a program that needs more gets
  a diagnostic rather than an answer. That is the right shape — a recursive process needs space
  linear in its depth and SICP says so — but 4,000 is a number chosen to fit a 64 MiB stack, not one
  measured against what programs need. Nothing surveys real programs for their depth.
- **Fuel now bounds an iterative process where the stack used to.** `count_to` runs somewhere between
  three and four million iterations before `evaluation ran out of fuel`, because the 50-million-node
  budget is per evaluation and a loop spends about thirteen nodes a turn. That is a *better* failure
  than the old one in every way — deterministic, diagnosable, replayable — but "an iterative process
  can go as deep as it likes" is what the diagnostic says and it is true only of space.
- **The stack guarantee is provided at four places and enforced at none.** An embedder that builds
  its own tokio runtime around `beck_rt::Runtime` and does not call `thread_stack_size` gets the
  abort back. `Backend::stack_bytes` is how it would find the number out; nothing makes it ask, and
  no test drives a served program past the ceiling to prove the worker threads were sized right —
  the CLI's `run` path is smoke-tested, not measured.
- **13% of the evaluator's throughput, and nothing gets it back.** A compiling backend would not pay
  it, which is an argument for the backend the roadmap has been deferring since Phase 1 rather than a
  defence of this one.
- **`Interp::apply` still costs a frame per call.** The runtime's entry points and the higher-order
  primitives (`map_list`, `filter_list`, `sort_by`) call closures through it, so a `map_list` over a
  million-element list nests once, not a million times — but a *primitive* that called a closure in
  tail position would still nest, and none does only because none is written that way.
- **Two walls stand**, and they are the two [`25`](25-benchmarks-and-expressiveness.md) §25.7 put
  last because they are the largest: the numeric tower (item 5, Phase 3's standard-library bullet)
  and user-written polymorphism (item 6, "the one most entangled with [`03`](03-type-and-effect-system.md)").
  Chapter 2 still stops inside §2.2 and chapters 3–5 are untouched.
- **`check.rs` is 3,012 lines, unchanged.** This work did not touch the checker at all — tail
  position is a property of `Core`, not something the front end has to be told about — so §22.6's
  request to move the test-checking pass out of it is still not done, for the fifth report running.
- Everything [`26`](26-arrangement-sharing-report.md) §26.9 lists is unchanged: no LLVM backend, no
  native codegen, no Mode B, no client polish, no `test --update`, no structured concurrency, no
  `Result`/error rows, no SQLite substrate, no standard library v1, no identity beyond a dev-mode
  actor, no LSP, no playground, no supply-chain tooling. Nine Phase 3 bullets, untouched. SQL read
  models, pgwire and query fusion are still nothing.

## 31.8 What this changes for the rest of Phase 3

1. **The evaluator now has two bounds, and neither is the host's.** Fuel bounds time, depth bounds
   space, both are counts, and both are functions of the program and the log. That is the property
   §3.7 needs and the first time the evaluator has actually had it — a stack overflow was always a
   hole in the determinism argument, and it was never listed as one.
2. **A refusal file is a better artefact than a report paragraph.** `tail.beck` was written to fail,
   asserted the exact `SIGABRT` it produced, and named in its header the three sections of SICP it
   blocked. Turning it round took an afternoon and left the chapter carrying the property as an
   exercise. §25.7's remaining two walls have files exactly like it.
3. **`Backend::stack_bytes` is the first thing on the seam that is about *resources* rather than
   about execution.** A native backend will have others — a code cache, a memory budget — and this
   is the shape they take: a defaulted method the runtime asks about, rather than a constant the
   runtime knows.
4. **The either/or in a plan is worth reading twice.** §25.7 offered "proper tail calls **or** a
   bounded-depth diagnostic in the interim", and taking the offer would have shipped a diagnostic
   that made chapter 1 fail instead of abort — strictly worse than what was there. The two items
   were not alternatives; one bounds the recursion a program should not be doing and the other
   removes the recursion it should not have been charged for.
