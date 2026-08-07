# 86 — Getting started

> **What this is**: how to write and run a Beck program, from an empty directory to a Kubernetes
> object graph. It is the thing [`08`](08-roadmap.md) §8.5.4 named as the *only* remaining reason
> Phase 3's exit criterion cannot be attempted — "what it measures is documentation an outside
> developer could build from, and there is none."
>
> It is a **guide**, not a report and not a reference. [`11`](11-language-tour.md) shows the language
> construct by construct; [`docs/reference/`](reference/README.md) is generated from the compiler's
> own tables. This is the path through them.
>
> **Every program below is compiled by a test.** `beck-cli/tests/getting_started.rs` extracts each
> ```` ```beck ```` block from this file and runs the front end over it, and extracts each `beck`
> command shown and checks the subcommand exists. A guide whose examples do not compile is worse
> than no guide, and this project's answer to that is the same one it uses for the language
> reference: gate it.

## 86.1 Build the compiler

There is no released binary yet ([`28`](28-releases-and-deployment.md) is the plan and says so), so
the compiler is built from this repository. The toolchain is pinned in `rust-toolchain.toml` and the
first build downloads it:

```text
$ cd compiler
$ cargo build --release
$ ./target/release/beck --version
```

Put it on your path or refer to it as `./target/release/beck`. Everything below writes `beck`.

## 86.2 A function, and a test

A Beck file is a module. Start with one function and a test of it:

```beck
def shout(name: Str) -> Str:
    return str_upper(name) + "!"

test "it shouts":
    expect shout("ada") == "ADA!"
```

```text
$ beck test shelf.beck
test "it shouts" … ok

1 passed, 0 failed, 0 skipped
```

Two things are already true and neither was written down. The function's **effect row is empty** —
it reads nothing and writes nothing — so the compiler knows it can run anywhere, and
`beck check` will call this module a *library*:

```text
$ beck check shelf.beck
ok: 1 definitions — a library: no merge point, so there is nothing to run;
`beck iface` publishes what it offers.
```

A library is a legitimate thing to have. What makes a module an *application* is three things, and
the next two sections add them.

## 86.3 State is a fold

Beck has no tables and no `UPDATE`. **State is a fold over an event stream**, which is
[`03`](03-type-and-effect-system.md) §3.7's rule and the reason replay is exact rather than
approximate. So the first thing to write is not a schema but the two types either side of the fold —
what is recorded, and what it folds into:

```beck
model Book:
    title: Str
    read: Bool

model Shelf:
    books: Map[Str, Book]

union Event:
    Added(title: Str)

def apply_event(s: Shelf, env: Envelope[Event]) -> Shelf:
    match env.body:
        case Added(title):
            return s.with(books=map_insert(s.books, title, Book(title=title, read=False)))

test "an event puts a book on the shelf":
    expect map_len(apply_event(Shelf(books={}), Envelope(body=Added(title="SICP"), at=0, actor="ada", seq=1)).books) == 1
```

Read `apply_event`'s signature carefully, because it is the whole design in one line. It takes a
state and an `Envelope[Event]` and returns a state — it is **pure**, so its row is empty, so it
compiles to every tier that needs it. That is why the same fold can run on the server, in the
browser and in a test with no second implementation.

An `Envelope` carries what the *runtime* decided rather than what the client asked for: `at` is the
instant the merge point stamped, `actor` is who the identity provider said it was, `seq` is the
event's position. **Time is data on the envelope**, never a clock the fold reads — that is what
makes a replay reproduce the run rather than re-run it.

`.with(…)` is a functional update: it returns a new record, and there is no assignment anywhere.

## 86.4 Authority is one function

Clients do not write events. They *propose commands*, and exactly one function turns a command into
events — which is where every rule about who may do what belongs, because it is the only place that
holds the accumulator:

```beck
model Book:
    title: Str
    read: Bool

model Shelf:
    books: Map[Str, Book]

union Command:
    Add(title: Str)
    Finish(title: Str)

union Event:
    Added(title: Str)
    Finished(title: Str)

union Rejection:
    Blank
    NotOnTheShelf

def apply_event(s: Shelf, env: Envelope[Event]) -> Shelf:
    match env.body:
        case Added(title):
            return s.with(books=map_insert(s.books, title, Book(title=title, read=False)))
        case Finished(title):
            return finished(s, title)

def finished(s: Shelf, title: Str) -> Shelf:
    match map_get(s.books, title):
        case Some(book):
            return s.with(books=map_insert(s.books, title, book.with(read=True)))
        case None:
            return s

def validate(s: Shelf, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Add(title):
            if str_len(str_trim(title)) == 0:
                return Err(error=Blank)
            return Ok(value=[Added(title=title)])
        case Finish(title):
            if not map_contains(s.books, title):
                return Err(error=NotOnTheShelf)
            return Ok(value=[Finished(title=title)])

def view(s: Shelf, session: Session) -> Html:
    return ui:
        main:
            h1: "reading list"
            p: (str(map_len(s.books)) + " books")

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, shelf, validate)
shelf: Signal[Shelf] = durable(fold(apply_event, Shelf(books={}), events))
page: Signal[Html] = per_session(shelf, view)

test "a book lands on the shelf":
    when Add(title="SICP")
    expect events == [Added(title="SICP")]

test "a blank title is refused":
    when Add(title="  ")
    expect Err(error=Blank)

test "finishing a book nobody added is refused":
    when Finish(title="SICP")
    expect Err(error=NotOnTheShelf)

test "the page counts what is on the shelf":
    given [Added(title="SICP"), Added(title="HtDP")]
    expect page contains "2 books"
