# ADR 0031 — Transcendentals are computed in the runtime library, and correctly rounded

**Status:** accepted
**Date:** 2026-08-16
**Context:** [`10`](../10-decisions.md) D3, [`14`](../14-review-findings.md) F9,
[`03`](../03-type-and-effect-system.md) §3.7, [`12`](../12-standards-and-conformance.md) §12.2,
[`0029`](0029-the-runtime-library-is-linked-and-owns-the-arena.md)

## The decision

`sin` and `cos` are **computed by `beck-prim`** (`beck_prim::math`), which every backend calls, and
the answer is the **correctly-rounded** one — the nearest double to the true value. No path in this
project reaches the platform's libm for either.

Two parts of that are the decision rather than the implementation:

- **The specification is the mathematics, not the code.** "Correctly rounded" names the answer
  without naming an implementation, so a future rewrite of `beck_prim::math` cannot change a
  single bit of a single log's replay. Vendoring a merely *accurate* implementation — the `libm`
  crate, a copy of fdlibm, musl's — would have been deterministic too, and would have made every
  later improvement to it a wire-format change.
- **A second entry point, `beck_prim_f64`, outside the arena protocol.** A function from a double
  to a double allocates nothing, so it takes the value and answers the value rather than a mark
  and an outcome record.

`sqrt` is deliberately *not* included and never will be: IEEE 754 requires it correctly rounded, so
every target's own instruction already computes the one answer.

## Why

[`10`](../10-decisions.md) D3 rests the whole data tier on a log folding to one state on any
machine, and [`03`](../03-type-and-effect-system.md) §3.7's purity is what is supposed to deliver
it. Purity is not enough. IEEE 754 requires `+`, `-`, `*`, `/` and `sqrt` to be correctly rounded
and requires **nothing at all** of `sin` and `cos`. Implementations differ in the last unit in the
last place — between libms, and between versions of one libm — so a fold that computed a sine
replayed to a different state on a machine with a different C library. That was
`DEFECTS.md::libm-determinism`, and the three-way differential could not see it, because all three
backends resolved to the same host's libm: they agreed because they were one implementation, not
because they computed one function.

**Why not pin a libm instead.** Pinning would mean shipping one, which is either a C dependency in
the link line of every compiled program or a Rust reimplementation in the workspace — and either
way the answer becomes "whatever that artefact does at the version we pinned", which is a promise
about a file rather than about a number. The costs land in the same places (a link line, an SBOM
row, a version to hold still) and the property bought is strictly weaker.

**Why correct rounding is affordable.** The hard part of a correctly-rounded sine is not accuracy
but *argument reduction*, and Payne and Hanek's reduction is exact integer arithmetic over a fixed
table of the bits of 2/π. Once the reduction is exact, the series only has to be carried far
enough, and 256 bits of fixed point is far more than the ~120 the hardest binary64 argument needs.
The whole of `beck_prim::math` performs **no rounded floating-point operation**: it takes the
argument apart into the `m` and `e` of `m·2^e`, works in integers, and rounds once at the end. So
determinism is not an argument about what a target contracts or fuses — there is no floating-point
arithmetic to contract.

## What it costs

**Speed, and by a lot.** A sine is **~640 ns against the platform's 11 ns**, 59×
(`cargo test -p beck-prim --release --test transcendentals -- --nocapture`). What is missing is not
a faster exact path — that is what an exact path costs — but a **fast path in front of it**: Ziv's
technique, where a double-double first pass answers unless its own error bound leaves the rounding
in doubt, and the exact path arbitrates the rest. That is scheduled in
[`08`](../08-roadmap.md) §8.5.4 and it changes no answer, because the exact path stays the
definition of what the answer is.

What the cost *is not* is a shape: `a_sine_costs_the_same_at_every_size` holds the one property
that would matter more than the constant — the reduction is a fixed window into 2/π, so a sine of
`10^300` costs what a sine of 1 costs, where the obvious reduction is a walk down the exponent.

The measured impact today is small enough to be worth stating rather than hiding: `awfy/cd.beck` is
the only program in the tree that calls either function, at **400 calls per run against ~230 ms of
work** — 0.1%, an order of magnitude below that benchmark's own run-to-run variance.

**The WebAssembly emitter refuses them.** A module reaches `beck-prim` only as an import the bundle
does not carry, and emitting the algorithm there instead would be a second implementation of the
one thing whose whole value is that there is one
([`103`](../103-the-wasm-emitter-report.md) §103.8). The refusal now names the link line rather
than F9's open question, which is closed.

**Two more constants to be right about.** π/2 and the 1472 bits of 2/π are the only things in the
module a person could get wrong by typing, so neither was typed: they are recomputed from Machin's
formula in `beck_prim::math`'s own tests, checked against the published hexadecimal expansion of π
and against `std::f64::consts::FRAC_PI_2`, and checked against each other by multiplication.

## The gate

`beck-prim/tests/transcendentals.rs` computes every answer a second time at 1408 bits by a
deliberately different route — π from the Bailey–Borwein–Plouffe series rather than Machin's,
reduction by binary long division rather than by a window into 2/π, a term recurrence rather than
Horner over a table — and asserts the module's answer is the nearest double to it.

The half worth reading is `the_host_libm_would_fail_this`, which asserts that some of those answers
are ones the platform's own `sin` does **not** give: 11 of 8000 on this machine. Without it the
suite would pass for both implementations, which is [`82`](../82-the-edge-report.md) §82.10's
pattern — a gate that cannot fail. With it, a change that quietly went back to `f64::sin` turns the
suite red on any host.
