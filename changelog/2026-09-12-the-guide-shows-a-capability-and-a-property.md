- **2026-09-12 — The guide shows a capability and a property, and its placement table is gated.**
  [`86`](../docs/86-getting-started.md) §86.4 is called "Authority is one function" and never showed
  a capability, which is the mechanism the title is about.
  [`03`](../docs/03-type-and-effect-system.md) §3.5's claim — forgetting an auth check is a compile
  error rather than a pentest finding — was a sentence in a report and not something a reader of the
  guide ever saw happen. It happens now: `owned` declares `uses cap.session`, and dropping the call
  to it from `validate` is `B0412`, "requires a capability nothing can discharge", because a
  `Session` reaches exactly one place in a Beck program.
  **The placement table is the demonstration rather than the prose.** §86.5 used to say "if you
  later give `validate` a call that reads a secret, the secret provably cannot reach the browser" —
  a promise about a program the guide did not contain. One `uses` clause on one definition moves
  **four rows** of that table: `owned` to the server because no tier below one discharges a
  capability, `validate` after it because it calls it, and `events` after that, off the data tier.
  `@on(client)` on `owned` is `B0401` rather than an override.
  **A `property` block is the second thing §86.12 listed and no longer does**, and it was added
  because it earns the space: break `apply_event` so `Finished` inserts a book and the generator
  finds it in 4 inputs and *shrinks* to `[Finished{title: }]` — one event, the shortest title there
  is, which is §21.3 rule 5's whole argument in one line of output.
  **And the table is now held to the compiler.** Everything else the guide asserts is run — the
  programs compile, their tests pass, the commands exist — but a **transcript** is prose that looks
  like evidence, and it was the one shape that could rot without the reader being able to tell.
  `getting_started.rs::the_placement_table_in_the_guide_is_the_one_the_compiler_prints` parses the
  rows out of the `$ beck explain place` block and matches them against `place::report` for
  whichever of the guide's programs places to them — by content, because the guide builds one
  program up over several sections and they are all called `shelf.beck`. Editing one tier in the
  table turns it red.
