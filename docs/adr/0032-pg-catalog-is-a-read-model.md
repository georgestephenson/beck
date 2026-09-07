# ADR 0032 — `pg_catalog` is a read model, not a case in the wire protocol

**Status:** accepted
**Date:** 2026-09-06
**Context:** [`23`](../23-incremental-views-report.md) §23.19,
[`12`](../12-standards-and-conformance.md) §12.5, [`08`](../08-roadmap.md)'s Phase 3 exit table,
[`0020`](0020-the-read-model-speaks-pgwire-by-hand.md)

## The decision

`psql`'s `\d` is answered by **fourteen relations that are read models** — `pg_class`,
`pg_namespace`, `pg_attribute`, `pg_type`, `pg_database` and nine that describe what a read model
has no object of — derived in `beck-core/src/pg.rs` from the same `Schema` the compiler already
produces, and read by the same parser, the same `Op::Join` and the same scan as `select * from
todos`.

Nothing in `beck-rt/src/pgwire.rs` knows what a backslash command is.

## The alternative, and why it is worse

The obvious implementation is to recognise the query text. `psql` sends a fixed set of statements
per version; a server can match on them and answer each from the schema directly. It is less code,
it needs no SQL beyond what already existed, and it works for exactly as long as the strings match.

What it costs is the thing this project keeps buying: **one code path**. A catalogue answered by
matching text is a second description of the schema, agreeing with the first by inspection. It
drifts the moment a table's columns are derived differently, and nothing catches the drift because
no `select` can reach it — a person cannot ask the catalogue a question the matcher was not written
for, so `select relname from pg_class where relnatts > 3` is not a query, it is a miss. The
[`99`](../99-the-data-tier-means-of-combination.md) §99.9 item 9 argument for compiling the SQL's
joins into the plan rather than interpreting them is the same argument one level up, and it applies
whether the second implementation is a join or a catalogue.

Making the relations *tables* has the opposite property. They cannot disagree with the schema
because they **are** the schema under another set of column names, they are filterable, joinable
and groupable like anything else, and every query that reads them exercises the operators every
other query uses.

## What it cost

The catalogue is not free: `psql` writes SQL that a read model's SQL did not have. What was added
is the **scalar** half of a query — `case`, a function call, a cast, `in`, `is`, `||`, the four
POSIX regular-expression operators, a `left join`, a comma join, `union`, and a scalar subquery.

Two of those are worth recording as decisions of their own.

* **A `case` is lazy, and a cast is refused at evaluation rather than at parse.** `psql` writes
  `case when c.reloftype = 0 then '' else c.reloftype::pg_catalog.regtype::pg_catalog.text end`,
  and this catalogue cannot compute the `else`. Refusing the statement for a branch no row reaches
  would refuse `\d`. So an expression is parsed in full and refused **by name** only when a row
  asks it for a value — which is also SQL's own rule for `case`, rather than a licence invented
  here.
* **A scalar subquery is answered only where it is provably empty.** Dropping the conditions that
  mention the outer query can only *add* rows, so a widened query that answers nothing proves the
  original answers nothing, and a scalar subquery with no rows is NULL — the same answer for every
  outer row, computed once. Where the widened query does have rows the answer depends on the
  correlation, and it is refused by name. Every scalar subquery `psql` sends the catalogue asks
  about a column default or a collation, and a read model has neither, so the empty case is the one
  that happens; the refusal is what would happen if that stopped being true.

The regular-expression matcher is a **simulation over a set of states**, not a backtracker, because
the pattern arrives from a client: `O(pattern × text)` for every pattern there is, rather than
linear for the ones people usually write and exponential for `^(a|a)*b$`.

## What is refused, and how

A relation that is not one of the fourteen is refused **by the name it was asked for**, with the
list of the ones that exist. `\df` is told there is no `pg_catalog.pg_proc`; it is not told that
the program has no functions, because those are different claims and only the first is true. The
five commands that work are `\d`, `\d <table>`, `\dt`, `\dn` and `\l`.

Every read model is `relkind = 'r'` — an ordinary table — including one maintained by an
arrangement. [`05`](../05-tier-lowering.md) §5.3's promise is that a tool "sees materialized views
as ordinary tables", and a relation answered as `'m'` sends `psql` looking for a view definition
this has no SQL to give it. What a table is derived *from* is `beck_columns`, which is the question
`pg_catalog` has no column for.

## What this does not change

[`0020`](0020-the-read-model-speaks-pgwire-by-hand.md)'s three decisions all stand: the protocol is
still written here, the port still has no authentication and no transport security and therefore
still binds to loopback only, and it is still off by default. A catalogue is a thing to *read*, and
reading is what this port already was. The only bullet of that record this changes is its "what
this rules out" list, which named `\d`.

## The gate

`beck-cli/tests/psql.rs` drives the **`psql` binary**, not a client written here.
[`82`](../82-the-edge-report.md) §82.10's pattern is that a gate written by the person who knows
the gap tests the shape of the fix rather than the shape of the gap, and a `\d` answered by our own
client would be a `\d` we had defined. It skips loudly without a `psql` on the path, and
`BECK_REQUIRE_PSQL=1` forbids the skip.

The negative half is the count: the fourteen catalogue relations *are* in `pg_class`, and `psql`
hides them with `nspname <> 'pg_catalog'` over a `left join`. A catalogue that put everything in one
namespace — or a `where` pushed into the null-supplying side of that join, which is the bug this
was written against — lists all sixteen. The gate asserts on the number of rows as well as on the
names, so it cannot pass by listing too much.
