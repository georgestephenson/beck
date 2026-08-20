# 103 — The WebAssembly emitter

**Built, for the scalar subset.** A third emitter — `Core` → WebAssembly, bytes written by hand,
held to the tree-walker by a differential of **12,852 calls run in a real WebAssembly engine**, with
a million-deep tail recursion proving `return_call` is a jump.

**What it does not establish, said first because it is the number that matters**: it compiles
**0 of the corpus's 220 definitions**. An application is records, lists and a page; the heap is not
laid out on this target, so [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) is **not
reversed** — Mode B still ships the interpreter, and this buys a running program nothing yet. What
it is, is the half of a Mode B code generator that has no heap in it, and the half where everything
was new.

## 103.1 What was new, and it was not the code generation

[`93`](93-the-native-backends-report.md) wrote two emitters and §93.8 is what writing the second
found. A third against the same subset ought to be a transcription — and the parts that are, are:
the monomorphiser, the trap codes, the refusal discipline, the layout module and the *fixtures* are
`beck-llvm`'s, taken as a dependency exactly as `beck-clif` takes them, because a third *emitter* is
evidence and a third *design* is drift. `support/scalar.rs`'s programs are now pointed at by four
backends, and the reason they were shared in the first place is the reason they scaled: a fourth
copy of "what the scalar subset is" would have been a fourth opinion.

What is not a transcription is everything about the **target**:

| | LLVM and Cranelift | WebAssembly |
|---|---|---|
| The artefact | a program | a module, meaningless without a host |
| Control flow | labels and branches | `block`/`loop`/`if`, and **no jumps** |
| A failure | a code in an error cell in the arena | a code in an **exported global** |
| Division by zero | guarded, because it is a signal | guarded, because it aborts the **instance** |
| A tail call | `tailcc` and `musttail` | `return_call`, a 2.0 proposal |
| Who runs it | a child process on a pipe | whoever loaded the module |

Four of those six rows are the same *decision* reached for a different reason, which is what a third
implementation is worth: the reasons are now separable from the answers.

## 103.2 A trap cannot be a trap

WebAssembly's `unreachable` and its trapping `i64.div_s` abort the whole instance, and a Beck
program that overflows has not aborted — it has failed the way its type says it can, and
`beck-eval` answers `"`+` overflowed"`. So every integer operator carries its own guard, and a
computation that cannot produce a value stores three facts in exported globals and returns a zero:
which failure, where, and — for the three `no match` codes — the value nothing matched.

The codes are [`beck_llvm::Trap`]'s and the message is `Trap::message`'s, so the differential
compares the *sentence* and not merely the fact of a failure. That is the property §93.1 called
"one wire, unforked between the two backends", holding at three.

The guards are arithmetic rather than flags, because WebAssembly has neither an overflow flag nor a
widening multiply: addition is the classic sign test, and multiplication is the division test with
its two undefined cases answered first — `a == 0`, and `a == -1`, where the check itself would trap.

## 103.3 Structured control flow, and a `match` that is a nest

There are no jumps, so a `match` is a nest of typed `if`s: the scrutinee into a local, one `if
(result T)` per arm, and the innermost `else` is the trap the checker proves unreachable. That falls
out of the format and is not interesting.

What *is* interesting is that WebAssembly validation is **stack-polymorphic after an unreachable
instruction**, which is what makes it work at all: an arm that ends in `return` — every trap does —
leaves nothing on the stack where the block type demands a value, and validates anyway. Without
that, a trap in the middle of an expression would need a zero of the right type pushed at every
site, and the emitter would have to know the block type at a point where it knows only the
function's.

## 103.4 A tail call is a jump, spelled `return_call`

[`93`](93-the-native-backends-report.md) §93.4 makes a tail call a **guarantee**, and on this target
the guarantee has a name: WebAssembly 2.0's tail-call instructions, in V8 since 2023 and therefore
in every browser Mode B has ever run in. Every call in tail position is one — self-recursive or
not, same arity or not, which is the case a C calling convention cannot express.

The gate is not a stack measurement. `sum_to(1_000_000, 0)` and `drain(1_000_000, 0)` are run in the
engine and asserted to answer, which they can only do if a million frames were a million jumps: an
emitter that stopped emitting the tail form fails this by *throwing*, not by being slow.

## 103.5 The engine is the host, so there is no host

