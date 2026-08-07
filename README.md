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

**Start here if you want to write one**: [`docs/86-getting-started.md`](docs/86-getting-started.md)
— build the compiler, write a function, turn it into an application, and see what the compiler
worked out on its own. Every program in it is compiled and run by a test.

The design and implementation plan live in **[`docs/`](docs/)**:

- [`docs/00-original-idea.md`](docs/00-original-idea.md) — the seed conversation the project grew
  from, sketch preserved verbatim
- [`docs/README.md`](docs/README.md) — plan overview and index
- [`docs/reference/`](docs/reference/README.md) — the **generated** language reference: the error
  index, the command reference, the effect and tier matrix, the prelude and the forms. Produced by
  `beck doc reference` from the compiler's own tables and gated on every pull request, so a page
  cannot disagree with the compiler

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
[`compiler/examples/todo.beck`](compiler/examples/todo.beck), 178 lines — and the runtime that serves it is Phase 0's, with the application arriving as compiled
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
runs.** Across a 29-program [corpus](compiler/corpus/) carrying no annotations at all, 52% of
everything placed is unplaced-pure — code with no tier, compiled to whichever tier calls it.
[`docs/20-phase-2-report.md`](docs/20-phase-2-report.md) has the measurements, what is still not
done, and the sixteen things that turned out harder than expected — including the discovery that the
Phase 1 CI workflow had never run.

**Phase 3 has started** — three of its fourteen bullets built, a fourth's engine, and a fifth
running. The phase's own exit criterion, "an outside developer builds a non-trivial app from
documentation alone", is **not met**, and
[`docs/08-roadmap.md`](docs/08-roadmap.md) tracks that literally rather than by summary.

**Tests written in Beck** ([`22`](docs/22-phase-3-report.md)). A test is a log, a command and an
expectation, so there is no fixture — state is a fold, and `given [ … ]` goes through the program's
real `apply_event`. There is no network either: `expect page(session("bo")) contains "milk"` renders
through the same view the server diffs, because the boundary is a placement of one graph rather than
a seam between two programs. And there are no mocks to write — a stub is a value for an *effect
atom*, every external call is stubbed by default with an inhabitant the compiler derives from its
return type, and `beck test --verbose` prints the complete list of what it stubbed. `expect
place(page) == client` is answered without running anything, and still passes with every `@on(...)`
deleted.

**The signal graph is sliced as a graph, and views are maintained rather than recomputed**
([`23`](docs/23-general-slicer-report.md), [`24`](docs/24-incremental-views-report.md),
[`26`](docs/26-arrangement-sharing-report.md)). Any number of `durable` folds, any depth and any
sharing above them; a view compiled to a dataflow of operators and updated from the change, with
full recompute kept as the CI oracle — every corpus program, every event, maintained page against
recomputed page, byte for byte. The operators that do not read the session are held **once for the
whole fanout**: 256 subscribers of a public feed do 55× less work per event. What is not built is
named as such — the page is still assembled and diffed rather than streamed as deltas, and the read
models, pgwire exposure and query fusion in that bullet are untouched.

**The language's own means of abstraction** — the bullet four phases had never pointed at
([`25`](docs/25-benchmarks-and-expressiveness.md) §25.6 measured six walls between Beck and the rest
of SICP). All six are down, and so are the three that removing them wrote. In order: recursive and
forward-referencing types ([`27`](docs/27-walls-report.md)), proper tail calls
([`31`](docs/31-tail-calls-report.md)), reals and user-written polymorphism
([`32`](docs/32-numeric-tower-and-polymorphism-report.md)), effect polymorphism and list patterns
([`33`](docs/33-effect-polymorphism-and-list-patterns-report.md)), parameterised types
([`36`](docs/36-parameterised-types-report.md)), and traits — declarations and impls
([`37`](docs/37-traits-report.md)), bounds ([`39`](docs/39-bounds-report.md)), the `.becki` boundary
([`40`](docs/40-traits-across-modules-report.md)) and the operators
([`41`](docs/41-generic-arithmetic-report.md)), so `one_third() + one_third()` prints `2/3`.
Between the four trait reports: no IR node, no evaluator case, no runtime change.

**SICP is the expressiveness benchmark** ([`compiler/sicp/`](compiler/sicp/), D18). Chapters 1 and 2
run as Beck libraries with the book's own printed answers as the oracle — 21 tests and 18 tests,
including four doubles asserted digit for digit and an iterative process a quarter of a million
levels deep. `sicp/refusals/`, which holds one file per wall still standing, is **empty**; its
README says why that is the narrow claim that every wall this project has *found* has been removed,
and not that Beck expresses SICP. Chapters 3, 4 and 5 are unattempted.

```console
$ cd compiler
$ cargo run -- check   examples/todo.beck      # typecheck, infer effects, place, slice
$ cargo run -- explain place examples/todo.beck todos   # candidates, costs, and why
$ cargo run -- explain flow  examples/todo.beck Id      # where a type reaches, and what is blocked
$ cargo run -- test    examples/todo.beck              # the program's own tests, written in Beck
$ cargo run -- iface   examples/todo.beck      # the published signature: types, effects, placements
$ cargo run -- doc module examples/documented.beck --format md --stdout   # the page, derived
$ cargo run -- doc reference --out ../docs/reference   # the language reference, from the compiler
$ cargo run -- explain error B0341             # what a diagnostic code means
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
