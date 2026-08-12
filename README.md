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
graph. Every program in it is compiled and run by a test. It is
[published on the site](https://georgestephenson.github.io/beck/guide/getting-started.html)
alongside the generated language reference and the compiler's API docs. In short:

```console
$ curl -fsSL https://raw.githubusercontent.com/georgestephenson/beck/main/install.sh | sh
$ beck --help
```

[`install.sh`](install.sh) downloads the tarball for your platform, checks it against the release's
`SHA256SUMS`, and puts `beck` in `~/.beck/bin`. **No release has been cut yet**
([`104`](docs/104-the-release-and-the-installer-report.md) §104.7), so until a tag is pushed, build
it from source — the same binary, and it needs a C compiler and CMake:

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
| [The site](https://georgestephenson.github.io/beck/) | The published documentation: the reference, the guide, two module pages and rustdoc — rebuilt from `main` on every push, and linked back here from every page |
| [`docs/`](docs/README.md) | The design, the plan and the build reports — indexed, and the place to start reading |
| [`docs/86-getting-started.md`](docs/86-getting-started.md) | The guide |
| [`docs/reference/`](docs/reference/README.md) | The **generated** language reference: errors, commands, effects and tiers, the prelude, the forms |
| [`compiler/`](compiler/) | The compiler, the runtime and the standard library |
| [`compiler/examples/todo.beck`](compiler/examples/todo.beck) | The sketch the project grew from, as a working program |
| [`phase0/`](phase0/) | The *output* the compiler generates, hand-written in Rust once so the architecture could be measured before anything was built on it |
| [`install.sh`](install.sh), [`release/`](release/README.md) | Installing a released `beck`, and building one — the executable half of the release pipeline, so it can be run before it is trusted |
| [`SECURITY.md`](SECURITY.md) | How to report a vulnerability; [`docs/43`](docs/43-threat-model.md) is who is defended and who is not |

## Status

**Phases 0, 1 and 2 are built; Phase 3 has nothing untouched left on its list.** Effects are
inferred and placement is solved, so deleting every `@on(...)` from a program leaves it compiling,
placing and running — 52% of everything placed across a 31-program corpus has no tier at all. Views
are maintained from the change rather than recomputed, and the operators that do not read the session
are held **once** for the whole fanout; a DBA can `psql` the read models they project. A program's
own tests are written in it. The language has traits, bounds, tail calls, reals, parameterised types,
error rows, nested patterns with guards, and `parallel:` scopes; the standard library has strings,
collections, JSON, time, HTTP, crypto, bignums and decimal. There is an LSP, a SQLite substrate, an
OIDC relying party with presence, a client-side WASM mode, a playground that runs the whole
application in a tab, two native code generators behind one seam, an OCI image builder that needs no
daemon, and — since [`docs/104`](docs/104-the-release-and-the-installer-report.md) — a release
pipeline and an installer.

**What is not built is named rather than implied.** Both code generators share one bound, and it is
half lifted: a record, a union and a newtype compile natively, and **text, collections, closures and
every effect do not**, so a program that touches a `Str` still walks. Mode B's codegen waits on that
same half, and lazy routes wait on a per-component boundary the language does not have. **No release
has been cut**, so the installer has nothing to install yet, and the binaries it would install carry
a checksum rather than a signature. Phase 3's exit criterion — *an outside developer builds a non-trivial app from
documentation alone* — is **not met**, and [`docs/08`](docs/08-roadmap.md) tracks that literally.

Every report in [`docs/`](docs/README.md) ends with what it refuses to claim. That is the house
style: "built", "runs" and "measured" are three different statements.

## Licence

MIT. See [`LICENSE`](LICENSE).
