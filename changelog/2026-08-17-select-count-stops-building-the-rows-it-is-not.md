- **2026-08-17 — `select count(*)` stops building the rows it is not going to return.**
  [`docs/23`](../docs/23-incremental-views-report.md) §23.19's last open row: "the plan's `list_len` is
  ±1 per delta; the SQL count is over the rows it scanned". So asking `psql` how many todos there
  are cloned every todo out of the collection and built a `Cell` per column of every one, to answer
  with a single integer — while `Op::Count` two layers down had read the size in `O(1)` since the
  engine existed. `read::Rows::count` is the seam, `Reader::len` answers it for a maintained
  arrangement, and a `Map` or a `list` in the accumulator answers it directly. **The default is
  "not without a scan"**, so a reader that does not implement it falls back and is exactly as
  correct and exactly as slow as it was — the seam cannot make a reader wrong, only faster. Gated by
  `read_models.rs::a_bare_count_is_answered_without_building_a_row`, whose instrument is the
  assertion rather than a measurement of one: a reader that knows the size and **refuses to produce
  a row**, so a query that scanned cannot be answered by it at all, with a second case proving the
  refusing reader really refuses. `a_count_that_narrows_anything_still_scans` fixes where the fast
  path stops — a `where`, `order`, `limit` or `offset` each still scan, because each is applied
  before the count collapses. [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.7
  attributed this row to grouping and is corrected: not every aggregate question is a grouping
  question, and the ungrouped one needed none of it.
