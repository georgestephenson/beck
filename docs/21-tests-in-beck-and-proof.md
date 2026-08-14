# 21. Tests written in Beck, and proof of what Beck writes

*§21.2 and §21.3 are **built** — see [`22`](22-phase-3-report.md), which also records the five
places where building them corrected this document. §21.4 rungs 1–5 are built and §21.5 is not. The
distinction is stated per section rather than left to be discovered — "built" and "runs" and
"measured" are three different claims, and "designed" is a fourth.*

Two questions that sound like one:

1. **What tests can somebody write about their own Beck program?** [`11`](11-language-tour.md)
   §11.10 sketches `test`/`property` blocks and [`13`](13-testing.md) §13.9 promises them as a
   language feature. Both are built now (`beck test`); the compiler's own suite — the differential
   harness, the replay harness, the corpus, the security suite — tests *the compiler*, which is a
   different thing entirely. §21.2 and §21.3 are the design, and
   [`22`](22-phase-3-report.md) is what got built from it.
2. **How do we know the files `beck build` writes are correct?** Not "have we tested them" but
   what *kind* of confidence each mechanism buys. §21.4 is the ladder, with the rung each defect in
   [`20`](20-phase-2-report.md) §20.4 item 13 was caught by; §21.5 is what a proof would and would
   not add.

---

## 21.1 Why Beck's testing story can be different, and not just nicer

Most testing pain is not about assertions. It is about *getting the system into a state where an
assertion is meaningful*, and about *cutting the system off from the world* so the assertion is
stable. Beck's semantics already answer both, and this is the whole reason to build a test construct
into the language rather than ship a library:

| what makes a test hard elsewhere | what Beck already has |
|---|---|
| arranging state: fixtures, factories, a database, migrations, teardown | **state is a fold over an event log**, so a state *is* a list of events, written inline |
| the system under test spans processes: a client, an API, a worker, a database | **one program**, whose tiers are a placement of the same graph — they can be co-located in one process, and `beck run` already does exactly that |
| time, randomness, identity make runs differ | **the merge point is the only place they enter** (§3.7): `env.at` is data, ids are minted at the edge, and everything downstream is a deterministic function |
| knowing what to stub | **the effect row says** — the compiler already computes, per definition, the complete list of interactions with the world |
| a failing test is not reproducible | **the log is the reproduction**, and `beck replay` already exists |

The fourth row is the one nobody else can have, and §21.3 is what it buys.

---

## 21.2 `test` blocks: cross-boundary by construction

*Built ([`22`](22-phase-3-report.md) §22.2). Two pieces of notation below are not what shipped, and
are marked where they appear: `page(session(…))` and `fold_of` are **clauses**, not expressions
([`22`](22-phase-3-report.md) §22.5 item 1), and `when session("ana") sends` carries an actor name
rather than a `Session` expression (§22.5 item 2). Both open questions at the end of this section
are answered there.*

### The shape

A test names a log, an input, and an expectation. The log is the state, because state is a fold —
there is no fixture, no factory and no `setUp`.

```python
test "an empty todo is rejected":
    given []
    when Add(id="1", text="   ")
    expect Err(Empty)

test "adding a todo shows it on the page":
    given [Added(id="1", text="milk", owner="ana")]
    expect page contains "milk"

test "toggling twice is identity":                    # §11.10, unchanged
    given [Added(id="1", text="milk", owner="ana")]
    when Toggle(id="1"), Toggle(id="1")
    expect state == fold_of [Added(id="1", text="milk", owner="ana")]
```

> **As built.** `fold_of` is a clause, not a function: folding a log is what the data tier does, and
> the runner drives the program's real fold over it. `given` and `fold_of` both take an optional
> `by "actor"`, because the envelope's actor is what a fold reads as data
> ([`22`](22-phase-3-report.md) §22.5 item 1).

Four blocks, and each is a value in a type the program already declares:

* `given` — `list[Event]`. The events go through the *real* `apply_event`, so a test cannot
  construct a state the program could not reach. That is a stronger guarantee than a factory: a
  fixture can build an impossible object; a log cannot.
* `when` — a `Command`, or several, optionally with a session (below). Goes through the *real*
  `validate`, so authorisation is exercised rather than bypassed.
* `expect` — an assertion about `state`, the emitted `events`, a `Rejection`, or the rendered
  `page`. Each of those is a named thing in the program, so the assertion needs no plumbing.
* `stub` — §21.3.

### What makes it cross-boundary

