# 93 — The native backends

**Built.** [`05`](05-tier-lowering.md) §5.2's dual code generation: `beck native --backend
llvm|cranelift` compiles a Beck definition to machine code, runs it in a child process, and answers
what the tree-walker answers. [`04`](04-compiler-architecture.md) §4.8's **differential between
backends** — carried since Phase 1 as a shape with the evaluator on both sides, and said so in the
file — is a three-way one: the tree-walker, LLVM and Cranelift on every call.

Across the corpus, both benchmark suites, both SICP chapters, the examples and the standard library,
**941 definitions compile and 137 are refused**, and `examples/todo.beck` compiles **all nine of its
definitions** — the sentence that used to name `validate` as the one left, for a Unicode table, was
out of date rather than made true again by §93.12: `validate` reaches none of the fifteen primitives
that section is about. Nine corpus programs compile their `apply_event` — the step function of a
`durable` fold — and twenty-one of thirty-two compile their `view`.

This chapter is the whole of Lane E. It arrived in fifteen pieces over as many changes, and for
fourteen of them the frame was the same: a thing gets a layout, both emitters learn it, the
differential holds them to the evaluator, and the refusal that used to name it says something else
instead. §93.6 is that sequence as a table, because the interesting content is the handful of
findings and not fourteen repetitions of the frame. The fifteenth is not that shape at all and has
a section of its own: a primitive whose correctness is *somebody else's artefact* is neither emitted
nor asked for but **linked** (§93.12).

**Three of those findings are worth the chapter on their own.** A refusal is a claim and nothing was
checking it, so four of them were false for whole reports at a time (§93.9). A `_` arm in a match on
a representation swallowed the same defect four times, in both emitters (§93.8). And a refusal
inherited from the *evaluator's implementation strategy* rather than from the language kept
`list_append` out for two reports on an argument every sentence of which was true (§93.7).

---

## 93.1 The shape: two emitters, one host, out of process

`unsafe_code = "forbid"` at the workspace root, inherited by every crate that can inherit it, is
[`43`](43-threat-model.md) §43.2's strongest claim — the only one that is *structural* rather than
tested or asserted. (The three that cannot are libraries with a `#[no_mangle]` export, which rustc
classifies as unsafe code; §93.12 is the one of them this chapter is about.) Both of the obvious
ways to build a native backend take it away: `llvm-sys`/`inkwell` is `unsafe` at the execution
engine, and running compiled code in process is a pointer transmuted into a function under any
name.

So neither backend runs what it emits:

```text
Core ──▶ LLVM IR as text  ──▶ clang -O2  ──┐
                                           ├──▶ an executable ──▶ a pipe ──▶ the host
Core ──▶ cranelift-object ──▶ a .o ──▶ cc ─┘
```

[`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md) and
[`adr/0024`](adr/0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md) are the two
decisions and the asymmetry between them. ADR 0021 declined to take LLVM as a *dependency* because
its Rust bindings are `unsafe` from the first call; ADR 0024 takes Cranelift as one, because
Cranelift is Rust and `beck-clif` inherits `forbid(unsafe_code)` with no exception. What ADR 0021
refused was never *emitting* code — it was **running** it, which is why there is no JIT here either:
`cranelift-jit` is the obvious way to use that crate and it finishes by turning a pointer into a
function.

Consequences worth having in front of a reader:

- **The dependency graph barely moved.** `beck-llvm` depends on `beck-core` and `beck-diag` and
  nothing else; LLVM is a **run-time** dependency of `beck native` and not a crate.
  [`07`](07-dependencies.md) §7.2's `inkwell` row is a plan that was not taken.
- **A machine without a toolchain builds everything and passes everything.** The LLVM half needs a
  `clang` and the Cranelift half needs any linker — a container with `gcc` and no LLVM can use the
  second. Absence is a printed skip, and `BECK_REQUIRE_LLVM=1` forbids it, which is what CI sets:
  [`19`](19-phase-1-report.md) §19.4 item 10's rule that a gate which can be silently skipped is not
  one.
- **The artefact is readable.** `beck native --out <dir>` leaves a `.ll` that `opt` will transform
  and a person can read. A codegen defect is a diff in a text file.
- **Nothing falls back silently.** `Artifact::call` on a refused definition is an error, not an
  evaluator call wearing a native backend's name — because a differential that quietly compared the
  evaluator with itself is the failure mode this whole subsystem exists to avoid, and
  `backend_seam.rs` spent two phases being honest about being exactly that.

The worker protocol is one protocol, unforked between the two backends: a header, eight bytes per
argument, a 24-byte reply. The host is the same host, and two spellings of one wire is the drift this
project spends its gates on.

## 93.2 The heap is an arena of offsets

One `malloc` at startup, a bump pointer, and no free. **A reference is an offset rather than a
pointer**, which makes an object graph a flat byte string — so the host marshals a `Value` in Rust,
once, and neither emitter generates a line of code for getting one across.
[`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md) is that decision with its costs, and
it is the single most load-bearing one in the subsystem: every layout below is a consequence of it,
and so is the fact that a closure cannot hold a code pointer and a view cannot hold a rendering.

Every object is a whole number of 8-byte words. Offset `0` is reserved, so no live object has it and
an allocation that trapped can return it.

| Value | Layout |
|---|---|
| **record, union, newtype** | word 0 the **tag** (`0` for a record or newtype); one word per field after it |
| **`secret[T]`, `internal[T]`** | the same, as the one-field newtype each already is at run time |
| **`Str`** | byte count, character count, then the bytes padded to a word |
| **`list[T]`** | a two-word **header** — count, and the data block's offset — over a data block of `cap`, `used`, then the elements |
| **`Map[K, V]`** | a weight-balanced tree: five words a node — subtree size, key, value, left, right — with `0` for the empty map |
| **closure** | the lambda's **rank** among the program's lambdas, then one word per capture in `VarId` order |
| **`Html`, `Attr`** | four words: a tag, a name, and a **deferred value** — the value's word beside the index of its repr |

Three of those rows are decisions rather than transcriptions, and each is argued where it arrives:
the list's split header is §93.7, the map's tree is §93.7, and the deferred value is §93.6's view
row.

**A tag is a variant's rank by name, and a field's slot is its rank by name too.** `Value`'s derived
`Ord` compares a record's type, then its variant *name*, then its fields, and
[`46`](46-standard-library-report.md) §46.16 established that a record's value order is by field name.
A layout in declaration order answers `<` backwards on a program that reads perfectly, which is why
`a_layout_is_ordered_by_name_and_not_by_declaration` pins both rules explicitly rather than leaving
them to the differential — a differential compares two things against an **oracle** and cannot see
the day the oracle moves.

**A real is normalised on the way into a field.** `Value::float` canonicalises `-0.0` to `0.0` and
every NaN to one NaN, so a heap field is the one place §93.3's invariant has to be paid for rather
than argued away.

**A literal is the host's, at a fixed offset.** The arena is reset before every call, so a string
literal cannot be allocated where it is written — the second iteration of a loop would allocate it
again. It cannot be a global either, because a value here *is* an offset into the arena. So a
module's literals are a **pool** the host writes as the first bytes of every request's heap, at
offsets decided when the module was emitted, and compiled code refers to one by a constant. The pool
is interned before either emitter writes anything, which makes it a function of the *program* rather
than of which definitions turned out to compile;
`the_literal_pool_is_a_function_of_the_program` is the gate, and its discriminating assertion is that
the survey alone already holds every literal — including a pattern's, since `case "one":` is not an
expression and is reached only because the walk was told to.

The arena is reset to just past the arguments before every call, so the bound is **per call** rather
than per process: 256 MiB, and `Trap::HeapExhausted` is a message with a span rather than a
`SIGSEGV`. A module with no object in it gets no `malloc`, no globals and no allocator at all, which
`a_program_with_no_object_has_no_arena` asserts on both backends rather than leaving to be believed.

## 93.3 Agreeing with the evaluator, and where the obvious lowering is wrong

A backend that is 173× faster and answers something else is worthless, and the interesting thing
about matching `beck-eval` is that the *obvious* lowering is wrong in five places on the scalar
subset alone. Four were found by reading the evaluator rather than by a failing test — which is the
wrong order, and is why every one now has a test that fails without it.

**Integer arithmetic is checked, so it cannot be an instruction.** `beck-eval` uses `i64::checked_*`:
an overflow is a *value* — `` `+` overflowed `` — and not a wrapped result. So `+`, `-` and `*` are
`llvm.s{add,sub,mul}.with.overflow.i64` with the overflow bit branched on, and `/` and `%` carry an
explicit guard for a zero divisor **and** for `i64::MIN / -1`, whose quotient is not representable
and which `sdiv` treats as immediate undefined behaviour. A missing guard there would not have been a
wrong answer; it would have been the optimiser deleting the code around it.

**Reals do not compare with `fcmp`.** [`27`](27-the-walls-come-down-report.md) §27.8 made Beck's `==`
on reals *structural*, because a fold's accumulator needs a total order and IEEE 754 supplies none:
`Value::Float` stores a monotone transform of the bits under which `NaN` is the maximum and equals
itself, and `fcmp` says `false` to both. A comparison bitcasts and compares the order keys.

**A real is normalised where a signed zero is *observable*, which is not everywhere.** The obvious
way to match `Value::float` is to normalise every float result. That is what shipped first and it
cost **3×** on float-heavy code. It is not needed: every float operation maps zeros to zeros, so a
register here differs from the evaluator's at most in the sign of a zero, and only three things
observe that — a comparison, a division's **divisor**, and a trap's payload. §93.5's mandelbrot table
is that mistake measured.

