# 11 — Language tour: what Beck looks like

The canonical full-app example is in [`01`](01-vision-and-premise.md) §1.3 (the todo sketch,
translated). This tour shows the rest of the language, construct by construct, as currently
designed. Everything here follows the rules fixed in [`02-syntax.md`](02-syntax.md): one homoiconic
AST, Python-shaped default surface, S-expression second surface, everything an expression.

> **A note on the code fences**: blocks are tagged `python` and `clojure` purely so Markdown
> renderers apply passable syntax highlighting — none of this is Python or Clojure. Beck is its own
> language with its own compiler; *why* it must be, rather than a framework on either host, is
> argued in [`10-decisions.md`](10-decisions.md) D9. (A `tree-sitter-beck` grammar replaces this
> hack the moment it exists.)

## 11.1 Modules, values, types

```python
# geometry.beck — a module is a file; a package is a directory with beck.toml
import std.math (sqrt, tau)
import shapes.svg as svg                # no wildcard imports, ever

radius = 4.5                            # immutable binding, type inferred (f64)
var hits = 0                            # mutability is explicit and local-only
hits += 1

type Meters = newtype[f64]              # zero-cost nominal newtype: Meters ≠ f64 to the checker

model Point:                            # a record
    x: f64; y: f64

union Shape:                            # an algebraic data type
    Circle(center: Point, r: Meters)
    Rect(a: Point, b: Point)
    Poly(points: list[Point])
```

## 11.2 Functions, matching, errors — everything is an expression

```python
def area(s: Shape) -> f64:
    match s:                            # exhaustive, or it does not compile —
        case Circle(_, r): tau / 2 * r.value ** 2      # this check carries the
        case Rect(a, b):   abs((b.x - a.x) * (b.y - a.y))   # migration story too
        case Poly(ps):     shoelace(ps)

def biggest(shapes: list[Shape]) -> Option[Shape]:
    shapes.max_by(area)                 # there is no null; Option is the absence type

label = if shapes.is_empty(): "none" else: f"{shapes.len()} shapes"   # if is an expression

def parse_shape(src: str) -> Result[Shape, ParseError]:
    tokens = lex(src)?                  # ? propagates the typed error upward
    shape_of(tokens)?                   # try:/catch e: sugar exists over the same Result
```

`parallel:` is an expression too — a scope whose bindings are its children, and whose tail runs once
after the join with all of them in scope:

```python
def screen(email: Str) -> Screening:
    return parallel:
        f = fraud_score(email)          # two outbound calls that do not wait
        r = reputation_score(email)     # for each other
        Screening(fraud=f, reputation=r)
```

No child may name another, and no child may perform an effect another child could observe — so the
scope's answer does not depend on which ran first, and both halves of that are compile errors rather
than conventions. The scope performs `spawn`, which §3.3's table places on the server and which the
published signature carries; a failure in a child crosses the scope, and the ordered join makes
*which* failure a function of the program rather than of a scheduler.
[`80`](80-structured-concurrency-report.md) is the build report, including what it deliberately
does not do.

## 11.3 Traits and derivation

```python
trait Drawable:
    def draw(self) -> Svg

impl Drawable for Shape:
    def draw(self):
        match self:
            case Circle(c, r): svg.circle(c.x, c.y, r.value)
            ...

@derive(Eq, Hash, Json)                 # decorators receive the definition's AST —
model Tag:                              # familiar Python notation, Lisp semantics
    name: str
```

## 11.4 The application core (recap)

The full shape is [`01`](01-vision-and-premise.md) §1.3; the skeleton, for orientation:

```python
union Command: ...                      # what clients may ask
union Event: ...                        # what the server records

commands: Stream[(Session, Command)] = merge_clients()          # time enters here, only here
events:   Stream[Event] = commands.filter_map(validate)         # the one authority chokepoint
state:    Signal[S] = durable(fold(apply_event, init, events))  # the database
view_of:  Signal[Html] = state.map(view)                        # the page
```

Purity is placement: `apply_event` and `view` carry no `@on(...)` and compile to both tiers;
`validate` holds the `Session` capability and can only live on the server. `now()` inside a fold is
a compile error — time is data on the event envelope.

## 11.5 Queries and comprehensions

```python
def recent(c: Ref[Customer], limit: int = 20) -> list[Order]:
    return from o in orders.values()
           where o.customer == c and o.at > clock() - 7.days
           order by o.at desc
           take limit
```

A comprehension is sugar for a pure function that is *guaranteed* incrementalizable; subscribed, it
compiles to a maintained dataflow plan; one-shot, to SQL against a read model. `clock()` makes the
time-dependence typed and visible.

## 11.6 UI and styles

```python
component TodoList(sess: Session):
    mine = todos.map(filter_by(sess.user))          # per-session signal — server-filtered, typed
    ui:
        section(cls="list"):
            for t in mine.values().sort_by(lambda t: t.text):
                TodoRow(t)                          # components compose like functions
        footer: f"{mine.len()} items"

styles = css:
    .list: {max_width: 40.ch, margin: "0 auto"}
```

`ui:` and `css:` are macros producing typed trees (the Hiccup lineage of the original sketch), not
privileged syntax. Interpolated HTML is escaped by type — XSS has no representation.

**Two things above are designed and not built**, and
[`104`](104-styling-and-the-component-library.md) §104.1 measures the gap. `component` is not a
keyword: a component today is a `def` returning `Html`, which composes across modules and may be
generic over the application's command (§104.7), and a program has one `page`
([`94`](94-the-client-report.md) §94.15). `css:` has no parser and no macro — the stylesheet a
running application serves is a Rust constant.