```

That is a complete application. The four lines at the bottom of the definitions are the whole
architecture:

| | |
|---|---|
| `merge_clients()` | **the merge point**: every connected client's proposals, interleaved. This is the one place nondeterminism enters, and there is exactly one of them |
| `decide(proposals, shelf, validate)` | the authority chokepoint: proposals in, events out, through *your* `validate`, holding the accumulator so first-writer-wins and ownership are decidable |
| `durable(fold(…))` | the database. There is no other one |
| `per_session(shelf, view)` | the page, as a function of state and session |

```text
$ beck test shelf.beck
test "a book lands on the shelf" … ok
test "a blank title is refused" … ok
test "finishing a book nobody added is refused" … ok
test "the page counts what is on the shelf" … ok

4 passed, 0 failed, 0 skipped
```

Look at what the tests did *not* need. No fixture, because `given` is a list of events and the state
is a fold of them. No mock, because `when` goes through the real `validate`. No server, because the
page is a pure function. [`21`](21-tests-in-beck-and-proof.md) is the design; the short version is
that a test names a log, an input and an expectation.

Run it:

```text
$ beck run shelf.beck
```

That serves the page, opens a websocket, and keeps the log in memory. No container, no cluster, no
database to install — [`06`](06-kubernetes-and-packaging.md) §6.6 calls this rung 0 of the parity
ladder, and it is the same code the other rungs run.

## 86.5 What the compiler worked out

Nothing above says where anything runs. Ask:

```text
$ beck explain place shelf.beck
name                 tier     kind       effects
apply_event          any      definition {}
finished             any      definition {}
validate             any      definition {}
view                 any      definition {}
proposals            server   signal     {ingress}
events               data     signal     {}
shelf                data     signal     {durable}
page                 client   signal     {}
```

Every one of those is **derived from the effect row**, and the rows are inferred. `proposals`
performs `ingress`, which only a server can discharge. `shelf` performs `durable`. The four
definitions perform nothing, so they are `any` — which means they compile to *every* tier that needs
them, and the duplication is the payoff rather than waste.

This is the property worth understanding before anything else: **you do not choose a tier, you write
what a function does, and the placement follows.** If you later give `validate` a call that reads a
secret, the secret provably cannot reach the browser — not because a reviewer noticed, but because
the row no longer fits the client tier, and the build fails.

`beck iface` writes the module's published contract — every signature with its row and its tier —
which is what a downstream module compiles against and what `beck check --wire-compat` diffs when
you change it.

## 86.6 What a deploy is

```text
$ beck build shelf.beck --out out
```

emits, from the same program:

```text
out/app.beck              the program
out/explain.txt           why each object exists
out/image.melange.yaml    the package build
out/image.apko.yaml       the image, built without a Dockerfile
out/k8s/000-namespace.yaml
out/k8s/020-service.yaml
out/k8s/030-log-service.yaml
out/k8s/040-secret.yaml
out/k8s/050-route.yaml
out/k8s/060-log-store.yaml
out/k8s/070-workload.yaml
out/k8s/080-snapshots.yaml
out/k8s/090-grants.yaml
out/k8s/100-policy.yaml
```

Nothing in `shelf.beck` mentions Kubernetes. The `durable` fold implied the volume, the snapshot
schedule and the database grants; `merge_clients()` implied the websocket route; the effect rows
implied the NetworkPolicy. Delete an effect and the object it implied disappears from the diff —
that is [`06`](06-kubernetes-and-packaging.md) §6.5's claim, and `beck-infra/tests/manifests.rs` is
where it is held to it.

## 86.7 Where to go next

| | |
|---|---|
| The language, construct by construct | [`11`](11-language-tour.md) |
| Every error code, the prelude, the effect and tier matrix | [`docs/reference/`](reference/README.md), generated from the compiler |
| Thirty-one worked programs, none with a placement annotation | [`compiler/corpus/`](../compiler/corpus/) |
| The todo sketch this project grew from | [`compiler/examples/todo.beck`](../compiler/examples/todo.beck) |
| Why any of it is shaped this way | [`01`](01-vision-and-premise.md), then [`03`](03-type-and-effect-system.md) |
| What is *not* built | every report's "what is not built" section, and [`43`](43-threat-model.md) §43.4 |

## 86.8 What this guide does not do, and what that means for the exit criterion

Stated plainly, because [`08`](08-roadmap.md) §8.5.4's exit criterion is a claim about a **person**
and this document cannot make it true on its own.

* **It does not establish that an outside developer can build from it.** That is the criterion, it
  requires an outside developer, and nobody outside this project has read this. What has changed is
  that the answer to "from what?" is no longer "there is nothing" — which was the stated blocker.
* **It covers one shape of program.** One fold, one view, one command union. Nothing here shows two
  folds, a module boundary, a trait, an outbound call, a macro or a `parallel:` scope, all of which
  exist and are documented in the reports rather than here.
* **There is no installation story.** §86.1 builds from source because there is no release; that is
  [`28`](28-releases-and-deployment.md)'s work and not a documentation gap.
* **The deployment stops at `beck build`.** `beck up` and `beck deploy` exist;
  [`82`](82-the-defaults-that-should-be-unavoidable-report.md) §82.4 is honest that nothing in this
  repository has applied a generated manifest to a real cluster, so this guide does not tell anybody
  to.

What it *is* is checked: every program compiles and every command exists, gated on every pull
request. That is the difference between documentation and a description of documentation, and it is
the same discipline [`34`](34-generated-documentation-report.md) applied to the reference.
