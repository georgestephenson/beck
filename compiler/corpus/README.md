# The corpus

Twenty-two programs, and the Phase 2 exit criterion measured against them:

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
- whether the program is an application at all — some are libraries, which have no merge point and
  publish a `.becki` instead of running;
- how many modules it takes.

Every file is a program someone could plausibly write. Where one exists to exercise a corner, its
first comment says which corner and why.
