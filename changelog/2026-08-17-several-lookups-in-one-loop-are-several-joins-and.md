- **2026-08-17 — Several lookups in one loop are several joins, and the refusal that used to
  replace them was silent.** Two follow-ons to the join below, both found by using it.
  `beck explain cost` printed the *cost* of a loop it had not read as a join and not the reason:
  the reason is recorded on the `map_list` the decomposition builds, every `ui:` loop then fuses that
  `map_list` into the `flatten` above it, and the survivor kept its own empty field — so the one
  shape the explanation exists for was the one shape that never printed it. Fusion now carries it,
  gated by `incremental.rs::a_loop_that_is_not_read_as_a_join_says_why_after_fusion`, which asserts
  the reason appears *under the line it explains*. What that then exposed was a restriction with no
  design behind it: a body looking up in two collections was refused, which left the capture in place
  and the whole collection reconsidered per event. Refusing a shape that keeps a program at `O(n)` is
  not the conservative choice. It is now a **chain** of joins, one per lookup, each taking the
  previous one's rows on its left — so `corpus/33-awareness.beck`, which renders a person's
  whereabouts *and* their note, costs its delta. One of its two lookups is against the **awareness
  roster**, which [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.5 decision 3
  expected would have to be refused because a roster moves when `seq` does not; it does not have to
  be, because the second clock is a problem for *sharing* and not for *joining*, and that paragraph
  is corrected in place. §99.3's sweep is re-run: **16 capture sites in 8 programs and one that moves
  per event**, down from 18 in 10 with three. The one left is `examples/board.beck`, which groups
  rather than looks up.