**The third thing this paragraph listed is now a compile error, and the block above trips it.** The
attribute is spelled `class`, not `cls`, and `ui:` used to check neither attribute nor event names,
so `cls=` reached the browser as an attribute nothing reads. It has a vocabulary now (§104.8):
`B0218` refuses `cls=` with `class` as the suggestion, `B0217` refuses an event the client does not
listen for, and `B0219`–`B0221` refuse an image with no alt text, a button with no accessible name
and a control with no label ([`12`](12-standards-and-conformance.md) §12.4). **And `class=` takes a
list**, so a conditional class is `class=["btn", "primary" if hot else "plain"]` rather than a string
built with `+` — which is the difference between a page whose classes the compiler can enumerate and
one whose classes exist only while it is rendering (`beck explain style` prints which of the two you
wrote). This sentence claimed
the opposite for as long as the vocabulary has existed, and nothing caught it, because **no gate
compiles the Beck in this document** — [`08`](08-roadmap.md) §8.5.6 is where that is recorded and
[`86`](86-getting-started.md) is the one document whose programs a test does compile and run.

## 11.7 Macros — the Lisp inheritance

```python
macro unless(cond, do):
    return quote:
        if not $cond:
            $do

retry(times=3, backoff=exponential):     # any call taking a block: the block rule (02 §2.3)
    charge(card, total)
```

Hygienic by default, capability-restricted at compile time, expansion cached. `beck fmt --sexpr`
prints any of this as the canonical S-expressions when you want to see the AST you're manipulating.

## 11.8 Services, deployment, identity

```python
service app:
    entry     = TodoList
    expose    = http(route="/", tls=auto)
    autoscale = between(2, 50, on=[cpu(70), p99_latency(50.ms)])

identity = managed()                     # bundled Keycloak; or external(issuer=...)

deployment prod:
    platform = kubernetes(context="prod-eu")
    include  = [app]
```

Everything else — images, manifests, policies, volumes, websocket routes, RBAC — is derived from
the program's effects, not declared ([`06`](06-kubernetes-and-packaging.md)).

## 11.9 Escape hatches, typed

*Not built — none of the four. `external store`, `extern def` and `python_service` are refused by
the parser (`B0307`, "unsupported top-level item"), and `sql"…"` has its **notation** and not its
macro: `name"body"` lexes and desugars ([`02`](02-syntax.md) §2.5, built), while a `sql_sigil` that
checks columns against a `store` and binds parameters does not exist and is not next — §2.5 says
why, and the short version is that a Beck program never writes SQL. This section is a sketch of
where each hatch would go, kept because the shapes are still the intended ones. §11.10 below carries
the same kind of note for the opposite reason: that one is built and the notation moved.*

```python
external store legacy = postgres(url=env("LEGACY_DB"))    # existing DB: honest effects, no fold guarantees
rows = sql"select id, total from invoices where due < {cutoff}"   # checked at compile time, bind-params only

extern def blake3(data: bytes) -> Hash from "libblake3"   # C ABI FFI

python_service scorer:                                    # typed sidecar, own container, generated stubs
    def score(features: Features) -> f64
```

## 11.10 Tests are part of the language

*Built, and the notation moved. This sketch predates the design in
[`21`](21-tests-in-beck-and-proof.md) §21.2 and the implementation in
[`22`](22-phase-3-report.md); the shipped form is below it.*

The sketch as first written:

```python
test "toggling twice is identity":
    s0 = {id: Todo(id, "milk", done=False)}
    assert apply_event(apply_event(s0, Toggled(id)), Toggled(id)) == s0

property "no event loses an id" (events: list[Event]):    # property-based, shrinking built in
    state = events.fold(apply_event, {})
    assert state.keys() <= ids_of(events)

test "placement is what we think":
    assert place(validate) == server
    assert secret_flows(ApiKey).client == none            # §3.5, as an executable assertion

test "the incident, replayed" (world = fork(log="tests/logs/incident-42.log")):
    assert world.at(seq=1041).remaining == 3              # determinism makes history a fixture
```

What `beck test` actually runs. The change is `s0 = {…}` becoming `given [ … ]`: a state is not
built, it is *folded*, so a test cannot arrange one the program could not reach —
[`21`](21-tests-in-beck-and-proof.md) §21.1.

```python
test "toggling twice is identity":
    given [Added(id=Id("1"), text="milk")] by "ana"
    when session("ana") sends Toggle(id=Id("1")), Toggle(id=Id("1"))
    expect state == fold_of [Added(id=Id("1"), text="milk")] by "ana"

property "no log the program can produce makes the page unrenderable"(log: list[Event]):
    given log
    expect page contains "remaining"

test "placement is what we think":
    expect place(validate) == server                      # answered without running anything
    expect flow(ApiKey) reaches nothing on client         # §3.5, as an executable assertion
```

`fork(log=…)` fixtures are Phase 4's, with `beck fork` itself
([`08`](08-roadmap.md)); everything else above runs today.

The full testing strategy — including what the *compiler's own* test suite looks like — is
[`13-testing.md`](13-testing.md).

## 11.11 The second surface

Any definition prints as canonical S-expressions and parses back losslessly:

```clojure
(def area (params (: s Shape)) (returns f64)
  (match s
    ((Circle _ r) (* (/ tau 2) (** (. r value) 2)))
    ((Rect a b)   (abs (* (- (. b x) (. a x)) (- (. b y) (. a y)))))
    ((Poly ps)    (shoelace ps))))
```

This is the notation of the original sketch, the reference manual, and macro debugging — one
language, two faithful projections ([`02`](02-syntax.md) §2.2).
