- **2026-08-21 — The differ is half of what an event costs, and it only became visible once the
  page stopped being copied.**
  Making a page's children shared handles took one event on a 5,000-row page from 14,827 µs to
  697 µs — and moved the bottleneck. `Feed::Dom` does two things per event: it maintains the page,
  and it structurally diffs it against the one the client holds.
  [`docs/23`](../docs/23-incremental-views-report.md) §23.8's table has only ever shown the first.
  Measured end to end, the diff is **54% of the per-event cost at 5,000 rows** — it was the smaller
  half before, because only one of the two got cheaper. (The trim below takes that share to 39%.)
  `measure_incremental.rs::what_one_event_costs_the_runtime_end_to_end` is the measurement and the
  table is in §23.8 beside the render's.
  **`diff_keyed` now trims the ends the two pages physically share** — a run of children that are
  the same allocation in both needs no ops and no examination — worth 203 µs → 71 µs at a thousand
  rows and 1,961 µs → 1,036 µs at five thousand, and available only because a child is a handle now.
  The gate is a **differential rather than an assertion**: every scenario runs twice, once shared and
  once through `rehash`, which rebuilds node by node and shares nothing, and the two op streams must
  be equal. The differ's other tests could not have caught a wrong index — they build every node
  fresh, so nothing in them is ever shared and the trim never runs.
  **What is left is not a differ problem.** The residue is `keyed`, which hashes every child's key
  to decide whether the list reconciles by key at all, before any trim can help; checking only the
  trimmed window would change which ops a mixed-key list emits, which is a worse trade than the
  constant is worth. Given two pages and nothing else, what moved has to be rediscovered — so both
  halves of the per-event cost now point at [`docs/08`](../docs/08-roadmap.md) §8.5.4's open item, the
  engine emitting patches from the changes it already holds.
