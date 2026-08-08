# beck

One language for the frontend, the backend, the database, the container, and the cluster.

> **Beck** — Cumbrian for a fast upland stream: becks merge into rivers, and "beck" also means a
> summons ("beck and call"), which is what a `Command` is.

Born from SICP: what if a working website were just `(my-javascript (my-css (my-html)))`? In beck
that expression is literal. **The page is a pure function of state, state is a durable fold over an
event stream, infrastructure is a function of the program, and a deploy is an event on the same
stream it deploys.** One program compiles into browser patch-streams, native services,
incrementally-maintained views, reproducible OCI images and a Kubernetes object graph — and
hand-written JavaScript never appears, because it is compiler residue.

```python
model Todo:
    id: Id
    text: Str
    done: Bool
    owner: Str

def apply_event(s: State, env: Envelope[Event]) -> State:      # the database
    match env.body:
        case Added(id, text):
            return s.with(todos=map_insert(s.todos, id,
                Todo(id=id, text=text, done=False, owner=env.actor)))

def view(s: State, session: Session) -> Html:                  # the page
    return ui:
        ul:
            for t in mine(s, session):
                li(key=t.id): t.text

todos: Signal[State] = durable(fold(apply_event, State(todos={}), events))
page:  Signal[Html]  = per_session(todos, view)
```

No routes, no JSON, no SQL, no migrations, no fetch calls, no Dockerfile — and no `@on(client)`
either: **placement is inferred**, and the compiler can show its working.

## Getting started

**[The guide](docs/86-getting-started.md)** takes you from an empty directory to a Kubernetes object
graph. Every program in it is compiled and run by a test. In short:

```console
$ cd compiler
$ cargo build --release          # the pinned toolchain downloads on the first build
$ ./target/release/beck --help
```

Then, on any `.beck` file:

```console
$ beck check   examples/todo.beck            # typecheck, infer effects, place, slice
$ beck test    examples/todo.beck            # the program's own tests, written in Beck
$ beck run     examples/todo.beck            # rung 0: no cluster, no container, no registry
$ beck build   examples/todo.beck            # the object graph its effects imply, + image configs
```

And to ask the compiler what it decided:

```console
$ beck explain place       examples/todo.beck todos   # candidates, costs, and why
$ beck explain flow        examples/todo.beck Id      # where a type reaches, and what is refused
$ beck explain incremental examples/todo.beck         # which views update by delta, and which recount
$ beck explain sql         examples/todo.beck         # the read models, as an outside client sees them
$ beck explain deploy      examples/todo.beck         # the infrastructure the effects imply
$ beck explain error       B0341                      # what a diagnostic code means
$ beck graph               examples/todo.beck         # every part, and what depends on what
$ beck impact              examples/todo.beck validate # what breaks if this changes — across the stack
```

`beck lsp` is the same front end in an editor; `beck fmt`, `beck iface`, `beck doc`, `beck replay`
and `beck up` round it out. The full command tree is in
[the reference](docs/reference/cli.md), which is generated from the compiler's own argument parser.

## Where things are

| | |
|---|---|
| [`docs/`](docs/README.md) | The design, the plan and the build reports — indexed, and the place to start reading |
| [`docs/86-getting-started.md`](docs/86-getting-started.md) | The guide |
| [`docs/reference/`](docs/reference/README.md) | The **generated** language reference: errors, commands, effects and tiers, the prelude, the forms |
| [`compiler/`](compiler/) | The compiler, the runtime and the standard library |
| [`compiler/examples/todo.beck`](compiler/examples/todo.beck) | The sketch the project grew from, as a working program |
| [`phase0/`](phase0/) | The *output* the compiler generates, hand-written in Rust once so the architecture could be measured before anything was built on it |
| [`SECURITY.md`](SECURITY.md) | How to report a vulnerability; [`docs/43`](docs/43-threat-model.md) is who is defended and who is not |

## Status

**Phases 0, 1 and 2 are built; Phase 3 is most of the way.** Effects are inferred and placement is
solved, so deleting every `@on(...)` from a program leaves it compiling, placing and running — 52%
of everything placed across a 31-program corpus has no tier at all. Views are maintained from the
change rather than recomputed, and the operators that do not read the session are held **once** for
the whole fanout. A program's own tests are written in it. The language has traits, bounds, tail
calls, reals, parameterised types and error rows; the standard library has strings, collections,
JSON, time, HTTP, crypto, bignums and decimal. There is an LSP, a SQLite substrate, structured
concurrency, and read models any Postgres client can query.

**What is not built is named rather than implied**: no native codegen (the tree-walker is the only
backend), no client-side WASM mode, no playground, no OIDC relying party, and no supply-chain
tooling. Phase 3's exit criterion — *an outside developer builds a non-trivial app from
documentation alone* — is **not met**, and [`docs/08`](docs/08-roadmap.md) tracks that literally.

Every report in [`docs/`](docs/README.md) ends with what it refuses to claim. That is the house
style: "built", "runs" and "measured" are three different statements.

## Licence

MIT. See [`LICENSE`](LICENSE).