**`trunc` saturates.** `f as i64` in Rust is saturating and toward zero, which is
`llvm.fptosi.sat.i64.f64` and *not* `fptosi` — plain `fptosi` is poison out of range, so
`trunc(1e300)` would have been whatever the register happened to hold.

**A NaN is canonicalised at the same three places, and the argument for not doing it was wrong.**
This work's first version said canonicalisation could be skipped because "on every target the
operations here produce the same default quiet NaN", and offered `nan_is_the_same_nan_on_both_sides`
as proof. It is false on x86-64: `0.0 * inf` yields the *indefinite* QNaN `0xFFF8…`, whose sign bit
is set, where `f64::NAN` is `0x7FF8…`. Under the order key one sorts below every number and the other
above, so the two backends disagreed about `(0.0 * inf) > 0.0`.

The test offered as proof could not have caught it, and **the reason is worth more than the bug**. It
calls a function that *returns* a NaN, and a returned NaN is canonicalised by the host on its way
back into a `Value` — so both sides agreed, and the agreement was manufactured at the boundary. The
rule that generalises: **a differential over a boundary that normalises tests the boundary.**
`product_order`, `product_is_zero` and `reciprocal_of_product` each compute their own awkward value
inside compiled code, and each goes red if its line in the normaliser is deleted.

**And one thing cost nothing, because somebody had already paid for it.** Short-circuiting `and` and
`or` are lowered to `CoreKind::If` in the *checker*, and the comment on that function says why in as
many words: "put it in `interp.rs` and the second backend has to remember it, which is the class of
bug the backend seam exists to prevent". There was no second backend when that was written. There is
now, it did not have to remember, and the prediction is the one thing in this section that was tested
rather than discovered.

### A defect the matching found

`negate` was the one integer operation that did **not** follow the language's own rule. It computed
`-x`, which for `i64::MIN` panics the compiler process in a debug build and silently wraps in a
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
depended on how the compiler was built**. It is `checked_neg` now, and
`overflow_and_division_by_zero_are_errors_not_panics` covers every integer operation that has an
input without an answer — `i64::MIN / -1` and `i64::MIN % -1` were untested too.

## 93.4 A tail call is a jump, and that is a guarantee

[`27`](27-the-walls-come-down-report.md) §27.2 makes "a call in tail position is free" a property of
the **language**. Until there was a second backend it was a property of one trampoline, and a native
backend that spent a frame per iteration would have made every Beck loop a stack overflow waiting for
a big enough input.

Both backends *guarantee* it rather than hoping for it. LLVM emits `musttail`, which refuses the
module if the frame cannot be discarded; Cranelift emits `return_call`, which verifies the same thing
and refuses the function. A build that succeeds is a build in which every tail call is a jump, which
is stronger than `-O2`'s sibling-call heuristic — "usually fires" is not what a language guarantee
can rest on.

The compiled functions use **`tailcc`** rather than the C convention, and that is not a detail.
`musttail` under C requires caller and callee prototypes to match, which is a rule about arity: `def
drain(n, acc)` tail-calling `def double(acc)` is an ordinary tail call in the language and would have
been a frame. Cranelift's x86-64 `return_call` additionally requires `preserve_frame_pointers`,
because its implementation restores the caller's frame through the frame pointer — so the setting is
in the ISA builder with the reason beside it rather than in a list of flags somebody copied.

| | |
|---|---|
| `sum_to(50_000_000, 0)` — self-recursive | answers `1250000025000000` |
| `drain(20_000_000, 0)` — tail-calls a definition of another arity | answers `40000000` |
| ten million applications **through a closure**, in tail position | constant stack, on both backends |

The last row is the one that had to be checked by making it red: with the application's own call site
emitted as an ordinary call it answers `SIGSEGV` at that size. Threading tail position through the
emitters is why `if`, `let` and `match` each take a destination rather than always producing a value —
the interesting call is almost never the outermost node of a body.

## 93.5 The numbers

`cargo test --release --test measure_native -- --nocapture --test-threads=1`, Ubuntu clang 18.1.3,
`-O2`, median of seven runs at the small size and three at the large one. Two sizes per benchmark,
per `AGENTS.md`: one measurement cannot tell a constant from a slope. Every ratio includes the pipe
round trip, measured at 35.6 µs in the same run.

**Nothing here is asserted as a rate.** [`13`](13-testing.md) §13.7 — a timing gate on a shared
runner cannot be held honestly — and there is a sharper reason as well: the native side is
`clang -O2` whatever profile `cargo` was run in and the evaluator is not, so the same code answers
2,460× under `cargo test` and 120× under `cargo test --release`. What is asserted is the **shape**,
and §93.14 lists the gates that assert it with no clock in them at all.

### Scalar arithmetic

| benchmark | size | evaluator | native | ratio |
|---|---:|---:|---:|---:|
| `fib` | 24 | 59.2 ms | 425.0 µs | 139.4× |
| | 30 | 1.095 s | 6.32 ms | **173.2×** |
| `sum_to` | 100,000 | 40.3 ms | 96.2 µs | 418.6× |
| | 1,000,000 | 414.1 ms | 559.7 µs | **739.9×** |
| `image` (mandelbrot) | 24×24 | 26.5 ms | 211.3 µs | 125.4× |
| | 96×96 | 369.3 ms | 2.42 ms | **152.6×** |
| `xor_sweep` | 2,000 | 18.2 ms | 98.3 µs | 184.8× |
| | 20,000 | 174.4 ms | 418.1 µs | **417.1×** |

**A call that computes nothing costs 43 µs.** That is the pipe: two writes, two reads and two context
switches, and it is on every row above and every row below. It is a *constant*, which is why the
ratios rise with size and why the small sizes understate the compiled code. It is also the honest
reason this backend is for compute: a program crossing the boundary a million times to do a
nanosecond of work each time would be slower than the tree-walker.

### Beside five other languages

[`25`](25-benchmarks-and-expressiveness.md) §25.9 rule 2 held every comparative claim until a second
backend existed. There is one, so here is the claim — `cargo test --release --test measure_xlang --
--nocapture`, with [`xlang/README.md`](../compiler/xlang/README.md) as the rules the ports are held
to and the longer list of what this does not measure.

| implementation | integers | `fib(30)` | `sum_to(1M)` | `image(96)` | `xor_sweep(20k)` |
|---|---|---:|---:|---:|---:|
| C 18, `-O2` | wrapping | 3.00 ms | 0.000 ms † | 2.09 ms | 0.201 ms |
| Rust 1.94, `-O` | checked | 3.88 ms | 0.609 ms | 2.13 ms | 0.270 ms |
| C 18, `-O2` | checked | 4.66 ms | 0.466 ms | 2.09 ms | 0.472 ms |
| **Beck, native** | **checked** | **6.17 ms** | **0.593 ms** | **2.45 ms** | **0.420 ms** |
| Node 22 (V8, warmed) | `f64` | 13.9 ms | 1.24 ms | 2.18 ms | 1.29 ms |
| Ruby 3.3 | bignum | 95.6 ms | 21.8 ms | 73.6 ms | 18.9 ms |
| Python 3.11 | bignum | 140 ms | 75.0 ms | 37.7 ms | 24.0 ms |
| Beck, evaluator | checked | 1.105 s | 406 ms | 370 ms | 167 ms |

Every row computes `832040`, `500000500000`, `3688` and `2220064`, and **that is the gate** —
`measure_xlang.rs` asserts the answers and only prints the times. Eight implementations agreeing on
four answers is a much stronger statement about the ports than eight files that look alike.

Against the two rows that carry Beck's own semantics — Rust and checked C — Beck is **1.2× to 1.6×
off** at worst, edges Rust on `sum_to` and beats checked C on `xor_sweep`. Against the rest: 2.1–3.1×
ahead of warmed V8 on three benchmarks of four and behind it on the mandelbrot, 15–45× ahead of Ruby,
and 15–126× ahead of Python.

Four things this table is not, in the order a reader will be tempted to forget them:

1. **It is the scalar subset**, which is the most flattering ground this backend has. `awfy/` and
   `clbg/` run whole programs and a whole program still has a fold and a view in it.
2. **The `integers` column is why the rows are not comparing the same thing.** Only Rust and checked
   C are like-for-like. † The wrapping-C `sum_to` is not a mis-measurement: LLVM's SCEV rewrites the
   loop's exit value to a closed form and deletes the loop. Checked arithmetic costs you that, and it
   is the clearest price in the table.
3. **One machine, medians of eleven, one run.** Five samples put `fib(30)` anywhere between 6.1 and
   8.6 ms across three runs of one binary on this runner, which is why the suite takes eleven.
4. **Nothing here is a claim about Beck the language.** It is a claim about four arithmetic kernels.

### What the mandelbrot gap was made of

`image` was **3× off C** when this was first measured, while the integer benchmarks were at parity —
which is the shape of a semantic cost rather than a codegen one. `xlang/escapes_variants.c` is the
diagnostic that says which: one loop, four spellings, the same `clang -O2`, only the semantics
changing.

| the same mandelbrot, in C | |
|---|---:|
| plain IEEE doubles and `>` | 2.09 ms |
| the order-key comparison — **what the emitters produce today** | 2.16 ms |
| …and every float result normalised — what they produced first | 5.58 ms |
| both | 5.80 ms |

So the code generation was never the problem: Beck's 2.45 ms is within 13% of C doing the same work,
and §27.8's order key costs about 3%. The whole 3× was normalising after every operation, which §93.3
now does not do. `AGENTS.md` says a bad number is a design question rather than a fact to write down;
this is what that looked like in practice — the first version of this measurement reported 6.17 ms
and 58×, and the fix was to ask what the operation *should* cost rather than how to make that one
faster.

