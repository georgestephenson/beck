# 0007 — The evaluator's recursion bound is a count, and its stack is declared on the backend seam

**Context.** A tree-walker spends host stack on Beck-level recursion that is not in tail position.
Until proper tail calls landed, a deep program aborted the process — no span, no message, nothing
catchable — and the only thing standing between a user's recursion and a `SIGSEGV` was whatever
stack the calling thread happened to have. `sicp.rs` worked round it with a 32 MiB thread and a
comment apologising for it.

Two bounds were available. A **stack-headroom** budget — compare the current frame's address
against the thread's base and stop when the remaining bytes run low — is self-calibrating across
build profiles and machines. A **fixed count** of nested evaluations is not, and needs somebody to
guarantee the stack it implies.

**Decision.** The bound is a fixed count (`beck_eval::DEFAULT_MAX_DEPTH`), and the host stack it
requires is declared by the backend through `Backend::stack_bytes` — a defaulted method on the seam,
zero for a backend that compiles to a loop, `beck_eval::STACK_BYTES` for the tree-walker. The CLI's
dispatch, the `run`/`up` tokio worker threads and `beck_rt::testing::run` supply it.

The deciding argument is determinism, not ergonomics. §3.7 requires a fold's result to be a function
of the log alone, and `beck replay --verify` has to agree with the run it is replaying about
everything — including where the evaluator gave up. A headroom budget would let the same program
over the same log succeed in a release build and refuse in a debug one, or on one machine and not
another. Fuel has been a count since Phase 1 for the same reason; this is its counterpart for space.

`stack_bytes` is on the seam rather than in `beck-eval` because the runtime is what spawns threads
and may not name a backend crate (`docs/19` §19.9). It has to ask.

**Consequences.** The ceiling is conservative in release builds, because the number is chosen from
the unoptimised per-level cost — which a harness measures rather than assumes, and which fails the
build if the ceiling ever outgrows the declaration. An embedder that drives the runtime on its own
threads has to call `thread_stack_size(backend.stack_bytes())`; nothing enforces that, and
`docs/27` §27.10 records it. Raising the ceiling means raising `STACK_BYTES` with it.

Full argument and measurements: [`docs/27-the-walls-come-down-report.md`](../27-the-walls-come-down-report.md) §27.2.
