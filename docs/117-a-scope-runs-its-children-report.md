# 117 — A scope runs its children

**Built.** `parallel:` runs its children **at the same time**, on a thread each.
[`80`](80-a-scope-owns-its-children-report.md) built the form — a scope whose bindings are its
children, with the two rules that make the order unobservable — and §80.5 said, in its first
sentence, that nothing ran two of them at once. That sentence is what this closes, and with it the
last named remainder of [`08`](08-roadmap.md)'s structured-concurrency bullet.

**The design was never the obstacle and §80.5 said so.** What it named was mechanical: two
interpreters, a shared budget, and a `Host` trait that would have to become thread-safe. The trait
became thread-safe **for an unrelated reason** — [`116`](116-the-host-answers-back-report.md) moved
the four host atoms onto `beck_core::host::Atoms` so that three backends could ask one question, and
`Send + Sync` was the price of a compiled worker sharing a host with the process that spawned it.
Nothing in that change was about concurrency. It is worth recording because the ordering was luck
rather than judgement: §8.5.5's lane table predicts which *files* two branches would collide over,
and it has nothing to say about a branch that removes another's blocker.

**What it is worth**, from `measure_concurrency.rs` (release, medians): two children that each wait
200 ms take **201.1 ms** where an ordered join takes 400.7 ms — **1.99×**, and the ceiling for two
children is 2×. Two children that *compute* get the same 2× once each is worth a thread, and the
crossover is measured rather than guessed: **0.34× at a child of ~170 µs, 1.30× at ~580 µs, 1.85×
at 7.9 ms**. §117.5 is where the fixed cost goes, and the answer is not the one the question
expects.

§117.4 is the one semantic change, stated rather than buried: **fuel is split, not shared.**

---

## 117.1 What was already true, and is doing all the work

The soundness of this change is not in it. [`80`](80-a-scope-owns-its-children-report.md) put it in
the **checker**, as two diagnostics:

- **`B0398`** — no child may name another. So there is no data dependency between children to
  schedule around.
- **`B0399`** — no child may perform an effect another child could observe. So there is no
  *observable* dependency either.

Together those are exactly the precondition for running them together, and they were tested from the
day the form landed. This report adds no analysis, no scheduler and no happens-before argument: it
starts two threads, because the program has already been proved not to care.

That is the shape [`38`](38-literature-survey.md) §38.4 asks for — "spawn/await as effect
operations, the scope as their handler" — arriving in the order it should. The rule came first and
the parallelism is an implementation of it.

## 117.2 What a child gets, and what it shares

| | |
|---|---|
| the host | **shared** — one `&dyn Host` behind an `Arc`, which is only expressible because [`116`](116-the-host-answers-back-report.md) made `Atoms` `Send + Sync` |
| a stack | **its own**, `beck_eval::STACK_BYTES` of it, because a tree-walker nests host frames on the program's recursion |
| the depth ceiling | **its own count** against the same ceiling, which is what a per-stack limit always meant |
| the globals cache | **its own**, rebuilt — sharing it wants a lock on the path a call takes, and [`70`](70-the-evaluator-gets-fast-report.md) §70.9 is what that path costs |
| fuel | **a share of what is left** (§117.4) |

**An `Interp` never crosses to a thread.** It cannot: its fuel, its depth and its globals cache are
`Cell`s, which is [`70`](70-the-evaluator-gets-fast-report.md)'s hot path and is deliberately not
`Sync`. What crosses is the host it borrows and two numbers, and each child builds an interpreter of
its own on the far side. The compiler enforces that rather than a comment — a closure that captured
`self` does not compile, which is how the first version of this was found to be wrong.

## 117.3 Failure, and what is *not* cancelled

The **first child in source order** that failed is the scope's failure, whichever thread finished
first. So the error a scope reports is a function of the program rather than of the scheduler.

`a_childs_failure_joins_at_the_scope_and_the_earliest_child_wins` is unchanged and still passes —
but **it is a stronger test now than the day it was written**, and that is worth saying rather than
letting a green tick carry it. Its third case runs two children that raise *different* errors and
expects the earlier one; under an ordered join the later child never ran at all, so the case could
not have failed. Now both run, and the assertion is the one the test's name always claimed: that
the answer is the order the children are *written* in and not the order they finished in.

**Nothing is cancelled.** §80.5's table forecast this precisely:

> A child that is cancelled when a sibling fails — **not built.** With an ordered join a later child
> has not started when an earlier one raises, so there is nothing to cancel *yet*. A backend that
> starts them together needs a cancellation signal, and nothing designs one.

That backend now exists and the signal still does not. The siblings of a failed child run to
completion and their answers are dropped. This is a real cost — a scope whose first child fails at
once still waits for a sibling's outbound call to come back or give up, and how long that is belongs
to the [`net`](../compiler/crates/beck-core/src/net.rs) implementation rather than to the scope — and
it is the largest single item §117.7 leaves open.

## 117.4 Fuel is split, not shared

Fuel bounds a run that will not stop ([`62`](62-fuel-report.md)). One budget, N children, and three
ways to spend it:

- **Share it atomically.** Exact, and it puts a read-modify-write on every evaluation step of every
  program whether or not it ever writes `parallel:` — the hot path
  [`70`](70-the-evaluator-gets-fast-report.md) spent a chapter on. It also makes *which* child runs
  out a race, so two runs of one program could report different errors.
- **Give each child the whole budget.** No contention, and a scope of N children may do N times the
  work a serial run could, which makes the backstop a function of how many children somebody wrote.
- **Split it.** Each child gets an equal share of what remains; the scope then charges the parent the
  sum of what the children *actually spent*, so the total is what a serial run would have spent.