The hardest tests to write in a conventional stack are the ones that span the boundary: a browser
sends something, a server decides, a database records it, and a *different* browser sees the
consequence. That test normally needs a running API, a real database, a headless browser, and a
tolerance for flakiness.

In Beck it is three lines, because the boundary is a placement of one graph rather than a seam
between two programs:

```python
test "one client's command reaches another client's page":
    given []
    when session("ana") sends Add(id="1", text="milk")
    expect page(session("bo")) contains "milk"
    expect page(session("ana")) contains "milk"
```

> **As built.** The notation is unchanged; what it *is* changed. `expect page(session(…)) contains …`
> is a clause, not an expression: rendering a page is `per_session(state, view)` applied — a role
> the runtime drives, not a function a test scope can bind. `session("ana")` likewise names an
> actor rather than constructing a `Session`, because §3.7 mints one from verified claims and a
> test that could build one out of an expression would be a way to forge one.
> [`22`](22-phase-3-report.md) §22.5 items 1–2.

`page(session)` is `per_session(state, view)` applied — the same function the server renders with.
No network is involved and none is being simulated: the test runs the same `Roles` the runtime
drives, with the tiers co-located. What it proves is what the boundary *means*, which is the
thing worth proving; what it does not prove is that the wire encoding round-trips, and that has its
own harness already (the differential and replay suites, §13.1).

The optimism story gets the same treatment, and it is the test people most want and least often
write:

```python
test "an optimistic client converges on the server's answer":
    given []
    when session("ana") optimistically sends Add(id="1", text="milk")
    expect page(session("ana")) contains "milk"     # before the server has seen it
    after settle:
        expect page(session("ana")) contains "milk" # …and after, with no flicker
```

`apply_event` is unplaced (`Tier::Any`) precisely so the client can apply an event with the same
code the server folds with — that is measured in the corpus today. This test is that property,
asserted by the person who depends on it.

### Assertions the type system already knows how to answer

§3.4 lists **assertability** as a non-negotiable guardrail and gives the example
`assert place(view, OrderPanel) == client`. Today that exists as `beck check --assert-place
page=client`, a CLI flag. It belongs beside the code:

```python
test "the key never reaches a browser":
    expect place(charge) == server
    expect flow(ApiKey) reaches nothing on client

test "v1 clients still work":
    expect wire_compatible_with "orders.v1.becki"
```

These are compile-time queries, not runtime assertions — `beck test` answers them without running
anything, from the same data `beck explain place` and `beck check --wire-compat` already produce.
That is the point of putting them in the language: a placement regression is caught by the same
command that catches a logic regression, and the reason for a placement gets to live next to the
code that depends on it.

### Determinism, and why these tests cannot flake

A test with no `stub` performs no effects: `given`/`when`/`expect` are pure functions of an explicit
log. There is no clock, no scheduler and no network for a flake to come from. When a test *does*
stub an effect, the stub is a value or a `match`, which is also deterministic. **A flaky Beck test
should be impossible**, and if one appears it is a compiler defect — which is a much better place
to be than "retry it three times".

### Open questions, stated rather than glossed

* **Where tests live.** In the module (a `test` block beside the code) reads best and makes the
  compiler's job harder: tests must be stripped by DCE, must not appear in `Roles`, and must not
  contribute to the published `.becki`. A separate `*.test.beck` compiled as a module that imports
  the one under test is simpler and worse to write. Recommendation: in the module, with the
  stripping asserted by a test of its own — the client-bundle search in `security.rs` is the
  template.
  **Answered: in the module**, and all three assertions exist — the interface digest is unmoved, the
  placement problem gains no node, the wire id does not shift, and the bundle search is the template
  it was said to be ([`22`](22-phase-3-report.md) §22.1).
* **Do test blocks have effect rows?** They must not: a test that performs a real `net.out` is a
  test that can fail because somebody else's server is down. A `test` block should be checked with
  an empty row, and any effect its subject performs must be discharged by a `stub`. That makes
  "you forgot to stub something" a **compile error naming the effect** rather than a hang.
  **Answered in two halves**, which pull in opposite directions and are both right: the test
  block's own row is checked empty (`B0700`, naming the effect), and the *subject's* effects are
  auto-stubbed per §21.3 rule 1 rather than demanded. So the error a person meets is "you wrote an
  effect into your test", not "you forgot to stub something"
  ([`22`](22-phase-3-report.md) §22.5 item 4).
