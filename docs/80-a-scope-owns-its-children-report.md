# 80 — Phase 3, part 49: a scope owns its children

**Built.** `parallel:` — a scope whose bindings are its children, whose tail runs after the join,
and whose claim is not that its children are fast but that **its answer does not depend on which
of them ran first**. Two rules hold that claim up and both are diagnostics rather than conventions.

This is [`08`](08-roadmap.md) §8.5.4's Wave 1 concurrency half, which that wave deliberately left
behind — "the error half is what Wave 2 waits on, and the concurrency half waits on nothing, so
pairing them would have meant designing a concurrency model inside an error model's change" — and
which Wave 4 has listed since as free-standing with no predecessor. `spawn` has been an effect atom
since Phase 2 with nothing in the language able to perform one.

Say plainly what is and is not here. The form is built, checked, published and placed. **Nothing
runs two children at the same time**: the tree-walker runs them in the order they are written, which
is a correct implementation of a form whose meaning is that the order is nobody's business, and
§80.5 is what would have to change for a backend to exploit it.

## 80.1 The shape, taken rather than invented

[`38`](38-literature-survey.md) §38.4 is prescriptive and this report is mostly it, built:

> The cross-language consensus … is: a scope owns its children, and errors and cancellation join at
> the scope. The effect-system literature says how Beck should express that: `spawn`/`await` as
> effect operations, the scope as their handler … at which point derived mocks, slicing and
> placement apply to concurrency with no new machinery, the same "desugar, don't extend the IR"
> trick.

```beck
def screen(email: Str) -> Screening:
    return parallel:
        f = fraud_score(email)
        r = reputation_score(email)
        Screening(fraud=f, reputation=r)
```

The children are the scope's `let`s. The tail is everything after the last of them, and it runs
once, after the join, with every child's result in scope. That is the whole notation.

It is a **form**, so it is lexically scoped by construction — the same argument
[`27`](27-the-walls-come-down-report.md) §27.7 made for `try:`, and §38.4's sharper Beck-specific version of
it: in a language where effects decide placement, an accidentally intercepted effect would be an
accidental *re-placement*. A nursery whose membership were decided at run time would be the dynamic
handler search POPL 2019 argues against.

The one place this goes further than §38.4's sketch is that `spawn` and `await` are **not separately
reachable**. The scope desugars to a single node —

```text
Prim::Parallel(thunk₁, …, thunkₙ, continuation)
```

— the children's bodies as nullary lambdas and the tail as one lambda taking their results. There is
no handle type, so a child cannot outlive its scope, and the only thing in the language that can
read a child's result is the lambda the scope itself built. Structured concurrency's central rule is
usually a discipline a nursery API enforces at run time; here it is the shape of one IR node.

**No IR node and one evaluator case.** [`27`](27-the-walls-come-down-report.md) §27.1 item 3 and
[`27`](27-the-walls-come-down-report.md) each cost neither, which is one better; this one needs a case because a
scope is the only thing in the language that runs several expressions and then a fourth, and
`CoreKind` has no node for that. What it does *not* need is a `Task` type, a handle, a scheduler or
a runtime — which is the part §38.4 said to aim for.

## 80.2 The two rules

The claim — the scope's answer is the same whichever order the children ran in — is worth exactly as
much as what enforces it.

**No child can name another.** Each initialiser is checked before any of the names is bound, so a
reference to a sibling does not resolve. What makes it a diagnostic rather than a puzzle is that the
checker remembers the names it has not yet bound:

```text
error[B0398]: `a` is another child of this `parallel:` scope, so it has not run yet — children
              cannot see each other, which is what lets them run together
  --> sib.beck:4:13
  |
4 |         b = a * 2
  |             ^
```

Without that list the message is `B0340`, "cannot find `a` in this scope", which is true and tells
the reader nothing. A child that could read a sibling would have to run second, and then it is not a
child but a next line — which is the answer to "why not schedule it": a dataflow scheduler would
make the *shape* of the concurrency a thing you have to work out by reading, and this form exists to
make it a thing you can see.

