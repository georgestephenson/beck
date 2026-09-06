- **2026-08-22 — The read model's SQL grows a `join`, a `group by` and a `distinct` by compiling
  into the plan.** [`docs/99`](../docs/99-the-data-tier-means-of-combination.md) §99.9 item 9, which
  closes [`docs/23`](../docs/23-incremental-views-report.md) §23.19 and
  [`docs/12`](../docs/12-standards-and-conformance.md) §12.5 together. A `psql` client could read every
  table a program has and could not relate two of them, and the way a hand-written SQL interpreter
  grows a join is a nested loop — a second join in this project, agreeing with the engine's by
  inspection, covered by nothing. **So the surface joins nothing.** `beck-core/src/query.rs` writes
  the Beck expression a person would have written — `select … join … on b.k = a.k` becomes
  `for x in a: for y in b where y.k == x.k`, `group by g` becomes the loop over `list_unique` that
  `corpus/35-workload.beck` writes by hand — and hands it to `Plan::of_query`, where §99.6's
  recogniser emits `Op::ArrangeBy`, `Op::Join`, `Op::GroupBy` and `Op::Distinct`. There is one join
  in this project, one of each aggregate, and one `distinct`. Joins are **left-deep**, one stage per
  `join`, because each stage has to be a `map_list` of its own for the recogniser to see it.
  **What it cost was two agreements rather than an operator**, and both would have been quiet bugs
  in a second interpreter: one rule for what a column *is* — `Table::row_values`, a newtype seen
  through and an `Option` flattened, shared by the scan's cells and the plan's rows, because a join
  comparing `Id("p1")` with `"p1"` answers no rows for two columns a person can see are equal — and
  a **named refusal** wherever SQL's equality is not `Value`'s: a nullable join key (a null would
  match a null) and a join across two types (no coercion). `sum` over anything but an `Int` is
  refused with [`docs/46`](../docs/46-standard-library-report.md) §46.16's own reason. Measured with
  the recognition switched off beside it per [`docs/08`](../docs/08-roadmap.md) §8.3 item 8: a join
  over two collections of 200 rows and of 1,600 costs **5,004 backend steps and 40,004** — flat per
  row — against **244,004 and 15,392,004** for the nested loop, gated by
  `scaling.rs::answering_a_join_in_sql_does_not_reconsider_every_pair`, whose instrument is
  `Work::steps` because the engine's own counters charge a refused join's inner scan as one
  application and see nothing. Ten gates in `read_models.rs` hold the answers, driven by
  `tokio-postgres`. `beck explain sql --query "<select>"` prints the operators and `--no-join` is the
  off switch. A query is answered **cold** — its plan is compiled, prepared and thrown away with the
  answer — so §23.19's "a read model costs nothing per event" is untouched.
