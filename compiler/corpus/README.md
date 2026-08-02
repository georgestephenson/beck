# The corpus

Twenty-eight programs, and the Phase 2 exit criterion measured against them:

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
  ([`docs/23`](../../docs/23-general-slicer-report.md)), and until it was built every one of them
  was either refused or — in 21's case — silently mis-sliced;
- **the shape of the view**: a nested loop, a conditional among an element's children, a list read
  by two operators, and a computation that does not read the session even though it sits inside a
  `per_session`. Program 24 exists for the incremental view engine
  ([`docs/24`](../../docs/24-incremental-views-report.md)) and every one of those shapes is a way
  for a maintained view to disagree with the recomputed one;
- **the shape of the types**: a type that mentions itself, two that mention each other, and a
  declaration that takes a type parameter and is used at two different arguments. Program 25 exists
  for recursive types ([`docs/27`](../../docs/27-walls-report.md)) and program 27 for parameterised
  ones ([`docs/36`](../../docs/36-parameterised-types-report.md)) — every pass here walks a type,
  and each is a place one of those shapes can be silently dropped.

Every file is a program someone could plausibly write. Where one exists to exercise a corner, its
first comment says which corner and why.
