# tier

One language for the frontend, the backend, the database, the container, and the cluster.

`tier` is a design in progress for a statically typed, homoiconic language with a Python-like surface
syntax, in which **execution tier is part of the type system**. You write one program; the compiler
infers and checks where each part runs, then lowers each partition through a different backend —
WebAssembly for the browser, native code for services, relational plans for the database, and an
OCI image plus a Kubernetes object graph for deployment.

No code yet. The implementation plan lives in **[`docs/`](docs/)** — start with
[`docs/README.md`](docs/README.md).

> **Note:** the plan's premise was reconstructed rather than transcribed — the source conversation
> link is not machine-readable. See the warning at the top of [`docs/README.md`](docs/README.md)
> for what to check and correct.