* **Property tests** (`property "…" (events: list[Event])`, §11.10) need generated values, which is
  the same machinery §21.3 needs for stub returns. Build the generator once.
  **Built once**, in `beck-core/src/gen.rs`, and used by both.
* **Golden/snapshot assertions** for pages (`expect page matches snapshot`) need an update flow
  (`beck test --update`). The compiler's own suite uses `insta` for exactly this and it works;
  the risk is snapshot rot, and the mitigation is the same one — review the diff.
  **Built** ([`22`](22-phase-3-report.md)): `expect page matches snapshot`, and
  `beck test --update` to record one. Not with `insta`, which snapshots a Rust value from a Rust
  test — this is a Beck page from a Beck test, keyed by that test's name. The mitigation above is
  the design: a missing snapshot **fails** rather than writing itself, writing is only ever the
  flag, and the failure names the column so the diff is in the message.

---

## 21.3 Mocks nobody writes

*Built ([`22`](22-phase-3-report.md) §22.3), with two exceptions marked where they appear: rule 3
(`case` inside a stub) is not built, and rule 1's auto-stubbing excludes `nondet` and `cap.*` for
reasons §22.3 gives. This section also **understates one rule**: a stub replaces the definition that
*performs* an atom, not every definition whose row inherits it — see §22.3, because getting that
wrong replaces the authority chokepoint itself.*

### The complaint, and why it is a design smell rather than a tooling gap

> Writing the mocks became utterly tedious: a method with many parameters, type parameters on
> `Func`s and `Expression`s — a very long and boring block of boilerplate that just means
> "any value".

That is an accurate description of mocking in a language where **the unit of mocking is an
interface**. `It.IsAny<T>()` exists because the framework must be told the *shape* of a call it does
not care about, and the shape is a method signature with generics in it. The boilerplate is the
signature, restated.

Beck can do better not by having a cleverer mock library but by mocking a different thing:

> **A mock is not a stand-in for an object. It is a value for an effect.**

There is no `IOrderRepository` to mock, because persistence is not an interface somebody injected —
it is the `durable` fold, and in a test it is *real* and in memory. There is no clock to mock,
because time is `env.at`, data on the envelope. There is no id generator to mock, because ids are
minted at the edge (§3.7 F3). Those three are the bulk of real-world mock boilerplate, and Beck's
semantics **delete** them rather than easing them.

What is left is the genuinely external: `net.out(host)`, `env`, `external.read/write(store)`,
`fs.read/write(path)`, `cap.*`, `nondet`. A small, closed, *named* set — and the compiler already
computes exactly which of them any definition performs.

### Rule 1 — everything is stubbed by default, so "any value" has no syntax

*Built, with two atoms deliberately excluded from the automatic half: `nondet`, because ids and the
clock are already data at the edge and the harness supplies them deterministically, and `cap.*`,
because a capability is discharged by the chokepoint `when` exists to exercise. Both may still be
stubbed by name ([`22`](22-phase-3-report.md) §22.3).*

The tedious case is the one you do not care about, so it should cost nothing:

```python
test "an order is recorded":
    when Place(sku="milk", qty=2)
    expect events == [Placed(sku="milk", qty=2)]
```

`validate` here reaches a function with `net.out(payments.example.com)`. The test does not mention
it. It is stubbed automatically, returns the canonical value of its return type, and the call is
recorded. Nothing was declared because there was nothing to say: **"any value" is the default, so
it needs no expression**, and no parameter list has to be restated to reach it.

This is only safe because it is bounded: the *complete* list of what got stubbed is the effect row,
which the compiler knows, so `beck test -v` can print it —

```console
test "an order is recorded" … ok
  stubbed automatically:
    net.out(payments.example.com)  by `charge`   → Receipt(id="")   called once
```

— which is the thing a hidden default must always do: say what it did.

### Rule 2 — when you care, name the effect, not the shape

```python
test "a declined charge rejects the order":
    stub net.out(payments.example.com): Declined
    when Place(sku="milk", qty=2)
    expect Err(PaymentDeclined)
```

One line. No method name, because the effect atom *is* the identity. No parameter list, because
parameters are not how the stub is selected. No type arguments, because `net.out(host)` is
monomorphic and the value's type is inferred from the call site the same way any other expression's
is.

### Rule 3 — matching by value uses the language's own `match`, so there is no mock DSL

