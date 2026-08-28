- **2026-08-22 — Interface state gets a home that is not the log: `gestures(step, init)`.**
  [`docs/10`](../docs/10-decisions.md) D30, correcting D1, and
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.8's Wall 1 comes down.
  `DEFECTS.md::non-durable-fold`, closed. A modal's open flag, a table's sort column and a
  combobox's highlighted option had nowhere to live but the durable log, so opening a panel was a
  `Command`, an `Event` and a permanent log entry — replicated, replayed, in the state digest. D1
  provided for "non-durable folds" and it was never built because the sentence named the right
  problem with the wrong mechanism: an accumulator that merely declines to be `durable` is still
  folded from the log, so replay would reproduce it whether or not the program asked. **Ephemerality
  comes from the stream, not from the absence of a wrapper** — `gestures` folds occurrences that
  were never proposed, validated or recorded. It takes no stream (a gesture has exactly one consumer
  by construction, so naming one buys no expressiveness — `awareness(f)` set the precedent) and its
  step takes the bare gesture rather than an `Envelope`, because a gesture has no position in the
  total order. It carries `dom`, which is its whole placement: the client is the only tier that
  discharges it, so the fold lands there, the page follows, and `durable` is unreachable from where
  it sits. D3's digest is untouched — the durable accumulator it covers is unchanged, and a gesture
  was never a candidate for it. Three refusals: the chokepoint may not decide from interface state
  (`B0523`), a page that reads it may not render on the server (`B0522`), and a variant may not be
  both a command and a gesture (`B0524`, which makes the client's routing total). `B0519`'s message
  stops calling the construct unbuilt and now names both fixes. Gated by
  `beck-cli/tests/gestures.rs`, which asserts the half the register said would be forgotten four
  ways — the log is empty, nothing is queued to send, a restart comes back to `init`, and the
  server's own decoder refuses a gesture — because any one alone could pass while the construct
  leaked through another. `compiler/examples/interface.beck` is the program. What a gesture costs
  was measured rather than assumed and came out against the prediction: **1.21× cheaper than a
  command at 100 cards and 1.18× at 1000**, a constant fraction rather than a growing one, because
  the render dominates and both paths pay it. So this is not a performance decision — the costs it
  moves are a log that does not fill with interface noise and a replay that does not reproduce it.