**No child may perform an effect another child could observe.** The list is the atoms that write
state the program or its own substrate holds:

| Refused in a child | Why |
|---|---|
| `durable` | two children appending in the other order is a different log, and §3.7 makes the log the only description of a program's history |
| `ingress` | §3.7: "there is exactly one of these" |
| `dom` | two writers to one document |
| `external.write(store)` | somebody else's database, but one this program is ordering |
| `fs(path)` | see below |

What is *absent* is as much of the argument as what is present. **`net.out(host)` is not on the
list**, deliberately: a remote host's state was never Beck's to order, §3.2 never claimed it was, and
two outbound calls are the case the whole form exists for — a rule that refused them would leave it
with nothing to do. **`nondet`** is not on it either: a clock or a fresh id is a *read* of something
outside the program, two children reading the clock do not interfere, and they already disagree run
to run. **`raises(E)`** and **`partial`** are control flow, ordered by the join (§80.3).
**`cap.*`** is an authority the caller holds, not state a child writes.

`fs(path)` is on the list for a reason worth recording rather than assuming, and it is a **finding**:
the atom does not distinguish a read from a write. Refusing concurrent writes therefore means
refusing the pair, so two children reading two files is something this form should allow and cannot.
That is a fact about the effect vocabulary rather than about concurrency: `fs(path)` is the only
atom in §3.2's list that names a resource without saying what is being done to it, and this is the
first thing that has needed the distinction. The shape of the answer is already in the list —
§3.8's escape hatches are `external.read(store)` and `external.write(store)`, two atoms for one
resource, and they were split for the same reason. Doing it to `fs` is
[`10`](10-decisions.md)'s to take rather than a feature's, because §6.5 derives a volume's mount
options from the same atom; it is named in §80.7 and not taken here.

## 80.3 Failure joins at the scope, and the join decides which

§38.4: "cancellation is the error row crossing the scope." Nothing was built for that either — a
child's row is the scope's row, so a fallible child makes the scope fallible and an enclosing `try:`
catches at the scope:

```beck
def both(x: Int, y: Int) -> Result[Int, Refusal]:
    return try:
        parallel:
            a = bad(x)
            b = bad(y)
            a + b
```

The interesting half is *which* failure, when two children could raise. An unordered join answers
"whichever finished first", which makes a program's result a function of a scheduler. The ordered
join answers **the earliest child in the order it is written**, so the answer is a function of the
program:

```text
both(2, 1) == Err(Second)     # child one raises `Second`; child two raises `First`
```

That is a test rather than a sentence
(`concurrency.rs::a_childs_failure_joins_at_the_scope_and_the_earliest_child_wins`), and it is the
property a backend that genuinely runs children together would have to keep: it may start them in
any order and finish them in any order, and it must report the failure of the earliest *written*
child that has one.

## 80.4 Everything downstream was already there

This is the part worth reporting, because it is the argument for rows.

| | Written for this | What it does |
|---|---|---|
| Inference | no | a caller of a spawning function spawns, and nobody wrote it down |
| `.becki` | no | `def screen(email: Str) -> Screening uses net.out(fraud.example.com), net.out(reputation.example.com), spawn` |
| Placement | no | §3.3's table says `server` discharges `spawn` and `client` does not, so `@on(server)` is solved rather than annotated |
| `--wire-compat` | no | a function that starts spawning is **breaking**, in the sentence §4.3 wrote for `net.out` |
| `uses` | no | a signature that does not declare `spawn` may not spawn (`B0370`) |
| Derived mocks | see §80.6 | a child is stubbed by what it *does*, one child at a time |

Not one of those is a special case, and none of them is a line in this change. The one thing that is
a line is that the scope charges `spawn` itself rather than inheriting it from what its children do:
a scope over two pure children still cannot run in a patch interpreter, so the atom belongs to the
form.

