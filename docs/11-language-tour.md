# 11 — Language tour: what Tier looks like

The canonical full-app example is in [`01`](01-vision-and-premise.md) §1.3 (the todo sketch,
translated). This tour shows the rest of the language, construct by construct, as currently
designed. Everything here follows the rules fixed in [`02-syntax.md`](02-syntax.md): one homoiconic
AST, Python-shaped default surface, S-expression second surface, everything an expression.

> **A note on the code fences**: blocks are tagged `python` and `clojure` purely so Markdown
> renderers apply passable syntax highlighting — none of this is Python or Clojure. Tier is its own
> language with its own compiler; *why* it must be, rather than a framework on either host, is
> argued in [`10-decisions.md`](10-decisions.md) D9. (A `tree-sitter-tier` grammar replaces this
> hack the moment it exists.)

## 11.1 Modules, values, types

```python
# geometry.tier — a module is a file; a package is a directory with tier.toml
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

## 11.7 Macros — the Lisp inheritance

```python
macro unless(cond, do):
    return quote:
        if not $cond:
            $do

retry(times=3, backoff=exponential):     # any call taking a block: the block rule (02 §2.3)
    charge(card, total)
```

Hygienic by default, capability-restricted at compile time, expansion cached. `tier fmt --sexpr`
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

```python
external store legacy = postgres(url=env("LEGACY_DB"))    # existing DB: honest effects, no fold guarantees
rows = sql"select id, total from invoices where due < {cutoff}"   # checked at compile time, bind-params only

extern def blake3(data: bytes) -> Hash from "libblake3"   # C ABI FFI

python_service scorer:                                    # typed sidecar, own container, generated stubs
    def score(features: Features) -> f64
```

## 11.10 Tests are part of the language

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
