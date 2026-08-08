# 88 — Phase 3 report, part 56: the read model is the arrangement

> **What this is**: [`05`](05-tier-lowering.md) §5.3's read models and their pgwire exposure, built
> — the largest remaining piece of the incremental-views bullet, and the one
> [`26`](26-arrangement-sharing-report.md) §26.9, [`51`](51-arrangement-lifecycle-report.md) §51.7
> and [`67`](67-sqlite-report.md) §67.1 have each named as untouched.
>
> The design changed while it was being built, and the change is the report. §5.3 says a read model
> is "generated tables in the same Postgres". It is not, and it should not be: **a read model is the
> collection the fold already holds and the arrangement the view engine already maintains,
> projected as relations.** Nothing is written on the append path, no projection exists to lag
> behind, and there is no second code path that can drift from the page. What decides which signals
> are tables is the *same cut* §5.3 draws for arrangement sharing — a table is a view that does not
> depend on who is asking — so this feature is `per_session` used a second time rather than a new
> analysis. [`10`](10-decisions.md) D26 is the decision and
> [`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md) the wire.
>
> Measured: across the 31-program corpus, **43 tables** with no annotation of any kind — 39 read
> from the accumulator, 4 from the maintained dataflow. Keeping one fresh costs **the same work at
> 200 rows and at 1,600**: two entries touched, one function applied, eight operators recomputed,
> at both sizes.

## 88.1 What §5.3 asked for, and which half of it was the point

The row, verbatim:

> | Read models | generated tables in the same Postgres | One-shot queries and **pgwire access for
> the outside world**: `psql`, BI tools, DBeaver see materialized views as ordinary tables — the
> single cheapest trust-builder for adopting teams |

Two claims, and building it separated them. The middle column is an *implementation*; the right
column is the *value*, and it is a claim about somebody outside the project being able to see what
an application holds using a tool they already have. [`09`](09-risks-and-open-questions.md) §9.1 R6
makes the same point from the other side, as the mitigation for "DBAs can't see the database":
"read models are ordinary Postgres tables, browsable via pgwire — the outside world sees tables, not
theory".

Nothing in that value depends on the rows being in Postgres. It depends on them being *reachable by
a Postgres client*, which is the right-hand column, and on their being correct, which is the part a
second copy makes harder rather than easier.

## 88.2 Why the durable projection is the wrong half, in the order the reasons bite

**It puts view maintenance on the write path.** This is not a new argument; it is
[`26`](26-arrangement-sharing-report.md) §26.2's, which asked who advances the shared dataflow and
answered "not the sequencer":

> Putting view maintenance under the write lock would move it onto the *write* path — every command
> paying for the views of every connected session before its ack — and it would do that work for
> states nobody is looking at.

A durable projection is that, with a weaker case. A subscriber at least exists while it is being
paid for; a read model's reader is a BI tool that connects twice a day, and paying per event for a
table nobody has queried since Tuesday is the same mistake with a longer idle period.

**It is a second code path over the same events.** The engine's correctness argument is that
recompute is the oracle: every corpus program, every event, maintained page against recomputed page,
byte for byte ([`24`](24-incremental-views-report.md)). A projection written beside the dataflow is
covered by none of that, and "the read model and the page disagree" is the class of bug that shows
up in a support ticket rather than in CI.

**It doubles the storage** of every maintained collection, to hold a copy of what is already in
memory, arranged in the order it is already arranged in.

So the tables are the arrangements. `beck_core::read::Schema` derives the relations; a query reads
them; nothing is written.

## 88.3 Where the tables come from, which is one rule applied twice

| Table | Rows | Read from |
|---|---|---|
| a collection-valued field of the accumulator | its elements | the state value |
| the accumulator's remaining scalar fields | exactly one | the state value |
| a declared signal that does not read the session | its elements, or exactly one | the maintained node |

The third row is the interesting one and it needed no new analysis. `Plan::per_session` has been a
field on a plan node since [`24`](24-incremental-views-report.md), and §5.3's fanout argument is
about exactly the operators for which it is false. A SQL client has no session — nothing about
`psql` says who is asking — so the signals it can be shown are the ones whose value does not depend
on that, which is the same set, arrived at from the other direction. `page` is excluded by its type
rather than by its name: `Html` is not a relation.

The first two rows are the base tables, and they are read from the accumulator rather than from an
arrangement on purpose. A base table's rows *are* the fold's collection; a scan is `O(rows)` in any
database, and asking the dataflow to maintain a copy of a map in order to serve a scan of that map
would be the doubling above in miniature.

What the corpus makes of that, with no annotation in any of the 31 programs:

| | |
|---|---|
| programs with at least one table | **31 of 31** |
| tables, excluding the catalogue | **43** |
| read from the accumulator | 39 |
| read from the maintained dataflow | 4 |
| one-row tables (a record or a scalar) | 15 |

`beck explain sql` prints the schema as `create table` statements — DDL nothing executes, because
there is nothing to create. It is the shape a person needs in order to write the query they were
going to write:

```console
$ beck explain sql examples/todo.beck
-- the elements of Todo, from state.todos
create table todos (
    id                   text not null,
    text                 text not null,
    done                 boolean not null,
    owner                text not null
);
```

`Id` is a `newtype[Str]` and is `text`: newtypes and aliases are resolved, models and unions are
not. The four SQL types are `bigint`, `double precision`, `boolean` and `text`, and the choice of
four is a wire decision rather than a taste one — every one of their OIDs is in every driver's
built-in table, so nothing ever has to ask a catalogue that does not exist what it just received.
Anything else — a list, a map, a nested record, a union variant — is `text` holding the JSON
`Value::to_json` already produces for a browser.

**`Option[T]` is where SQL's null comes from, and it is the only place.** Beck has no null; a
column is nullable exactly when its field is an `Option`, `None` is `NULL`, and a non-nullable
column can never hold one. That fell out of the type mapping rather than being designed, and it is
the tidiest thing in this change.

## 88.4 The accumulator is not a table, and one field is called `distinct`

Two things the first working version got wrong, both found by reading its output over the corpus
rather than by a test.

**A `Signal[State]` was a table.** Every corpus program declares its fold as a signal —
`feed: Signal[State] = durable(fold(…))` — so the derivation dutifully produced a one-row table
called `feed` whose only column was `posts`, holding the entire map rendered as JSON. The same data
as the `posts` table beside it, in the shape nothing can query. The rule now is that a fold's own
name is not a table: its collections and its scalars are, which is what the three rows in §88.3 say
and what makes `select * from posts` the obvious thing rather than one of two.

**`corpus/17-derived.beck` has a `Summary` with a field called `distinct`.** It is an ordinary
Beck identifier and a SQL reserved word, and `create table summary (distinct bigint)` does not
parse anywhere. Beck's namespace and SQL's are not the same namespace and never will be, so the
generated DDL quotes any name that needs it, and the person reading the schema sees
`"distinct" bigint not null` — which is where they find out they have to write
`select "distinct" from summary`. A projection into real tables would have hit this at
`CREATE TABLE` time in production rather than in a generated page.

## 88.5 A query is a reader, and it holds a snapshot

Two decisions about *when* a query sees things, and both are consequences of there being no
projection.

**A query advances the dataflow itself.** [`26`](26-arrangement-sharing-report.md) §26.2's rule is
that the first subscriber to render at a new version advances the shared prefix; a SQL query is a
renderer that produces rows instead of a page, so it takes exactly that path. The consequence is
the freshness story, and it is the strongest thing here: **a `select` issued after an ack sees that
ack's event**, with no subscriber connected, no projection written and no lag waited out. There is
nothing to be stale.

**A pgwire connection is a member of the reader set.** [`51`](51-arrangement-lifecycle-report.md)
made the shared dataflow release its arrangements when the last reader goes; a SQL client holding a
connection is a reason to keep them — it is going to ask again — and a client that has disconnected
is not. So `SharedDataflow::reader()` enters the same set `subscriber()` does, and its `Drop`
leaves it. Its frontier stays at `UNRENDERED`, deliberately: a reader that never applies a delta
cannot use the change history, and pinning any of it for this reader would retain history nobody
reads.

**A query runs under the accumulator's read lock.** The sequencer commits under the write lock, so
while a query runs nothing can move the state — and therefore nothing can advance the shared
dataflow past the version the base tables were read at. Two tables in one query cannot disagree
about which events have happened.

The cost is stated rather than hidden: **a scan of a large table delays the next commit by the
length of the scan.** The alternative was to clone the accumulator (`O(1)`, it is persistent) and
let the arrangements move underneath, which is cheaper for the writer and gives a query that sees
two versions at once. For a read model whose entire argument is that it cannot disagree with the
page, that is the wrong trade.

### What it costs per event, at two sizes

Two claims, both counted rather than timed, because
[`beck_core::engine::Work`](../compiler/crates/beck-core/src/engine.rs) counts entries touched and
operators recomputed and a wall-clock threshold on a shared runner is
[`13`](13-testing.md) §13.7's flake.

1. **Nothing per event.** A connected client that has asked nothing leaves the write path exactly as
   it was: `advances()` is **0** after 200 and after 1,600 committed events.
2. **The delta per query.** The first query after an event advances the dataflow, and that advance
   does not grow with the collection:

| rows already in the collection | entries touched | functions applied | operators recomputed | entries materialised |
|---|---|---|---|---|
| 200 | 2 | 1 | 8 | 0 |
| 1,600 | 2 | 1 | 8 | 0 |

`scaling.rs::a_read_model_costs_nothing_per_event_and_a_delta_per_query` is the gate, and it is a
shape rather than a rate: eight times the rows for the same work, with a 3× bound because
[`64`](64-compile-speed-report.md)'s pattern is what does not flake.

## 88.6 What is not built, itemised

The honest column, because "read models and pgwire" was one line in a bullet and is now several
different states of built.

| | Status |
|---|---|
| Tables derived from the program, with no annotation | **built** — 43 across the corpus |
| A Postgres client can read them | **built** — `tokio-postgres` drives both protocols in CI |
| `beck explain sql` | **built** |
| A durable projection, and "append and project in one transaction" | **not built, and now deliberately so.** [`67`](67-sqlite-report.md) §67.1 was loud that SQLite's transaction property is *available and unused*, and it still is. [`10`](10-decisions.md) D26 is the record of why, and of what would reopen it: a read model that has to survive the process, or be reachable by a tool that cannot reach this port |
| Joins, subqueries, `group by`, aggregates other than `count(*)`, `distinct` | **not built**. One table at a time. A query language is what [`04`](04-compiler-architecture.md) §4.2's `Query` sub-language is *for*, and this is not it |
| `count(*)` without scanning | **not built**. The plan has a `list_len` operator that is ±1 per delta ([`24`](24-incremental-views-report.md)); the SQL count is over the rows it scanned, so `select count(*)` is `O(rows)` where the engine already knows the answer. The obvious next thing, and not done |
| `pg_catalog`, and therefore `psql`'s `\d` | **not built**. `select * from beck_columns` is the substitute and it is a table rather than a special form |
| TLS | **not built**. `sslmode=prefer` negotiates down and works; `sslmode=require` does not connect |
| Authentication | **not built**, and the port is loopback-only and off by default because of it ([`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md), [`43`](43-threat-model.md) §43.4) |
| Writes | **refused by name**, at any privilege. The log is the only way state changes |
| A `per_session` view as a table | **not built and not planned**. A SQL client has no session |
| The event log as a table | **not built**. It is a scan of the store rather than of memory, and it is the one table that would need the substrate |
| Query fusion on symbolic plans, `beck explain query`, `beck explain cost` | **nothing**, unchanged from [`26`](26-arrangement-sharing-report.md) §26.9 |
| The render lock | **still here**, unchanged from [`51`](51-arrangement-lifecycle-report.md) §51.7 |

