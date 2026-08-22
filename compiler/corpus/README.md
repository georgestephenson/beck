# The corpus

Thirty-seven single-file programs and a three-module project, and the Phase 2 exit criterion measured
against them:

> On a corpus of 20+ programs, placement is inferred with no annotations for the common cases.

So **no program here contains an `@on(...)`**, with two deliberate exceptions that say so in their
own first line. The harness in [`../crates/beck-cli/tests/corpus.rs`](../crates/beck-cli/tests/corpus.rs)
asserts that, compiles every file, and checks each one's placement against expectations recorded
beside it — because "it compiled" and "it was placed where it should be" are different claims.

They are deliberately *different* programs rather than twenty copies of the todo sketch. A corpus
of one shape would prove that the shape works, which Phase 1 already did. What varies:

- the accumulator: a counter, a map, a list, a record of several;
- the effect rows: `cap.*` for authority, `env` and `secret[T]` for credentials, `net.out` for a
  named host and for the own origin, `log` for ambient telemetry, `nondet` for ids and clocks;
- what the log is allowed to hold: `internal[T]` for a fact that is recorded and never rendered;
- whether the program is an application at all — some are libraries, which have no merge point and
  publish a `.becki` instead of running;
- how many modules it takes;
- **the shape of the signal graph**: one fold or two, a derived signal read once or twice, a
  `filter_map` between the chokepoint and a fold. Programs 21–23 exist for the general slicer
  ([`docs/23`](../../docs/23-incremental-views-report.md)), and until it was built every one of them
  was either refused or — in 21's case — silently mis-sliced;
- **the shape of the view**: a nested loop, a conditional among an element's children, a list read
  by two operators, and a computation that does not read the session even though it sits inside a
  `per_session`. Program 24 exists for the incremental view engine
  ([`docs/23`](../../docs/23-incremental-views-report.md)) and every one of those shapes is a way
  for a maintained view to disagree with the recomputed one;
- **the shape of the types**: a type that mentions itself, two that mention each other, a
  declaration that takes a type parameter and is used at two different arguments, and a `trait`
  with three impls. Program 25 exists for recursive types
  ([`docs/27`](../../docs/27-the-walls-come-down-report.md)), 27 for parameterised ones
  ([`docs/27`](../../docs/27-the-walls-come-down-report.md)) and 28 for traits
  ([`docs/27`](../../docs/27-the-walls-come-down-report.md)) — every pass here walks a type, and each is a place
  one of those shapes can be silently dropped;
- **the shape of the control flow**: a `try:` at the boundary rather than a `Result` threaded by
  hand (program 29, [`docs/27`](../../docs/27-the-walls-come-down-report.md)), and two outbound calls in one
  `parallel:` scope that do not wait for each other (program 30,
  [`docs/80`](../../docs/80-structured-concurrency-report.md)) — where what is being checked is
  that the scope's `spawn` reaches the published signature and the placement without anybody
  writing either down;
- **who is asking**: a program that declares `identity = external(issuer=…)` and whose `validate`
  refuses a command unless the *issuer* said which tenant is asking (program 31,
  [`docs/48`](../../docs/48-identity-report.md)). It is the only file here with a
  top-level form that is neither a definition nor a signal, so it is what holds the printer, the
  round-trip property and the placement property to it;
- **what is not in the log**: a program whose page reads `presence()` — who is connected now —
  beside the accumulator (program 32, [`docs/48`](../../docs/48-identity-report.md)). It is the only
  file here with an input that a replay does not reproduce, so it is where the rule that keeps that
  input away from the chokepoint has something to be about.
- **the shape of a *relationship***: a program that holds two collections and whose page is the
  sentence relating them (program 34, [`docs/99`](../../docs/99-the-data-tier-means-of-combination.md)).
  Program 27 contains a join by accident and its key is unique, so nothing there exercises **many**
  rows waiting on one — which is the interesting half of the delta rule, because renaming one person
  has to move every issue assigned to them and none of the issues that are not. The differential
  harness is what holds that, since it compares the maintained page with the recomputed one after
  every event and an in-language `contains` can only ask about a string somebody thought to name.
- **a question *about* a group rather than for its members** (programs 35, 36 and 37,
  [`docs/99`](../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 6). Program 35 asks
  each group how big it is, which a tally answers; program 36 asks what its rows say — the lowest
  and the highest bid on a lot — which a tally cannot; program 37 asks for a ledger account's
  balance, which is the aggregate that reads nothing at all, since a running total moves by `±n` and
  the group's rows are never a thing the operator holds. No page here ever holds a group. The second
  and third are where the events a generated log reaches only by luck are written out: for the
  extremes, the standing minimum withdrawn, the standing maximum withdrawn, half of a tie and the
  last row of a group; for the total, a credit voided, one of two identical amounts voided, and a
  group emptied to `0` and then rebuilt — `0` rather than nothing, because the sum of no numbers is
  a number where the smallest of no rows is not.

- **a relationship asked only whether it *exists*** (program 38,
  [`docs/99`](../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7). Programs 34 to 37
  ask the second collection a question and show the answer; this one asks it nothing but whether it
  holds the key, and the answer decides which of two lists a row is on — the algebra's difference,
  and the intersection beside it. Both halves index the stock once. It is the only program here
  whose page moves because of an event that touched **nothing the page shows**: stocking an item
  moves every order waiting on it and visits no order that wants something else, which is the half
  of the delta rule no test over a single collection can see. Its awkward events are written out
  rather than generated — an order amended while it was off the page, so what comes back is what it
  is now; and one cancelled while it was on, so a change on the right cannot resurrect it.

- **a page that shows a *set*** (program 39,
  [`docs/99`](../../docs/99-the-data-tier-means-of-combination.md) §99.9 item 7). Every other
  program here shows rows, or a question asked of rows; this one shows the topics its notes are
  filed under, each once, as a chip row above the notes. It is the only program whose page has an
  entry that can **move without its value changing** — a note arriving before the one that is
  currently first under its topic republishes that topic somewhere else — and it is what caught the
  operator's one real defect, on a generated log: a value leaving a position another value is
  arriving at. Its awkward events are written out for that reason.

Every file is a program someone could plausibly write. Where one exists to exercise a corner, its
first comment says which corner and why.