The third is built. What it costs is that a child which would have used more than its share runs out
where a serial run would have let it continue — a real behaviour change, on programs already at the
edge of a backstop. It is written here rather than discovered later.

Two things make it a smaller change than it reads as. Fuel is "a runaway-program backstop, not a
performance knob" (`DEFAULT_FUEL`'s own documentation), and `spawn` is discharged by
**`Tier::Server` alone** — not `Client`, not `Data` — so no `parallel:` scope is inside a fold and
none of this can reach a replay.

## 117.5 What it costs, and where the cost goes

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
dearer than one, and the crossing is between the first two rows. That is §80.5's question answered
in the units it was asked in.

**Where the fixed cost goes** is the part worth reading twice, because the answer is not the one the
question expects. `AGENTS.md` says a number like "a tenth of a millisecond per child" is a design
question rather than a fact to write down, so it was asked:

| | |
|---|---|
| a thread with the default stack | 94.4 µs |
| a thread with `STACK_BYTES` (256 MiB) | 123.8 µs |
| the difference | **29.3 µs** |

The stack reservation is the **smaller** half. On this machine a bare thread already costs most of
what a child costs, before anything has been reserved or built. Neither half is a knob: the
reservation is what the depth ceiling needs in order to be a diagnostic instead of a `SIGSEGV`
([`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)), and a `parallel:` child may
recurse exactly as deep as anything else; the thread is what running two things at once *is*.

What would move the number is a pool — threads reserved once and reused — which is a different
change with a different question in front of it (how many, and owned by whom), and is not made here.

## 117.6 The gates

- **`concurrency.rs::two_children_actually_overlap`** — and it is not a timing test. The host's
  `fetch` will not answer until *every* child has reached it, so a serial evaluator cannot pass it
  at any speed: the first child blocks waiting for an arrival that cannot happen until it returns.
  A concurrent one passes at once. **Checked by making it fail**: forced back to an ordered join, it
  goes red in ten seconds with "a child waited ten seconds for its sibling and gave up, which is
  what a serial evaluator does" — which is [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md)
  §84.5's discipline, asking what would have to be true for a gate to go red and checking that the
  thing it guards against would make it so.
- **`concurrency.rs::a_lone_child_is_not_worth_a_thread`** — the one case decided by argument rather
  than by measurement, since a thread that overlaps with nothing is pure cost. It asserts the
  *answer*, because what it is there to catch is a scope that stopped being correct when it stopped
  spawning.
- **The seventeen tests that were already there pass unchanged**, including the failure ordering and
  the nested scope. That is the claim §117.1 makes, as a test rather than as an argument: the form's
  meaning did not change, only what runs it.
- **`measure_concurrency.rs`** asserts exactly one thing and it is on the side where it cannot be
  close — two children that each wait 200 ms must finish in well under the 400 ms an ordered join
  takes. Every rate is printed rather than gated ([`13`](13-testing.md) §13.7).

## 117.7 What this does not establish

- **Nothing about cancellation.** §117.3. A scope whose first child fails still waits for its
  siblings, and designing a signal is the next item rather than a detail of this one.
- **Nothing about a scope over a collection.** `parallel for x in xs` is still a different form with
  a different rule about what its children may perform (§80.5), and the children here are still
  written out.
- **Nothing about a thread pool.** Every child is a fresh thread; §117.5 is what that costs and what
  a pool would be worth, and neither this report nor §80.5 designs one.
- **Nothing about the compiled backends.** [`116`](116-the-host-answers-back-report.md) §116.10 says
  a worker holds its pipe for a whole call, so two children that both reach compiled code serialise
  behind one lock — further from concurrency than the tree-walker now is.
  [`93`](93-llvm-backend-report.md) §93.7's "the first thing a second version would change" has been
  the same sentence for four reports and is now blocking something real.
- **Nothing about more children than cores.** Every child gets a thread, and a scope of forty on a
  four-core machine is forty threads. Nothing here measures that and nothing bounds it.
- **Nothing in the browser.** `wasm32` has no threads, and this crate is compiled to it twice — for
  a Mode B client and for the playground. A *client* cannot reach a scope at all (`spawn` is
  `Tier::Server`'s alone, and `B0401` refuses one pinned to the browser), but the playground runs a
  whole application in a tab including its server half, so it can. There the children run **in
  order**, which is a correct implementation of the form for §117.1's reason and loses the overlap
  rather than an answer. `cargo check -p beck-eval --target wasm32-unknown-unknown` is what says the
  build still stands; nothing runs a scope in a tab and asserts it.
- **Nothing about `parallel:` reaching the data tier.** `Tier::Server` alone discharges `spawn`, so a
  fold still cannot spawn — unchanged, and the reason §117.4's split cannot affect a replay.

## 117.8 What this corrects

- [`80`](80-a-scope-owns-its-children-report.md) §80.5's first sentence — *"Nothing runs two children
  at the same time"* — and its summary, *"this change makes concurrency **expressible and
  checkable**, and leaves it **unexploited**. Those are two of the three claims in 'built, runs,
  measured' and the third is absent on purpose."* All three now hold, and the third is §117.5.
- **§80.5's list of what stood in the way** was right about all of it and wrong about the order. It
  named the `Host` trait's thread-safety as a change to the execution half of the seam that "wants
  its own measurement"; it was made by a report about compiling `http_fetch`, which wanted it for a
  reason that has nothing to do with two children.
- **`concurrency.rs`'s module documentation** said "nothing here runs two children at the same time".
  It does now, and the sentence names the gate instead.
- [`08`](08-roadmap.md)'s concurrency-and-errors bullet said what was left of it was "structured
  concurrency's missing backend". There is no remainder now — what §117.7 lists are new items rather
  than the rest of this one.
