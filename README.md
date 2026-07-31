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

**Phase 0 is built.** [`phase0/`](phase0/) is the *output* the compiler will generate for the todo
sketch, hand-written in Rust so the architecture could be measured before anything was built on top
of it — ingress and envelope stamping, a durable fold over Postgres and redb, server-side `view`
with structural diff, a ~2 KB patch-interpreter client, `(subscription, seq)` resumption across a
killed process, and a Kubernetes object graph derived from the program's effects. The measurements,
the kill/pivot gates they answer, and the list of what turned out harder than expected are in
[`docs/18-phase-0-report.md`](docs/18-phase-0-report.md).

**Phase 1 is built.** [`compiler/`](compiler/) is the compiler: two surfaces onto one homoiconic
AST, a hygienic macro expander, Hindley–Milner inference, a typed `Core` IR, placement verified
against effects, and a splitter that slices the signal graph into the roles a runtime drives —
executed by whichever backend the process chooses, behind a `Backend` seam the runtime cannot see
past. The todo sketch is now *source* —
[`compiler/examples/todo.beck`](compiler/examples/todo.beck), 132 lines — and the runtime that serves it is Phase 0's, with the application arriving as compiled
`Core` instead of hand-written Rust. `beck up` puts it in a real cluster, where a killed pod
recovers by folding the log. What it deliberately does not do — native codegen, effect inference —
is in [`docs/19-phase-1-report.md`](docs/19-phase-1-report.md), along with the four defects that
only running it found.

**Phase 2 is built** — the moat. Effects are now **inferred**, as row-polymorphic rows over a wide
atom set, so `map_list` is one function whatever its argument does and an effect two calls deep is
still an effect. Placement is **solved** rather than annotated: candidates come from the row, the
choice comes from a cost model, and the answer is deterministic, stable across edits (`beck.lock`),
assertable (`--assert-place`) and explainable (`beck explain place`). `secret[T]` is not Sendable
and the compiler says which field reaches it; a `.becki` publishes each module's types, effects and
placements so downstream compiles against a signature and a body edit costs nothing; and
`beck check --wire-compat` refuses a release that would break an open browser tab.

The headline: **delete every `@on(...)` from the todo sketch and it still compiles, places and
runs.** Across a 22-program [corpus](compiler/corpus/) carrying no annotations at all, 44% of
everything placed is unplaced-pure — code with no tier, compiled to whichever tier calls it.
[`docs/20-phase-2-report.md`](docs/20-phase-2-report.md) has the measurements, what is still not
done, and the eight things that turned out harder than expected — including the discovery that the
Phase 1 CI workflow had never run.

```console
$ cd compiler
$ cargo run -- check   examples/todo.beck      # typecheck, infer effects, place, slice
$ cargo run -- explain place examples/todo.beck todos   # candidates, costs, and why
$ cargo run -- explain flow  examples/todo.beck Id      # where a type reaches, and what is blocked
$ cargo run -- iface   examples/todo.beck      # the published signature: types, effects, placements
$ cargo run -- check   examples/todo.beck --wire-compat previous.becki
$ cargo run -- graph   examples/todo.beck      # every part, and what depends on what
$ cargo run -- impact  examples/todo.beck validate   # what breaks if this changes
$ cargo run -- run     examples/todo.beck      # rung 0: no cluster, no container, no registry
$ cargo run -- replay  examples/todo.beck --verify
$ cargo run -- build   examples/todo.beck      # the object graph the effects imply, + image configs
```

**The program is its own AppHost.** Aspire draws a resource graph because you write a second program
declaring the topology; Beck has no such program, because placement, the splitter and the
effect-derived object graph already *are* it. So `beck impact validate` answers across the whole
stack — three signals and seven Kubernetes objects, with hop counts — and `beck run` serves the same
model at `/_beck` as a dashboard, alongside live metrics and OTLP export. The graph for the todo
program is 35 nodes and 67 edges, built in 81 µs. See
[`docs/19-phase-1-report.md`](docs/19-phase-1-report.md) §19.8, which also draws the line between
what the event log records and what telemetry has to: the boundary is determinism.