## 88.7 Three things this leaves open

**Nothing in the language says "publish these read models".** The flag is a runtime decision, which
means [`06`](06-kubernetes-and-packaging.md) §6.5's whole argument — that what a program exposes is
derived from what the program says — does not apply to this port. There is nothing to derive it
from. An effect atom or a signal annotation is what would give it one, and that is a language
decision rather than a runtime one.

**The catalogue is a table this project invented.** `beck_columns` is discoverable, honest and not
what any tool expects. A tool asks `pg_catalog`; the correct long-run answer is probably a small
read-only emulation of `pg_class`, `pg_attribute` and `pg_namespace` — which needs joins, which
needs the query language above. The two are the same item.

**Nothing has been tried against a BI tool.** [`12`](12-standards-and-conformance.md) §12.5 claims
verification "against `psql`, JDBC and BI drivers in CI" and this delivers one Rust driver. The row
is corrected rather than left to be read as met.

## 88.8 What landed alongside, and why it is in this report

Two documentation changes, recorded here because they are part of the same commit and neither is
large enough to be its own report.

**The getting-started guide is now published.** [`86`](86-getting-started.md) has existed since it
removed [`08`](08-roadmap.md) §8.5.4's named blocker, and the site
`.github/workflows/docs.yml` builds carried the generated reference, two module pages and rustdoc —
and not the guide. So the one document written *for* an outside developer was the one the outside
world could not read without cloning the repository. `beck doc guide` renders it into the same
shell as everything else, `--link-base` rewrites its repository-relative links against the commit it
was built from, and the site index links to it.

