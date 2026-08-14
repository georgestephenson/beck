# 80 — Structured concurrency

**Built.** `parallel:` — a scope whose bindings are its children, whose tail runs after the join,
which runs its children **on a thread each**, and which **stops the children after one that fails**.
Its claim is not that its children are fast but that **its answer does not depend on which of them
ran first**, and two rules hold that claim up: both are diagnostics rather than conventions.

Three things are worth the chapter.

**The soundness is in the checker, not in the scheduler** (§80.2). No child may name another and no
child may perform an effect another could observe, so running them together needed **no analysis, no
scheduler and no happens-before argument** — it starts two threads, because the program has already
been proved not to care.

**"After it" is the whole of cancellation, and it cost a defect to learn** (§80.7). The obvious
signal — any failing child stops every sibling — passed its own gate and broke a test that had been
green for two reports, **eight times in forty runs of the whole suite and never when run alone**.

**And a language feature can be almost entirely a set of refusals** (§80.11). The form adds one
primitive and one evaluator case; everything that makes it *mean* something was already there. The
measure of how well the literature's advice was followed is that **the longest section of the first
report about it was about a printer.**

---

## 80.1 The shape, taken rather than invented

[`38`](38-literature-survey.md) §38.4 is prescriptive and this is mostly it, built:

> The cross-language consensus … is: a scope owns its children, and errors and cancellation join at
> the scope. The effect-system literature says how Beck should express that: `spawn`/`await` as
> effect operations, the scope as their handler … at which point derived mocks, slicing and placement
> apply to concurrency with no new machinery, the same "desugar, don't extend the IR" trick.

```beck
def screen(email: Str) -> Screening:
    return parallel:
        f = fraud_score(email)
        r = reputation_score(email)
        Screening(fraud=f, reputation=r)
```

The children are the scope's `let`s. The tail is everything after the last of them, and it runs once,
after the join, with every child's result in scope. **That is the whole notation.**

It is a **form**, so it is lexically scoped by construction — the same argument
[`27`](27-the-walls-come-down-report.md) §27.7 made for `try:`, and §38.4's sharper Beck-specific
version of it: **in a language where effects decide placement, an accidentally intercepted effect
would be an accidental *re-placement*.** A nursery whose membership were decided at run time would be
the dynamic handler search POPL 2019 argues against.

The one place this goes further than §38.4's sketch is that **`spawn` and `await` are not separately
reachable.** The scope desugars to a single node — the children's bodies as nullary lambdas and the
tail as one lambda taking their results. There is no handle type, so a child cannot outlive its
scope, and the only thing in the language that can read a child's result is the lambda the scope
itself built. **Structured concurrency's central rule is usually a discipline a nursery API enforces
at run time; here it is the shape of one IR node.**

No new IR node and one evaluator case — a scope is the only thing in the language that runs several
expressions and then a fourth, and `CoreKind` has no node for that. What it does *not* need is a
`Task` type, a handle, a scheduler or a runtime, **which is the part §38.4 said to aim for.**

## 80.2 The two rules

The claim — the scope's answer is the same whichever order the children ran in — is worth exactly as
much as what enforces it.

**No child can name another** (`B0398`). Each initialiser is checked before any of the names is
bound, so a reference to a sibling does not resolve. What makes it a diagnostic rather than a puzzle
is that the checker remembers the names it has not yet bound:

```text
error[B0398]: `a` is another child of this `parallel:` scope, so it has not run yet — children
              cannot see each other, which is what lets them run together
```

Without that list the message is "cannot find `a` in this scope", which is true and tells the reader
nothing. **A child that could read a sibling would have to run second, and then it is not a child but
a next line** — which is the answer to "why not schedule it": a dataflow scheduler would make the
*shape* of the concurrency a thing you have to work out by reading, and this form exists to make it a
thing you can see.

**No child may perform an effect another child could observe** (`B0399`). The list is the atoms that
write state the program or its own substrate holds:

| Refused in a child | Why |
|---|---|
| `durable` | two children appending in the other order is a different log, and §3.7 makes the log the only description of a program's history |
| `ingress` | §3.7: "there is exactly one of these" |
| `dom` | two writers to one document |
| `external.write(store)` | somebody else's database, but one this program is ordering |
| `fs(path)` | see below |

