# 97 — Phase 3, part 65: the second code generator

**Built.** Cranelift — [`07`](07-dependencies.md) §7.3's *development* code generator and the half
of [`05`](05-tier-lowering.md) §5.2's dual codegen that [`93`](93-llvm-backend-report.md) §93.6
listed as not existing. `beck native --backend cranelift` compiles the same scalar subset the LLVM
backend compiles, to the same semantics, and §4.8's differential *between backends* is now a
**three-way** one: the tree-walker, LLVM and Cranelift on every call.

The interesting part is not that there are two compilers. It is what having two **independent
emitters held to one subset** turns out to be worth, which §97.4 is about — and it was worth
something within a minute of the first one running.

## 97.1 The shape: a crate, an object, a linker, a process

[`adr/0024`](adr/0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md) is the decision
and the asymmetry it has to answer for. [`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)
declined to take LLVM as a *dependency* because its Rust bindings are `unsafe` from the first call;
this takes Cranelift as one, and the difference is that **Cranelift is Rust**. `beck-clif` inherits
the workspace's `forbid(unsafe_code)` and needs no exception.

What ADR 0021 refused was never emitting code — it was **running** it, and that is unchanged:

```text
Core ──▶ cranelift-frontend ──▶ cranelift-object ──▶ a .o ──▶ cc ──▶ an executable ──▶ a pipe
```

No JIT, for that reason: `cranelift-jit` is the obvious way to use this crate and it finishes by
turning a pointer into a function. The worker protocol is [`beck_llvm::Worker`]'s, unchanged and
unforked — eight bytes of header, eight per argument, a 24-byte reply — because the host is the same
host and two spellings of one wire is the drift this project spends its gates on.

The requirement it puts on a machine is *weaker* than the other backend's, not stronger: `beck
native --backend llvm` needs `clang`, and this needs any linker. A container with `gcc` and no LLVM
can use this one.

## 97.2 What is Cranelift's rather than LLVM's

Four things the second emitter does differently, and none of them is a semantic difference:

* **A tail call is `return_call`** under the `tail` calling convention. Cranelift *verifies* that
  the frame can be discarded and refuses the function otherwise — the same guarantee `musttail`
  gives, which is what [`27`](27-the-walls-come-down-report.md)'s language property needs from a backend
  rather than hopes for. Sixty million tail calls run in constant stack, and so does a tail call
  into a definition of a *different arity*, which is the case a C-convention `musttail` cannot
  express.
* **A block parameter replaces a `phi`.** The joins are the same joins; `cranelift-frontend`
  constructs the SSA rather than the emitter writing it out. An `if` whose two arms both `return`
  reaches a join nothing jumps to, and that block still needs a terminator — which is a shape the
  textual emitter never had to think about.
* **A `Bool` is an `I8` holding 0 or 1**, because Cranelift has no one-bit integer. Every
  comparison produces one, so `and` and `or` are `band`/`bor` — and `not` is `bxor 1` rather than a
  complement, which would answer 254.
* **There are no intrinsics.** `sqrt` and `fabs` are instructions; `sin` and `cos` are calls into
  the C library, which is exactly what `clang` lowers `llvm.sin.f64` to, so both backends reach the
  same `libm` the evaluator's `f64::sin` reaches.

And one thing that is not a difference but an *assert*: Cranelift's x86-64 `return_call` requires
`preserve_frame_pointers`, because its implementation of a tail call restores the caller's frame
through the frame pointer. A backend that guarantees §31's tail calls therefore keeps frame
pointers, and the setting is in the ISA builder with the reason beside it rather than in a list of
flags somebody copied.

## 97.3 The same subset, written twice on purpose

The selection — which definitions compile, and the reason each refusal gives — is a **second
implementation** rather than an import. That is deliberate and it is the whole argument for this
report:

> `cranelift.rs` asserts the two emitters accept and refuse exactly the same definitions, over
> every program in the suite and every program in the corpus. A shared implementation would make
> that agreement true by construction and therefore worth nothing.

What *is* shared is the vocabulary and the wire: `Scalar`, `Signature`, `Refusal` and `Trap` are
`beck-llvm`'s types, and `Trap`'s codes are a protocol the host decodes. A second copy of those
would be two spellings of one contract, which is the opposite kind of duplication.

The fixed point is the same shape in both: emit every body, drop whichever will not emit, repeat —
so a definition that calls a refused one is refused in turn, and mutual recursion survives or is
refused as a pair. Here it re-emits the whole module each round, because "compiles" and "emits" are
one question: an analysis that *predicted* emissibility would be a second implementation of the
emitter, and the two would drift.

## 97.4 What two emitters found

**The bug.** The first program the new backend ran was five definitions long, and one of them was
`def negative(x: Float) -> Bool: return x < 0.0`. It answered `false`.

`beck_core::Value` stores a real as an **order key** — a monotone transform of the bits, so the
derived `Ord` is the numeric order with `-0.0 < 0.0` and NaN at the top ([`27`](27-the-walls-come-down-report.md)
§27.8). Both backends compute the key the same way. What the new one got wrong was the comparison
*of* the key: the transform maps every real onto the **unsigned** order, and this compared signed.
`key(-1.0)` is `0x400F…` and `key(0.0)` is `0x8000…`, so signed says `-1.0` is the larger.

It is worth being precise about what caught it, because it was not the differential's clever half.
The 676-pair float sweep would have caught it too, and had not been written yet; what caught it was
five lines and one call. That is not an argument against the sweep — it is
[`85`](85-what-the-generator-found-report.md) §85.1's lesson in the ordinary direction: the cheap
test that runs first is the one that finds the bug that is *everywhere*, and the expensive one
earns its keep on the bug that is in one corner. Both are in the suite, and the sweeps have found
nothing since.

**The parity gate.** Holding the two emitters to one subset also gates the *order*: a compiled
definition gets the same dispatch index in both, so the table is a property of the program's
declaration order rather than of a hash map's iteration in either. Nothing reads one backend's
table with the other's indices, and asserting it is the cheapest available check that neither is
sorting by something incidental.

## 97.5 The numbers

§7.3's reason for a second code generator is a *build* time: "~40% faster whole-compile and ~10×
faster codegen step than LLVM". Measured here, program to executable, in a release build:

| definitions | cranelift | llvm + `clang -O2` | × |
|---|---|---|---|
| 50 | **48.8 ms** | 259.1 ms | 5.3 |
| 400 | **141.5 ms** | 1.5 s | 10.5 |

`cargo test --release --test measure_native -- --nocapture`, on this machine, both numbers
including starting the worker process.

Two sizes rather than one, per `AGENTS.md`, and they say more than the ratio does. Eight times the
definitions costs Cranelift **2.9×** and LLVM **5.8×**: most of the Cranelift column is the fixed
cost of running `cc`, and most of the LLVM column is not. That is why the ratio *grows* — 5.3× at
50 and 10.5× at 400 — and it is what "~10× faster codegen step" looks like once the link is
included in both.

Two things about how it is measured. First, **program to executable** rather than codegen alone,
because that is what a developer waits for: Cranelift's own codegen is the fast half, and `cc` is a
process. Second, it is in a **release-only** file, and the reason is a mistake this report nearly
made: `cargo test` builds this workspace in *debug*, so a comparison there is our unoptimised build
of Cranelift against a distribution's optimised `clang` — which runs the other way, by about a
factor of two. `cranelift.rs` says so and asserts nothing about time;
`measure_native.rs` is where the number lives, per `AGENTS.md`.

Neither number is asserted. [`13`](13-testing.md) §13.7: a timing gate on a shared runner cannot be
held honestly.

## 97.6 The differential

**13,152 calls**, over the fixtures `native.rs` already had — 6,136 on integer arithmetic, 5,960 on
reals, 650 on control flow, 406 on recursion — plus 60,000,000 tail calls and a million into a
definition of a different arity. The programs are shared with the LLVM suite rather than copied:
`support::scalar` is one set of programs and arguments, and a second copy of them would be a second
opinion about what the subset is.

The subset itself is asserted over **37 programs** — the five fixtures and all 32 corpus programs —
and the two emitters compile the same **44 definitions** between them and refuse the same rest.

One of those numbers deserves to be read twice: across all 32 corpus programs, the two emitters
compile **nothing at all**. Every corpus program is records, maps and strings, and neither backend
has a heap; what the corpus exercises here is that the *emitter* survives every shape in it, not
that it compiles one. [`93`](93-llvm-backend-report.md)'s opening said the same thing about the
sketch, and it is still the number to read first.

Every one of those is compared three ways — the evaluator, LLVM and Cranelift — and the *whole
outcome* is compared, not just the successes: an integer overflow is a value in this language, so a
backend that wrapped, or failed for a different reason, or failed where another succeeded, is a
divergence. Where a machine has a linker and no `clang`, the LLVM leg drops out and the suite says
so rather than skipping.

The three real-normalisation programs [`93`](93-llvm-backend-report.md) §93.2 arrived at the hard
way — `product_order`, `product_is_zero`, `reciprocal_of_product` — are in the shared fixtures, so
the second emitter had to make the same three decisions independently and is held to them by the
same tests. `(0.0 * inf) > 0.0` is the one that matters: on x86-64 that product is the *indefinite*
QNaN with its sign bit set, which sorts below every number under the order key where `f64::NAN`
sorts above every one.

## 97.7 What is not built, and why Mode B codegen is not a remainder of this

**The heap, which is what bounds both code generators.** No record, list, string, map, union or
closure compiles, and no effect does: there is no allocator and no collector behind either emitter.
That is unchanged from [`93`](93-llvm-backend-report.md) §93.6 and it is the single largest thing
between this and §5.2's "compiles to native binaries, one per `service`".

**Mode B codegen, which is the same missing heap.** [`08`](08-roadmap.md)'s Mode B bullet lists
"no codegen" as its remainder and [`94`](94-mode-b-report.md) §94.8 already says what it is waiting
for:

> The seam now has two implementations behind it and **neither of them can render a page**:
> `beck-llvm` refuses anything needing a heap, which a view is made of. The heap is the shared
> prerequisite, and it is Phase 4's rather than this bullet's.

That sentence is now true of three implementations. Cranelift does not change it and could not: it
compiles Cranelift IR to *machine code*, which is the opposite direction from the WebAssembly a
browser needs — Wasmtime uses Cranelift to compile wasm, not to produce it. A Mode B code generator
is a **third** emitter against a **wasm** target, and everything it would have to emit is heap:
`Html` is a tree, a view builds one per render, and `str` is a heap value in the first line of
almost every page.

There is also a measurement that says which of those two to build first, and it is not this one.
[`94`](94-mode-b-report.md) §94.14 measured a Mode B interaction and found **97% of it in `view`**,
growing with the collection — and that the server's incremental engine is *slower* on the same
interaction than the browser's interpreter. A code generator divides the constant and leaves the
growth. So "Mode B codegen" is not a small remainder of this work; it is the heap, then a wasm
emitter, then a measurement that says what it bought — and the honest thing is to say that rather
than to ship a scalar-subset wasm backend that cannot render anything.

**Also not built here**: `beck dev` itself, which is what §7.3 buys a fast code generator *for* —
there is no watch loop, no hot reload and no incremental recompile, so the number in §97.5 is a
build time nothing is yet waiting on. No `--opt-level` switch. And no in-process execution, which
is refused rather than missing ([`adr/0024`](adr/0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md)).

## 97.8 What this corrects, elsewhere

| Where | What |
|---|---|
| [`42`](42-security-assurance.md) §42.1, [`43`](43-threat-model.md) §43.2 | "all ten crates" is **twelve**. The claim is unchanged and `beck-clif` inherits `unsafe_code = "forbid"` like every other member — and this is the first crate where the lint cost *nothing*, because Cranelift's API is safe. §42.1's note that "the tenth crate is `beck-llvm`, and the lint is why it exists in the shape it does" is worth reading beside [`adr/0024`](adr/0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md): the same lint produced a text-and-subprocess design there and an ordinary dependency here, because the `unsafe` was in *running* code rather than in generating it |
| [`05`](05-tier-lowering.md) §5.2 | The Cranelift row is no longer a plan, and "the two must agree observably — enforced by differential tests" is a test rather than a sentence |
| [`07`](07-dependencies.md) §7.3 | Cranelift is taken as a **crate** and is not a JIT, which that row's "Dev codegen / JIT" heading implies. The row now says which |
| [`93`](93-llvm-backend-report.md) §93.6 | "**Cranelift**, and therefore §5.2's *dual* codegen — **not built.** One half exists" — both halves exist now. Every other row of that table is unchanged, including the one that matters most: there is still no heap |

## 97.9 What this establishes

§5.2's "the two must agree observably — enforced by differential tests" is a sentence that could
not be true while there was one backend. It is true now, over the scalar subset, and the shape of
the evidence is worth naming: three implementations, two of them compilers written against
different IRs, agreeing on every call in the suite; and one subset, decided twice, asserted equal.

What it does not establish is anything about a Beck *application* running natively. Both code
generators compile arithmetic. A fold, a view and every effect still run on the tree-walker, `beck
run` and `beck up` are unchanged, and the number this report would most like to print — what a
Beck service costs when it is machine code — is behind the heap, exactly where §93.6 left it.
