# 22 — Phase 3 report, part 1: tests written in Beck

Phase 3 of [`08-roadmap.md`](08-roadmap.md) is headed **"Make it real for developers."** It is
twelve bullets wide — LLVM, incremental views, Mode B, client polish, `test` blocks, structured
concurrency, SQLite, a standard library, identity, an LSP, the playground, and supply-chain
tooling — and it is the longest phase in the plan.

**One of those twelve is built. Eleven are not, and §22.6 lists them by name.** This report covers
the one, and the title says "part 1" so that nothing here can be read as a claim about the phase.

The one is the item [`20`](20-phase-2-report.md) §20.5 singles out:

> **No `test` construct in the language.** … Every harness in this report tests *the compiler*; a
> person writing a Beck program has `beck check --assert-place`, `beck replay --verify`, and no way
> to write a test about their own code. This is the largest gap between what Phase 2 built and what
> an outside developer will reach for first, and [`21`](21-tests-in-beck-and-proof.md) §21.2–§21.3
> is the design that Phase 3 should build.

It is built. `beck test examples/todo.beck` runs eight tests written in Beck, in about 10 ms, with
no network, no database, no fixture and no mock — including one that asserts a command from one
browser reaches a *different* browser's page, and one that runs a hundred generated event logs
through the real fold. §21.3's "mocks nobody writes" is built too: 100% of the effects in a program
are stubbed by default, and the report says what it stubbed.

Phase 3 also leaves Phase 2's named debt unpaid. §20.5's first item, the general slicer, was
assigned to Phase 2, not built there, and is **still not built**. §22.6 says so first rather than
last.

## 22.1 What was built, against what was asked

