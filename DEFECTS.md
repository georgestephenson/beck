# Defects

**What is wrong right now.** [`CHANGELOG.md`](CHANGELOG.md) is what has been fixed; this is what has
not. An entry is **deleted by the change that fixes it**, in the same commit, and the CHANGELOG
bullet for that change is where it goes on record. So this file is always the current list and never
a history — git holds the history, as it does for everything else in this repository
([`AGENTS.md`](AGENTS.md)).

**What belongs here: something that behaves wrongly.** Silent, misleading, or contrary to what a
document says. **What does not: something that is merely absent.** A feature nobody has built is a
line in [`docs/08`](docs/08-roadmap.md) §8.5, which is the only place that holds an order; putting
absences here would turn the register into a second roadmap that disagrees with the first.

**Every entry names the gate a fix owes.** This project has repeatedly shipped fixes behind gates
that could not have failed ([`docs/82`](docs/82-the-edge-report.md) §82.10), and the cure is to write
down *what would have to go red* while the defect is still in front of you. A fix that lands without
its gate has not been fixed; it has been made invisible.

**Ids are slugs, not numbers, and are never reused.** Entries are deleted, so a number would imply a
sequence that does not survive.

This register was opened alongside [`docs/104`](docs/104-styling-and-the-component-library.md) and is
**seeded rather than complete**: it holds what that audit found plus one older defect already
recorded in a report. Anything you find that meets the admission rule above belongs here, whether or
not you are the one to fix it.

---

## `non-durable-fold` — a decided construct is unbuilt, and what blocks it is a decision

**What is wrong.** [`docs/10`](docs/10-decisions.md) D1 provides for non-durable folds — "high-churn
ephemera get non-durable folds — same semantics, no log persistence" — and
[`docs/15`](docs/15-scale-and-distribution.md) assigns hot ephemeral state to them. A `fold` that is
not wrapped in `durable` does not make a signal graph, does not get a page, and does not run.

**Half fixed.** It used to report the program as *a library with no durable state*, which sends its
author to add the `durable` they deliberately left off. It now says what it is: `B0519`, naming the
construct, its status, and what stands in the way. The construct itself is still unbuilt.

**What stands in the way is not plumbing, and this is the finding.** An accumulator outside the log
is **not a function of the log**, and three things in this project rest on it being one:

- `beck-cli/tests/replay.rs` asserts `digest(replayed) == digest(live)`. A fold that is not
  replayed into makes the two differ by construction; one that *is* replayed into is not ephemeral,
  it is derived.
- [`docs/10`](docs/10-decisions.md) D3 rests on that digest — "replaying from the first event must
  always reproduce everything".
- [`docs/03`](docs/03-type-and-effect-system.md) §3.7 logs **every validated event**, so a fold over
  the one stream Beck has is reconstructible from the log whatever it is called. The *volume* half
  of D1's motivation — a cursor that moves a hundred times a second — is not addressed by an
  unlogged accumulator at all, because the events are what there are a hundred of.

So the construct needs an answer to **what the state digest covers**, and possibly a second,
un-journalled stream. Both are decisions rather than implementations, and they are D-numbers rather
than a branch.

**Two things that look like this construct and are not**, checked rather than assumed:

- `beck-rt/src/presence.rs` is a map mutated on connection join and leave. Its own module
  documentation states the distinguishing fact: it is "the only input to a view that moves
  **without** an event". It is D6's first-class non-durable `Signal` and a compiler-provided source,
  not a fold.
- `beck-rt/src/quota.rs` runs *before* an event exists, and is a **sharded** fixed table on purpose:
  a per-actor map is unbounded memory keyed by a name the client chooses, which is the denial of
  service it exists to prevent ([`docs/82`](docs/82-the-edge-report.md) §82.5). A fold would be that
  map.

Nothing in the tree is a non-durable fold, so there is no machinery to generalise.

**The gate a fix owes.** Unchanged, and the second is still the one that will be forgotten: a
program with a non-durable fold runs and its page reflects it, **and** the fold's state does not
appear in the log after a restart. A fix that only satisfies the first has built a durable fold with
a different spelling. `ui.rs::a_fold_nobody_wrapped_in_durable` holds the half that is done.

**Where it is argued.** [`docs/104`](docs/104-styling-and-the-component-library.md) §104.8, Wall 1.
The *language* question of where interface state should live is a decision rather than a defect and
is scheduled separately in [`docs/08`](docs/08-roadmap.md) §8.5.4 — and this entry now knows it is
the same decision rather than a separable one, because the client-local stream Wall 1 says does not
exist is the same missing thing.

---

## `union-merge-is-local-only` — every pull request that touches `CHANGELOG.md` reads as conflicting