*Built ([`22`](22-phase-3-report.md) §22.3). Two things the notation below leaves implicit are
decided there: the arms match the **stubbed definition's parameter**, so the atom has to be
performed by exactly one definition for a body to have arguments to take (`B0707`); and a guard is
written as Beck's conditional expression, since `match` has no `if` clause. The general form — an
ordinary body with those parameters in scope — is what the `case` sugar is a case of.*

```python
test "large charges are declined":
    stub net.out(payments.example.com):
        case Charge(amount) if amount > 10000: Declined
        case _: Approved
    when Place(sku="yacht", qty=1)
    expect Err(PaymentDeclined)
```

`case`/guard is ordinary Beck pattern matching (§11). There is nothing to learn, nothing that
composes differently from the rest of the language, and no `Expression<Func<…>>` to satisfy —
because Beck has no expression-tree type and never needs one: the compiler already has the AST.

> **As built.** Against `def gateway(req: Request) -> Answer uses net.out(payments.example.com)`,
> the shipped form is:
>
> ```python
> stub net.out(payments.example.com):
>     case Charge(amount):
>         return Declined if amount > 10000 else Approved
>     case Refund(amount):
>         return Approved
> ```
>
> — the guard is the language's conditional rather than a `if` clause on `case`, because `match` has
> no such clause and giving the stub one would be the mock DSL this rule exists to avoid. A block of
> anything else is the general form: the stubbed definition's parameters in scope, any expression.

### Rule 4 — interaction assertions are queries over what happened, not expectations set in advance

Half of mock boilerplate is *arranging* an expectation and then *verifying* it, which states the
same fact twice and fails in the wrong place when it is wrong. Every effect performance in a test is
recorded, so verification is a query:

```python
    expect no net.out                                   # nothing left the process
    expect net.out(payments.example.com) once
    expect net.out(payments.example.com) with Charge(amount=2000)
```

Nothing had to be arranged for these to be answerable. A test that does not ask is not paying for
the recording either — the recording is per-test and discarded.

### Rule 5 — where a value must be invented, the type invents it

Stub return values, property-test inputs and `given` gaps are the same problem: produce an
inhabitant of a known type. The compiler has the full type, including `newtype`s, unions and
records, so it can derive:

* a **canonical** inhabitant (first variant, empty collection, zero, `""`) for the don't-care case;
* an **arbitrary** one, with shrinking, for `property` blocks;
* and it can refuse, with a diagnostic, for a type with no inhabitant it can construct — `secret[T]`
  being the interesting one, since inventing a secret in a test is exactly the sort of thing that
  should require somebody to type it out.

This is one generator, used by three features, and it is the piece to build first because §21.2's
property tests need it too.

### What this does *not* solve

* **A stub is still a lie.** If the payment provider returns a shape the program does not expect,
  a stub agrees with the program rather than with the provider. Effect-typed mocks make the lie
  *small and enumerated* — you can list every place the program touches the world, because the
  compiler does — but contract testing against a real provider is a different activity and Beck
  does not remove the need for it.
* **`external.read/write`** is the escape hatch (§3.2), and a stub for one is as good as the
  author's understanding of the legacy system behind it. That is the price of the escape hatch and
  it is already recorded as such.
* **Mode B and real browsers.** §21.2's cross-boundary tests run the tiers co-located. That proves
  what the boundary means, not that a particular browser renders it. Phase 3's Mode B will need a
  browser-in-the-loop harness, and it will be a different suite with a different failure budget.
  — ***Built*** ([`94`](94-the-client-report.md) §94.13): `beck-cli/tests/browser.rs` drives headless
  Chromium over the DevTools Protocol and asserts, in both modes, that the DOM is the page the
  server would have rendered. It found three defects nothing else could see. The failure budget is
  the one predicted: it skips without a browser, and every assertion about the page is a wait with
  a deadline rather than a read after a sleep. **One** browser and **one** page, which is the part
  still owed.

---

## 21.4 How we know a generated artefact is right: the ladder

*Rungs 1–5 are built as of Phase 2's follow-on work. Each is stated with what it buys and what it
cannot see.*

The artefact under discussion is the Kubernetes object graph, because that is the one the compiler
emits into somebody else's system. The same ladder applies to the wire format, the `.becki`
interfaces and the images.