| §21 asks for | Status | Where |
|---|---|---|
| `test` blocks with `given`/`when`/`expect` (§21.2) | done | `beck-syntax/src/parser.rs`, `beck-core/src/{testing,check}.rs` |
| …cross-boundary: one client's command, another client's page | done — `expect page(session("bo")) contains …` | `beck-rt/src/testing.rs` |
| …assertions the type system already knows: `place`, `flow`, wire compatibility | done — answered without executing anything | `beck-rt/src/testing.rs` |
| …determinism: "a flaky Beck test should be impossible" | done — the envelope's `at` is the sequence position, the actor is written in the test, the generator is seeded from the test's name | `beck-rt/src/testing.rs`, `beck-core/src/gen.rs` |
| …tests live in the module, with the stripping asserted (§21.2's open question) | done — three assertions: not in the `.becki`, not in the placement, not in the bundle | `beck-cli/tests/tests_in_beck.rs` |
| …a test block's row must be empty (§21.2's open question) | done — `B0700`, naming the effect | `beck-core/src/check.rs` |
| `stub <atom>: <value>` — name the effect, not the shape (§21.3 rule 2) | done | `beck-core/src/check.rs` |
| Everything stubbed by default; "any value" has no syntax (rule 1) | done, **and it says what it did** | `beck-rt/src/testing.rs` |
| Matching stub arguments with `case`/guards (rule 3) | done — the arms match the stubbed definition's parameter, and a block of anything else is an ordinary body with its parameters in scope | `beck-core/src/check.rs`, `beck-rt/src/testing.rs` |
| Interaction assertions as queries: `expect no net.out`, `… once`, `… with` (rule 4) | done | `beck-rt/src/testing.rs` |
| One type-directed generator for stubs, properties and gaps (rule 5) | done — canonical, arbitrary, shrinking, and a refusal for `secret[T]` | `beck-core/src/gen.rs` |
| `property` blocks with generated inputs (§11.10, §21.2) | done — 100 inputs, shrunk counterexample | `beck-core/src/gen.rs`, `beck-rt/src/testing.rs` |
| `beck test --update` for page snapshots (§21.2's open question) | **not done** — §22.6 | — |
| A `Backend` seam that can install stubs (**not** a §21 item) | added — `Backend::intercepting`, defaulted to "cannot", so a backend that cannot is *skipped* rather than silently run for real | `beck-core/src/backend.rs` |
| `beck test` as a command, and as a CI gate (§8.3) | done, and the gate was executed by hand before being trusted | `beck-cli/src/main.rs`, `.github/workflows/compiler.yml` |

396 tests, no failures, no compiler warnings, no clippy warnings — up from Phase 2's 340.

## 22.2 The construct, as it is actually written

This is the sketch's own test suite, checked in at the bottom of
[`examples/todo.beck`](../compiler/examples/todo.beck) and run by CI:

```python
test "an empty todo is rejected":
    given []
    when Add(id=Id("1"), text="   ")
    expect Err(error=BlankText)

test "adding a todo shows it on the page":
    given [Added(id=Id("1"), text="milk")] by "ana"
    expect page(session("ana")) contains "milk"

test "toggling twice is identity":
    given [Added(id=Id("1"), text="milk")] by "ana"
    when session("ana") sends Toggle(id=Id("1")), Toggle(id=Id("1"))
    expect state == fold_of [Added(id=Id("1"), text="milk")] by "ana"

test "a todo is nobody else's to toggle":
    given [Added(id=Id("1"), text="milk")] by "ana"
    when session("bo") sends Toggle(id=Id("1"))
    expect Err(error=NotOwner)
    expect page(session("bo")) contains "0 remaining"

test "the page is the browser's job and the fold is the data tier's":
    expect place(page) == client
    expect place(todos) == data
    expect place(apply_event) == any

property "no log the program can produce makes the page unrenderable"(log: list[Event]):
    given log
    expect page contains "remaining"
```

Four things in that listing are worth pointing at, because each is a claim §21.1 makes and each is
now mechanised rather than argued.

**There is no fixture.** `given` is a `list[Event]` and it goes through the *real* `apply_event`,
with the envelope the sequencer would have built. §21.1: "a fixture can build an impossible object;
a log cannot" — and the type system enforces the first half, since `given [1, 2, 3]` is a type
error against the program's own `Event` union.

**There is no network.** `page(session("bo"))` is `per_session(state, view)` applied — the same
function the server renders with, called through the same `Roles` the runtime drives. The
cross-boundary test is four lines because the boundary is a placement of one graph, not a seam
between two programs.

**There is no clock and no id source.** The envelope's `at` is the sequence position and its `actor`
is written in the test, so two runs produce the same state bit for bit. `beck test` run twice
produces byte-identical output, and a `property` that fails on input 37 fails on input 37 on any
machine.

**The placement assertions cost nothing to run.** §3.4 lists assertability as a non-negotiable
guardrail and gives `assert place(view, OrderPanel) == client` as the example; Phase 2 built it as a
CLI flag. It now lives beside the code, and it is answered from the same data `beck explain place`
prints, with no execution at all. The stronger version of that claim: **strip every `@on(...)` from
the sketch and the same test still passes**, because the solver lands on the same placement — which
is the whole point of putting the assertion in the program.

## 22.3 Mocks nobody writes, measured

[`compiler/crates/beck-cli/tests/tests_in_beck.rs`](../compiler/crates/beck-cli/tests/tests_in_beck.rs)
carries a small program with one genuinely external call: `charge(qty) -> Answer uses
net.out(payments.example.com)`, reached from `validate`. Here is what `beck test --verbose` prints
for a test that says nothing at all about payments:

```console
test "an order is recorded" … ok
  stubbed:
    net.out(payments.example.com) by `charge`  → Approved   called 1× (automatically)
```

That is §21.3 rules 1 and 5 working together: nothing was declared, because there was nothing to
say; the return value is the canonical inhabitant of `Answer`, which the compiler derived from the
type; and the hidden default declared itself, which is the thing a hidden default must always do.

Naming it is one line, with no method name, no parameter list and no type arguments:

```python
test "a declined charge rejects the order":
    stub net.out(payments.example.com): Declined
    when Place(sku=Sku("milk"), qty=2)
    expect Err(error=PaymentDeclined)
```

And verification is a query over what happened rather than an expectation arranged in advance:

```python
    expect no net.out(payments.example.com)
    expect net.out(payments.example.com) once
    expect net.out(payments.example.com) with 2
```

### Answering from the call, without a mock DSL

§21.3 rule 3 asks for a stub that matches on what it was called with. The stubbed definition's
parameters come into scope under their own names, so a stub is written the way the definition is
read — and `match`, `if` and every other expression in the language work inside it. Against
`def gateway(req: Request) -> Answer uses net.out(payments.example.com)`:

```python
test "large charges are declined":
    stub net.out(payments.example.com):
        case Charge(amount):
            return Declined if amount > 10000 else Approved
        case Refund(amount):
            return Approved
    when Place(sku=Sku("yacht"), qty=3)
    expect Err(error=PaymentDeclined)
    expect net.out(payments.example.com) with Charge(amount=15000)
```

Bare `case` arms are sugar: there is no scrutinee written because only the compiler knows what
performs the effect, and therefore what its argument is. The general form is a block of anything —

```python
    stub net.out(payments.example.com):
        return Declined if amount > 10000 else Approved
```

— which is what makes this "the language's own `match`" rather than a mock language. Two things the
design leaves implicit are decided here, both as diagnostics rather than as guesses (`B0707`):

* **A body needs one definition to take arguments from.** Two definitions can share a stub *value*,
  because a value looks at nothing; they cannot share a body, because a body names parameters and
  there is no reason theirs agree. The fix is the one the effect vocabulary already offers, and the
  diagnostic says it: give the one you mean its own atom — a second host, a second store.
* **Bare `case` arms need one argument to be about.** A definition taking two is told to write the
  `match` out.

A stub body is *test code*: it is checked in the test's own effect scope, so a stub that tried to
perform something is `B0700` like any other expression in a test, and it is prepared by the base
backend rather than the intercepting one — stubbing a stub would be a loop.

### The rule §21.3 does not state, and without which the whole thing is a lie

§21.3 says "a mock is … a value for an effect", and leaves *which definition* the value replaces to
be inferred. The obvious reading — every definition whose effect row contains the atom — is wrong,
and wrong in the worst available way.

An effect row **propagates to callers**. `validate` calls `charge`, so `validate`'s inferred row
contains `net.out(payments.example.com)` too, and so does everything that calls `validate`. Stubbing
by row would therefore replace the authority chokepoint itself with a canned `Result`, and §21.2's
central claim for `when` — "goes through the *real* `validate`, so authorisation is exercised rather
than bypassed" — would be false of every program that talks to anything. The test would pass while
exercising nothing.

The rule that is built is: **a stub replaces the definition that *performs* the atom, not the ones
that inherit it.** A definition performs an atom itself when it declares it in a `uses` clause
(§3.6's clause is the only way to introduce a non-primitive effect) or when its own body applies a
primitive carrying it; everything else in the row arrived from a callee, and the callee is where the
stub belongs. `beck_core::testing::performs_itself` is the predicate, and
`a_stub_replaces_what_performs_the_effect_not_everything_that_inherits_it` is the test that pins it.

The defect was found by running the design as written: the first version keyed on the row, and the
first program with a payment call reported that `charge` and `validate` "perform the same effect
with different return types". That diagnostic was correct about a rule that was wrong.

### Two atoms are deliberately not stubbed

§21.3's list of "the genuinely external" is `net.out(host)`, `env`, `external.read/write(store)`,
`fs(path)`, `cap.*`, `nondet`. Two of those are not auto-stubbed here, and the reasons are the
design working rather than the implementation shirking:

* **`nondet`** — ids and the clock are supplied deterministically by the harness, because §3.7
  already makes them data at the edge. A stub would be a second answer to a solved problem.
* **`cap.*`** — a capability is discharged by the authority chokepoint, which is the thing `when` is
  supposed to exercise. An explicit `stub cap.x:` is accepted, because saying it out loud is the
  point; nothing is stubbed automatically.

`durable` and `ingress` are not stubbable at all, and the diagnostic says why: "the clock is data on
the envelope, ids are minted at the edge, and the durable fold is real and in memory". §21.3's claim
that Beck *deletes* the bulk of real-world mock boilerplate rather than easing it is that sentence,
enforced.

## 22.4 The seam, and the failure mode it exists to prevent

Stubbing needs to replace a call and record its arguments. Doing that inside the evaluator would
have made `beck test` an evaluator feature and the native backend a rewrite, which is exactly what
[`19`](19-phase-1-report.md) §19.8 created `beck_core::backend::Backend` to prevent. So the seam
grew one defaulted method:

```rust
fn intercepting(&self, _by: Arc<dyn Interceptor>) -> Option<Arc<dyn Backend>> { None }
```

Defaulted rather than required, because installing stubs is not part of *executing a program* and a
backend that only ever runs an application in production has no reason to carry it.

The default is `None`, and what the runner does with `None` is the point. It does **not** fall back
to running the real thing: it reports the test as *skipped*, with the reason. A harness that ran a
payment call for real and printed "ok" would be the one outcome a test runner must never produce,
and `a_backend_that_cannot_stub_skips_rather_than_running_the_real_thing` pins that. Its sibling —
`a_program_with_no_effects_needs_no_interceptor_at_all` — runs the whole todo suite on a backend
that returns `None`, so the seam stays an addition rather than a requirement.

Everything else the runner needs goes through the existing seam. Expectations are evaluated by
wrapping the expression as a `Core` lambda over `state`, `events` and `result` and asking
`Backend::function` for it, so there is no second execution path and no `Value::Closure` — the
tree-walker's representation — anywhere in the runner.

## 22.5 The corrections Phase 3 makes to [`21`](21-tests-in-beck-and-proof.md)

§21.2 and §21.3 were written as design, and building them moved five things. Each is applied to that
document in this commit; they are listed here so the diff is reviewable as a set.

1. **`page(session("bo"))` and `state == fold_of [ … ]` are clause syntax, not expressions.** §21.2
   writes them as if `page` and `fold_of` were functions a test scope binds. They are not, and
   making them so would have required binding *closures* into a test's environment — which means
   `Value::Closure`, which is the tree-walker's representation and would have tied `beck test` to
   one backend. Rendering a page is a role the runtime drives; folding a log is what the data tier
   does. Both are now forms the runner executes, and `state`, `events` and `result` — the three
   names a test's expressions *do* bind — are plain data every backend can hold.
2. **`when session("ana") sends …` takes an actor, not a `Session` expression.** §3.7 says a
   `Session` is minted by the identity subsystem with verified claims mapped to typed capabilities.
   A test that could build one out of an arbitrary expression would be a way to forge one, so the
   surface keeps §21.2's notation and the AST holds a string.
3. **A stub replaces what performs an effect, not what inherits it** — §22.3. This is the
   load-bearing rule and §21.3 does not state it.
4. **§21.2's two open questions about effects resolve in opposite directions, and both are right.**
   §21.2 says a missing stub should be "a compile error naming the effect"; §21.3 rule 1 says
   everything is stubbed by default so that "any value" needs no expression. Rule 1 wins for the
   *subject*, and §21.2 wins for the *test block itself*: an expression inside a test is checked
   with an empty row and `B0700` names the effect if it is not. So "you forgot to stub something" is
   not the common error — "you wrote an effect into your test" is.
5. **A test body admits clauses and nothing else** (`B0705`). §21.2's examples contain no local
   bindings, and admitting them would have meant answering what a binding means across a `when` that
   changes the state under it. Refusing is the smaller claim, and the diagnostic says what a test is
   instead of what it is not.

## 22.6 What Phase 3 is not

**This is one bullet of twelve.** Nothing below has been started, and the roadmap's Phase 3 exit
criterion — "an outside developer builds a non-trivial app from documentation alone" — is not met
and is not close.

- **The general slicer is still not built**, and it is now debt with two phases' names on it.
  [`19`](19-phase-1-report.md) §19.9 assigned it to Phase 2; [`20`](20-phase-2-report.md) §20.5
  named it as the one item that phase was handed and did not deliver, and §20.6 item 6 said "Phase 3
  pays for it either way". Phase 3 has not. The splitter still understands one topology and still
  refuses anything else by name rather than mis-slicing it, which is what keeps the narrowness
  survivable — but §5.3's incremental views need the graph treated as a graph, so nothing downstream
  can start until this is done. **It should be the next thing built.**
- **No LLVM backend, and still no native codegen at all.** Unchanged from Phase 1 and Phase 2: a
  `Core` evaluator stands in for Cranelift behind the `Backend` seam. Phase 3 asks for a *second*
  backend and a differential test between them; there is still one.
- **No incremental views.** No differential-dataflow plans, no arrangement sharing, no SQL read
  models, no pgwire, no query fusion. Views are still full recompute per event, which is
  semantically final and operationally what §5.3 exists to fix. `beck explain query` and `beck
  explain cost` remain unbuilt for the same reason [`20`](20-phase-2-report.md) §20.5 gave.
- **No Mode B.** No per-component WASM, no optimistic application with `seq` reconciliation, no
  freshness-typed pending state, no per-component size budget. [`20`](20-phase-2-report.md) §20.6
  item 1 says the cost model is ready for the Mode A/B decision and cannot make it; it still cannot,
  because there is still only one lowering to choose between.
- **No client polish**: no router, no forms, no lazy routes, no focus/scroll preservation, no
  devtools extension.
- **No `test --update`, and no page snapshots.** §21.2 lists golden assertions as an open question
  with a known answer (`insta`'s update flow, which the compiler's own suite already uses); a page
  assertion is `contains` and nothing else today.
- **A stub cannot vary with the call *sequence*.** Rule 3 makes a stub a function of the
  arguments, and nothing more: there is no "fail the first time, succeed the second", because a
  stub body is checked with an empty effect row and therefore cannot hold state. Retry logic is
  testable only through the arguments it passes. Whether that is a limit or the point is a real
  question — a stateful stub is a small step from a mock framework — and it is recorded as open
  rather than decided.
- **A stub cannot name a definition, and cannot replace a pure one.** The unit is the effect atom,
  so `stub charge: …` is `B0702`. That is §21.3's position rather than an omission — the granularity
  of an interface-mock is what makes mocking tedious — but it means two definitions performing one
  atom cannot be told apart except by splitting the atom.
- **Interception is only at a direct call of a named definition.** A function passed as a value has
  lost its name by the time it is applied, so a program that stores its payment gateway in a record
  and calls it through a field would not be stubbed. Nothing in the corpus does this; the limit is
  recorded rather than left to be discovered, and the fix — naming closures at their binding site —
  belongs with the general slicer's work on the graph.
- **`beck test` needs a program the splitter accepts.** A library module — a policy, a domain, the
  thing §3.6's separate compilation exists to serve — cannot have its tests run, because there is no
  `Roles` to drive and the runner is built on `Placed`. Even a test that only asks compile-time
  questions needs the program to slice. `B0706` explains the checker's half of this; the runner's
  half is a real gap for exactly the modules that most want unit tests.
- **No parallelism and no test-level isolation beyond the value level.** Cases run in sequence.
  At 10 ms for eight tests plus a hundred generated inputs this is not yet a problem, and saying so
  is different from claiming it scales.
- **No structured concurrency, no `Result`/error rows, no `SQLite` substrate, no standard library
  v1, no identity beyond a dev-mode actor, no LSP, no playground, no `beck init ci`, no in-process
  apko, no cosign, no SBOM.** All Phase 3 bullets, all untouched.
- **`check.rs` grew from 2,166 lines to 2,644 (+22%).** [`19`](19-phase-1-report.md) §19.9 said to
  watch it and [`20`](20-phase-2-report.md) §20.5 repeated the number; this is the third phase in a
  row it has grown by about a fifth. The test-checking pass is a genuinely separate concern — it
  runs *after* the item loop, because a clause's type depends on the whole signal graph — and it is
  the first thing in that file with a natural seam around it. The next phase to open `check.rs`
  should move it out rather than adding to it.

## 22.7 What this changes for the rest of Phase 3

1. **The generator is built, so two later features are cheaper than they look.** §21.3 rule 5 said
   to build it once because three features need it; two of the three (stub returns, `property`
   inputs) use it today, and the third — filling `given` gaps — is a surface question, not a new
   mechanism. §13's DST work and the log-backed property tests of Phase 4 inherit it.
2. **`beck test` is the acceptance test's instrument.** Phase 3's exit criterion is "an outside
   developer builds a non-trivial app from documentation alone, without asking the team a question."
   The first question they would have asked is now answerable by a command. Every remaining bullet
   should ship with tests written in Beck, in the corpus, rather than only with Rust tests about the
   compiler.
3. **The corpus can now assert behaviour, not only placement.**
   [`compiler/corpus/`](../compiler/corpus/) was 23 programs measuring one thing: whether placement
   is inferred without annotations. Two of them now also say what they *do*, in Beck, and
   `every_test_written_in_a_corpus_program_passes` is a gate. A program added to the corpus can
   carry its own meaning with it.
4. **The `Interceptor` seam is where a fault injector goes.** [`13`](13-testing.md)'s deterministic
   simulation testing needs exactly this shape — replace a call, record it, decide per call — and it
   now exists behind the backend interface rather than inside one evaluator.
5. **A test block is a fourth thing the module holds, and nothing downstream knows about it.** The
   `.becki` interface, the placement problem and the wire id are all provably unchanged by adding
   tests, each asserted. That is what makes "tests live in the module" (§21.2's recommendation, now
   taken) safe to keep.

## 22.8 The gate, executed before it was trusted

[`20`](20-phase-2-report.md) §20.6 item 5: "A workflow is an artefact. §8.3's 'every phase ships a
demo that runs' should be read as applying to the thing that runs the demo. Phase 3 should start by
executing its own gates once by hand."

Done, and in that order. The new CI step —

```yaml
      - name: the programs' own tests pass
        run: |
          ./target/release/beck test examples/todo.beck --verbose
          for f in corpus/*.beck; do ./target/release/beck test "$f"; done
          # A failing test has to fail the build, or the gate is decorative.
          …
          ! ./target/release/beck test /tmp/failing.beck
```

— was run line by line against a release build before it was committed, including the last two
lines, which are the ones that matter: a gate that cannot fail is a gate that is not there. The
workflow file was also parsed, because [`20`](20-phase-2-report.md) §20.4 item 8 records a Phase 1
workflow that was invalid YAML from the day it was written and therefore silently absent for a whole
phase.

The other steps in that job were re-run by hand too, because
[`examples/todo.beck`](../compiler/examples/todo.beck) changed: `beck check`, both directions of the
`beck fmt` round-trip, the annotation-free placement assertions, `beck iface` with `--wire-compat`,
and `beck build`. The S-expression surface carries the test blocks as faithfully as the Python one,
and `beck test` on the printed `.sx` file runs the same eight tests — which is §2.2's dual-surface
claim reaching a construct that did not exist when it was made.

## 22.9 The corrections Phase 3 makes to the design documents

| Document | Correction |
|---|---|
| [`21`](21-tests-in-beck-and-proof.md) §21.2 | `page(session(…))` and `fold_of` are clauses, not expressions; `when session("ana") sends` carries an actor, not a `Session`; the two open questions about rows and about where tests live are answered (§22.5) |
| [`21`](21-tests-in-beck-and-proof.md) §21.3 | A stub replaces what *performs* an atom, not what inherits it; `nondet` and `cap.*` are not auto-stubbed; rule 3's arms match the stubbed definition's parameter, and its guard is the language's conditional rather than a clause on `case` (§22.3) |
| [`08`](08-roadmap.md) | Phase 3's `test` bullet marked built, with this report as its evidence — and with the eleven bullets it does not cover named in the same breath |