**What is wrong.** [`.gitattributes`](.gitattributes) sets `merge=union` on
[`CHANGELOG.md`](CHANGELOG.md) so that two branches each prepending a bullet under `## Unreleased`
do not conflict. Git honours it. **GitHub does not read the file at all** — neither the
`mergeable_state` it reports on a pull request nor the merge its button performs consults a merge
driver — so the driver is in force exactly where nobody is looking and absent where everybody is.
Since every change is required to add a bullet at the top of that list, and the list has no topic
headings on purpose, *every* pull request open across another one's merge is reported as conflicting.

**Why it is a defect rather than an inconvenience.** The report is not merely noisy, it is
**misleading in the direction that costs the most**: a reviewer reading "this branch has conflicts
that must be resolved" has no way to tell the one file the driver would have settled from a real
disagreement in the compiler, and the honest response to the message — resolve the conflict by hand
— is the one thing the flat-list design was built to make unnecessary. The comment in
`.gitattributes` asserted the conflict was solved; it is solved on a clone and not on the forge, and
that comment has been corrected in place.

**The workaround, which is not the fix.** Merge the base branch down into the branch locally, where
the driver applies, and push the merge. The pull request then has nothing left to merge and reports
clean. This works and is what has been done, but it puts a merge commit on every branch that
outlives one other merge, and it requires somebody to know why.

**The gate a fix owes**, and it is the half that will be forgotten: a real fix removes the *reliance*
on the driver rather than teaching the forge about it, most likely by giving each change its own
file so two branches never write the same line. So the gate is **not** "the union driver keeps both
bullets" — that passes today and pins the defect in place. It is that two branches each recording a
change merge cleanly **in a tree with no `.gitattributes` at all**, which is the configuration
GitHub runs: build the two branches, drop the file, merge, and assert no conflict.

**Model the absent driver by removing the file, not by configuration.** `core.attributesFile` names
the *global* attributes file and does not suppress the one in the tree, so a gate written that way
runs with the driver still in force and passes for the wrong reason — [`docs/82`](docs/82-the-edge-report.md)
§82.10's pattern, arrived at from the other direction. Checked both ways while this entry was
written: two branches each prepending a bullet conflict with the file absent and merge clean with it
present, so the gate goes red today and green on a fix.

---

## `work-cannot-see-inside-an-application` — the engine's counters report a whole page as one unit

**What is wrong.** `beck_core::engine::Work` counts what the *engine* does — per-element functions
applied, arrangement entries touched, pointwise operators re-evaluated, entries materialised — and
counts one application for a call whatever that call goes on to do. When a plan's per-element
function is a whole page, the counter reports a number that has nothing to do with the work.

`examples/board.beck` with the join refused is the case that found it. Its loop over the three
columns filters every card three times and rebuilds all three `<ul>`s, on every event, at every
collection size — and `Work` reports **3 applications, 3 touched, 3 materialised and 3 recomputed at
200 cards and the identical four numbers at 1,600**, while the clock over the same two renders goes
from 2.3 ms to 21 ms. Both halves are printed side by side by
`cargo test --release --test measure_incremental what_a_grouped_join_is_worth -- --nocapture`.

**Why it is misleading rather than merely incomplete.** `beck explain cost` ends with "`Work` is
what `Engine::render` counts, so `measure_incremental` checks this arithmetic against the count
rather than against a clock", and `scaling.rs`'s header calls `Work` "the deterministic instrument".
Both are true of a plan whose operators are the work and false of one whose work is inside an
operator — and the failure is silent and in the flattering direction: the plan that hides the most
reports the smallest number. Every shape gate in `scaling.rs` that reads `Work` is therefore blind
to exactly the pessimisation an opaque operator can hide, which is the class
[`docs/82`](docs/82-the-edge-report.md) §82.10 is about.

**What is *not* wrong.** The plan's own report is right: `beck explain cost` says of the refused
board that a per-element function captured the state "so the whole collection is reconsidered on
every event", and names the operator. The arithmetic and the count disagree, and it is the count
that is wrong.

**The gate a fix owes.** Not "the number is bigger" — a partial fix that counted only the arms
somebody remembered would move the number and keep the blindness. It is that
**two plans that do the same work report the same `Work`**: render one program's view through a plan
whose collection operators the decomposition entered, and through a plan where the same expression
was forced into a single `Op::Pointwise` (`beck explain incremental` already names which construct
does that), over the same log, and assert the two totals agree within a stated factor. That is red
today by roughly the collection's size, and no arm-by-arm patch makes it green.

**What a fix costs, since it is why this is recorded rather than done.** The count has to come from
whatever executed the code, so `Backend` — the seam `beck-rt` reaches execution through — is where
it would go, and there are three implementations of it in the workspace plus the two the
backend-seam harness supplies ([`docs/19`](docs/19-phase-1-report.md) §19.9). That is a cross-cutting
instrument change with a lane of its own, not a line in the operator that found it.