The native backends have a worker, a pipe and a protocol, because [`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)
refused to turn a pointer into a function. None of that is available in a browser tab and none of it
is needed: a module has no meaning without a host, and the host that matters is the one the code was
compiled for.

So the differential drives the module through a **JavaScript engine** — `node`, or `BECK_JS` — which
is the same execution environment production uses. `BECK_REQUIRE_WASM_RUN=1` forbids the skip.
[`adr/0030`](adr/0030-the-webassembly-emitter-writes-its-own-bytes.md) records the alternative that
was declined: taking Wasmtime as a dependency would remove the skip and would test the module in an
engine that will never run it.

Two details of that harness are decisions:

- **Reals cross as bit patterns.** A differential that round-tripped a real through JSON's decimal
  would be comparing two printers, and the cases that matter here are a signed zero and a NaN.
- **The driver is written by the test**, not checked in, so the protocol has one definition.

## 103.6 What it compiles, which is the honest half

| | Compiled | Refused |
|---|---|---|
| [`corpus/`](../compiler/corpus/) — 32 applications | **0** | 195 |
| [`awfy/`](../compiler/awfy/) — Are We Fast Yet | 58 | 344 |

The corpus row is the finding, and it is [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md)'s
argument arriving as a measurement rather than a forecast: "a component's `view` is nothing but
heap… the work that would let it — a value representation, an allocator, string and collection
primitives, closures through an indirect call table, and a collector or a refcounting discipline —
is the work that is missing on *both* targets." It still is. **138 of the corpus's 195 refusals are
one shape**: a parameter that is a `Str`.

So this emitter is a foundation and not a feature, and the two rows say which. It is also why the
tally is printed by its own test rather than gated: a threshold on a number nobody is optimising
would be a threshold about nothing.

## 103.7 Two opcodes, and what they cost to get wrong

Both were found by *running* the module rather than by reading the specification, which is the same
order §93.3 complains about and the same fix — each now has a case that fails without it.

- **`i64.trunc_sat_f64_s` is `0xFC 6`, not `0xFC 2`.** The saturating-conversion table puts the four
  `i32` conversions first, so the obvious index truncates to the wrong width. The engine caught it
  as a validation error, which is the cheap version of this mistake.
- **`f64.lt` is not the language's `<`.** `Value::Float` stores a monotone transform of the bits, so
  the language's order puts NaN above every number and says the two zeros are one value; IEEE's
  comparison says neither. Every real comparison here normalises both operands and compares *order
  keys as unsigned integers* — and the expensive version of this mistake is that it passes almost
  every test. `a_signed_zero_and_a_nan_are_the_languages_and_not_the_engines` is five cases wide and
  it is the only thing that catches it: removing the normalisation reddens exactly that test and
  nothing else in the suite.

## 103.8 What is not built

- **The heap**, which is the whole of the remaining work and is what §103.6 measures. Records,
  strings, lists, maps, closures and `Html`, in a browser's linear memory, with §5.1's unanswered
  question about the GC proposal in front of it.
- **The host effects.** `now`, `uuid`, `secret_env` and `http_fetch` are upcalls on the native
  backends; here they would be imports the loader supplies, and nothing supplies them yet.
- **`sin` and `cos`**, refused for the link line rather than for effort. Nothing pins their
  digits, so no backend emits them: the other two **call** `beck-prim`, which computes one answer
  ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)), and a
  WebAssembly module reaches that library only as an import the bundle does not carry. Emitting
  the algorithm here instead would be a second implementation of the one thing whose whole value
  is that there is one. `sqrt` compiles, because IEEE-754 pins it to one correctly-rounded answer,
  and that difference is what makes this a rule rather than a mood.
- **A fuel or a depth ceiling.** Neither native backend has one either (§93.15), and the one thing
  that is better here is that an engine's stack exhaustion is a catchable exception rather than a
  dead process.
- **A place in Mode B.** Nothing loads this module: `beck-wasm`'s kernel still interprets, the
  bundle format is unchanged, and a component bundle carrying compiled code needs bundle format 2
  and a type table, which [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) already
  anticipated.
- **The WebAssembly spec-suite obligation** [`12`](12-standards-and-conformance.md) §12.3 pins to
  core 3.0. What exists is a differential against the *language's* semantics, which is a different
  claim from conformance to the format; the emitter's output is validated by a real engine on every
  run, which is the half of it that is cheap.
