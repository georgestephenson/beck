- **2026-08-16 — The client-local fold, scoped: it splits where awareness splits, and the decision
  is one sentence.** [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.8. What it
  needs is a **stream**, not an accumulator: the only stream is `merge_clients()` and §3.5 places it
  on the server, so every `on_click` in the language becomes a proposal. A second, client-placed
  source over a second union — a `Ui` where `Command` is the one the chokepoint sees — routes an
  interaction by its *type*, and `merge_clients()` stays the sole chokepoint because a `Ui` value can
  never reach `validate`. In **Mode B** that needs no wire at all and does not touch the digest, so
  `DEFECTS.md::non-durable-fold`'s open question is not even asked; in **Mode A** the page renders
  where the state is, so a browser-held value reaches it only by being sent — and then it is a
  per-connection accumulator the server folds, which is presence's shape rather than the log's. The
  decision, and it wants a D-number: *does a client-local fold exist only where the client renders,
  or does Mode A get a per-connection accumulator so it works there too?* Also corrected in place:
  §104.8's list said "four homes" over a five-item list and used "the fifth home" for the fourth —
  the homes are now counted right and the one that has to be built is **named** rather than
  numbered, since the numbering is what drifted.
