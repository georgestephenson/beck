# ADR 0022 — Mode B ships the backend it has

**Status:** accepted
**Date:** 2026-08-09
**Context:** [`05`](../05-tier-lowering.md) §5.1, [`94`](../94-mode-b-report.md),
[`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md),
[`0007`](0007-evaluator-stack-is-declared-not-discovered.md)

## The decision

Mode B's client kernel is `beck-eval` — the tree-walking backend the server runs — compiled to
`wasm32-unknown-unknown`, executing the component's `Core` from a bundle. It is **not** a
WebAssembly code generator for `Core`, which is what [`05`](../05-tier-lowering.md) §5.1's "the
component's pure code compiled to WASM (GC proposal where available; Perceus-style refcounting
fallback)" describes.

The bundle format, the data-patch protocol, the reconciliation rule and the browser shim are all
written against [`beck_core::backend::Backend`](../../compiler/crates/beck-core/src/backend.rs) and
know nothing about which implementation is behind it.

## Why

A WebAssembly code generator for `Core` is a **backend**, and the backend seam now has two
implementations behind it — the evaluator and `beck-llvm`
([`93`](../93-llvm-backend-report.md)). The second one is the argument for this decision rather
than against it, because of *what it compiles*: a definition whose parameters and result are `Int`,
`Float` or `Bool`, whose body is arithmetic, comparison, `if`, `match` and direct calls. **No heap.**

A component's `view` is nothing but heap. Records, lists, strings, a map, and an `Html` tree at the
end of it. So the tree's compiling backend could not execute a view today either, and the work that
would let it — a value representation, an allocator, string and collection primitives, closures
through an indirect call table, and a collector or a refcounting discipline — is the work that is
missing on *both* targets. Doing it for WebAssembly first, inside a change about rendering, would
be doing the hardest half of Phase 4 as a side effect of a client feature.

What Mode B is about is four questions that have never been answered anywhere:

1. When may a component render in the browser at all, and what does the compiler have to refuse?
2. What crosses the wire when it does?
3. How does a guess become a fact?
4. Is the page the browser renders the page the server would have sent?

None of those depend on how the view is executed, and all of them are cheaper to get wrong. Doing
them against a backend that already exists gets them checked now, with a differential gate that
asserts equality rather than similarity — because both sides run the same `Core`.

## The cost, named

- **Size.** The kernel is 724,031 bytes of WebAssembly, 179,195 brotli — measured, `--release`,
  without `wasm-opt`. §5.1 budgets "< 150 KB brotli for a typical Mode-B component bundle", so the
  kernel is **17% over a budget written for compiled output**. What the budget was protecting is
  better described by the marginal cost: a *component* is ~5 KB, the kernel is a fixed download
  every component of every Beck application shares, and it is cacheable across deployments because
  it does not depend on the program.
- **Speed.** A tree-walker in the browser is the same tree-walker, with whatever WebAssembly costs
  on top. [`94`](../94-mode-b-report.md) §94.6 says what has and has not been measured; the honest
  summary is that a local render is bounded by the interpreter and not by the network, which is
  still the point of the mode.
- **Types are erased from the bundle.** A compiling client backend needs them, so it needs bundle
  format 2. That is a version bump the format was built to take
  ([`beck_core::bundle`](../../compiler/crates/beck-core/src/bundle.rs)) rather than a redesign.
- **`forbid(unsafe)` gains its one exception.** `#[no_mangle]` is how a WebAssembly module exports
  anything and rustc classifies it as unsafe code, so `beck-wasm` denies rather than forbids, with
  four `#[allow]`s on export attributes and no `unsafe` block anywhere in the crate. The extent is
  gated by `mode_b.rs` rather than promised here.

  [`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md) took the opposite route on the
  same lint — textual IR and a child process, so that a native backend needs no exception at all —
  and the comparison is worth stating. That option is not available here: there is no out-of-process
  anything in a browser tab, and a module that exports nothing cannot be called. Where the lint
  could shape the design it did; where it could not, the exception is four attributes wide and has a
  test around it.

## What would reverse it

A `Core → WASM` backend, which is now a smaller step than it was: `beck-llvm` has already written
down what a compiling backend for this language has to match — checked integer arithmetic, the
structural order on reals, `Value::float`'s normalisation, saturating `trunc` — and any second one
inherits that list rather than rediscovering it. What it does not inherit is the heap, which is
still the whole of the remaining work.

When one exists it arrives as a different argument to `Client::load`, and the bundle grows a type
table under a new format version. Nothing else in this decision's blast radius — the render-mode
rule, the eligibility refusal, the data patch, the reconciliation, the shim — is about the
interpreter, which is the property this ADR is buying.