### Cranelift against LLVM, on build time

[`07`](07-dependencies.md) §7.3's reason for a second code generator is a *build* time. Measured
program-to-executable in a release build, because that is what a developer waits for:

| definitions | cranelift | llvm + `clang -O2` | × |
|---|---|---|---|
| 50 | **48.8 ms** | 259.1 ms | 5.3 |
| 400 | **141.5 ms** | 1.5 s | 10.5 |

Two sizes say more than the ratio does. Eight times the definitions costs Cranelift **2.9×** and LLVM
**5.8×**: most of the Cranelift column is the fixed cost of running `cc`, and most of the LLVM column
is not. That is why the ratio *grows*, and it is what "~10× faster codegen step" looks like once the
link is included in both. The measurement is release-only for a reason this work nearly got wrong:
`cargo test` builds this workspace in *debug*, so a comparison there is our unoptimised build of
Cranelift against a distribution's optimised `clang`, which runs the other way by about a factor of
two.

### The heap, per value kind

| benchmark | size | evaluator | native | ratio |
|---|---:|---:|---:|---:|
| **records** `build_and_sum` — allocate a spine and walk it | 10,000 | 11.91 ms | 193.4 µs | 61.6× |
| | 100,000 | 136.55 ms | 1.367 ms | **99.9×** |
| **records** `folded` — `with` in a loop, which is a fold's shape | 100,000 | 71.20 ms | 448.2 µs | **158.8×** |
| **records** `scan` — the control: same loop, no allocation | 100,000 | 43.74 ms | 168.4 µs | 259.8× |
| **text** `walk` — index every character | 16,000 | 9.221 ms | 326 µs | **28.3×** |
| **text** `hunt` — search repeatedly | 16,000 | 8.419 ms | 351 µs | **24.0×** |
| **text** `grown` — append to an accumulator | 1,000 | 398 µs | 377 µs | 1.06× |
| | 4,000 | 1.245 ms | 7.506 ms | **0.17×** |
| **list** `walk` — read every element by index | 16,000 | 11.616 ms | 320 µs | **36.3×** |
| **list** `windows` — a four-element slice at every position | 16,000 | 10.037 ms | 314 µs | **31.9×** |
| **list** `doubled` — walk and append | 8,000 | 6.116 ms | 870.3 µs | **7.03×** |
| **list** `summed` — the control: same walk, nothing built | 8,000 | 5.527 ms | 70.4 µs | **78.46×** |
| **map** `lookup` — 2,000 searches | 2,000 | 1.436 ms | 130.2 µs | **11.0×** |
| **map** `walk` — every entry through `map_keys` | 2,000 | 75.483 ms | 3.195 ms | **23.6×** |
| **closures** `apply_often` — build and apply in a loop | 160,000 | 126.48 ms | 207.7 µs | **609.1×** |
| **closures** `mapped` / `folded` | 16,000 | 3.06 / 3.32 ms | 395 / 303 µs | 7.7× / 11.0× |
| **failure** `caught` — raised, unwound 3,000 frames, caught | 3,000 | 1.576 ms | 92.7 µs | **17.01×** |
| **failure** `down` — the same frames, no failure | 3,000 | 1.296 ms | 64.7 µs | 20.03× |
| **a page** — the `ui:` block, keys and handlers | 1,600 rows | 11.84 ms | 8.87 ms | **1.33×** |
| **a page** — the same page, text only | 1,600 rows | 1.891 ms | 2.081 ms | **0.91×** |

Six rows deserve reading rather than skimming.

**The controls are the point.** `scan`, `summed`, `spin` and `down` do the same loop without the
thing being measured. Net of the round trip, an allocate-a-`Node`-and-walk-it step is about 13 ns and
the control's step is about 1.3 ns — so the arena costs roughly ten times a bare loop step and is
still about a hundred times the tree-walker. Without the control the first rows would read as though
compiling the *loop* were the win. The map table is the case where the control says the table cannot
answer its own question: `spin` costs within a few percent of `lookup` at both sizes, so at 250 and
2,000 entries a binary search is smaller than the tail-recursive loop that calls it and nothing there
distinguishes halving from scanning.

**Text's accumulator is slower than the tree-walker, on purpose, and gets slower as it grows.**
1.06× at 1,000 appends and 0.17× at 4,000 — four times the work and twenty times the time, which is
what a quadratic looks like at two sizes. §93.7 is the argument.

**A page is not a win, and the report of it says so before anything else.** A compiled `view` builds
the *call* and the host bakes the tree, so the rendering is the same `Value::display` either way and
the pipe is additional. What is compiled is the program's own logic — the loops, the conditionals,
the field reads — which on these pages is not where the time goes. [`94`](94-the-client-report.md)
§94.12 measured the same thing from the other end: 97% of an interaction is `view`, and what grows is
`view` being a pure function of the whole state.

