# beck

One language for the frontend, the backend, the database, the container, and the cluster.

> **Beck** — Cumbrian for a fast upland stream: becks merge into rivers, and "beck" also means a
> summons ("beck and call"), which is what a `Command` is. Formerly working-named *tier*; see
> [`docs/10-decisions.md`](docs/10-decisions.md) D10.

Born from SICP: what if a working website were just `(my-javascript (my-css (my-html)))`? In `beck`,
that expression is literal — the page is a pure function of state, the database is a durable fold
over an event stream, infrastructure is a function of the program, and a deploy is an event on the
same stream it deploys. The compiler partitions one program into browser patch-streams, native
services, incrementally-maintained views, reproducible OCI images, and a Kubernetes object graph.

The design and implementation plan live in **[`docs/`](docs/)**:

- [`docs/00-original-idea.md`](docs/00-original-idea.md) — the seed conversation the project grew
  from, sketch preserved verbatim
- [`docs/README.md`](docs/README.md) — plan overview and index

**Phase 0 is built.** No compiler yet: [`phase0/`](phase0/) is the *output* the compiler will
generate for the todo sketch, hand-written in Rust so the architecture could be measured before
anything is built on top of it — ingress and envelope stamping, a durable fold over Postgres and
redb, server-side `view` with structural diff, a ~2 KB patch-interpreter client, `(subscription,
seq)` resumption across a killed process, and a Kubernetes object graph derived from the program's
effects. The measurements, the kill/pivot gates they answer, and the list of what turned out harder
than expected are in [`docs/18-phase-0-report.md`](docs/18-phase-0-report.md).

```console
$ cd phase0 && cargo run -p beck-p0-server -- run --store memory
```
