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
> **Every program below is compiled by a test, and its own tests are run.**
> `beck-cli/tests/getting_started.rs` extracts each ```` ```beck ```` block from this file and puts
> it through the front end exactly as `beck test` would — a block whose first line names a file,
> `# books.beck`, is a module the next block may import — then extracts each `beck` command shown
> and checks the subcommand exists. A guide whose examples do not compile is worse than no guide,
> and this project's answer to that is the same one it uses for the language reference: gate it.
> §86.12 is what the guide does *not* cover, and the same test holds the list to being shorter than
> it was: two folds, a module boundary, a trait, an outbound call, a macro and a `parallel:` scope
> are each asserted from a compiled program here.

## 86.1 Install the compiler

> **No release has been cut yet.** The installer and the pipeline that feeds it are built and
> [`92`](92-supply-chain-and-release-report.md) §92.13 says which parts of them have been
> executed — but until a tag is pushed there is nothing on the releases page, and the command below
> will tell you it could not resolve a version. **Build from source** until then; it is the second
> half of this section and it is the same binary.

One binary is the whole toolchain ([`04`](04-compiler-architecture.md) §4.6), so installing it is
downloading one file:

```text
$ curl -fsSL https://raw.githubusercontent.com/georgestephenson/beck/main/install.sh | sh
$ beck --version
beck 0.3.0 (3f3316bdc1d9 x86_64-unknown-linux-gnu)
```

[`install.sh`](../install.sh) works out your platform, downloads that platform's tarball and the
release's `SHA256SUMS`, **refuses to go on unless the two agree**, and puts `beck` in `~/.beck/bin`
— it tells you if that is not on your `PATH`. `BECK_INSTALL_DIR` puts it somewhere else, and
`BECK_VERSION` installs a version other than the latest. There are builds for x86-64 and 64-bit ARM
on Linux and macOS; there is no Windows build.

