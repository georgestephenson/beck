# 67 — Phase 3, part 36: SQLite, and the 26× that was a durability setting

**Built.** `beck_rt::SqliteLog` — a third durable `LogStore`, beside redb and Postgres, and
`--store sqlite` on `beck run` and `beck replay`.

One of the bullets [`26`](26-arrangement-sharing-report.md) §26.9 lists as untouched, scheduled by
[`08`](08-roadmap.md) and argued for by [`07`](07-dependencies.md) §7.8.1.

## 67.1 Why it was added, which is not speed

§7.8.1 said so in advance and the measurement did not change it:

> the same "log and read model in one transaction" property at single-node scale, so rungs 0–2
> become *semantically* identical to production rather than merely similar

**redb cannot offer that at any speed**, because it has no query language for a projection to be
written in. Postgres has it and needs a server. SQLite has it in a file.

That property is asserted rather than asserted-about:
`sqlite_can_append_and_project_in_one_transaction` creates an ordinary table, appends an event and
moves a projection **in one transaction**, then does it again and rolls back — and checks that
neither the event nor the projection survived. The second half is what makes the first half worth
anything.

The store reaches past `LogStore` to the connection to do it, because the trait cannot express the
property. That is the honest way to assert it today, and it is a signal about where the read-model
work goes rather than a hole.

## 67.2 What it is

Three tables in SQLite's dialect, the same shape as the Postgres DDL. `INTEGER PRIMARY KEY` is the
one spelling SQLite aliases to its own `rowid`, which is what makes it monotonic and gap-free for an
append-only table — the property `BIGSERIAL` gives above. There is no BRIN analogue and none is
wanted: the `rowid` *is* the physical order, so `WHERE seq > ? ORDER BY seq LIMIT ?` is already a
range scan of the table.

WAL, because a reader must not block the single writer — which is the shape of a log appended to
while it is being replayed. The connection is behind a `Mutex` rather than a pool: one writer is the
invariant §3.7 already depends on, and a pool would add contention to model concurrency the log does
not have.

The format stamp is the same refusal Postgres makes, for the same reason — postcard is not
self-describing, so a log read under another encoding does not fail, it produces plausible nonsense.
`a_sqlite_log_written_in_another_format_is_refused_too`.

## 67.3 The finding: a 26× that was a promise, not an engine

The first version set `synchronous = NORMAL`, which is WAL's commonly recommended pairing and which
`beck bench log` reported like this:

| substrate | append (batch) |
|---|---|
| redb | 9,218 /s |
| sqlite | **241,657 /s** |

**That number is real and it is not about SQLite.** At `NORMAL` a commit survives a process crash
and can be lost in a power loss; at `FULL` it is on the platter before it is acknowledged, which is
what redb and Postgres do. The table was comparing a weaker promise to a stronger one and calling
the difference speed.

At equal durability, three runs, on the same shared host:

| substrate | append (batch) | append (serial) |
|---|---|---|
| redb | 8,456 / 11,888 / 17,184 | 1,155 / 1,007 / 1,233 |
| **sqlite (`Fsync`)** | 7,194 / 8,391 / 15,073 | 485 / 997 / 1,153 |
| sqlite (`Relaxed`) | 186,446 / 219,975 / 224,380 | 57,664 / 54,937 / 60,089 |

Read as a shape rather than as rates — the run-to-run spread on redb alone is 2× — **the two durable
substrates are within noise of each other, and the relaxed one is about 19× either.** Which is
[`08`](08-roadmap.md)'s expectation ("the durable substrates are within ~16% of each other") holding
for a substrate it was not measured on, and it is why §67.1's reason for adding SQLite is the only
reason.

