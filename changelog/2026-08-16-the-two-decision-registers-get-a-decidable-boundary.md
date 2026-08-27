- **2026-08-16 · #69 — The two decision registers get a decidable boundary, and ADR identities get
  a gate.** "Design decisions there, engineering decisions here" was a judgement about intent and
  went both ways at least six times — [`adr/0010`](../docs/adr/0010-generic-arithmetic-through-a-prelude-trait.md),
  [`0011`](../docs/adr/0011-identifiers-are-snake-case-in-the-python-surface.md),
  [`0013`](../docs/adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md),
  [`0014`](../docs/adr/0014-a-keyed-digest-is-the-one-declassifier.md),
  [`0017`](../docs/adr/0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md) and
  [`0007`](../docs/adr/0007-evaluator-stack-is-declared-not-discovered.md)/[`0012`](../docs/adr/0012-the-front-end-counts-its-own-recursion.md)
  each state a rule a Beck program lives under. The rule is now **a D-number is a rule a Beck
  program lives under; an ADR is a choice only the compiler lives under**, tested by whether a user
  could observe it without reading our source. Nothing is moved: a record is immutable and cited by
  identity from `front_end_bound.rs`, `lib/README.md` and `AGENTS.md`, so relocating one would break
  the citations and the immutability that is the difference between the registers.
  `docs/adr/README.md` also stated "D1–D20 stays as is" while the file held D1–D29.
  **The defect the check was written for**: `0023-tls-and-the-signature-it-brings.md` was titled
  `ADR 0022` — a real record's number — since the day it was written, so a citation to 0022 landed
  on the wrong decision, and `docs.rs`'s numbering gate excludes `docs/adr/`.
  `docs.rs::an_adr_is_numbered_for_the_file_it_is_in_and_is_listed` now holds three properties —
  title agrees with filename, no two records claim one number, the index names every record — each
  proved red by perturbation before the fix went in.