**What is absent is as much of the argument as what is present.** `net.out(host)` is not on the list,
deliberately: a remote host's state was never Beck's to order, and two outbound calls are the case
the whole form exists for — **a rule that refused them would leave it with nothing to do.**
`nondet` is not on it either: a clock or a fresh id is a *read* of something outside the program, and
two children reading the clock do not interfere. `raises(E)` and `partial` are control flow, ordered
by the join. `cap.*` is an authority the caller holds, not state a child writes.

`fs(path)` is on the list for a reason worth recording rather than assuming, and it is a **finding**:
**the atom does not distinguish a read from a write.** Refusing concurrent writes therefore means
refusing the pair, so two children reading two files is something this form should allow and cannot.
That is a fact about the effect vocabulary rather than about concurrency — `fs(path)` is the only
atom in §3.2's list that names a resource without saying what is being done to it, and this is the
first thing that has needed the distinction. The shape of the answer is already in the list: §3.8's
escape hatches are `external.read(store)` and `external.write(store)`, two atoms for one resource,
split for exactly this reason. Doing it to `fs` is [`10`](10-decisions.md)'s to take rather than a
feature's, because §6.5 derives a volume's mount options from the same atom.

**Together those two rules are exactly the precondition for running the children together**, and they
were tested from the day the form landed. §80.5 adds no analysis: it starts two threads.

## 80.3 Failure joins at the scope, and the join decides which

§38.4: "cancellation is the error row crossing the scope." Nothing was built for that either — a
child's row is the scope's row, so a fallible child makes the scope fallible and an enclosing `try:`
catches at the scope.

The interesting half is *which* failure, when two children could raise. **An unordered join answers
"whichever finished first", which makes a program's result a function of a scheduler.** The ordered
join answers the **earliest child in the order it is written**, so the answer is a function of the
program:

```text
both(2, 1) == Err(Second)     # child one raises `Second`; child two raises `First`
```

That is a test rather than a sentence, and it is the property a backend genuinely running children
together has to keep: it may start them in any order and finish them in any order, **and it must
report the failure of the earliest *written* child that has one.**

That test has since become **stronger than the day it was written**, and it is worth saying rather
than letting a green tick carry it. Under an ordered join, its third case — two children raising
*different* errors — could not have failed, because the later child never ran at all. Now both run,
and the assertion is the one the test's name always claimed. One report later it caught the thing it
was newly able to catch (§80.7).

## 80.4 Everything downstream was already there

This is the part worth reporting, because it is the argument for rows.

| | Written for this | What it does |
|---|---|---|
| Inference | no | a caller of a spawning function spawns, and nobody wrote it down |
| `.becki` | no | `def screen(email: Str) -> Screening uses net.out(…), net.out(…), spawn` |
| Placement | no | §3.3's table says `server` discharges `spawn` and `client` does not, so `@on(server)` is solved rather than annotated |
| `--wire-compat` | no | a function that starts spawning is **breaking**, in the sentence §4.3 wrote for `net.out` |
| `uses` | no | a signature that does not declare `spawn` may not spawn |
| Derived mocks | §80.11 | a child is stubbed by what it *does*, one child at a time |

**Not one of those is a special case, and none of them is a line in this change.** The one thing that
is a line is that the scope charges `spawn` itself rather than inheriting it from what its children
do: a scope over two pure children still cannot run in a patch interpreter, so the atom belongs to
the form. `corpus/30-parallel.beck` carries the whole of this in a file with no annotations in it.

## 80.5 A scope runs its children, on a thread each

**The design was never the obstacle, and the first report said so.** What it named was mechanical:
two interpreters, a shared budget, and a `Host` trait that would have to become thread-safe.

The trait became thread-safe **for an unrelated reason**.
[`93`](93-the-native-backends-report.md) moved the four host atoms onto one trait so that three
backends could ask one question, and `Send + Sync` was the price of a compiled worker sharing a host
with the process that spawned it. **Nothing in that change was about concurrency.** It is worth
recording because the ordering was luck rather than judgement: §8.5.5's lane table predicts which
*files* two branches would collide over, **and it has nothing to say about a branch that removes
another's blocker.**