The renderer is a subset of Markdown rather than an implementation of it — headings, fenced code,
quotes, tables, bullets, inline code, emphasis, links — which is the same argument
[`adr/0016`](adr/0016-the-language-server-speaks-json-rpc-directly.md) and
[`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md) make about a protocol: what is
needed is bounded, and a book is what [`07`](07-dependencies.md) §7.7's mdBook line is for. It
found one thing worth knowing, which is that `docs/86` quotes a fenced block inline using a run of
**four** backticks; a renderer that treats the next backtick as the close cuts the sentence in half.
Code spans are delimited by a run, and the closing run has to be the same length.

The site has two kinds of page now and they are honest in different ways, which is worth saying
plainly: a reference page is *derived* from the compiler and a drift gate holds it there; a guide is
*written*, and what holds it is that `beck-cli/tests/getting_started.rs` compiles and runs every
program in it. `docs.rs::the_published_guide_is_the_checked_guide` asserts the published page is
made from that same file, and counts its code blocks — a renderer that silently dropped one would
publish a tutorial with a step missing.

**[`08`](08-roadmap.md)'s exit-criterion table said "there is none" about the tutorial.** It has
been wrong since [`86`](86-getting-started.md) landed, in the row that is the criterion, which is
the worst place for a document to rot. Corrected in the same change.

**And the instruction that would have caught this change's own defect did not run.** `AGENTS.md`'s
verification list ends with a rustdoc build under `-D warnings`, written without quotes around the
flag. A shell reads that as the assignment `RUSTDOCFLAGS=-D` and then runs the *next word* as the
command: it fails with "command not found", `cargo` is never invoked, and the step looks like it
verified something. It has presumably never run. CI is unaffected — the workflow sets the variable
in YAML, where no shell splits it — so the failure mode is exactly the dangerous one: the local
check is silent and the remote one is the first to speak. Two broken intra-doc links in this change
reached CI that way.

The fix is one pair of quotes, and the gate is
`docs.rs::every_shell_command_in_the_instructions_runs`: an environment assignment in `AGENTS.md`
whose value begins with a flag must be quoted, or the word after it is the command. It was checked
against the original text before the original text was fixed — which is the discipline
[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5 asks for, and the fifth gate in this
project's history that could not have failed. The pattern holds for all five: this one, too, would
have been written by the person who already knew the answer.

## 88.9 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`05`](05-tier-lowering.md) §5.3 | "Read models: generated tables in the same Postgres" — the middle column is superseded ([`10`](10-decisions.md) D26). The right-hand column is met |
| [`03`](03-type-and-effect-system.md) §3.10 item 5 | "materialized read models; pgwire exposure … untouched" — built, and not *materialized* in the sense the line assumed |
| [`07`](07-dependencies.md) §7.4 | The pgwire row's alternatives column said "none", which meant there is no alternative *protocol*. There are alternative *implementations* and one was refused; the row now says which and why |
| [`12`](12-standards-and-conformance.md) §12.5 | "verified against `psql`, JDBC and BI drivers in CI" — one Rust driver, and `psql`'s backslash commands do not work |
| [`43`](43-threat-model.md) §43.4 | A new absence: no authentication on the read-model port, with the loopback bound as the compensating control |
| [`67`](67-sqlite-report.md) §67.1 | "this substrate is what it would be built on" — it is not, and D26 is why. The transaction property is still available and still unused |
| [`08`](08-roadmap.md) exit criterion | The tutorial row said "There is none". [`86`](86-getting-started.md) is one, and it is now published |
| `AGENTS.md`'s verification list | Its rustdoc step never ran: `RUSTDOCFLAGS` was assigned an unquoted value beginning with a flag, so the shell ran the next word as the command. Quoted, and gated (§88.8) |

## 88.10 What Phase 3 is still not

The incremental-views bullet is now **most of a bullet**: the plans, the recompute oracle, the
arrangement sharing, the lifecycle, and the read models with their pgwire exposure. What is left of
it is **query fusion on symbolic plans**, and `beck explain query` and `beck explain cost` behind
it, for [`20`](20-phase-2-report.md) §20.5's unchanged reason.

Beyond it, and unchanged: **no LLVM backend and no native codegen**; **no Mode B and no client
polish**; **no playground**; **no supply-chain tooling**; the OIDC relying party, `managed()`
provisioning, the claims mapping and presence ([`48`](48-identity-report.md) §48.5); the page is
still assembled and diffed rather than streamed as deltas ([`24`](24-incremental-views-report.md)
§24.6); `parallel:` still has no backend that runs two children at once
([`80`](80-a-scope-owns-its-children-report.md) §80.5). The exit criterion is a claim about a
person, and no outside developer has read the guide this change published.
