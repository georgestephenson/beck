# ADR 0019 — a modern allocator for the evaluator

**Status:** accepted
**Date:** 2026-08-05
**Context:** [`75`](../75-what-the-profiler-said-report.md), [`07`](../07-dependencies.md),
[`0004`](0004-full-cargo-deny-gate.md)

## The decision

`mimalloc` is the `#[global_allocator]` of the `beck` binary. The dependency is on `beck-cli`
only — the crates below it name no allocator, and a host embedding `beck-core` or `beck-rt` keeps
whatever allocator it already has.

## Why

A tree-walking evaluator is an allocator benchmark wearing a language's clothes. A call allocates
its argument vector and its frame; a `let` allocates its scope; a record, a list and a string each
allocate on construction — and nearly all of them are freed a few microseconds later.

Profiling `awfy/json.beck` under callgrind said so precisely: **35% of every instruction the
process executed was inside glibc's `malloc` and `free`**, across about 1.06 million allocations.
That is not a number a language aiming to be fast can leave alone, and it is not a number any
amount of restructuring inside the evaluator would have removed — the allocations that remain are
the ones the work actually needs.

Measured, release, minimum of five, the two binaries interleaved: **3.5% to 15.7% faster** across
eight benchmarks, with `havlak` — the most allocation-heavy — at the top.

## Why not fewer allocations instead

Both, in that order, and the order was decided by measurement rather than preference.
[`74`](../74-the-cost-of-a-call-report.md) §74.6 and [`75`](../75-what-the-profiler-said-report.md)
§75.4 record two attempts to remove allocations from the call path — an argument stack, and a
small-vector for arguments — and both made the evaluator **slower**. The bookkeeping that avoids a
short-lived allocation costs more than a good allocator charges for it. That is the reason this ADR
is not an admission of defeat about the design: the design's allocation *count* was examined first,
and what is left is work.

## The cost, named

- **A C build step.** `libmimalloc-sys` compiles mimalloc from vendored C, so a C compiler is a
  build requirement for `beck-cli`. It is the second such requirement, after
  [`0017`](0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md)'s bundled SQLite, so it
  does not change what somebody has to install to build this repository.
- **Licences.** `mimalloc` MIT, `libmimalloc-sys` MIT — inside
  [`0004`](0004-full-cargo-deny-gate.md)'s allowlist, and the gate is what enforces that rather
  than this sentence.
- **A second allocator's failure modes.** mimalloc has its own fragmentation and page-reclaim
  behaviour, and a memory number measured here is now a number about mimalloc too. The peak
  resident figures in [`72`](../72-space-and-constants-report.md) were measured under glibc and are
  historical for that reason as well as the others.

## What would reverse it

A backend that compiles rather than walks. The allocations this buys back are the tree-walker's,
and a compiling backend that puts frames on the machine stack does not make them — at which point
the dependency is paying for the runtime's own allocations only, and the measurement should be
taken again rather than assumed.
