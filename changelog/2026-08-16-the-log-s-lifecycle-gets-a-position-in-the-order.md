- **2026-08-16 · #64 — The log's lifecycle gets a position in the order.** Segment archival,
  retention and the analytical substrate — Parquet on object storage, DataFusion over the archive —
  are scheduled in [`docs/08`](../docs/08-roadmap.md) Phase 4 and §8.5.4 (class G); five documents had
  committed to them and none gave them a position. Nothing is built, and the corrections ride
  along: ClickBench waits on the archive rather than the incremental engine, `docs/03` §3.7's
  present-tense `durable(retain=…, snapshot=…)` does not parse and now says so,
  [`docs/09`](../docs/09-risks-and-open-questions.md) R6 catches up with D26, and a visualization
  vocabulary is recorded as an open question rather than a plan (`docs/09` §9.6).