| | |
|---|---|
| the host | **shared** — one `&dyn Host` behind an `Arc`, only expressible because of the change above |
| a stack | **its own**, `STACK_BYTES` of it, because a tree-walker nests host frames on the program's recursion |
| the depth ceiling | **its own count** against the same ceiling, which is what a per-stack limit always meant |
| the globals cache | **its own**, rebuilt — sharing it wants a lock on the path a call takes, and [`70`](70-the-evaluator-gets-fast-report.md) §70.9 is what that path costs |
| fuel | **a share of what is left** (§80.6) |

**An `Interp` never crosses to a thread.** It cannot: its fuel, its depth and its globals cache are
`Cell`s, which is [`70`](70-the-evaluator-gets-fast-report.md)'s hot path and is deliberately not
`Sync`. What crosses is the host it borrows and two numbers, and each child builds an interpreter of
its own on the far side. **The compiler enforces that rather than a comment** — a closure that
captured `self` does not compile, which is how the first version of this was found to be wrong.

## 80.6 Fuel is split, not shared

Fuel bounds a run that will not stop ([`62`](62-fuel-report.md)). One budget, N children, three ways
to spend it:

- **Share it atomically.** Exact, and it puts a read-modify-write on every evaluation step of every
  program whether or not it ever writes `parallel:` — the hot path
  [`70`](70-the-evaluator-gets-fast-report.md) spent a chapter on. **It also makes *which* child runs
  out a race**, so two runs of one program could report different errors.
- **Give each child the whole budget.** No contention, and a scope of N children may do N times the
  work a serial run could, **which makes the backstop a function of how many children somebody
  wrote.**
- **Split it.** Each child gets an equal share of what remains; the scope then charges the parent the
  sum of what the children *actually spent*, so the total is what a serial run would have spent.

The third is built. What it costs is that **a child which would have used more than its share runs
out where a serial run would have let it continue** — a real behaviour change, on programs already at
the edge of a backstop. It is written here rather than discovered later.

Two things make it a smaller change than it reads as. Fuel is "a runaway-program backstop, not a
performance knob", and `spawn` is discharged by **`Tier::Server` alone** — not `Client`, not `Data` —
so no `parallel:` scope is inside a fold and **none of this can reach a replay.**

## 80.7 A scope stops the children after a failure

The temptation is a mechanism: a token, a scheduler, a cooperative-yield protocol. **None of that is
needed, because of what §80.2 already proved in the checker.** A child cannot name another and cannot
perform an effect another could observe, so stopping one is not a coordination problem — nothing is
waiting on it, and nothing can tell whether it finished. So the signal is a flag, and the only two
questions are **where it is read** and **what a stopped child's error means**.

**It rides the step counter.** Cancellation is read in `Interp::burn`, the one place every evaluation
step already passes through, and that is the cheap answer rather than the obvious one. **A checkpoint
at call boundaries would miss a loop**: a call in *tail* position does not nest —
[`27`](27-the-walls-come-down-report.md) §27.2 is the whole point — so a child spinning in an
iterative loop would never reach one. And **"the program is making progress" is when stopping it is
both possible and worth doing**: a child blocked in `http_fetch` is not spending steps, and this does
not interrupt it. The field is an `Option` and it is `None` for every interpreter that is not a
`parallel:` child, which is every one the runtime, the LSP and `beck test` build.

### The finding: the obvious signal is wrong, and a flaky test is what said so

The first version was a flag any failing child sets, stopping every sibling. It passed its own new
gate, passed the concurrency suite when that suite was run alone, and **failed eight times in forty
runs of the whole suite.**

The failure was §80.3's ordering test, whose third case runs two children that raise *different*
errors and expects the earlier one. **With a stop-everybody flag that case is a race**: whichever
child got to its `raise` first cancels the other before it can raise, so the scope answers `Second`
or `First` depending on the scheduler. That is precisely the property §80.3 exists to keep — *the
error a scope reports is a function of the program rather than of the scheduler* — **broken by the
feature added to serve it.**