**Failing costs about a sixth more than not failing**, at both depths, on both implementations — and
nothing per frame. The sizes are six times apart rather than eight because the *evaluator* bounds the
larger one: [`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)'s declared nesting
ceiling is 4,000 and that recursion is not in tail position. The compiled side has no such ceiling,
which is itself worth knowing and is §93.15's regression.

**`map_keys` inside a loop is 55× worse for eight times the entries, on *both* backends.** That is a
quadratic in the **program** rather than in either implementation, and it is in the fixture
deliberately: a reader who sees `map_keys(m)` in a loop should see this number.

**Every ratio that grows with size is the pipe being amortised**, not the loop getting better. The
list and closure rows are worse off for a second reason — 16,000 elements is 128 KB of arena crossing
the pipe in each direction — so those rows measure the marshalling at least as much as the loop, and
the honest claim from them is the larger size only, as a lower bound.

### Asking the host

| the question | live elements | without it | with it | the question |
|---|---|---|---|---|
| `now()` | 16 | 26.4 µs | 50.9 µs | **24.5 µs** (0 B carried) |
| | 4,096 | 48.4 µs | 77.3 µs | **29.0 µs** (0 B carried) |
| `secret_env` | 16 | 25.2 µs | 52.0 µs | **26.8 µs** (664 B) |
| | 4,096 | 47.4 µs | 210.1 µs | **162.7 µs** (163,864 B) |

A question that carries nothing is a round trip and stays one, across a 256× change in what is live.
A question whose argument could point into the arena carries the arena, and grows with it. So
`secret_env` inside a loop over a large heap is a cost a developer can meet and `now()` is not — which
is stated rather than smoothed because a reader deciding where to put the call needs it.

**These are the numbers that decided §93.12.** A question is the right price for an answer only the
process outside has; it is the wrong price for one that is a function of its arguments, and §93.12
is the fifteen primitives that were paying it.

## 93.6 What compiles, and the order it arrived in

Each row is one change. "Compiled" is the total of `beck native <file>`'s own two headline lines over
`corpus/ awfy/ clbg/ sicp/ examples/ lib/`, each file compiled alone, taken before and after.

| # | What arrived | Compiled | The finding it left behind |
|---|---|---:|---|
| 1 | **The scalar subset.** Constants, `let`, `if`, `match` on scalar constants, direct calls, arithmetic, comparison, logic — no heap at all, so `examples/todo.beck` compiled **nothing** | 136 | `negate` on `i64::MIN` panicked in debug and wrapped in release (§93.3) |
| 2 | **Cranelift**, the same subset written a second time | 136 | A signed comparison of an unsigned order key, caught by five lines and one call rather than by the 676-pair sweep (§93.8) |
| 3 | **Records, unions, newtypes** — the arena, and a value that does not fit in a register | 283 | A `match` arm's guard had carried unresolved types since Phase 2; a pattern test cannot be a conjunction, and that is a memory-safety requirement, not a style (below) |
| 4 | **Text.** A layout, a literal pool, `+`, six comparisons and seven primitives | 344 | `str_slice` was charged the length the caller wrote rather than the length it takes, so `str_slice(s, 0, 1_000_000)` on a five-character string cost a million steps of fuel |
| 5 | **A list, read-only.** `list_get` answers an `Option` **without a branch**, by `select`ing between the element's address and the list's own header | 403 | The host's decoder recursed, so `MAX_DEPTH = 2048` was a claim about the *thread's stack*: a debug build aborted at about 1,600. It is iterative now (§93.9) |
| 6 | **A map, read-only.** Keys in one run and values in another — the first makes the search binary, the second makes `map_keys` a `memcpy` | 452 | Of 1,026 refusals across the tree, **472 blamed a `Map`**; afterwards **no refusal anywhere blamed a collection for having no layout**. Nine corpus `apply_event`s compiled, the first time anything at the centre of what Beck is *for* reached machine code |
| 7 | **Closures.** A rank rather than a code pointer, an application as a switch into a direct call, five generated loops through it | 619 | A closure never crosses the boundary — §93.15. The gate built at row 5 **fired**, because giving a closure a shape made "a closure, which has no layout here" false while the refusal saying it was still there |
| 8 | **A view, as a recipe.** Not the tree: the *call* `html_el(tag, attrs, children)` would have been given, baked by the host with the evaluator's own builder | 688 | 42 refusals blamed a view; 38 compiled and 4 were re-refused for something already true of them. A view has **no order**, which is a refusal rather than an omission (below) |
| 9 | **`raise` and `try:`.** The error cell every function already returns through *is* an unwinder; what this adds is a code, two words and a handler label | 711 | Clearing a `u32` code is not clearing a cell the worker's loop reads as an `i64` (§93.8) |
| 10 | **A list grows.** The header split from the data block, so an append writes a slot no reader can see | 895 | The refusal had been inherited from the **evaluator's implementation strategy**, and every sentence of it was true (§93.7) |
| 11 | **A map grows**, as the weight-balanced tree `beck_core::pmap` already is | 1,137 † | A user definition could take a runtime symbol's name: `awfy/richards.beck` has a `dispatch`, and so did the module. Latent since row 1, surfaced only when `dispatch` first compiled |
| 12 | **A generic definition**, once per type it is used at | 870 † | A bounded search that gives up should give up on the **whole thing** — sixty-four individually true refusals said nothing together (§93.10) |
| 13 | **The four primitives that ask the host** — `now()`, `uuid()`, `secret_env`, `http_fetch` | 889 | The lane row was one item and the work was two: `secret[T]` had no layout, which is what had made `HttpRequest` unlayoutable too. And a limit on compiled *time* is not a limit on a *call* (§93.11) |
| 14 | **A list pattern**, `case [first, *rest]` — the last pattern form either emitter refused | 905 | The refusal it replaced had been false for three reports, and the gate written for exactly this class **could not fire**, because the sentence named no type (§93.9) |
| 15 | **The runtime library.** Fifteen primitives that are a table, a grammar or somebody else's parser, linked rather than emitted or asked for (§93.12) | **941** | An emitter's `Bool` arm that had never executed, because none of row 13's four primitives answers one |

† Rows 11–13 are counted over a different file set from row 10 and from each other, as the refusal
profile was re-measured; the delta within each row is a delta between two runs of one command, which
is what each is claiming.

Three of those rows carry an argument rather than a layout, and they are stated here rather than left
in a table cell.

**A pattern test cannot be a conjunction.** The scalar emitters computed "does this pattern match?"
as one boolean and branched once, which is right when a pattern takes nothing apart. `Some(Circle(r))`
does: it reads a field, and reading the field of a variant that is *not* present gives a word meaning
something else — and if that word is an `Int` it is a perfectly good `i64` being used as an offset. So
a pattern is **control flow**: each test branches, and a field is read only in a block the tag check
dominates. The differential could not have found this. It is not a wrong answer, it is a segfault
waiting for the right `Int`. The same rule is why a list pattern tests the **length** before it reads
an element (row 14), and why two alternatives binding one name to different words are emitted **once
per alternative** — a single block reached from both needs a `phi` per binder, discovered while
emitting the test that produced them, and one copy per alternative is the same behaviour with no join
to get wrong and a bound of 16 on the copying.

**A view has no order.** An `Html` in the arena is a recipe, so two nodes that render the same page
can be different objects: `html_text(3)` and `html_text("3")` are two words apart and one tree, while
`beck_core::Html`'s derived `Ord` compares the *pages*. A compiled comparison answering from the
recipe would disagree with the evaluator on exactly the programs nobody writes, which is worse than
refusing. So the representation table grew an `Absent` case with the reason, and the demand side grew
a walk: a `model Card { body: Html }` has no order because its field has none, a `list[Html]` because
its element has none. Every demand asks *first*, so the refusal names the definition that wanted the
comparison — and both emitters write **no comparison at all** for such a repr, so a bug in that rule
is a missing symbol at link time rather than a page that sorts by where it was allocated.

**A view's leaves are deferred values, and that is what made it cheap.** Four of the five `Html`
primitives take an `A`: `html_text(x)` is `x` displayed, an attribute's value is displayed, and a
handler's command is *JSON*. Compiling them the obvious way means generating a `display` per repr and
a `to_json` per repr, in both emitters, holding them to `beck_core`'s to the digit. Writing down
*what was asked* instead — the value's word beside the index of its repr — costs two words and no
generated code at all, and lets `html_text(x)` compile for every `x` that has a shape. The rules the
differ depends on live in one lifted function, `beck_core::html::element`: an attribute whose value is
empty is dropped, a handler becomes `data-b-<event>` carrying JSON, and a key is not an attribute. A
second spelling of any of them would be a compiled page differing from an interpreted one in a way no
type can catch and no rendering can show — two identical-looking trees whose structural hashes differ,
so the differ skips a subtree that did change.

## 93.7 The rule: never a worse asymptote than the evaluator's

**This backend does not ship an operation whose asymptote is worse than the evaluator's.** A program
that reads collections gets compiled; one that builds them keeps the tree-walker, and the refusal
says which. That rule refused `list_append` and `map_insert` for two reports each, and the two
refusals were retired for opposite reasons — which is the most useful thing in this chapter about how
a refusal should be read.

**`list_append`: the reason was true and the conclusion did not follow.** The argument on record was
*ownership*. The tree-walker pushes in place when [`70`](70-the-evaluator-gets-fast-report.md)'s
last-use analysis proves the accumulator is nobody else's; an arena has no ownership in it; therefore
`O(n)`; therefore refuse — otherwise the idiom every loop in this language is written as would be
`Θ(n²)` where the evaluator is `Θ(n)`, and [`46`](46-standard-library-report.md) §46.14's quadratic
would be rebuilt on purpose.

The step that does not follow is the second. **In-place mutation is one way to make an append cheap
and it is not the only one** — it is the one a `Vec` behind an `Arc` has available, because a `Vec`
is a length beside its buffer and the length is the thing that has to change. What an arena of offsets
has instead is that allocation is free and nothing is ever freed. Split the count from the elements:

| | word 0 | word 1 | word 2.. |
|---|---|---|---|
| **header** | how many elements | the data block's offset | — |
| **data block** | how many it can hold (`cap`) | how many are written (`used`) | the elements |

Appending writes at index `used` — a slot **no header covers**, because every header over a block has
a count of at most `used` — bumps `used`, and answers a *new* header. Nothing a reader can see is ever
rewritten. The soundness is one paragraph: every header's count is at most `u`, so a reader of a list
of count `c` reads slots `0 … c-1`, all `< u`; a slot is written exactly once, by the append that then
bumps `used`; an append from a header whose count is below `u` finds the slot taken and copies
instead. The one mutable word only ever grows, and is read and written by one compare-and-store. That
needs no ownership analysis, no reference count and no last-use flag.

The cost is one load per operation rather than per element — every generated loop takes the data
pointer before it starts — and a bigger empty list: `8 + 8n` bytes became `32 + 8n`. Three arena-shape
gates carry those constants and each was updated rather than relaxed, because what they assert is that
the number does not grow with `n`. `an_appended_accumulator_is_linear` reads the arena rather than the
clock and finds **4.0× for 4× the elements**.

**The part of this worth carrying is where the wrong reason came from.** `beck-eval` solves the
problem with uniqueness because `beck-eval` is written in Rust over `Arc<Vec<T>>`. The backend copied
the *shape* of that answer, found it unavailable, and stopped. The gate built at §93.9 asks whether a
refusal's stated reason is **true**; every sentence of this one was, and the refusal was still wrong.

**`map_insert`: the reason survived, and a different representation removed it.** A map is ordered by
key, so an insert lands in the middle and every entry after it shifts — a sorted run cannot be extended
in place anywhere but the end, and a program inserting in key order would be the only one that ever hit
the fast path. That is a property of the run and not of where its count sits, so no amount of layout
separation helps, and the gap is `O(n)` against `O(log n)` rather than a constant. What removes it is
the structure `beck_core::pmap` already uses: a weight-balanced tree with the same `DELTA` and `RATIO`
the evaluator's own module argues for. Insert walks down comparing keys, rebuilds the path on the way
out, rebalances at each step, and shares every subtree it did not touch — `O(log n)` fresh nodes, and a
node is never written after it is built, so §93.7's soundness argument comes free here rather than
needing a design.

The division of labour is what kept it to one change: **everything that moves nodes is one function for
the whole module**, because rebalancing shuffles words and never asks what a key is. Only `find`,
`insert`, `remove`, `merge` and the two-map order are generated per repr, because those compare.
`a_fold_over_a_map_is_not_quadratic` reads the arena and finds **4.9× for 4× the entries**, and asserts
`< 2 × steps` rather than a tight bound on purpose: what separates linearithmic from quadratic here is
a factor of three, and a gate that split them at 5.1 would be measuring the balance constants.

Reads pay for it — `O(log n)` with a pointer chase per level where the sorted run had an indexed probe,
and `map_keys` goes from one `memcpy` to an in-order walk. Both are stated here rather than left to be
found.

**Where the rule still bites, it bites text.** `acc + s` allocates the whole accumulator every step, so
§93.5's `grown` row is 0.17× at 4,000 appends. The measurement *asserts that row is below one*, so a run
in which the compiled accumulator caught up would go red — it would mean the evaluator had lost its
in-place append, which is a finding rather than good news. Text shipped `+` anyway where `list_append`
was refused, and the difference is not principle but what refusing costs: `+` is the **only** way to
build a `Str`, so refusing it would mean a `Str` could be received and read and never combined. The
same split §93.7 gave a list is available for text and is not obviously right, because a `Str`'s bytes
are read by `memcmp` and `memcpy` in six runtime functions that currently need no indirection at all.

## 93.8 Two emitters, one layout, and what writing it twice found

The **subset** — which definitions compile, and the reason each refusal gives — is a second
implementation rather than an import. That is deliberate and it is the whole argument for two
backends:

> `cranelift.rs` asserts the two emitters accept and refuse exactly the same definitions, over every
> program in the suite and every program in the corpus. A shared implementation would make that
> agreement true by construction and therefore worth nothing.

The **layout** is not that kind of thing. It is a contract between *three* parties — both emitters and
the host that writes a `Value` into it and reads one back — and two of the three are not emitters. So
`beck_llvm::heap` holds the layouts, the tags, the field order, the encoder and the decoder, and the
line is: **what a program means is written once; how one backend says it is written twice.** The
monomorphisation pass (§93.10) is shared for the same reason — it is not a code generator, it is the
program both of them are handed.

The fixed point is the same shape in both — emit every body, drop whichever will not emit, repeat — so a
definition that calls a refused one is refused in turn and mutual recursion survives or is refused as a
pair. `a_refusal_travels_to_whoever_calls_it` checks both directions: a sound mutually recursive pair
still compiles, so the fixed point is not just refusing every cycle. Cranelift re-emits the whole
module each round, because "compiles" and "emits" are one question: an analysis that *predicted*
emissibility would be a second implementation of the emitter and the two would drift.

Four things the second emitter does differently, none of them a semantic difference: a tail call is
`return_call` under the `tail` convention; a block parameter replaces a `phi`, and an `if` whose two
arms both return reaches a join nothing jumps to, which still needs a terminator; a `Bool` is an `I8`
holding 0 or 1, so `not` is `bxor 1` rather than a complement, which would answer 254; and there are no
intrinsics, so `sin` and `cos` are calls into the same `libm` `clang` lowers `llvm.sin.f64` to.

### What writing it twice actually caught

**The first program the new backend ran** was five definitions long, and one was
`def negative(x: Float) -> Bool: return x < 0.0`. It answered `false`. Both backends compute the order
key the same way; what the new one got wrong was the comparison *of* the key, which maps every real onto
the **unsigned** order and this compared signed. It is worth being precise about what caught it: not the
676-pair float sweep, which had not been written, but five lines and one call. That is
[`82`](82-the-edge-report.md) §82.7's lesson in the ordinary direction — the cheap test
that runs first finds the bug that is *everywhere*, and the expensive one earns its keep on the bug in
one corner.

**Then the same defect four times, in both emitters, over three reports.** A record's field comparison
compared a **reference by its offset**, so two equal values compared unequal whenever they were
allocated at different places:

| | where | caught by |
|---|---|---|
| text | Cranelift, a `Str` field | the differential's **pairs** |
| lists | LLVM, a `list` field | the differential's pairs |
| maps | Cranelift, a `Map` field | the differential's pairs |

Every time, the single-value sweep would have missed it and the pairs found it —
`Named(label="") < Named(label="")` answering `true`, `Bag{items: []} < Bag{items: []}` answering
`true`. The shape of it is: **adding a representation variant is a compile error at every site that
matches exhaustively and silence at every site with a `_`.** That is
[`90`](90-pattern-matching-report.md) §90.5's `Arm` gaining a field, one enum over.

The list report named what would stop it — an accessor the comparison is written against instead of a
match — and the map report built it. `Repr::order` answers one of four things: compare the words
(signed or not), take `beck_core`'s order key first, **call this symbol**, or `Absent` with a reason.
It is the only place in either backend that names a comparison function, every consumer matches on those
four cases rather than on the representation's six, and a new reference kind is now a compile error *in
`Repr::order`*, where its comparison has to be named.

The order of events is the useful part: the accessor **did not prevent the fourth occurrence** — one of
five sites had not yet been converted when the map differential ran — it made the defect one line to fix
in one place instead of a hunt. What prevents the fifth is that all five sites now go through it. When
the view arrived it did exactly what it was built for: a new repr, a new case in one enum, and a compile
error at every site that has to say what it means.

**And one defect a layer below any of that.** The first end-to-end call of a caught raise came back as
*"the compiled program answered with offset 64, and its heap is 0 bytes"* — a decode failure on a call
that had answered correctly. The handler cleared the trap code with `store i32 0`. The cell's first
eight bytes are a `u32` code **and** a `u32` span, and the worker's loop reads those eight bytes as one
`i64` to decide whether the call answered — so a caught failure came back with the raise's span sitting
in the high half of a word the protocol compares against zero. Everything *inside* the function was
right. What was wrong is that two pieces of one program disagree about what the cell **is**, and
"cleared" is a different act under the two readings. The fix is one character; the reason is that this
is the layout problem one level down, in the one piece of shared shape written as three constants rather
than as a type.

## 93.9 A refusal is a claim, and what it took to keep it true

Everything this backend does not compile is refused **by name, with the reason**, and the reason is the
command's main output rather than a footnote:

```console
$ beck native examples/todo.beck
8 compiled to native code:
  if_owned … mine … remaining … view … render … done_class … apply_event … toggled

1 left to the evaluator:
  validate      `str_trim` trims Unicode whitespace …
```

That output is a claim the compiler makes to a reader, and **four of those claims were false at some
point, each for at least one whole report**. This is the most transferable finding in the chapter,
because none of it is about code generation.

**A reason that was false.** `str_index_of` was refused because "its `Option` has no layout here" — and
the prelude's `Option` had had one since row 3, with a definition returning `Option[Int]` compiling in
the fixture *beside* the refusal while that sentence was being written.

**Every gate around it was green**, and that is the part worth reading twice.
`what_the_heap_does_not_reach_is_refused_by_name` asserted the reason contained a string;
`the_two_emitters_accept_and_refuse_the_same_definitions` asserted both emitters said something;
`what_cannot_be_compiled_is_refused_by_name_and_with_a_reason` asserted the reason was non-empty. All
three assert that a refusal **said** something and none asks whether what it said is **so** — which is
[`82`](82-the-edge-report.md) §82.10's pattern in the one place this project
had not looked for it: a proxy for a control is defeated by naming, and a reason is a proxy for a fact.

So: `a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one`. It takes every type a refusal
blames for having no layout, asks the heap whether it has one, and asserts the control the missing
assertion was — `Option[Int]` **does** have one, so no refusal may say otherwise.

**The gate fired, twice, and was right both times.** When closures arrived, `Heap::repr` stopped
answering "a function value, which is a closure" for a function type, because a closure now had a
layout. What is true afterwards is narrower and needed writing in two parts: the shape **exists**, and
the **boundary** is what refuses it. The gate now asserts both, and the reason a definition sees names
the boundary rather than the layout.

**Then it could not fire, and that is the second half of the finding.** *"A collection is not on this
heap yet"* **names no type** — there is no `Ty` in it to resolve — so a gate that resolves types had
nothing to look at, and the sentence survived three reports after collections arrived. A gate tests the
*shape of the gap* it was written for; this one was written for a reason that named a type, and what it
did not cover was a reason that named a **class** — which is exactly what a report retires. So the gate
grew a second half in the corpus pass where every refusal is already collected: **a list of sentences
this backend may no longer say about itself.** "not on this heap", "no collection", "text is not" — a
refusal containing any of them fails the suite, by name, and it was checked by putting the old refusal
back and watching it name its six definitions.

**A reason that could not fire at all.** The refusal table had an entry for the seven higher-order
collection primitives — "takes a function, and a function value is a closure". True, and unreachable:
the emitter evaluates a primitive's *arguments* before it looks at the operator, so the argument fails
first with something truer and more specific. A reason that cannot be produced is
[`23`](23-incremental-views-report.md) §23.16's rule that could not fire, and it got the same answer:
deleted.

**A reason that came from a default rather than from an arm.** "`raise` is not one of the scalar
primitives" came from the catch-all, which is why it read as a statement about a category instead of
about the thing that was missing. A refusal inherited from a default is a claim nobody wrote.

Two smaller pieces of the same discipline. The **refusal lists** in the test fixtures move a name from
the refused side to the *control* side in the same commit that makes it compile — that test's own
documentation says each row "goes red the day its row starts compiling, which is the day the row should
be deleted", and five rows have made that journey. And **prose drifts where a list with a test attached
cannot**: `beck_llvm`'s own module documentation was stale three separate times — "a `list`, a `Map`, a
closure, `Html` and `Unit` are refused" after two of the five had layouts, "text, collections, closures
and every effect are still the tree-walker's" after text arrived, and "growing a **map** is refused"
after it stopped being. Each was caught by a reader rather than by a gate, which is
[`82`](82-the-edge-report.md) §82.10's point about prose lists.

**And the ceiling that was not reachable.** `Heap::decode` walked a reply by recursing, and
`MAX_DEPTH = 2048` bounded that recursion. Adding a `list` arm made the frame bigger and a `cargo test`
build aborted the process on a value 800 deep; splitting the arms bought back the 800, and the
measurement that followed is the part worth recording — a debug build then managed **1,200 and aborted
at 1,600**, against a declared ceiling of 2,048. The number in the source had never been the limit. The
limit was the thread's stack, so *which replies could be read depended on how the compiler was built*,
which is the same shape as §93.3's `negate` and the property
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) states: a ceiling is declared, not
discovered. The decoder is iterative now, with its stack on the heap, and
`a_value_at_the_declared_ceiling_decodes_rather_than_aborting` builds a value exactly that deep by hand
on the same default-stack thread `cargo test` gives everything else.

## 93.10 A generic definition, once per type it is used at

A definition with type parameters compiles as one function per instantiation. The refusal was true as
stated — *"generic over `T` — a type parameter has no machine representation here"* — and what it did
not say is that the **program does not need one**: every call site says what `T` was, and by the time a
backend sees the program it has been saying so all along.

There is no type-argument list in `Core`, no instantiation table, and no substitution that survives
checking. Looking for one is what makes this sound like a compiler project. **Every `Core` node carries
its solved type**: a call to a generic definition is `App { func: Global(name), … }`, inference
instantiates the callee's scheme with fresh variables and writes the result onto that `Global` node,
and `resolve_types` grounds every node at the end of checking. So the `Global` node's type is the
*instantiated* function type while the definition's own parameters still name the rigid type the checker
minted.

| | `firstly`'s declaration | the node at `of_ints`'s call |
|---|---|---|
| parameters | `list[T]`, `T` | `list[Int]`, `Int` |
| result | `T` | `Int` |

Walking those two together, one structure at a time, reads `T := Int` straight off. That is thirty
lines and it is the whole mechanism. The checker had been carrying the answer since
[`27`](27-the-walls-come-down-report.md) gave the language generics, and nothing had read it.

It is a **backend pass** because [`38`](38-literature-survey.md) §38.1 had already settled that:
dictionaries are the semantics and monomorphisation is a backend choice, on the grounds that
whole-program specialisation fights incrementality. So it runs on a `Program` clone inside the native
backends; `beck-core` does not change, the evaluator still executes one generic definition uniformly,
and `beck test`, `beck run` and `beck up` cannot tell the pass exists.

The name is `firstly@Int`, on the separator `Trait::method@Target` already uses. Two things about it
are load-bearing: it is keyed on the **whole type** rather than the head constructor — `dictionary`
deliberately keys impls on the head so `Tree[Int]` and `Tree[Str]` share one, which is right for a
dictionary and wrong here, where they are two layouts — and it is keyed on the type rather than the
**representation**, since `Int` and `Bool` are both one immediate word and a backend that merged them
would answer `1` where the evaluator answers `true`. Two type parameters are read off in **use** order,
so `swapped[Str, Int]`, whose body is `paired(b, a)`, calls `paired@Int,Str`: the same instantiation
another caller asks for directly. The differential asserts that sharing rather than the count, because a
positional recovery reading the arguments off the declaration would mint a second function and still
compute the right answers.

Three refusals, each real. **Polymorphic recursion**, where the program is finite and the set of
instantiations is not — `MAX_INSTANTIATIONS` is 64, against 65 templates and 28 instantiations across
the whole tree with a maximum of three for any one definition, so the budget is a bound rather than a
policy. **A call where nothing decides the type**: `list_len(anything())` finishes inference with a
*variable*, and minting `anything@?3` would make a symbol a function of an inference counter rather than
of the program — a determinism defect wearing a feature's clothes. And a **bounded** definition, which
is not a template here at all, because `expand_bounds` already turned its bounds into value parameters
holding function values — §93.15's closure boundary, and not something this pass should appear to have
answered.

**The finding is about giving up.** The first version of the pass gave up part way through: on the
polymorphically recursive program it built sixty-four instantiations, discovered it could not finish,
and stopped — leaving sixty-four refusals, every one of them true, none of which named a definition the
reader had written. So a round that keeps a template it had been specialising is **thrown away** and
re-run with that template forbidden from the start, leaving exactly one refusal naming exactly one
definition. *A bounded search that gives up should give up on the whole thing*: the intermediate results
of an abandoned search are individually correct and collectively misleading, and a refusal a reader
cannot act on has failed at its one job.

One number needs explaining rather than defending. Refusals blaming a type parameter went 63 → 38, and
**25 of the 38 are `lib/collections.beck`** — measured the way every number in this chapter is measured,
each file compiled *alone*. A library's generic definitions have no caller in their own file, so nothing
asks for an instantiation. Linked, they specialise: a program importing `collections` and calling
`size(set_of(xs))` compiles eight instantiations from five templates, and what stays refused is what that
program does not call.

## 93.11 The host answers back

Every feature above is the same motion: the host asks a question, the worker computes an answer, one
message goes each way. Four primitives are not computations at all, and no amount of machine code
produces any of them:

| primitive | its atom (§3.2) | what the answer depends on |
|---|---|---|
| `now()` | `nondet` | when the call happened |
| `uuid()` | `nondet` | a source of randomness |
| `secret_env(name)` | `env` | the process's environment |
| `http_fetch(host, req)` | `net.out(host)`, `raises(HttpError)` | a peer |

The worker has to stop mid-call and ask. A question is 32 bytes with the same five fields a reply has,
told apart by its first word — a marker that is `u32::MAX`, no trap code and not zero — followed by the
shape the answer should have, the shape a failure would carry, and a shape and a word per argument.

**The shapes are the design.** They are indices into the module's word table, so the host decodes and
encodes through the same `heap` module without a second table saying what `secret_env` takes and what
`http_fetch` answers. It is the deferred value of §93.6's view row one subsystem over: the repr becomes
a *datum*, and the host — which holds `Value` and every function that builds one — does the rest. The
four match arms in `perform` are the only per-primitive code on the host's side.

**The answer is appended, never assigned.** The worker sends its arena's high-water mark; the host sends
back bytes to put *at* it, never a whole arena. So nothing a live compiled value points at can be
rewritten by an answer, and that is a property of the protocol rather than a discipline somebody has to
keep.

**The arena travels only when an argument could point into it.** `now()` and `uuid()` take nothing, so
their question is a header and four words however much the program has allocated; `secret_env` is handed
text the program built and `http_fetch` a record. §93.5 is what that costs, and
`what_a_question_carries_is_a_decision_and_not_an_accident` is the gate with no clock in it: 0 bytes at
both sizes for `now()`, 664 and 163,864 for `secret_env`, and a definition minting four ids asks **four**
questions rather than one.

**One host, asked by three backends.** The tree-walker reached these four through a trait in the
evaluator's crate. A compiled backend reaching them a second way would make the differential a comparison
of *two descriptions of a host* rather than of the program, which is the one thing a differential must
not be. So they moved to `beck_core::host::Atoms`, every method has a default, and every default is the
seam [`14`](14-review-findings.md) F11 asks for: `beck_core::clock` for the wall clock,
[`net`](../compiler/crates/beck-core/src/net.rs) for the outbound call, the process environment for a
secret, and a UUIDv7 minted from the injected clock. Two backends reading the process clock one after
the other are not in the same millisecond, so a differential that did not hand all three *one stated
host* would be asserting that two calls happened at once. The `HttpRequest` → `net::Request`
translation moved with the trait for the same reason, and it is also where §3.5's one legitimate
unwrapping of a `secret[Str]` happens.

**The type that was actually in the way.** `secret_env` answers a `secret[Str]`, which no program
declares and the prelude does not declare as a `TyDecl` — so it had no layout, and `HttpRequest`'s
`secrets: Map[Str, secret[Str]]` field made the record `http_fetch` takes unlayoutable too. At run time
a `secret[T]` is already a one-field object **on purpose**: the wire format and the digest have to tell
one from the `T` it holds, which is what §3.5's claim rests on. So it is laid out as the newtype it
already behaves like. Unwrapping would have been smaller and would have made the two indistinguishable
in compiled code, which is exactly the property the type system is asserting.

**A secret's bytes now cross into the worker process**, and that is worth saying out loud rather than
leaving in a diff. It weakens nothing §43 claims — a `secret[T]` still cannot reach a client partition,
the placement solver is unchanged, `digest_keyed` is still the one declassifier — and the worker is a
child of the process that already holds the secret, spawned by it, reading only from its pipe. What it
means is that the set of processes a credential is in has grown by one on a machine running
`beck native`, and [`43`](43-threat-model.md)'s A1 row is where that lives.

**A failure is an answer, and nothing was added to make that work.** `http_fetch` fails by raising
`HttpError`; the host answers with the raise code and a two-word pair, which is precisely what a compiled
`raise` builds, and the program's own `try:` catches it without knowing an upcall happened. One thing is
written by the *worker* rather than the host: the offset of the error type's name in the literal pool,
because only the module knows which offset that is and the host holding an opinion would be two spellings
of one name.

**The finding: a limit on compiled time is not a limit on a call.** The worker carries an optional
wall-clock limit and a watchdog that kills it, because there is no fuel in compiled code — it bounds a
program that will not stop. An `http_fetch` that waits thirty seconds for a peer is not such a program,
and under the first version it was killed as though it were. Both halves were correct: the limit is right
about compiled code and blocking on a peer is right about a network. What was wrong is that one clock was
asked to mean two things. So the deadline is **stood down while the host works** and re-armed as a *fresh*
deadline afterwards — not the old one, because charging the compiled half for time the host spent is the
same conflation in smaller print.

## 93.12 A primitive that is somebody else's table is linked

Fifteen primitives are neither a computation an emitter can produce nor a question only the process
outside can answer. `digest`, `digest_keyed`, `digest_eq`, `hex_encode`, `hex_decode`,
`base64_encode`, `base64_decode`, `uuid_parse`, `uuid_version`, `str_upper`, `str_lower`,
`str_to_int`, `str_replace`, `time_format`, `time_parse` — each one a **pure function of its
arguments whose correctness is somebody else's artefact**: a Unicode case table, BLAKE3's round
function, RFC 4648's alphabet, Rust's `i64` parser, the civil calendar. Both emitters refused all
fifteen, and the refusals said why in as many words:

> `str_upper` **is Unicode case mapping, which is a table rather than an operation** — and a
> compiled half-answer that folded ASCII only would disagree with the evaluator on the first letter
> that is not

There were three possible answers and only one is both fast and exactly right:

| | Fast | Exactly the evaluator's answer |
|---|---|---|
| **Ask** (§93.11's upcall) | no — a pipe round trip per call | yes |
| **Emit the algorithm** | yes | only as far as it has been tested |
| **Link the implementation** | yes | yes, because there is one implementation |

So a compiled program **links a static library**, `beck-prim`, and it is not new code:
`beck-core`'s digests and encodings and `beck-eval`'s civil calendar moved into it, and the
evaluator calls it too. "Both backends compute the same digest" is therefore a property of there
being one function rather than a claim a differential supports. This is what a compiler normally
does — `rustc` ships `libstd`, `clang` ships `compiler-rt` — and nothing in the subset had needed it
before, because arithmetic is instructions and a `Str` is §93.2's `memcpy`.

**The measurement is the argument**, and the mechanism it was chosen over is measured beside it in
the same run: `cargo test --release --test measure_native -- --nocapture`, with each primitive
called `n` times *inside one compiled call* so that what is timed is the primitive rather than the
worker's round trip.

| definition | calls | evaluator/call | native/call | ratio |
|---|---:|---:|---:|---:|
| `digest` | 1,000 | 785 ns | **262 ns** | 3.0× |
| | 10,000 | 1,309 ns | **274 ns** | 4.8× |
| `str_upper` | 1,000 | 662 ns | **122 ns** | 5.4× |
| | 10,000 | 1,463 ns | **122 ns** | 12.0× |
| `str_to_int` | 1,000 | 691 ns | **65 ns** | 10.6× |
| | 10,000 | 1,596 ns | **61 ns** | 26.2× |
| `now()`, **asked** rather than linked | 100 | 415 ns | **7,816 ns** | 0.1× |
| | 1,000 | 445 ns | **5,198 ns** | 0.1× |

A question costs **5.2 µs** and a linked primitive **61–274 ns**, so a digest asked for across the
pipe would cost nineteen times what computing it costs and a `str_to_int` eighty-five times. The
last two rows are the shape all fifteen would have had: the *evaluator* ten times faster than
compiled code, because the compiled code is waiting on a pipe and the tree-walker is calling a
function.

That 5.2 µs is smaller than §93.5's 24.5 µs for the same primitive, and the difference is what each
is measuring: there, a `now()` and the *call* that carried it, one worker round trip included; here,
a `now()` issued from inside a call already in flight, a thousand of them to one round trip. The
cheaper of the two numbers is still nineteen times the work it would be replacing.

**No pointer crosses the ABI, and that was the decision** rather than a detail
([`adr/0029`](adr/0029-the-runtime-library-is-linked-and-owns-the-arena.md)). §93.1's
`forbid(unsafe_code)` is structural, and a library whose entry points took `(*const u8, usize)`
would need `slice::from_raw_parts` in the first line of every one of the fifteen. So the arena is
turned around: **the library owns the heap**. Generated `main` asks `beck_prim_arena` for it instead
of calling `malloc`, and every call after that carries offsets into a `Vec<u8>` the library holds,
where reading one is an index and a bad one is a bounds check.

```c
uint8_t *beck_prim_arena(int64_t bytes);
int64_t  beck_prim(int32_t op, int64_t mark, int64_t a0, int64_t a1, int64_t a2);
```

That is §93.2 paying for itself a second time: a value that is an **offset and not a pointer** was
made so that a heap could cross a pipe as bytes, and the same property lets one cross a C ABI as a
number. `beck-prim` contains no `unsafe` block and no raw-pointer read — only the two attributes
rustc demands on an export, counted exactly by the gate that counts `beck-wasm`'s.

**The answer comes back above the water line.** `beck_prim` allocates from the mark it was given and
writes a two-word outcome record — a status and a word — immediately *above* everything it
allocated, answering with that offset; the caller stores it as the new mark and reads the record
from it. The record is scratch, live exactly as long as the caller needs it, so a call costs no
arena beyond its answer. Below the mark it would be correct and would leak sixteen bytes a call
without changing a single answer, which is why the gate for it counts bytes rather than seconds
(§93.14).

**What the library does not do is build a value.** Five of the fifteen fail on bad input, and a
failure here is a *declared* value — `EncodingError.BadEncoding(encoding = "hex", why = …)` — whose
layout belongs to whichever emitter asked. So the failure is **described** rather than built:
`beck_prim::Op::raises` names the type, the variant, the fields the primitive fixes and the field
the message goes in; the library produces the message; each emitter builds the value and stores the
type's name in the error cell for a `try:` to compare, exactly as `raise` does. `str_to_int` is the
same division for a different reason — it answers `Option[Int]`, so the library says *there is no
value* as a third status and the emitter builds the `None` its own layout calls for.

**What it costs.** The archive is 21.4 MiB of `staticlib`, 6.1 MiB compressed, which is what the
`beck` binary carries; it is not stripped, because the embedded bytes would then depend on whether
the machine that built `beck` had `strip` and [`92`](92-supply-chain-and-release-report.md)'s
provenance is worth more than the sixth it would save. A program that *links* it goes from 16 KiB to
**4.9 MiB** — almost none of it the digest, most of it Rust's standard library and most of *that*
the panic hook's backtrace symboliser. So "only when it is called" is a gate rather than an
intention, and it decides the arena's source too: a module that reaches none of the fifteen names
none of the library's symbols and still calls `malloc`.

**The finding is one an emitter written twice produces** (§93.8's theme, in a new place).
`digest_eq` is the first primitive in this backend's history whose answer arrives as a word and is a
**`Bool`**, and Cranelift's `narrow` — the function turning a protocol's eight bytes into the value
its repr says it is — had a `Bool` arm that extended an `I8` to an `I8`, which the verifier rejects.
The arm had been there since §93.11 and had never executed: the four host primitives answer an
`Int`, a `Str` and a record, and not one of them answers a `Bool`. A helper written for *n* callers
is tested by *n* callers, and the arm no caller reaches is the arm that is wrong.

## 93.13 What it costs, said plainly

**Memory is not reclaimed inside a call.** There is no collector. A loop that allocates a million objects
holds a million objects whether or not the program can still reach them, and the ceiling is 256 MiB. That
is a real difference from the evaluator, which frees what an `Arc` drops.

**A reply carries the used arena, not the value.** The worker cannot tell which objects the answer reaches
without a walk it has no code for, so a call answering with an object sends back everything it allocated.
A call answering with a scalar sends nothing at all. This is why §93.5's list and closure rows lose ratio
at the larger size while their controls hold theirs: what grows is the reply.

**`with` always builds a fresh object.** [`70`](70-the-evaluator-gets-fast-report.md)'s analysis lets the
tree-walker rebuild a record in place when the base is a last use, and
[`63`](63-expressiveness-report.md) §63.11 made that hold for `x.with(f = g(x.f))` too.
Neither is available here, and the answers are identical — a cost, not a divergence.

**The literal pool is on the wire on every call.** `examples/todo.beck` has 22 literals and pays 560
bytes, `lib/dates.beck` 20 and 480. Against a 35.6 µs round trip this is not currently measurable, and it
is written down because it is a cost that grows with a *program* rather than with a call. The version
that does not pay it copies the pool into the arena once at startup and shifts every argument offset,
which is a protocol change rather than a tidy-up.

**A slice always allocates**, and `str_slice(s, 0, str_len(s))` builds a copy where the evaluator's own
constructor also builds one — a match rather than a cost. But there is no substring sharing its parent's
bytes, and an arena of offsets could have one; it does not, because a shared substring is a second kind of
`Str` and the host would have to know about both. The same holds for a list tail: `*rest` binds a **fresh**
list, and the tempting borrowed suffix is wrong for a specific reason — the data block's own header carries
`used`, so a suffix header offset into the element run would have an element read as that count, and an
append onto it would write wherever that element pointed. The evaluator copies too, so both backends are
the same shape of slow, which is the property that matters for a differential.

**A slice of non-ASCII text is a walk.** Constant time when the byte and character counts agree, and
otherwise character by character, where the evaluator has a chunked index. Every program in this tree
slices ASCII; the fix, if one ever needs it, is the same index in the same header.

**A value the host decodes is bounded at 2,048 deep**, which the evaluator is not. It is what stops a
compiler bug from becoming a blown host stack, and it is a limit on what a compiled definition may
*answer* with.

**A long accumulator leaves about 4× its elements behind**, because a data block that doubles holds up to
twice what it needs and an arena frees nothing. The gate asserts that is linear; it is not a claim that it
is small.

**The arena's base is loaded per access on one backend and hoisted on the other.** The LLVM emitter writes
text and can insert a line into the entry block after the fact; the Cranelift one marks the load
`readonly` — true, because `main` writes it before any compiled code runs — and lets the alias analysis
fold the repeats. Two answers to one question, and only the first is measured.

**`filter_list` allocates for every element and writes the count at the end.** The arena needs a size
before it has elements, so either the predicate runs twice per element or the list is allocated for all of
them. It is the second: one pass, and the words past what was kept are arena nobody reads, given back when
the arena is reset. A predicate called twice would be a cost the evaluator does not have, to save memory
the next allocation does not need.

## 93.14 The gates

The differential is the point, and it compares the **whole outcome** — the value, or the failure and its
message — because an integer overflow is a value in this language and a backend that failed for a
different reason has diverged. `native.rs` is the LLVM pair and `cranelift.rs` the three-way, over one
shared set of programs and arguments rather than two copies, because a second copy would be a second
opinion about what the subset is.

| What agrees | Calls |
|---|---:|
| scalar arithmetic, control flow, recursion (evaluator ↔ LLVM) | 15,441 |
| the same, three-way | 13,152 |
| plus 60,000,000 tail calls and a million into a definition of another arity | |
| records and unions | 1,222 |
| text | 2,893 per backend |
| lists | 1,425 per backend |
| maps | 1,216 |
| closures | 1,108 per backend |
| views | 253 (LLVM), 127 (three-way) |
| failure | 84 each |
| generics | 103 / 100 |
| the host effects | 16 each |
| list patterns | 50 each |
| the runtime library | 808 each |

Each alphabet is chosen so that a specific mistake would show, rather than to be large: an embedded NUL,
so nothing can reach for `strlen`; two-, three- and four-byte characters; a prefix pair, because
`memcmp("ab", "abc", 2)` answers `0` and the length has to decide *after* the bytes; a map key **between
two** others, which is the case a window that shrinks wrongly never leaves; two closures of one lambda with
different captures, which must compare **equal** and which a compiled backend guessing would guess
otherwise; a key that is not an attribute; a handler whose command is JSON; and, for list patterns, the
empty list, where an element read before the length test would fault.

**The differential's pairs are what caught the four-times defect** (§93.8), and the reason they are pairs
rather than values is that a comparison is the only place two equal things can be distinguished.

Beyond the differential, three kinds of gate matter more than a timing one:

- **Shape gates with no clock in them.** `chain(100)` and `chain(800)` leave 40 bytes an element at both
  sizes; a slice costs its answer and not what it was taken from, at 200 and 1,600; a page of text costs
  144 bytes a row and 576 a page, at 100 rows and 800, and a page with a key, a conditional class and a
  handler on every row costs the **same bytes for equal steps of rows** — 145,600 for each 200 — at 200,
  400 and 600, which is three sizes because two points always fit a line; a fold that builds nothing
  costs one closure and its answer
  at both sizes; an appended accumulator is 4.0× for 4× the elements and a map fold 4.9×; a raise caught
  25 frames up and one caught 200 frames up leave the **same 168 bytes**; and 800 more `digest` calls
  cost 800 answers and nothing else, which is the deterministic form of §93.12's claim that a call
  costs no arena beyond what it answers with. Every one of these fails on a
  design that is quadratic in the arena and answers correctly at every size a test would run. It is
  [`64`](64-compile-speed-report.md) §64.1's pattern — gate the shape, print the rate — applied to memory.
- **Parity gates.** `the_two_emitters_accept_and_refuse_the_same_definitions` over every program in the
  suite and the corpus, which also gates the *order*: a compiled definition gets the same dispatch index
  in both, so the table is a property of the program's declaration order rather than of a hash map's
  iteration in either. And `a_corpus_fold_compiles` asserts by name, with the other side asserted too so
  it cannot pass by everything compiling.
- **Control gates.** The stated host tallies its outbound calls and the differential asserts the count is
  exactly twice the cases that reach `http_fetch`, because a run in which one side quietly fell back to the
  evaluator would agree on every value and be worth nothing. `scalar_and_fine` has to still compile, so a
  list of refusals cannot pass by refusing everything. And a module that reaches none of §93.12's
  fifteen primitives must name none of the runtime library's symbols: on the Cranelift side getting
  that wrong is a link that fails rather than a large binary, because an imported symbol nothing
  resolves has nothing to resolve against.

One thing the gates found rather than asserted: **a raise inside `map_list`'s generated loop already
worked**, on both backends, with nothing written for it. A loop applies a closure and checks the error cell
after it, and a raise is a code in that cell — so "leave the loop rather than run the next element" was
true the moment a raise could happen at all. That is the argument for reusing the fault path rather than
building a second one, cashed.

## 93.15 What is not built, and what is open

### Not built

**The signal vocabulary.** `merge_clients`, `fold`, `durable`, `decide`, `per_session`, `presence` and
`freshness` are not primitives a compiled body calls: the splitter reads them out of the program and wires
the runtime accordingly (§3.7), and the evaluator refuses to evaluate one for that reason. So the fold,
`validate`, the view-as-a-signal-node and `parallel:` still cross the seam. A compiled *fold* is a
different item, and the four host primitives are not a down payment on it.

**A function value at a boundary**, which is the largest remaining refusal class by far — every
`parameter f is a function value`, and every `Seq[T]` in SICP's chapter 3, whose
`Cons(head, rest: () -> Seq[T])` puts a closure in a *field*. A closure may be built, bound, captured and
applied inside one compiled call, and every place the host would have to read one back refuses it by name.
The refusal is not a gap in an emitter: it is what the host would have to do with one, which is to produce
an evaluator's closure out of bytes. It is *possible* — the module knows all four parts — and it buys
almost nothing on its own, because the case that matters is a compiled definition calling another compiled
definition through a function parameter, and **that needs no marshalling at all**. What it needs is the
signature rule relaxed for a call *between compiled definitions* while the worker's protocol keeps refusing
one, which is a distinction the `Signature` type does not currently make. That is a smaller change than it
looks, and it is the one that would move the class.

**A bounded definition**, which is the same thing: a dictionary is a function value.

**Two calls in flight.** The worker's pipe is behind a mutex and a call holds it for its whole duration,
upcalls included, so a fold blocked on a peer blocks a view that wants the same worker. The runtime calls
the fold from a sequencer task and a view from a connection task, so this is a real constraint the day
anything but a benchmark uses this backend, and it is the first thing a second version changes.

**Mode B's codegen**, which is *not* a remainder of this work. Cranelift compiles Cranelift IR to machine
code, which is the opposite direction from the WebAssembly a browser needs — Wasmtime uses Cranelift to
compile wasm, not to produce it. A Mode B code generator is a **third** emitter against a wasm target
([`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md)), and
[`94`](94-the-client-report.md) §94.12's measurement says what it would buy: a code generator divides the
constant and leaves the growth.

