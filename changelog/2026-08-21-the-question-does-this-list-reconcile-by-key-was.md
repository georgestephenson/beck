- **2026-08-21 — The question "does this list reconcile by key" was most of what an event paid,
  and it was being asked twice.**
  With the trim and the rank query in place, what remained of the per-event diff was `keyed`:
  hashing every child's key into a set, on **both** lists, before either can help. On a page where
  one row changed that was **62% of the diff at 1,000 rows, 87% at 5,000 and 89% at 8,000** —
  measured, after [`docs/23`](../docs/23-incremental-views-report.md) §23.8 had twice claimed from
  reasoning that the remainder was not a differ problem. Both claims were wrong and the second is
  corrected in place, because the pattern is the finding.
  The children at the shared ends are the same allocations in both lists and so carry the same keys,
  so hashing them once answers for both; only the windows are hashed twice. **1.94–2.04× on the
  whole diff**, measured A/B in one process alternating between the two predicates so nothing about
  the machine separates them (53.7→26.3 µs at 1,000, 343.3→174.2 at 5,000, 593.2→306.0 at 8,000).
  The predicate computes the same answer, so no op moves — the shared/copied differential and the
  rank-structure oracle both hold it.
  **The bigger win was available and refused.** Narrowing the question to the window would leave one
  child to hash instead of 8,000, but then a page whose shared prefix repeats a key would reconcile
  by key while the same page built without sharing would not — and "the same two pages produce the
  same ops however they were built" is the property the trim is only sound under.
  `diff::tests::a_repeated_key_in_the_part_two_pages_share_forces_the_positional_path` is the gate,
  written because **nothing else in the file could fail for it**: a predicate that skipped the shared
  ends passed all fifteen other tests, which is `docs/82` §82.10's pattern caught in the act.