[`03`](03-type-and-effect-system.md) §3.7 makes the log the only description of a program's history,
so "an acknowledged event may vanish" is a change to what the system *means* rather than a tuning
knob. It is therefore a public `Durability` enum defaulting to `Fsync`, it is visible in `kind()` —
the relaxed store reports `sqlite-relaxed`, so no measurement can label the two the same — and
`beck bench log` prints **both rows**, so the flattering number is never the only one on screen.
[`adr/0017`](adr/0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md) records the
decision.

The general lesson is one this project keeps re-learning from a different direction, after
[`59`](59-havlak-report.md) §59.5's workload with no oracle and [`61`](61-deltablue-report.md)
§61.2's oracle with no workload: **a benchmark comparing two systems is comparing their
configurations, and a flattering result is a reason to check which.**

## 67.4 What it costs to depend on

A C compiler, for the first time in `beck-rt`. `bundled` compiles SQLite from the vendored
amalgamation rather than linking against whatever the host has, so a log written on a laptop and a
log written in CI are written by the same engine at the same version — worth a build step for a
history §3.7 makes load-bearing.

And a pin: **`libsqlite3-sys` 0.38 does not build on the pinned toolchain**, because its build
script uses `cfg_select!`, which is still unstable. `rusqlite` is therefore held at 0.37. That is a
thing to remember at the next toolchain bump rather than a defect, and it is written down here
because the failure mode is a confusing build error two versions from now.

Licences are inside [`adr/0004`](adr/0004-full-cargo-deny-gate.md)'s allowlist: `rusqlite` MIT,
`libsqlite3-sys` MIT, `hashlink`, `fallible-iterator` and `fallible-streaming-iterator` MIT/Apache,
and SQLite itself public domain.

## 67.5 What is **not** built

| | Status |
|---|---|
| A SQL read model | **not built**, and this is the point worth being loudest about. The transaction property §67.1 asserts is *available* and **unused** — nothing in Beck projects into SQL. [`26`](26-arrangement-sharing-report.md) §26.9's "SQL read models and pgwire" is untouched, and this substrate is what it would be built on |
| Rung 0's default changing | **not changed.** `beck run` still defaults to redb. §67.3's numbers do not justify a change, and "measure and let the number pick" — [`08`](08-roadmap.md)'s instruction — is answered with *the number does not pick*, which is a result rather than an omission |
| Litestream / LiteFS replication | **not built.** §7.8.1 names them as what makes SQLite viable beyond a laptop; nothing here touches replication |
| `beck build` emitting SQLite DDL | **not built.** `SQLITE_DDL` is a constant the store applies, not an artefact the compiler emits — unlike `DDL`, which §4.3 stage 4 does emit |
| A concurrency number | **none**, unchanged. §3.7 gives the log exactly one writer, so a concurrency measurement would be measuring something this system does not do |
| `--store sqlite` on `beck up` | **not wired.** Rungs 2 and 3 are Postgres; the substrate that makes a laptop production-shaped is a laptop's |

## 67.6 What this corrects

- **[`08`](08-roadmap.md)'s SQLite bullet is built**, and its "measure with `beck bench log` and let
  the number pick rung 0's default" is answered: the number does not pick, per §67.3.
- **[`07`](07-dependencies.md) §7.8.1's "add — Phase 3" is discharged.** Its stated reason — the
  transaction, not the speed — survived the measurement, which is worth recording because the first
  measurement appeared to say something much more exciting.
- **`beck bench log`'s table gains two rows and a wider first column**, and the wider column is not
  cosmetic: `sqlite-relaxed` did not fit, and a substrate whose name is truncated is a number
  attributed to the wrong thing.

## 67.7 What Phase 3 is still not

The exit criterion is not met. **Five of the fourteen bullets remain untouched** — Mode B, client
polish, structured concurrency, the playground, supply-chain tooling — with no LLVM backend,
identity holding its seam and not its relying party, and the incremental-views bullet still without
read models, pgwire or fusion.

That last one is now the interesting gap rather than one of a list: the substrate a SQL read model
would be built on exists, is tested, and is used for nothing.
