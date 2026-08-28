- **2026-08-16 — A non-durable fold says what it is, and the reason it is unbuilt is written down.**
  A program whose only accumulator is a `fold` nobody wrapped in `durable` was reported as *a
  library with no durable state* — which sends its author to add the `durable` they deliberately
  left off. **B0519** names the construct instead ([`docs/10`](../docs/10-decisions.md) D1), says it is
  decided rather than built, and says what stands in the way. The construct itself is still unbuilt,
  and the investigation is why: an accumulator outside the log is **not a function of the log**,
  `replay.rs` asserts `digest(replayed) == digest(live)`, and D3 rests on that digest — so the first
  question is what the digest covers, which is a decision and not a branch. The volume half of D1's
  own motivation is untouched by any of it, because [`docs/03`](../docs/03-type-and-effect-system.md)
  §3.7 logs **every validated event**: a cursor that moves a hundred times a second writes a hundred
  entries whether or not the accumulator is durable, so an un-journalled accumulator is not an
  un-journalled stream. `DEFECTS.md::non-durable-fold` is rewritten around that finding, and
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.8's Wall 1 gains a survey of what
  Redux, Remix, SwiftUI, Akka and Phoenix LiveView do — they agree that the lifetime is a
  declaration and that the assignment is by audience — and a recommended order of four homes, marked
  as a recommendation because adopting it wants a D-number.