A checksum is not a signature: it says the download was not corrupted, and nothing about the page it
came from. What does say something about the page is the **build provenance** the release attests
over every artefact it publishes ([`92`](92-supply-chain-and-release-report.md)) — check it by asking, which
needs the [GitHub CLI](https://cli.github.com):

```text
$ curl -fsSL https://raw.githubusercontent.com/georgestephenson/beck/main/install.sh \
    | BECK_VERIFY_PROVENANCE=1 sh
```

That refuses to install unless the tarball's digest is the subject of a Sigstore-signed statement
naming this repository's release workflow as its builder. It is off by default because `gh` is not a
tool this script can assume you have; with it set and no `gh`, the install fails rather than
quietly skipping the check. What is still unsigned is the release *listing* itself —
[`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md) is exact about the
line between those two, and [`92`](92-supply-chain-and-release-report.md) §92.15 is where a signature a
`cosign` user could check sits unbuilt.

**Or build it from source**, which is the same binary and needs a C compiler and CMake. The
toolchain is pinned in `rust-toolchain.toml` and the first build downloads it:

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
    owner: Str

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
    NotYours

def apply_event(s: Shelf, env: Envelope[Event]) -> Shelf:
    match env.body:
        case Added(title):
            return s.with(books=map_insert(s.books, title, Book(title=title, read=False, owner=env.actor)))
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
            return owned(s, p, title)

def owned(s: Shelf, p: Proposal, title: Str) -> Result[list[Event], Rejection] uses cap.session:
    match map_get(s.books, title):
        case Some(book):
            if book.owner != p.session.actor:
                return Err(error=NotYours)
            return Ok(value=[Finished(title=title)])
        case None:
            return Err(error=NotOnTheShelf)

def additions(log: list[Event]) -> Int:
    return list_len(filter_list(log, is_addition))

def is_addition(e: Event) -> Bool:
    match e:
        case Added(title):
            return True
        case Finished(title):
            return False

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

test "somebody else's book is not yours to finish":
    given [Added(title="SICP")] by "ana"
    when session("bo") sends Finish(title="SICP")
    expect Err(error=NotYours)

test "the page counts what is on the shelf":
    given [Added(title="SICP"), Added(title="HtDP")]
    expect page contains "2 books"

property "no log puts more books on the shelf than it added"(log: list[Event]):
    given log
    expect map_len(state.books) <= additions(log)
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
test "somebody else's book is not yours to finish" … ok
test "the page counts what is on the shelf" … ok
test "no log puts more books on the shelf than it added" … ok (100 inputs)

6 passed, 0 failed, 0 skipped
```

Look at what the tests did *not* need. No fixture, because `given` is a list of events and the state
is a fold of them. No mock, because `when` goes through the real `validate`. No server, because the
page is a pure function. [`21`](21-tests-in-beck-and-proof.md) is the design; the short version is
that a test names a log, an input and an expectation.

**`uses cap.session` is the line that makes this section's title true.** `owned` says it needs a
session's authority, and a `Session` reaches exactly one place in a Beck program: `validate`, which
is the only function handed a `Proposal`. So a `cap.*` anywhere the validator does not reach is a
requirement with no holder — which is what a missing auth check looks like from the type system's
side. Delete the call and keep the function:

```text
$ beck check shelf.beck
error[B0412]: `owned` requires a capability nothing can discharge
  --> shelf.beck:45:1
   |
45 | def owned(s: Shelf, p: Proposal, title: Str) -> Result[list[Event], Rejection] uses cap.session:
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ needs {cap.session}
   |
  = note: a `Session` reaches exactly one place in a Beck program: the validator `decide` is given, which is the only function handed a `Proposal`. Authority is one chokepoint (docs/03 §3.5), so a capability required outside it has no holder
  = help: call this from `validate` — or, if it genuinely needs no authority, drop the `cap.*` from its `uses`
```

That is [`03`](03-type-and-effect-system.md) §3.5's claim as a compile error rather than a
convention: *forgetting the check is a build failure, not a pentest finding.* The capability is
declared rather than inferred because it is the one effect a body cannot be read off — checking
ownership is ordinary comparison, and nothing about `book.owner != p.session.actor` says that the
right to do it was the point.

**The last block is a `property`, not a `test`.** It names a typed input, and the generator supplies
a hundred of them from the type — `list[Event]` is a log, so what it generates is logs, folded
through the same `apply_event` as everything else. Break the fold so that `Finished` inserts a book
and it does not merely fail, it *shrinks*:

```text
test "no log puts more books on the shelf than it added" … FAILED (4 inputs)
  expected true, got false
    with log = [Finished{title: }]
```

One event, the shortest title there is — because a counterexample nobody can read is a counterexample
nobody acts on ([`21`](21-tests-in-beck-and-proof.md) §21.3 rule 5).

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
validate             server   definition {cap.session}
owned                server   definition {cap.session}
additions            any      definition {}
is_addition          any      definition {}
view                 any      definition {}
proposals            server   signal     {ingress}
events               server   signal     {cap.session}
shelf                data     signal     {durable}
page                 client   signal     {}
```

Every one of those is **derived from the effect row**, and the rows are inferred. `proposals`
performs `ingress`, which only a server can discharge. `shelf` performs `durable`. Five of the
definitions perform nothing, so they are `any` — which means they compile to *every* tier that needs
them, and the duplication is the payoff rather than waste.

This is the property worth understanding before anything else: **you do not choose a tier, you write
what a function does, and the placement follows.** The previous section is the demonstration rather
than a promise about one: `owned` is the only function that gained a line, and `uses cap.session`
moved *four* rows of this table. `owned` is on the server because no tier below one discharges a
capability; `validate` followed it there because it calls it; `events` followed `validate`, and it
had been on the data tier. Nothing about placement was edited, and nothing could have been — a
`@on(client)` on `owned` would not be an override, it would be `B0401: `owned` is placed on
`client`, which cannot discharge `cap.session``. That is the same mechanism that stops a secret
reaching a browser.

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
out/sbom.cdx.json         what went into the image, as CycloneDX
out/styles.css            the classes the program's pages can carry
```

Nothing in `shelf.beck` mentions Kubernetes. The `durable` fold implied the volume, the snapshot
schedule and the database grants; `merge_clients()` implied the websocket route; the effect rows
implied the NetworkPolicy. Delete an effect and the object it implied disappears from the diff —
that is [`06`](06-kubernetes-and-packaging.md) §6.5's claim, and `beck-infra/tests/manifests.rs` is
where it is held to it.

## 86.7 A second program, and a second fold

The shelf is one person's. A **club** is several people's, and the moment it is, the page wants
something no shelf holds: who has finished what. That is not a function of the books — it is a
different accumulation of the same events — so it is a **second fold**, and it reads a field of the
envelope the first one ignores.

```beck
model Book:
    isbn: Str
    title: Str

model Shelf:
    books: Map[Str, Book]

model Readers:
    finished: Map[Str, Int]

union Command:
    Nominate(isbn: Str, title: Str)
    Finish(isbn: Str)

union Event:
    Nominated(isbn: Str, title: Str)
    Finished(isbn: Str)

union Rejection:
    AlreadyOnTheShelf
    NotOnTheShelf

def apply_book(s: Shelf, env: Envelope[Event]) -> Shelf:
    match env.body:
        case Nominated(isbn, title):
            return s.with(books=map_insert(s.books, isbn, Book(isbn=isbn, title=title)))
        case Finished(isbn):
            return s

def apply_reader(r: Readers, env: Envelope[Event]) -> Readers:
    match env.body:
        case Nominated(isbn, title):
            return r
        case Finished(isbn):
            return r.with(finished=map_insert(r.finished, env.actor, read_count(r, env.actor) + 1))

def read_count(r: Readers, who: Str) -> Int:
    return unwrap_or(map_get(r.finished, who), 0)

def validate(s: Shelf, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Nominate(isbn, title):
            if map_contains(s.books, isbn):
                return Err(error=AlreadyOnTheShelf)
            return Ok(value=[Nominated(isbn=isbn, title=title)])
        case Finish(isbn):
            if not map_contains(s.books, isbn):
                return Err(error=NotOnTheShelf)
            return Ok(value=[Finished(isbn=isbn)])

def render(s: Shelf, r: Readers) -> Html:
    return ui:
        main:
            h1: "the club"
            ul:
                for b in map_values(s.books):
                    li(key=b.isbn): b.title
            ul:
                for who in map_keys(r.finished):
                    li: (who + " has finished " + str(read_count(r, who)))

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, shelf, validate)
shelf: Signal[Shelf] = durable(fold(apply_book, Shelf(books={}), events))
readers: Signal[Readers] = durable(fold(apply_reader, Readers(finished={}), events))
page: Signal[Html] = map2(render, shelf, readers)

test "a nomination lands on the shelf":
    given [Nominated(isbn="0262510871", title="SICP")]
    expect page contains "SICP"

test "each reader is counted against the name the runtime supplied":
    given [Nominated(isbn="0262510871", title="SICP"), Nominated(isbn="0262560995", title="HtDP")]
    given [Finished(isbn="0262510871"), Finished(isbn="0262560995")] by "ana"
    given [Finished(isbn="0262510871")] by "bo"
    expect page contains "ana has finished 2"
    expect page contains "bo has finished 1"

test "finishing a book nobody nominated is refused":
    given []
    when Finish(isbn="0262510871")
    expect Err(error=NotOnTheShelf)
```

Two `durable` folds, one `merge_clients()`. A program has **one log** — that is
[`03`](03-type-and-effect-system.md) §3.7 and it is not negotiable — so two folds are two
projections of it rather than two databases; `beck explain flow` prints the accumulator they
compile to, which is one record with a field per fold. `map2` is the two-signal `signal_map`: this
club's page is the same page for everybody, so it takes no `Session` and §86.4's `per_session` is
what puts one back when a page differs by who is reading it.

The reason to keep the folds apart is visible in what each one reads. `apply_book` never looks at
`env.actor`, and `apply_reader` looks at almost nothing else. **Nobody in this program asks who is
logged in.** The actor is on the envelope because the merge point put it there — from the identity
provider, not from the client — so a proposal that claimed to come from somebody else would still be
counted against whoever sent it, and no line here is responsible for that.

```text
$ beck test club.beck
test "a nomination lands on the shelf" … ok
test "each reader is counted against the name the runtime supplied" … ok
test "finishing a book nobody nominated is refused" … ok

3 passed, 0 failed, 0 skipped
```

## 86.8 Asking somebody else

Members should not have to type a title. The club knows the ISBN; a catalogue service knows what the
book is called, and the library's stock service knows how many copies there are. Two questions, and
neither one needs the other's answer.

```beck
import http
import json

model Book:
    isbn: Str
    title: Str
    copies: Int

model Shelf:
    books: Map[Str, Book]

model Readers:
    finished: Map[Str, Int]

union Command:
    Nominate(isbn: Str)
    Finish(isbn: Str)

union Event:
    Nominated(book: Book)
    Finished(isbn: Str)

union Rejection:
    AlreadyOnTheShelf
    NotOnTheShelf
    NoAnswer

def apply_book(s: Shelf, env: Envelope[Event]) -> Shelf:
    match env.body:
        case Nominated(book):
            return s.with(books=map_insert(s.books, book.isbn, book))
        case Finished(isbn):
            return s

def apply_reader(r: Readers, env: Envelope[Event]) -> Readers:
    match env.body:
        case Nominated(book):
            return r
        case Finished(isbn):
            return r.with(finished=map_insert(r.finished, env.actor, read_count(r, env.actor) + 1))

def read_count(r: Readers, who: Str) -> Int:
    return unwrap_or(map_get(r.finished, who), 0)

def as_document(body: Str) -> Result[Json, JsonError]:
    return try: json_parse(body)

def field_of(doc: Json, name: Str) -> Str:
    match doc:
        case JsonObject(fields):
            match map_get(fields, name):
                case Some(JsonStr(value)):
                    return value
                case _:
                    return ""
        case _:
            return ""

def title_of(body: Str) -> Str:
    match as_document(body):
        case Ok(doc):
            return field_of(doc, "title")
        case Err(why):
            return ""

def catalogue(isbn: Str) -> Str:
    return title_of(require_ok(http_fetch("catalogue.example.com", accepting_json(get("/isbn/" + isbn)))))

def copies_of(isbn: Str) -> Int:
    return unwrap_or(str_to_int(str_trim(require_ok(http_fetch("stock.example.com", get("/copies/" + isbn))))), 0)

def look_up(isbn: Str) -> Book:
    return parallel:
        title = catalogue(isbn)
        copies = copies_of(isbn)
        Book(isbn=isbn, title=title, copies=copies)

def nominated(isbn: Str) -> Result[list[Event], Rejection]:
    looked = try: look_up(isbn)
    match looked:
        case Ok(book):
            return Ok(value=[Nominated(book=book)])
        case Err(why):
            return Err(error=NoAnswer)

def validate(s: Shelf, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Nominate(isbn):
            if map_contains(s.books, isbn):
                return Err(error=AlreadyOnTheShelf)
            return nominated(isbn)
        case Finish(isbn):
            if not map_contains(s.books, isbn):
                return Err(error=NotOnTheShelf)
            return Ok(value=[Finished(isbn=isbn)])

def render(s: Shelf, r: Readers) -> Html:
    return ui:
        main:
            h1: "the club"
            ul:
                for b in map_values(s.books):
                    li(key=b.isbn): (b.title + " — " + str(b.copies) + " copies")
            ul:
                for who in map_keys(r.finished):
                    li: (who + " has finished " + str(read_count(r, who)))

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, shelf, validate)
shelf: Signal[Shelf] = durable(fold(apply_book, Shelf(books={}), events))
readers: Signal[Readers] = durable(fold(apply_reader, Readers(finished={}), events))
page: Signal[Html] = map2(render, shelf, readers)

test "the title is read out of the catalogue's document":
    expect title_of("{\"title\":\"SICP\",\"year\":1985}") == "SICP"
    expect title_of("not a document at all") == ""

test "a nomination asks both services and records what they said":
    stub net.out(catalogue.example.com): "SICP"
    stub net.out(stock.example.com): 2
    when Nominate(isbn="0262510871")
    expect events == [Nominated(book=Book(isbn="0262510871", title="SICP", copies=2))]
    expect net.out(catalogue.example.com) once

test "a nomination nobody answers is refused rather than recorded":
    stub net.out(catalogue.example.com): raise HttpUnreachable(host="catalogue.example.com", why="connection refused")
    when Nominate(isbn="0262510871")
    expect result == Err(error=NoAnswer)

test "the page shows what the shelf and the leaderboard each hold":
    given [Nominated(book=Book(isbn="0262510871", title="SICP", copies=2))]
    given [Finished(isbn="0262510871")] by "ana"
    expect page contains "SICP — 2 copies"
    expect page contains "ana has finished 1"
```

Four things arrived there, and none of them is larger than the step that wanted it.

**`import`.** `http` and `json` are standard-library modules carried inside the compiler
([`46`](46-standard-library-report.md)), so there is nothing to fetch and no version to pin —
`import` resolves against your own directory first and the library second. `http` is itself written
in Beck, over one primitive.

**The host is written at the call site.** `http_fetch("catalogue.example.com", …)` takes the host as
a literal first argument rather than as a field of the request, and that is why the library has no
`get(host, path)` and cannot have one: the call performs `net.out(catalogue.example.com)`, and
[`06`](06-kubernetes-and-packaging.md) §6.5 derives the cluster's egress rules from exactly those
atoms. A host that arrived in a variable would be an outbound call the deployment could not be told
about.

**`parallel:`** is an expression whose bindings are its children and whose tail runs once, after the
join, with all of them in scope. No child may name another, and no child may perform an effect
another could observe — both are compile errors rather than conventions
([`80`](80-structured-concurrency-report.md)) — so the scope's answer cannot depend on which one
finished first. `net.out(host)` is deliberately *not* on that forbidden list: a remote host's state
was never Beck's to order, and that is what is left for the form to be about.

**`try:`** turns a raise into a value. `http_fetch` and `require_ok` both raise `HttpError`, so
`catalogue` and `copies_of` do too — nothing in their signatures says so, because the row is
inferred — and `nominated` writes `try:` once, at the point where the shape the runtime wants is a
`Result`.

Now look at the tests, because there is no server in them and no mock either.
`stub net.out(catalogue.example.com): "SICP"` names the **effect**, not a method: the atom is the
identity, so there is no parameter list to restate and nothing to keep in step with the code. A call
that is not stubbed is not an omission — every outbound call is stubbed by default and returns the
canonical value of its type ([`21`](21-tests-in-beck-and-proof.md) §21.3), so the case you do not
care about costs nothing. And `expect net.out(catalogue.example.com) once` is the other direction:
verification as a query over what happened, rather than an expectation arranged in advance and
checked at the end.

The second test is the arm you would otherwise never run. A stub stands in for the *definition*, so
it may answer the way that definition may answer — and one of `catalogue`'s answers is failure,
because its inferred row says `raises(HttpError)`. `stub …: raise HttpUnreachable(…)` unwinds
exactly where the real call would, the `try:` in `nominated` catches it, and `NoAnswer` is reached.
A stub that raised something the signature does not declare is refused (`B0708`), for the same
reason the row exists: every caller was checked against what the signature publishes.

Ask where it all landed:

```text
$ beck explain place club.beck
```

The program's own rows, out of a table that also lists everything the standard library brought:

```text
catalogue            server   definition {net.out(catalogue.example.com), raises(HttpError)}
copies_of            server   definition {net.out(stock.example.com), raises(HttpError)}
look_up              server   definition {net.out(catalogue.example.com), net.out(stock.example.com), spawn, raises(HttpError)}
validate             server   definition {net.out(catalogue.example.com), net.out(stock.example.com), spawn}
events               server   signal     {net.out(catalogue.example.com), net.out(stock.example.com), spawn}
shelf                data     signal     {durable}
readers              data     signal     {durable}
page                 client   signal     {}
```

Nothing in the program says `server`. `net.out` and `spawn` are atoms only a server discharges
([`docs/reference/effects.md`](reference/effects.md)), the rows are inferred, and the placement
follows — so `validate` moved to the server the moment it reached a service, and it took the
chokepoint with it. Nothing else moved: the folds are still on the data tier and the page is still
on the client.

## 86.9 Two files, and a macro for the half that is drudgery

`club.beck` is now doing two jobs. It says what a book *is* and what happened to it; and it says who
may do what, and who to ask. Those change for different reasons and at different rates, which is the
usual reason to split a file — and here a second one arrives at the same time.

The club has a chat channel and should tell it what happened. Sending a `Book` means building a
`Json` out of it field by field, and building it again every time the model changes. So don't:

```beck
# books.beck
import json

derive_json:
    model Book:
        isbn: Str
        title: Str
        copies: Int

model Shelf:
    books: Map[Str, Book]

model Readers:
    finished: Map[Str, Int]

union Event:
    Nominated(book: Book)
    Finished(isbn: Str)

impl ToJson for Event:
    def to_json(self):
        match self:
            case Nominated(book):
                return JsonObject(fields={"what": JsonStr(value="nominated"), "book": book.to_json()})
            case Finished(isbn):
                return JsonObject(fields={"what": JsonStr(value="finished"), "isbn": JsonStr(value=isbn)})

def apply_book(s: Shelf, env: Envelope[Event]) -> Shelf:
    match env.body:
        case Nominated(book):
            return s.with(books=map_insert(s.books, book.isbn, book))
        case Finished(isbn):
            return s

def apply_reader(r: Readers, env: Envelope[Event]) -> Readers:
    match env.body:
        case Nominated(book):
            return r
        case Finished(isbn):
            return r.with(finished=map_insert(r.finished, env.actor, read_count(r, env.actor) + 1))

def read_count(r: Readers, who: Str) -> Int:
    return unwrap_or(map_get(r.finished, who), 0)

test "a model's json is its fields, and the union's is what this file decided":
    expect json_render(Book(isbn="0262510871", title="SICP", copies=2).to_json()) == "{\"copies\":2.0,\"isbn\":\"0262510871\",\"title\":\"SICP\"}"
    expect str_contains(json_render(Finished(isbn="0262510871").to_json()), "\"what\":\"finished\"")
```

```beck
# club.beck
import http
import json
import books

union Command:
    Nominate(isbn: Str)
    Finish(isbn: Str)

union Rejection:
    AlreadyOnTheShelf
    NotOnTheShelf
    NoAnswer

def field_of(doc: Json, name: Str) -> Str:
    match doc:
        case JsonObject(fields):
            match map_get(fields, name):
                case Some(JsonStr(value)):
                    return value
                case _:
                    return ""
        case _:
            return ""

def as_document(body: Str) -> Result[Json, JsonError]:
    return try: json_parse(body)

def title_of(body: Str) -> Str:
    match as_document(body):
        case Ok(doc):
            return field_of(doc, "title")
        case Err(why):
            return ""

def catalogue(isbn: Str) -> Str:
    return title_of(require_ok(http_fetch("catalogue.example.com", accepting_json(get("/isbn/" + isbn)))))

def copies_of(isbn: Str) -> Int:
    return unwrap_or(str_to_int(str_trim(require_ok(http_fetch("stock.example.com", get("/copies/" + isbn))))), 0)

def look_up(isbn: Str) -> Book:
    return parallel:
        title = catalogue(isbn)
        copies = copies_of(isbn)
        Book(isbn=isbn, title=title, copies=copies)

def announce[T: ToJson](what: T) -> Bool:
    return is_ok(http_fetch("hooks.example.com", post("/club", json_render(what.to_json()))))

def announced(e: Event) -> list[Event]:
    _ = try: announce(e)
    return [e]

def nominated(isbn: Str) -> Result[list[Event], Rejection]:
    looked = try: look_up(isbn)
    match looked:
        case Ok(book):
            return Ok(value=announced(Nominated(book=book)))
        case Err(why):
            return Err(error=NoAnswer)

def validate(s: Shelf, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Nominate(isbn):
            if map_contains(s.books, isbn):
                return Err(error=AlreadyOnTheShelf)
            return nominated(isbn)
        case Finish(isbn):
            if not map_contains(s.books, isbn):
                return Err(error=NotOnTheShelf)
            return Ok(value=announced(Finished(isbn=isbn)))

def render(s: Shelf, r: Readers) -> Html:
    return ui:
        main:
            h1: "the club"
            ul:
                for b in map_values(s.books):
                    li(key=b.isbn): (b.title + " — " + str(b.copies) + " copies")
            ul:
                for who in map_keys(r.finished):
                    li: (who + " has finished " + str(read_count(r, who)))

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, shelf, validate)
shelf: Signal[Shelf] = durable(fold(apply_book, Shelf(books={}), events))
readers: Signal[Readers] = durable(fold(apply_reader, Readers(finished={}), events))
page: Signal[Html] = map2(render, shelf, readers)

test "the title is read out of the catalogue's document":
    expect title_of("{\"title\":\"SICP\",\"year\":1985}") == "SICP"
    expect title_of("not a document at all") == ""

test "a nomination asks both services, records what they said, and tells the chat":
    stub net.out(catalogue.example.com): "SICP"
    stub net.out(stock.example.com): 2
    when Nominate(isbn="0262510871")
    expect events == [Nominated(book=Book(isbn="0262510871", title="SICP", copies=2))]
    expect net.out(hooks.example.com) once

test "a chat that refuses the message does not cost the club what a member did":
    stub net.out(hooks.example.com): False
    given [Nominated(book=Book(isbn="0262510871", title="SICP", copies=2))]
    when Finish(isbn="0262510871")
    expect events == [Finished(isbn="0262510871")]
```

`derive_json:` is a **macro**. It reads the fields out of the declaration it is handed and writes
the `impl` somebody would otherwise have written, at compile time — so what runs is ordinary code
and there is no reflection anywhere in the program
([`102`](102-the-macro-interpreter-report.md) is the interpreter that makes a macro body ordinary
Beck). It takes a *declaration*, which is why the `model` is written inside it.

`impl ToJson for Event` is written out by hand, and the contrast is the point. A model's fields are
drudgery; **a union's shape is a decision** — what tags a variant, and what the reader at the other
end will match on — and a macro that guessed it would be one you had to fight. The two compose: the
arm written here calls `book.to_json()`, which the macro wrote.

`announce[T: ToJson]` is a **bound** — one function that posts anything the club can write down,
knowing nothing about its argument except that it can be written. `announced` is where the club
decides what a chat outage costs it, and the answer is nothing: `try:` makes the failure a value,
and `_ =` is the club saying it will not read it. Whether the chat heard is not the club's state;
what a member did is, and an outage at somebody else's host is not a reason to lose it.

The split itself is one `import` and one contract:

```text
$ beck iface books.beck
wrote books.becki
```

and `books.becki` reads, in part:

```text
impl ToJson for Book
impl ToJson for Event

@on(any)
def apply_book(s: Shelf, env: Envelope[Event]) -> Shelf
```

Types, signatures, rows, tiers and the impls — no bodies. That file is what `club.beck` compiles
against, `beck check --wire-compat books.becki` is what says whether a change to `books.beck` is one
a running deployment can take, and `@on(any)` is there because a published placement is part of a
published signature: an imported definition is placed where its own module placed it. Note what is
*not* in it. The standard library `books.beck` imported is not re-exported, because a module
publishes what it owns — and the impl the macro wrote is published beside the one written by hand,
because a call in another module cannot resolve `book.to_json()` without knowing that it exists.

`beck test` on the root runs both files' tests, which is what makes a domain module worth writing
tests in at all:

```text
$ beck test club.beck
test "a model's json is its fields, and the union's is what this file decided" … ok
test "the title is read out of the catalogue's document" … ok
test "a nomination asks both services, records what they said, and tells the chat" … ok
test "a chat that refuses the message does not cost the club what a member did" … ok

4 passed, 0 failed, 0 skipped
```

## 86.10 The deploy learned three names

`beck build` on the club emits the same sixteen files §86.6 listed, and the same ten Kubernetes
objects. One of them is different:

```text
$ beck build club.beck --out out
$ grep egress-hosts out/k8s/100-policy.yaml
    beck.dev/egress-hosts: "catalogue.example.com,hooks.example.com,stock.example.com"
```

Three hosts nobody wrote into a manifest: they are the `net.out(host)` atoms, sorted. Delete the
`announce` call and `hooks.example.com` leaves that line, because the row it was derived from no
longer carries it. `beck explain deploy club.beck` prints the derivation object by object, and its
`Policy` line names every definition that put a host there.

**A core `NetworkPolicy` cannot say "may talk to `catalogue.example.com`."** Its egress peers are IP
blocks and selectors, and a DNS name is neither. So what is generated is egress on 443 to everything
*except* the private ranges, with the host list in an annotation beside it — which is what a mesh or
a gateway reads. [`06`](06-kubernetes-and-packaging.md) §6.5 is exact about the line between what is
enforced and what is recorded, and the reason to say so here is that a guide which showed the
annotation without the sentence would have promised a firewall.

## 86.11 Where to go next

| | |
|---|---|
| The language, construct by construct | [`11`](11-language-tour.md) |
| Every error code, the prelude, the effect and tier matrix | [`docs/reference/`](reference/README.md), generated from the compiler |
| 39<!--c:corpus-programs--> worked programs, none with a placement annotation | [`compiler/corpus/`](../compiler/corpus/) |
| The todo sketch this project grew from | [`compiler/examples/todo.beck`](../compiler/examples/todo.beck) |
| Why any of it is shaped this way | [`01`](01-vision-and-premise.md), then [`03`](03-type-and-effect-system.md) |
| What is *not* built | every report's "what is not built" section, and [`43`](43-threat-model.md) §43.4 |

## 86.12 What this guide does not do, and what that means for the exit criterion

Stated plainly, because [`08`](08-roadmap.md) §8.5.4's exit criterion is a claim about a **person**
and this document cannot make it true on its own.

* **It does not establish that an outside developer can build from it.** That is the criterion, it
  requires an outside developer, and nobody outside this project has read this. What has changed is
  that the answer to "from what?" is no longer "there is nothing" — which was the stated blocker.
* **It covers two shapes of program and there are more.** Neither one is optimistic about a client:
  nothing here shows `gestures`, presence or awareness ([`94`](94-the-client-report.md)), or a view
  the engine maintains by delta rather than recomputing
  ([`99`](99-the-data-tier-means-of-combination.md)). Those are in the reports and in
  [`compiler/corpus/`](../compiler/corpus/), and they are not here. **Two that were on this list are
  now in §86.4**: a capability a chokepoint has to hold (`cap.*`,
  [`03`](03-type-and-effect-system.md) §3.5), shown as the four table rows it moves and as the
  `B0412` that arrives when the check is dropped; and a `property` block, shown with the
  counterexample it shrinks to.
* ~~**There is no installation story.**~~ There is one — §86.1 — and it is
  [`92`](92-supply-chain-and-release-report.md)'s work: an installer that verifies what it
  downloaded, and a tag-triggered pipeline that builds what it installs. §92.13 is careful about
  which half of that has been executed: the installer has, against a real artefact; the workflow has
  not, because no tag has been cut. **So the command above will not work until one is**, and
  building from source is what this guide can promise today.
* **The deployment stops at `beck build`.** `beck up` and `beck deploy` exist;
  [`82`](82-the-edge-report.md) §82.8 is honest that nothing in this
  repository has applied a generated manifest to a real cluster, so this guide does not tell anybody
  to.

What it *is* is checked: every program compiles, every program's own tests pass, every command
exists, and the list above is held to being shorter than it was — gated on every pull request. That
is the difference between documentation and a description of documentation, and it is the same
discipline [`34`](34-generated-documentation-report.md) applied to the reference.
