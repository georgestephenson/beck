- **2026-09-07 — `psql`'s `\d` works, because `pg_catalog` is a read model.**
  The last clause of [`docs/08`](../docs/08-roadmap.md)'s Phase 3 exit row "Can my DBA see the
  data?" — the SQL had joins and there was nothing for one to join.
  [`beck-core/src/pg.rs`](../compiler/crates/beck-core/src/pg.rs) derives fourteen relations from
  the `Schema` the compiler already produces: `pg_class` has a row per read model, `pg_attribute` a
  row per column, `pg_namespace` two, `pg_type` the four scalar types, and the nine that describe
  what a read model has no object of — policies, publications, inheritance, access methods,
  defaults, collations — have their columns and **no rows**, which is what `\d` needs to hear on
  its way to printing a table. `\d`, `\d <table>`, `\dt`, `\dn` and `\l` all answer.
  **They are tables, not a case in the wire protocol**
  ([`adr/0032`](../docs/adr/0032-pg-catalog-is-a-read-model.md)), and that is the whole of the
  design. A catalogue answered by matching `psql`'s query text is less code and works until the
  strings change; what it costs is a second description of the schema that no `select` can reach,
  agreeing with the first by inspection. These are scanned, filtered and joined by the same parser,
  the same `Op::Join` and the same operators as `select * from todos`, so they cannot disagree with
  the schema — they *are* the schema under another set of column names — and
  `select relname from pg_class where relnatts > 3` is a query rather than a miss. Nothing in
  [`pgwire.rs`](../compiler/crates/beck-rt/src/pgwire.rs) knows what a backslash command is.
  What it cost the SQL is the **scalar** half of a query, because `psql` writes one: `case`, a
  function call, a cast, `in`, `is`, `||`, the four POSIX regular-expression operators, a
  `left join`, a comma join, `union`, and a scalar subquery. Two of those are decisions rather than
  features. **A `case` is lazy and a cast is refused at evaluation rather than at parse** — `psql`
  writes `case when c.reloftype = 0 then '' else c.reloftype::regtype::text end`, and refusing the
  statement for a branch no row reaches would refuse `\d`. **A scalar subquery is answered only
  where it is provably empty**: dropping the conditions that mention the outer query can only add
  rows, so a widened query that answers nothing proves the original answers nothing, and NULL is
  then the same answer for every row — computed once. Where it does have rows the answer depends on
  the correlation and is refused by name. The regular-expression matcher is a **simulation over a
  set of states**, `O(pattern × text)` for every pattern rather than exponential for `^(a|a)*b$`,
  because the pattern arrives from a client.
  Two defects fell out on the way. `a = 1 or b = 2 and c = 3` parsed as `(a or b) and c`, because
  the `where` was a conjunction of disjunctions and could not hold the other reading; it is one
  expression now and binds the way SQL says. And a `where` was resolved against the table's own
  name rather than the name the query gave it, so `from people p … where p.name = 'Ada'` found no
  column `p.name` on the path that pushes a filter into a scan.
  **The gate is a real `psql` binary** ([`beck-cli/tests/psql.rs`](../compiler/crates/beck-cli/tests/psql.rs)),
  not a client written here: [`82`](../docs/82-the-edge-report.md) §82.10's four gates that could
  not fail were each checked by something written beside them, and a `\d` answered by our own
  client would be a `\d` we had defined. It skips loudly without one and `BECK_REQUIRE_PSQL=1`
  forbids the skip. Its negative half is the row count — the fourteen catalogue relations *are* in
  `pg_class`, hidden by `nspname <> 'pg_catalog'` over a `left join`, so a catalogue in one
  namespace lists all sixteen. That assertion is what caught the bug it was written against: a
  `where` over the null-supplying side of a left join **must not** be pushed into that table's
  scan, because removing a row there does not remove the joined row, it fills it with nulls the
  term would have rejected — and `\d` answered with every relation it had asked to hide.
  The cost is stated at two sizes, because one cannot tell linear from quadratic: `\d` over 50 read
  models is **9,674 backend steps** and over 400 is **62,524** — 0.8× per relation across 8× the
  relations, so the catalogue is built once per query rather than once per row
  (`cargo test -p beck-cli --test read_models answering_a_catalogue -- --nocapture`). The
  regular-expression matcher is `O(pattern × text)` by construction and the expression evaluator is
  `O(1)` per node per row.
  What is refused is what a read model has no object of, **by the name of the relation asked for**:
  `\df` is told there is no `pg_catalog.pg_proc`, not that the program has no functions. Those are
  different claims and only the first is true.