**The fix is to stop the children *after* the failure, and only those.** What is stored is not
"somebody failed" but the **lowest index that has failed**, and a child stops only when a child
before it has. **That set is exactly the one an ordered join would never have reached**: under one, a
failure at child `i` means the children after `i` never ran and the children before `i` had already
finished. So cancellation becomes a change in *when work stops* and not in *what the scope answers*,
**which is the only version of this feature that can be correct.**

Two things about how it was caught are worth keeping. **It passed alone and failed in company** —
sixty runs of the test on its own were green, and the failures only appeared when the rest of the
suite was competing for cores. **A gate that is only run in isolation would have shipped this.** And
the test had been written for the ordered join and was, at the time, a case that could not fail; the
report before this one noticed that it had become load-bearing when the children started running
together, and said so.

**The signal is a chain**, because a scope may nest inside a scope and a grandchild has to stop when
an enclosing scope's earlier child fails. Each scope keeps the scope it is inside and its own index
within it, and asking walks the chain. **A stopped child's error is discarded rather than answered
with**: it did not fail, it was not allowed to finish — belt and braces given the rule above, and
what makes the message a program sees honest.

## 80.8 What it costs

From `measure_concurrency.rs`, release, on this machine.

**Children that wait** — the case the form exists for:

| each child waits | in order | together | ratio |
|---|---|---|---|
| 20 ms | 40.5 ms | 20.7 ms | **1.96×** |
| 200 ms | 400.7 ms | 201.1 ms | **1.99×** |

**Children that compute** — where the crossover is:

| each child counts to | one child alone | in order | together | ratio |
|---|---|---|---|---|
| 8 | 172 µs | 161 µs | 474 µs | **0.34×** |
| 1,000 | 581 µs | 1.07 ms | 825 µs | **1.30×** |
| 20,000 | 7.9 ms | 15.4 ms | 8.3 ms | **1.85×** |
| 100,000 | 40.4 ms | 75.7 ms | 40.5 ms | **1.87×** |

So a scope is a **loss** on children cheaper than a thread and approaches the core count on children
dearer than one, and the crossing is between the first two rows. **That is §80.5's question answered
in the units it was asked in.**

**Where the fixed cost goes is the part worth reading twice, because the answer is not the one the
question expects.** `AGENTS.md` says a number like "a tenth of a millisecond per child" is a design
question rather than a fact to write down, so it was asked:

| | |
|---|---|
| a thread with the default stack | 94.4 µs |
| a thread with `STACK_BYTES` (256 MiB) | 123.8 µs |
| the difference | **29.3 µs** |

