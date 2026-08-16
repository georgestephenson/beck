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
