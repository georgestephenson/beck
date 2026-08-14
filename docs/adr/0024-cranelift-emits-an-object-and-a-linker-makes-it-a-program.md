# ADR 0024 — Cranelift emits an object and a linker makes it a program

**Status:** accepted
**Date:** 2026-08-09
**Context:** [`93`](../93-the-native-backends-report.md), [`07`](../07-dependencies.md) §7.3,
[`05`](../05-tier-lowering.md) §5.2, [`43`](../43-threat-model.md),
[`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md)

## The decision

`beck-clif` takes **Cranelift as a crate**, emits a relocatable **object file** through
`cranelift-object`, and asks the host for a **linker** to turn it into a program. The compiled
program then runs as a child process over the same pipe protocol
[`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md) established.

Three things it is not:

- **Not a JIT.** `cranelift-jit` exists, is the obvious way to use Cranelift, and would be faster
  than writing a file and linking it. It also finishes by turning a pointer into a function.
- **Not in-process execution by any other spelling** — `libloading`, `mmap`, an execution engine.
- **Not a replacement for `beck-llvm`.** §7.3 chooses two code generators on purpose: LLVM for
  release code, Cranelift for a fast build. This is the second, and `--backend` is the switch.

## Why a crate here and a run-time dependency there

[`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md) declined to take LLVM as a
dependency because the only Rust bindings to it are `unsafe` from the first call, and
[`43`](../43-threat-model.md) §43.2's `forbid(unsafe_code)` is the strongest claim in the threat
model precisely because it is structural. That argument does not apply to Cranelift and it is worth
saying why rather than leaving the asymmetry to be noticed:

**Cranelift is Rust.** `cranelift-codegen` builds a `Function`, `cranelift-frontend` builds SSA out
of blocks and jumps, `cranelift-object` writes an ELF or Mach-O object — and every one of those
APIs is safe. `beck-clif` inherits the workspace's `forbid(unsafe_code)` and needs no exception, no
wrapper and no argument.

The `unsafe` that ADR 0021 refused was never in *emitting* code. It was in **running** it, which is
why this crate still does not: the object is linked into an executable, the executable is a child
process, and the compiler never jumps into anything it generated. What changed between the two ADRs
is where the code generator lives, not what the process boundary is for.

## What it costs

- **A dependency, and 24 crates with it** — what `Cargo.lock` gained, the rest of the transitive
  set being crates this workspace already had. `cranelift-codegen`, `-frontend`, `-module`,
  `-object` and `target-lexicon` are Apache-2.0 WITH LLVM-exception; the 24 are that or
  MIT/Apache-2.0, and every one is already on `deny.toml`'s allow-list — which is the list being a
  *policy* rather than an inventory, as its own comment says. §7.10 puts
  Cranelift in tier 1 — "semantics or security ride on it" — so an upgrade is a dedicated pull
  request with the differential green.
- **A linker.** An object file is not a program, so `cc`, `clang` or `gcc` has to be on the path
  (`BECK_LINKER` names one). That is a *weaker* requirement than `beck-llvm`'s, not a stronger one:
  every machine with `clang` has a linker, and a machine with only `gcc` can use this backend and
  not the other.
- **The link, on the clock.** Cranelift's own codegen is the fast half; `cc` is a process, and
  starting one costs what starting one costs. [`93`](../93-the-native-backends-report.md) §93.5 measures
  program-to-executable on both paths rather than codegen alone, because the second number is what
  a developer waits for.

## What it buys

- **A second emitter, and therefore a three-way differential.**
  [`93`](../93-the-native-backends-report.md) §93.15's finding is that a differential compares what somebody
  thought to write down. Two independent emitters held to one subset make a class of mistake
  visible that one emitter cannot: `cranelift.rs` asserts they accept and refuse exactly the same
  definitions, and the first bug this backend had — a signed comparison of an unsigned order key —
  was found in the first minute by the smallest program in the suite.
- **A native backend on a machine with no LLVM.** Which is most containers.
- **`return_call`.** Cranelift verifies that a tail call discards its frame and refuses the function
  otherwise, which is the same guarantee `musttail` gives and what
  [`27`](../27-the-walls-come-down-report.md)'s language property needs from any backend.

## What was considered and refused

**`cranelift-jit`.** The reason is ADR 0021's, unchanged: execution in process is `unsafe`, and the
threat model's structural claim is not for sale for a build-time saving. It is also worth naming
what the refusal costs, since a JIT is the shape `beck dev` eventually wants: about 36 µs of pipe
round trip per call ([`93`](../93-the-native-backends-report.md) §93.5), and the link step.

**Sharing `beck-llvm`'s subset analysis.** It would have made the two emitters agree by
construction — and an agreement by construction is not evidence. The selection is written twice and
the gate holds the two to each other; §93.8 is what that found.

**Emitting Cranelift IR as text and shelling out.** Symmetric with ADR 0021 and pointless here:
there is no `cranelift` binary on anybody's machine to hand it to, and the crate is the interface.