**`beck dev`**, which is what a fast code generator is *for* — there is no watch loop, no hot reload and no
incremental recompile, so §93.5's build-time number is a time nothing is yet waiting on. No `--opt-level`
switch either.

**A non-empty map literal**, which would have to sort at run time — a sort in emitted code, twice, for a
form that is almost always written empty. Every `durable` fold in this tree starts at `{}`, which was the
case that mattered.

**Four primitives, and the two `Json` ones.** `str` of a **`Float`**, whose shortest round-tripping
decimal is an algorithm rather than a loop and would have to be Rust's to the digit — `str` of an `Int`
compiles. `list_flat_map`, which answers a list whose length is a sum over the lists its function
answers, and `list_zip_with`, which answers a list of pairs where there is no pair type to lay out.
And `json_parse`/`json_render`: a `Json` document's object variant is a `Map[Str, Json]`, whose shape
in the arena is the emitter's, so the runtime library of §93.12 cannot build one and reading one back
is the same problem inverted. The way in is visible — the library parses with `serde_json` and writes
a flat node array, and each emitter generates a builder that walks it with its own layout and its own
`map_insert` — and it is a piece of work rather than a line.

*Corrected in place:* this list used to carry `str_upper`, `str_lower`, `str_trim`, `str_replace`,
`str_repeat`, `str_to_int` and `sort_by` as well. All seven compile — the first four and `str_to_int`
through §93.12, the other two before it — and the probe that says so is one definition per primitive
through `beck native`.

