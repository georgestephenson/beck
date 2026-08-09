# ADR 0021 — the native backend writes IR and runs a process

**Status:** accepted
**Date:** 2026-08-09
**Context:** [`93`](../93-llvm-backend-report.md), [`05`](../05-tier-lowering.md),
[`07`](../07-dependencies.md), [`43`](../43-threat-model.md),
[`0007`](0007-evaluator-stack-is-declared-not-discovered.md)

## The decision

Two decisions, taken together because neither survives without the other.

1. **`beck-llvm` emits LLVM IR as text and hands it to the host's `clang`.** There is no
   `llvm-sys`, no `inkwell`, no `build.rs` that probes for a library, and no entry in
   `Cargo.toml` that changes when the host's LLVM does.
2. **The compiled program runs as a child process**, started once and spoken to over a pipe with a
   fixed-width binary protocol. The compiler never executes a byte of the code it generated.

## Why

[`43`](../43-threat-model.md) §43.2 claims "no memory-unsafety in first-party code" as a
**structural** property: `unsafe_code = "forbid"` at the workspace root, inherited by every member
crate, tested by the build rather than by a reviewer. It is the strongest claim in the threat
model, and it is the only one that is true by construction rather than by care.

Both of the obvious ways to build a native backend take it away.

- **Binding LLVM** means `llvm-sys`, whose every function is `extern "C"` and whose every call is
  `unsafe`. `inkwell` is a safe *wrapper*, which means the `unsafe` is in somebody else's crate
  rather than absent — a real improvement for correctness and no change at all to what §43.2 says
  about *first-party* code, because `beck-llvm` would still need `unsafe` at the execution engine.
- **Executing in process** means turning a pointer into a function, which is `unsafe` in any
  spelling — `libloading`, `mmap`, an execution engine, all of them.

So the question was what a native backend costs if the property is not negotiable. The answer
turned out to be surprisingly little, because LLVM's textual IR is not a workaround: it is a
documented, versioned, stable interface to the same compiler, and it is the interface LLVM's own
test suite is written against.

What that buys, beyond the property:

- **No new dependency.** `beck-llvm` depends on `beck-core` and `beck-diag` and nothing else.
  [`07`](../07-dependencies.md) §7.9 pins everything; the thing it cannot pin is a native library
  the host installed, and this crate does not have one. The SBOM
  ([`92`](../92-sbom-report.md)) is unchanged by the whole backend.
- **A machine without LLVM still builds the compiler and passes every other test.** The toolchain
  is discovered at run time, and its absence is a printed skip rather than a build failure. A
  linked `libLLVM` would make the whole workspace unbuildable on a machine that lacked it.
- **The artefact is readable.** `beck native --out <dir>` leaves a `.ll` a person can read, `opt`
  can transform and `llc` can inspect. A defect in code generation is a diff in a text file rather
  than a debugger session inside a builder API.
- **LLVM's version is the host's problem, and it is a small problem.** The emitted IR uses opaque
  pointers, four arithmetic-with-overflow intrinsics, `llvm.fptosi.sat`, and `tailcc` — all of them
  LLVM 15 and older. There is no bitcode, so there is no bitcode compatibility window.

## What it costs, named

- **A pipe round trip per call**, measured at **36–43 µs** across §93.5's two harnesses
  ([`93`](../93-llvm-backend-report.md)) — one constant, read twice on a noisy runner. It is a constant, so it is the whole cost of a call that computes nothing and a rounding
  error on one that computes for a millisecond — but it means this backend is for *compute*, and a
  program that crosses it a million times to do a nanosecond of work each time would be slower than
  the tree-walker. §93.7 says what would remove it.
- **One process, one pipe, one lock.** Two threads calling at once serialise on the pipe. The
  runtime calls the fold from a sequencer task and a view from a connection task, so this is a real
  constraint and not a theoretical one — it is the first thing a second version changes.
- **`clang` is a run-time requirement of `beck native`**, and a build requirement of nothing. It
  joins the C compiler that [`0017`](0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md)
  and [`0019`](0019-a-modern-allocator-for-the-evaluator.md) already need, so it changes what a
  person has to install by approximately nothing — but the *version* of it is now a variable in a
  performance number, which is why [`93`](../93-llvm-backend-report.md) quotes it.
- **`-O2` runs on every build.** Compiling nine definitions and linking took 195 ms
  ([`93`](../93-llvm-backend-report.md) §93.5). That is fine for `beck build` and it is exactly the
  cost §5.2 buys Cranelift to avoid for `beck dev`, so this ADR does not settle the dual-codegen
  question — it builds one half of it and measures the reason the other half exists.
- **No fuel, and no depth ceiling.** [`62`](../62-fuel-report.md)'s step budget and
  [`0007`](0007-evaluator-stack-is-declared-not-discovered.md)'s counted recursion depth are both
  properties of walking a tree, and neither survives compilation. What replaces them is coarser: a
  wall-clock limit on one call, and a message when the worker dies. §93.7 is the honest list.

## What was considered and refused

**Cranelift instead.** [`05`](../05-tier-lowering.md) §5.2 names it for `beck dev` and it would
have been the easier first backend — a Rust API, no external toolchain. It has the same problem:
executing what it produced needs `unsafe`, and `cranelift-jit`'s `get_finalized_function` returns a
raw pointer. Choosing LLVM first was therefore not a judgement about the two compilers; it was that
LLVM has a text format and Cranelift's CLIF, while textual, has no shipped ahead-of-time driver in
the way `clang` is one.

**A generated C file instead of IR.** Shorter to emit and portable to a machine with no LLVM at
all. Refused because C's semantics are *not* Beck's at exactly the points that matter: signed
overflow is undefined behaviour where Beck's is an error, and reproducing `i64::checked_mul`
faithfully in C means the same explicit guards with a compiler that is free to assume they cannot
fire. LLVM's `llvm.smul.with.overflow.i64` is the operation, exactly.

**Linking the compiled code into the compiler as a shared object.** Faster than a pipe by four
orders of magnitude, and it is `dlopen` plus a transmute. This is the trade the whole ADR is about.

## What would reverse it

A decision that `forbid(unsafe)` should be a claim about *most* of the workspace rather than all of
it — with one crate carved out, audited, and named in [`43`](../43-threat-model.md) §43.2 as an
exception. That is a real option and it is not obviously wrong: an in-process backend is faster,
simpler, and the `unsafe` involved is a handful of lines around a function pointer. It is a
decision about the project's central security claim, though, and it should be taken as one and not
as a side effect of wanting a faster benchmark number.

The trigger that would force it is a *tier* rather than a benchmark: if the server partition ever
runs compiled rather than walked — the fold, `validate`, the view, all of which cross the seam per
event — the round trip stops being amortisable and the process boundary stops being affordable.
Nothing here is on that path yet, and [`93`](../93-llvm-backend-report.md) §93.6 says why.