| rung | mechanism | buys | blind to | built |
|---|---|---|---|---|
| 1 | **types** — objects built from `k8s-openapi` structs, generated from the API's own OpenAPI schema | a misspelled field, a missing required field or a wrong field type **does not compile** | anything about two objects agreeing; CRDs, which have no Rust type | ✅ |
| 2 | **round-trip** — emit with our writer, read back with a third-party YAML parser, compare | the document says what the object said; a quoting bug (`no` → `false`) | whether the object was right to begin with | ✅ |
| 3 | **cross-object invariants, by example** — 15 checks over the canonical program | a selector matching no pod, a dangling `secretKeyRef`, a route to a port nothing serves | anything the canonical program does not exercise | ✅ |
| 4 | **cross-object invariants, generated** — the same 15 checks over thousands of generated effect rows and adversarial module names | the property holds for programs nobody wrote; turns "these manifests are consistent" into "the emitter cannot produce inconsistent manifests" *for the properties we thought of* | a property nobody stated | ✅ |
| 5 | **conformance** — `kubectl apply --dry-run=server` against a real API server in CI | admissibility, decided by the only authority there is: admission chains, defaulting, webhooks, and the CRDs no Rust type covers | whether an admissible object *behaves* as intended | ✅ |
| 6 | **proof** — §21.5 | the properties hold for *all* inputs rather than the sampled ones | the same properties; a proof does not invent them | ✗ |

Two things this table is for.

**First, it says which rung each real defect needed.** Every defect in [`20`](20-phase-2-report.md)
§20.4 item 13 is placed:

| defect | found at | would rung 1 have caught it? |
|---|---|---|
| `net.out(host)` rendered as `podSelector: {app: host}` | 3 | no — a pod selector with a string in it is perfectly well-typed |
| no DNS egress rule at all | 3 | no — an absent rule is not a type error |
| policy admitted `gateway`, route used `gateway-system` | 3 | no — both are valid namespace names |
| two-digit file prefixes, so the 11th object sorted before the 2nd | 3 | no |
| the container read credentials from a Secret that a non-`durable` program never emits | **4** | no — and no example test could see it either, because every corpus program has a durable fold |
| a module name over 47 characters produced a 76-character Secret name | **4** | no — `String` has no length in its type |
| `kubectl apply -f <out>` submitted `image.apko.yaml` as an object | **5** | no |

Rung 1 caught none of them, and rung 1 is still worth having — it removes a class so completely that
there is nothing left to test. But **the defects that actually happen are cross-object**, and the
mechanism that finds them is a check that walks the objects, run over inputs nobody chose.

**Second, it says where the effort goes next**, and that is §21.5.

### The rung that is not on the ladder, and outranks all of them

Before reaching for a solver: the cheapest and strongest way to make an invariant hold is to
**collapse the two things that have to agree into one thing**. It is not verification; it is
deletion, and it takes the failure mode out of existence rather than detecting it:

| invariant | how it was enforced | how it is enforced now |
|---|---|---|
| a workload's `matchLabels` matches its own pod template | a test | one `pod_labels()` function, called twice |
| the route's backend port is the container's port | a test | one `APP_PORT` constant |
| the policy admits the namespace the route's gateway is in | a test (added after it was wrong) | one `GATEWAY_NAMESPACE` constant |
| the credentials' URL names the log store's Service | a test | one `log_url()` built from the same app name |

Each of those still has its test, because the collapse can be undone by a future edit and the test
is what says so. But the test is now a guard on a design rather than the design's only support. **Do
this first, every time, and reach for §21.5 only for what cannot be collapsed.**

---

## 21.5 Rung 6: what could actually be proved, and what could not

*Not built. This section is a plan and a set of recommendations, with costs.*

### 6a. Bounded verification of the invariants — recommended

The rung-4 property suite samples: 512 cases per run, more on demand. A bounded model checker
replaces "no counterexample in 8 000 samples" with "**no counterexample exists** for any effect row
of length ≤ N and any app name of length ≤ M". That is a genuine quantifier, and for this problem
the bound is not much of a limitation: the derivation is monotone in the effect row, so the
interesting behaviour is all at small N.

The tool is **Kani** (CBMC for Rust), and the reason it is the right one is that it verifies *the
actual Rust*, not a model of it. A model would be a second implementation to keep in sync, and a
proof about the wrong artefact is worse than a test about the right one.

```rust
#[kani::proof]
#[kani::unwind(8)]
fn no_effect_row_produces_an_inconsistent_manifest_set() {
    let effects: Vec<Effect> = kani::any_with(/* ≤ 5 atoms from a bounded alphabet */);
    let app: String = bounded_name();
    let g = derive(&sanitise(&app), &pairs(effects), kani::any());
    let objects = k8s::objects(&g, "id");
    assert!(invariants::all(&objects).is_ok());
}
```