**The stack reservation is the *smaller* half.** On this machine a bare thread already costs most of
what a child costs, before anything has been reserved or built. Neither half is a knob: the
reservation is what the depth ceiling needs in order to be a diagnostic instead of a `SIGSEGV`
([`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)), and the thread is **what
running two things at once *is***. What would move the number is a pool — threads reserved once and
reused — which is a different change with a different question in front of it.

**What the cancellation check costs a program with no scope in it**, median of nine:

| iterations | with the check | without it |
|---|---|---|
| 40,000 | 388.3 ns | 385.0 ns |
| 400,000 | 380.1 ns | 374.7 ns |

**The honest reading is that this harness cannot resolve it.** The difference is about 1%, and an
earlier run of the *same* build measured 410.8 ns at the smaller size — **so the gap between the two
columns is smaller than the gap between two runs of one column.** What the two sizes *do* establish
is the shape: the cost per iteration does not grow with the number of iterations, which is what
"loop-invariant branch" predicts and what a check put somewhere worse would violate. **"About 1%, and
below what this can measure" is the claim. "Free" would be a stronger claim than the evidence
supports.**

## 80.9 Which wasm can have threads

The question was asked properly, and **the first answer was wrong**. "Wasm has no threads" is false
as stated: **`wasm32-wasip1-threads` is a stable Rust target and it really spawns.** The evidence is
the compiled module rather than the documentation — the same probe built for each target: on
`wasm32-unknown-unknown` `thread::Builder::spawn` compiles to the string `operation not supported on
this platform`, `std`'s stub; on `wasm32-wasip1-threads` the module imports `thread-spawn`.

**But that is WASI, and the two places this repository emits wasm are *browser* targets.** There
threads need the `atomics` feature `rustc` reports as unstable on the pinned channel, a `std` rebuilt
with it (nightly), and cross-origin isolation plus a worker per thread in the page — of which the
first is decisive. `wasm32` is also the only bitness on stable; there is no `wasm64` target. **Where
the threading target *will* matter is the server-side WASM tier [`05`](05-tier-lowering.md) §5.4
names and nothing emits yet.**

A *client* cannot reach a scope at all — `spawn` is `Tier::Server`'s alone, and a scope pinned to the
browser is refused — but the playground runs a whole application in a tab **including its server
half**, so it can. There the children run **in order**, which is a correct implementation of the form
for §80.2's reason and **loses the overlap, never an answer.** A `cargo check` against the target is
what says the build still stands; nothing runs a scope in a tab and asserts it.

**A flat "no" was an overclaim and a flat "yes" would be a worse one**, which is why the three facts
are separated here.

## 80.10 The gates

- **`two_children_actually_overlap`** — and it is not a timing test. The host's `fetch` will not
  answer until *every* child has reached it, **so a serial evaluator cannot pass it at any speed**:
  the first child blocks waiting for an arrival that cannot happen until it returns. **Checked by
  making it fail**: forced back to an ordered join it goes red in ten seconds with "a child waited
  ten seconds for its sibling and gave up, which is what a serial evaluator does".
- **`a_failing_child_stops_its_siblings`** — gated by a **count, not a clock**: the stopped sibling
  reached its peer *more than zero* times (so it was running) and *far fewer than 400* (so it was
  stopped); it reaches 4 or 5. **Checked by disabling the signal and watching it hit 400/400**, and
  the ordering gate beside it was run **forty times rather than once**, which is the only reason
  §80.7's race was found at all.
- **`a_lone_child_is_not_worth_a_thread`** — the one case decided by argument rather than by
  measurement, since a thread that overlaps with nothing is pure cost. It asserts the *answer*,
  because what it is there to catch is a scope that stopped being correct when it stopped spawning.
- **The seventeen tests that were already there pass unchanged**, including the failure ordering and
  the nested scope. **That is §80.2's claim as a test rather than as an argument**: the form's
  meaning did not change, only what runs it.
- **`measure_concurrency.rs`** asserts exactly one thing and it is on the side where it cannot be
  close — two children that each wait 200 ms must finish in well under the 400 ms an ordered join
  takes. Every rate is printed rather than gated ([`13`](13-testing.md) §13.7).

## 80.11 What building it found

**`spawn` was listed among the auto-stubbable atoms, and is not on §21.3's own list.** The doc comment
above the predicate quotes §21.3 and the code had one more entry. It is not external: **a stub for
`spawn` would delete the children rather than the boundary they cross.** Removed, and with it a
`stub spawn:` clause is refused, because what a test wants stubbed is what a child *does*.

**A `test` block may run a scope.** `B0700` — "a test block's own row must be empty" — is there
because "a test that performs a real `net.out` is a test that can fail because somebody else's server
is down". `spawn` crosses no boundary and reaches no host, so it is the one atom on §3.3's list the
rule does not apply to, and it is excluded by name.

**Ten programs did not print back as themselves, and the harness that was supposed to say so did not
exist.** This is the largest finding here and it was found by asking a small question — *does the new
form print?* — of a property the printer states in its own first paragraph:

> Round-tripping is lossless *modulo formatting*: `parse(print(parse(src)))` is structurally equal to
> `parse(src)`, which is the property `tests/roundtrip.rs` asserts over the corpus.

**There was no `tests/roundtrip.rs`.** What asserted the property was three hand-written snippets
inside the printer's own test module, none of which used a shape that could go wrong. There is one
now, over **every** `.beck` file in the tree, through both surfaces, asserting three things: printing
is idempotent, the printed text re-parses, and it re-parses to a structurally equal tree. **Ten
programs failed it, in three ways:**

- **A block form used as an operand printed as a call.** `try:` is an *expression*, and
  `expect (try: benchmark()) == Ok(True)` is how eight files in this tree assert a fallible answer.
  Every one printed as `try(…)`, which is not surface syntax — **so `beck fmt` on four standard
  library files and four benchmark ports emitted a file that does not compile.**
- **A conditional expression printed without parentheses.** `a + (b if c else d)` came back as
  `(a + b) if c else d` — **a different program that still parses, which is the worse kind of the
  two.**
- **`() -> T` printed as `fn-type[T]`.** [`63`](63-felleisen-report.md) §63.3 found this exact
  off-by-one in the parser and the checker, because such a node is its parameters followed by its
  result and a function taking none has *one* argument. The printer counted the same wrong way and
  **leaked its internal head into a file it had just written.**

The lesson is [`70`](70-the-evaluator-gets-fast-report.md) §70.7's, arrived at from the other
direction: that report found three things wrong with a gate that could not fail. **This one found a
gate that was cited by name in the code it was supposed to guard and had never been written. A
property stated in a doc comment is a property nothing checks.**

**And the round-trip harness needed the declared front-end stack.** Its first version overflowed a
test thread's default stack before it asserted anything, which is
[`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md)'s rule reaching the one caller of
`beck-syntax` that is not `beck-cli`. Not a defect — the declaration exists precisely so a caller can
honour it — but worth recording, because **a harness is a caller and this is the first one that had
to know.**

## 80.12 What is not built

| | Status |
|---|---|
| **Stopping a child blocked in the host** | **Not built**, and it is the largest open item. Cancellation rides the step counter, so a child blocked in `http_fetch` is stopped only when the call returns — a scope whose first child fails still waits for a sibling's outbound call to come back or give up. That is a **deadline on the [`net`](../compiler/crates/beck-core/src/net.rs) seam** rather than a change to the scope |
| A scope over a **collection** — `parallel for x in xs` | **Not built.** The children are written out, so their number is a property of the source. A dynamic fan-out is a different form with a different rule about what its children may perform — and the rule it needs is not this one: with `n` children written out, "no child names another" is a scope check; with a dynamic fan-out it is a property of one lambda, which is a different proof |
| A **thread pool** | **Not built.** Every child is a fresh thread; §80.8 is what that costs and what a pool would be worth, and nothing here designs one — how many, and owned by whom, is the question in front of it |
| More children than cores | **Unbounded.** A scope of forty on a four-core machine is forty threads. Nothing measures that and nothing bounds it |
| The compiled backends | **Further from concurrency than the tree-walker now is.** [`93`](93-the-native-backends-report.md) §93.14 says a worker holds its pipe for a whole call, so two children that both reach compiled code serialise behind one lock. That report's "the first thing a second version would change" has been the same sentence for several reports and is now blocking something real |
| Threads in a browser | **Not available** (§80.9), and the playground's ordered fallback is correct rather than degraded |
| `spawn` reaching the data tier | **Not built and not wanted.** `Tier::Data` does not discharge it, so a fold cannot spawn — and `Effect::breaks_replay` still says `spawn` breaks replay, which is now unreachable rather than wrong. Left alone: an ordered join is replay-safe, but nothing can observe that, and **a claim nothing can test is not worth the edit** |
| **`fs(path)` is one atom for a read and a write** | §80.2. Splitting it would let a scope read two files, and it would make §6.5's derived least-privilege policy finer at the same time — a volume mounted read-only is a different manifest from one mounted read-write. That is a [`10`](10-decisions.md) decision with consequences past this form, and it is **named rather than taken** |

### What this establishes

**That a language feature can be almost entirely a set of refusals.** `parallel:` adds one primitive
and one evaluator case; everything that makes it *mean* something — the row, the placement, the
published contract, the breaking-change classification, the derived stubs — was already there, and
the two rules that make its central claim true are both compile errors. The literature's advice was
"add row labels and handlers, not mechanisms".

**That the rule coming first is what made the parallelism an implementation of it.** No analysis, no
scheduler and no happens-before argument were needed, because the checker had already proved the
program does not care.

**And that a property nothing checks is a property that is false.** The round-trip claim had been in
the printer's opening paragraph since Phase 1, naming the file that would assert it. Ten of the
tree's programs violated it, four of them in the standard library, **and the way it was found was
writing a new form and wondering whether it printed.**