**In-process execution**, which is refused rather than missing.

### Open

**There is no fuel in compiled code.** [`53`](53-are-we-fast-yet-report.md)'s per-call step budget is a property of
walking a tree; machine code has no step to count without paying for the counter on every one. What is here
instead is coarser and says so: an optional **wall-clock limit** on one call, after which the worker is
killed and the call is an error naming the limit. It bounds a program that will not stop; it does not bound
one that is merely slow, and it is not a quota.

**There is no depth ceiling either, and that is a regression against
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md).** A recursion that is *not* in tail
position spends a real frame and nothing counts them: the tree-walker's "evaluation nested 4000 deep" has no
native counterpart, and what happens instead is the process dying. The host at least *explains* it rather
than passing on "failed to fill whole buffer". Closing it properly costs a depth parameter threaded through
every non-tail call — one increment and one compare — and that number has not been measured, so it has not
been spent. `pascal(0, 1)` in `sicp/ch1.beck` is the concrete case: outside the function's domain,
recursing without bottoming out. The evaluator answers with a diagnostic; the worker dies.

**The differential's arguments are chosen, not swept.** Because there is no fuel, a generated
`factorial(i64::MAX)` would be a differential that hangs. Every argument is either a boundary value or
small, and the file says which of its definitions are total and which are bounded by their input.
Property-based generation over the scalar subset, with a terminating-by-construction generator, is the
obvious next gate and is not written.