Cost: days, not weeks — the invariants and the generator both exist, and the work is making the
inputs symbolic and getting the unwind bounds to terminate. `serde_json` in the loop is the risk;
the mitigation is to check the invariants against the typed objects rather than their JSON, which
is a refactor worth doing anyway.

Value: it closes the specific gap rung 4 leaves — that a property held for the inputs *sampled*.

### 6b. A verified serialiser — not recommended

Extracting the YAML writer from Lean or Coq, or annotating it with Verus, would prove
`parse(write(v)) == v`. It is a real proof of a real property.

It is also the property rung 2 already tests, on a hundred-line function, with a third-party parser
as the oracle. The proof would cost weeks and remove a risk that has produced no defects. **Effort
here is effort not spent on 6a**, which addresses the failure mode that has actually occurred seven
times. Recommend against, and record the reason so it is not revisited on aesthetics.

### 6c. Making inconsistency untypeable — recommended in its weak form only

The strongest possible statement is that an inconsistent manifest set does not typecheck. In a
dependently typed language that is natural: index the Service by the label set it selects and the
Deployment by the labels it produces, and a mismatch is a type error.

In Rust it is possible in fragments — newtypes for label maps and ports, a `Selector<L>` phantom —
and each fragment buys less than it costs in readability. The pragmatic version is the "collapse"
row above, which achieves the same *effect* for the cases that matter by making the two values one
value. Recommend: keep collapsing; do not build a phantom-type framework.

Worth recording for a later phase: if Beck's own type system grows refinement types (§9's open
questions), the emitter could eventually be written *in Beck*, and this stops being a Rust problem.
That is a Phase 5+ thought, not a plan.

### What cannot be proved, at any cost

A proof is about a specification. Three things here have no specification we own:

1. **That an admissible object behaves as intended.** We can prove the NetworkPolicy has the peers
   we meant. Whether those peers actually restrict the traffic we think depends on Kubernetes'
   semantics *and the CNI's implementation of them* — Cilium and Calico do not agree about
   everything. The only evidence is traffic in a real cluster, which is an end-to-end test, not a
   proof. **This is the honest limit on §3.5's "least-privilege infra, computed" claim**, and
   [`06`](06-kubernetes-and-packaging.md) §6.5.1 now states it.
2. **That the properties are the right properties.** Every rung, proof included, checks what
   somebody thought of. The seven defects above were each invisible to every mechanism that
   existed at the time. What finds unstated properties is a different kind of activity: adversarial
   review, and production.
3. **That the cluster is the one we compiled against.** The emitter targets `v1_34` types; the
   cluster is whatever it is. Rung 5 checks that against *a* cluster in CI. Nothing checks it
   against yours.

### The recommendation, in order

1. Keep collapsing invariants into single definitions (free, ongoing, highest value).
2. **6a — Kani over the invariants**, once the invariants read the typed objects rather than JSON.
3. An end-to-end cluster test that asserts traffic is actually blocked, if and when §3.5's
   least-privilege claim is being sold rather than described. That is evidence for the one thing no
   proof reaches.
4. Not 6b.

---

## 21.6 What is built, as of this writing

| | status |
|---|---|
| §21.2 `test` blocks in Beck | **built** ([`22`](22-phase-3-report.md)): `beck test` runs a program's own `test` and `property` blocks — and `beck test --update` with it ([`22`](22-phase-3-report.md)), so nothing in §21.2 is outstanding |
| §21.3 inferred mocks | **built** ([`22`](22-phase-3-report.md)): a stub is a value for an effect atom, defaulted from the atom's return type by the one type-directed generator `property` blocks share |
| §21.4 rung 1 — typed objects | built (`beck-infra/src/k8s.rs`) |
| §21.4 rung 2 — YAML round-trip | built (`beck-infra/tests/invariants`, read back with `serde_norway`) |
| §21.4 rung 3 — invariants by example | built (`beck-infra/tests/manifests.rs`, 15 checks) |
| §21.4 rung 4 — invariants over generated graphs | built (`beck-infra/tests/manifest_properties.rs`) |
| §21.4 rung 5 — conformance against an API server | **written; proved by CI, not by hand.** The harness skips without a cluster and fails without one when `BECK_REQUIRE_CLUSTER=1`; the `conformance` job in `.github/workflows/compiler.yml` sets it. It has never been run on a developer machine, because the container this was written in has no container runtime — so the first real run is CI's, and that is stated here rather than implied |
| §21.5 rung 6 — proof | not built; §21.5 is the plan |