```text
error[B0401]: `both` is placed on `client`, which cannot discharge `spawn`
```

`corpus/30-parallel.beck` is the program that carries the whole of this in a file with no
annotations in it.

## 80.5 What is not built, and why

**Nothing runs two children at the same time.** *(Corrected: [`117`](117-a-scope-runs-its-children-report.md) runs them on a thread each, and [`118`](118-a-scope-stops-its-children-report.md) stops the ones after a failure. What follows is the analysis of why it had not been done, which held up — including that the `Host` trait would have to become thread-safe, which [`116`](116-the-host-answers-back-report.md) did for an unrelated reason.)* The tree-walker applies each child's thunk in order
and then the continuation. That is correct — the form's meaning is that the order is unobservable,
and one order is an order — but it is not what somebody reading the word `parallel` will assume, so
it is the first thing this report says and it is said again here.

What stands in the way is not the design. `Interp` holds its fuel in a `std::cell::Cell` and its
host behind a `&dyn Host` that is not `Sync`, so two children need two interpreters and a shared
budget, and the `Host` trait — which test harnesses, the runtime and the LSP all implement — would
have to become thread-safe. That is a change to the *execution* half of the seam
[`19`](19-phase-1-report.md) §19.9 exists to keep load-bearing, and it wants its own measurement:
two children of a scope are worth running together exactly when each costs more than a thread, which
is a number nobody here has.

The honest summary is that this change makes concurrency **expressible and checkable**, and leaves
it **unexploited**. Those are two of the three claims in "built, runs, measured" and the third is
absent on purpose.

Also not built:

| | |
|---|---|
| A child that is cancelled when a sibling fails | **Built** ([`118`](118-a-scope-stops-its-children-report.md)) — and only the children *after* the failure, since stopping every sibling made the scope's answer a race. Originally: With an ordered join a later child has not started when an earlier one raises, so there is nothing to cancel *yet*. A backend that starts them together needs a cancellation signal, and nothing designs one |
| A scope over a *collection* — `parallel for x in xs` | **not built.** The children are written out, so their number is a property of the source. A dynamic fan-out is a different form with a different rule about what its children may perform |
| `spawn` reaching the data tier | **not built and not wanted.** `Tier::Data` does not discharge it, so a fold cannot spawn — and `Effect::breaks_replay` still says `spawn` breaks replay, which is now unreachable rather than wrong. Left alone: an ordered join is replay-safe, but nothing can observe that, and a claim nothing can test is not worth the edit |

## 80.6 What this corrects

**`spawn` was listed among the auto-stubbable atoms, and is not on §21.3's own list.** The doc
comment above `is_auto_stubbable` quotes §21.3 — "`net.out(host)`, `env`, `external.read/write
(store)`, `fs(path)`, `cap.*`, `nondet`" — and the code had one more. It is not external: a stub for
`spawn` would delete the children rather than the boundary they cross. Removed, and with it a
`stub spawn:` clause is refused, because what a test wants stubbed is what a child *does*.

**A `test` block may run a scope.** `B0700` — "a test block's own row must be empty" — is there
because "a test that performs a real `net.out` is a test that can fail because somebody else's
server is down". `spawn` crosses no boundary and reaches no host, so it is the one atom on §3.3's
list the rule does not apply to, and it is now excluded by name. This is
[`52`](52-crypto-and-identifiers-report.md) §52.5's finding met again with a different answer
available: there the layer holding a key was the layer Beck could not test and the answer was to make
it two lines; here the atom simply was not an effect on the world.

**Ten programs did not print back as themselves, and the harness that was supposed to say so did not
exist.** This is the largest finding in the change and it was found by asking a small question —
does the new form print? — of a property the printer states in its own first paragraph:

> Round-tripping is lossless *modulo formatting*: `parse(print(parse(src)))` is structurally equal to
> `parse(src)`, which is the property `tests/roundtrip.rs` asserts over the corpus.

There was no `tests/roundtrip.rs`. What asserted the property was three hand-written snippets inside
the printer's own `#[cfg(test)]` module, none of which used a shape that could go wrong. There is one
now — `beck-cli/tests/roundtrip.rs`, over **every** `.beck` file in the tree rather than over the
corpus, through both surfaces, asserting three things: printing is idempotent, the printed text
re-parses, and it re-parses to a structurally equal tree. Ten programs failed it, in three ways:

* **A block form used as an operand printed as a call.** `try:` is an *expression*
  ([`27`](27-the-walls-come-down-report.md) §27.7), and `expect (try: benchmark()) == Ok(True)` is how eight
  files in this tree assert a fallible answer. Every one printed as `try(…)`, which is not surface
  syntax — so `beck fmt` on `lib/bignum.beck`, `lib/dates.beck`, `lib/decimal.beck`,
  `lib/money.beck`, `clbg/pidigits.beck`, `awfy/deltablue.beck`, `awfy/json.beck` and
  `awfy/towers.beck` emitted a file that does not compile. §2.3's single-line block form is the
  notation, and the parentheses are what make it an operand.
* **A conditional expression printed without parentheses.** `a + (b if c else d)` came back as
  `(a + b) if c else d` — a different program that still parses, which is the worse kind of the two.
  `clbg/fannkuchredux.beck`.
* **`() -> T` printed as `fn-type[T]`.** [`63`](63-felleisen-report.md) §63.3 found this exact
  off-by-one in the parser and the checker, because a `fn-type` node is its parameters followed by
  its result and a function taking none has *one* argument. The printer counted the same wrong way
  and leaked its internal head into a file it had just written. `sicp/felleisen.beck`.

The lesson is [`70`](70-the-evaluator-gets-fast-report.md) §70.7's, arrived at from the other
direction: that report found three things wrong with a gate that could not fail. This one found a
gate that was *cited by name* in the code it was supposed to guard and had never been written. A
property stated in a doc comment is a property nothing checks.

**The round-trip harness needed the declared front-end stack.** Its first version overflowed a test
thread's default stack on `awfy/havlak.beck` before it asserted anything, which is
[`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md)'s rule reaching the one caller of
`beck-syntax` that is not `beck-cli`. Not a defect — the declaration exists precisely so a caller can
honour it — but worth recording, because a harness is a caller and this is the first one that had to
know.

## 80.7 What it leaves for somebody else

- **`fs(path)` is one atom for a read and a write** (§80.2). Splitting it would let a scope read two
  files, and it would make §6.5's derived least-privilege policy finer at the same time — a volume
  mounted read-only is a different manifest from one mounted read-write. That is a
  [`10`](10-decisions.md) decision with consequences past this form, and it is named rather than
  taken.
- **A scope over a collection** (§80.5). The rule it needs is not this one: with `n` children written
  out, "no child names another" is a scope check; with a dynamic fan-out it is a property of one
  lambda, which is a different proof.
- **A number.** Nothing here is measured, because there is nothing to measure until something runs
  two children at once. When there is, the thing to measure first is the one §80.5 names: what a
  child has to cost before it is worth a thread.

## 80.8 What this establishes

**That a language feature can be almost entirely a set of refusals.** `parallel:` adds one primitive
and one evaluator case; everything that makes it *mean* something — the row, the placement, the
published contract, the breaking-change classification, the derived stubs — was already there, and
the two rules that make its central claim true are both compile errors. The literature's advice was
"add row labels and handlers, not mechanisms", and the measure of how well that was followed is that
this report's longest section is about a printer.

**And that a property nothing checks is a property that is false.** The round-trip claim had been in
`print.rs`'s opening paragraph since Phase 1, naming the file that would assert it. Ten of the
tree's programs violated it, four of them in the standard library, and the way it was found was
writing a new form and wondering whether it printed.