**The span a trap carries is not compared.** Both backends carry one and both point into the same file, but
the evaluator's is the `Core` node it was walking and the native one is what the emitter recorded for the
trapping instruction. The *message* is compared word for word; the span is checked to exist.

**A raise cannot cross into the evaluator.** `ExecError` carries a message and a span and not a raised
value, so a failure leaving a compiled call cannot be caught by a `try:` in an interpreted caller. It does
not arise today — a compiled definition only calls compiled definitions — and it is what would have to
change first if execution ever mixed the two mid-expression.

**A lookup into a `Map[Str, Html]` is refused**, because the demand for a map's comparison covers its
values as well as its keys even though the search only compares keys. No program in the tree holds a page in
a map; the fix if one does is to split the demand rather than to weaken the rule.

**Nothing here has run on a machine that is not x86-64 Linux with clang 18.** The IR names no target triple,
so it should be host-neutral, and "should be" is the phrase this project treats as a bug report.

**Nothing here measures code size.** One function per instantiation is the trade monomorphisation always is,
and twenty-eight instantiations from sixty-five templates is small enough that the question has not arisen.
Nor does anything measure how many arms an application can afford: every family in this tree has one or two,
so the difference between a chain of comparisons and a jump table is not claimed either way.

### What this establishes, and what it does not

That a Beck *definition* compiles — a record, text, a collection that is read and grown, a closure, a page,
a failure, a generic definition and the four primitives that ask the host — to two independent code
generators, agreeing with the tree-walker on every call in the suite, over layouts checked against the two
orders a plausible-looking wrong layout gets backwards. And that the way to get a value across a process
boundary without generating code for it is to stop it containing pointers.

What it does not establish is the sentence [`05`](05-tier-lowering.md) §5.2 wants. A Beck *service* is a
fold, a `validate` and a view, and those are signal nodes rather than definitions a body calls — so
`beck run` and `beck up` are unchanged, and the number this chapter would most like to print, what a Beck
service costs when it is machine code, is behind the signal vocabulary and not behind the heap.
