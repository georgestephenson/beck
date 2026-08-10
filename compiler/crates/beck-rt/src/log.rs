//! The log engine — "the log is the database" (§5.3).
//!
//! Phase 0 wrote this against one hand-written `Event` type. Phase 1 writes it against
//! [`beck_core::Value`], because the event type now comes from the *program*: `union Event` in the
//! source is what decides what a log record contains, and the runtime must not know its shape.
//!
//! The contract every substrate keeps is unchanged, and is the thing all the determinism rests on:
//!
//! * `seq` is assigned **here and nowhere else**, densely, from a single writer (§3.7).
//! * A batch of events from one command is appended **atomically at contiguous `seq`s** — no fold
//!   ever observes half a command's consequences.
//! * Reads are ordered and replayable from any position; that is what `beck replay`,
//!   `(subscription, seq)` resumption and forked worlds are made of.
//!
//! We do not write a storage engine (`docs/01-vision-and-premise.md` §1.5). This is a small log
//! engine on top of proven storage: redb for rung 0, PostgreSQL for everything above it.

use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use redb::ReadableTable;
use rusqlite::OptionalExtension;

/// The records a log is made of, defined in [`beck_host::record`] because a browser tab holds a log
/// too and an envelope has to mean the same thing in both ([`docs/17`](../../../../../docs/17-playground.md)
/// §17.2). What is *here* is the engine: the substrates, and the contract above.
pub use beck_host::record::{Envelope, Instant, Pending, Seq, Snapshot};

#[async_trait]
pub trait LogStore: Send + Sync + 'static {
    /// Which substrate this is — reported by the CLI and the harnesses, because a number without
    /// its substrate is meaningless.
    fn kind(&self) -> &'static str;
    async fn head(&self) -> Result<Seq>;
    async fn floor(&self) -> Result<Seq>;
    /// Append a batch atomically at contiguous `seq`s. Returns the stamped envelopes.
    async fn append(&self, batch: &[Pending]) -> Result<Vec<Envelope>>;
    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<Envelope>>;
    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()>;
    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>>;
}

// ---------------------------------------------------------------------------------------------
// In-memory — for tests, and for measuring everything else without a disk in the way.
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
pub struct MemoryLog {
    inner: Mutex<MemoryInner>,
}

#[derive(Default)]
struct MemoryInner {
    events: Vec<Envelope>,
    snapshots: Vec<Snapshot>,
}

impl MemoryLog {
    pub fn new() -> MemoryLog {
        MemoryLog::default()
    }
}

#[async_trait]
impl LogStore for MemoryLog {
    fn kind(&self) -> &'static str {
        "memory"
    }

    async fn head(&self) -> Result<Seq> {
        Ok(self
            .inner
            .lock()
            .expect("log mutex")
            .events
            .last()
            .map(|e| e.seq)
            .unwrap_or(0))
    }

    async fn floor(&self) -> Result<Seq> {
        Ok(0)
    }

    async fn append(&self, batch: &[Pending]) -> Result<Vec<Envelope>> {
        let mut inner = self.inner.lock().expect("log mutex");
        let mut next = inner.events.last().map(|e| e.seq).unwrap_or(0);
        let mut out = Vec::with_capacity(batch.len());
        for p in batch {
            next += 1;
            out.push(Envelope {
                seq: next,
                at: p.at,
                actor: p.actor.clone(),
                body: p.body.clone(),
            });
        }
        inner.events.extend(out.iter().cloned());
        Ok(out)
    }

    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<Envelope>> {
        let inner = self.inner.lock().expect("log mutex");
        Ok(inner
            .events
            .iter()
            .filter(|e| e.seq > after)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        self.inner
            .lock()
            .expect("log mutex")
            .snapshots
            .push(snapshot.clone());
        Ok(())
    }

    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>> {
        let inner = self.inner.lock().expect("log mutex");
        Ok(inner
            .snapshots
            .iter()
            .filter(|s| s.seq <= seq)
            .max_by_key(|s| s.seq)
            .cloned())
    }
}

// ---------------------------------------------------------------------------------------------
// redb — rung 0. "`beck run` needs no server; the log file is still replayable" (§5.3).
// ---------------------------------------------------------------------------------------------

const EVENTS: redb::TableDefinition<u64, &[u8]> = redb::TableDefinition::new("events");
const SNAPSHOTS: redb::TableDefinition<u64, &[u8]> = redb::TableDefinition::new("snapshots");
/// The encoding the two tables above are written in — see [`beck_core::repr::FORMAT`].
const META: redb::TableDefinition<&str, u32> = redb::TableDefinition::new("meta");

pub struct RedbLog {
    db: redb::Database,
}

impl RedbLog {
    pub fn open(path: &std::path::Path) -> Result<RedbLog> {
        let db = redb::Database::create(path)
            .with_context(|| format!("opening the log at {}", path.display()))?;
        // Create the tables eagerly so a read on a fresh database does not fail.
        let tx = db.begin_write()?;
        {
            let events = tx.open_table(EVENTS)?;
            let empty = events.first()?.is_none();
            let _ = tx.open_table(SNAPSHOTS)?;
            let mut meta = tx.open_table(META)?;
            // Stamp a fresh store; refuse one written in another encoding. Reading old bytes with
            // a new decoder does not fail — postcard would happily interpret JSON text as some
            // `Repr` — and a log that decodes to something other than what was written is the one
            // outcome an append-only history may not have.
            let stamped = meta.get("format")?.map(|v| v.value());
            match stamped {
                Some(found) if found != beck_core::repr::FORMAT => bail!(
                    "the log at {} was written in format {found} and this build reads format {}. \
                     Replay it through the older build and export, or start a fresh log — reading \
                     it as-is would decode to something that is not what was written",
                    path.display(),
                    beck_core::repr::FORMAT
                ),
                Some(_) => {}
                // No stamp and no events is a store this build created; no stamp *with* events is
                // a Phase 2 log, which is format 1.
                None if empty => {
                    meta.insert("format", beck_core::repr::FORMAT)?;
                }
                None => bail!(
                    "the log at {} carries no format stamp, so it was written by a build before \
                     format {} — its events are JSON text and this build reads postcard",
                    path.display(),
                    beck_core::repr::FORMAT
                ),
            }
        }
        tx.commit()?;
        Ok(RedbLog { db })
    }
}

#[async_trait]
impl LogStore for RedbLog {
    fn kind(&self) -> &'static str {
        "redb"
    }

    async fn head(&self) -> Result<Seq> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(EVENTS)?;
        let head = table.last()?.map(|(k, _)| k.value()).unwrap_or(0);
        Ok(head)
    }

    async fn floor(&self) -> Result<Seq> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(EVENTS)?;
        let floor = table
            .first()?
            .map(|(k, _)| k.value().saturating_sub(1))
            .unwrap_or(0);
        Ok(floor)
    }

    async fn append(&self, batch: &[Pending]) -> Result<Vec<Envelope>> {
        let tx = self.db.begin_write()?;
        let mut out = Vec::with_capacity(batch.len());
        {
            let mut table = tx.open_table(EVENTS)?;
            let mut next = { table.last()?.map(|(k, _)| k.value()).unwrap_or(0) };
            for p in batch {
                next += 1;
                let env = Envelope {
                    seq: next,
                    at: p.at,
                    actor: p.actor.clone(),
                    body: p.body.clone(),
                };
                table.insert(next, env.encode()?.as_slice())?;
                out.push(env);
            }
        }
        // One commit for the whole batch: group commit is the difference between per-command fsync
        // and a respectable events/s number, and it is what makes the batch atomic.
        tx.commit()?;
        Ok(out)
    }

    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<Envelope>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(EVENTS)?;
        let mut out = Vec::new();
        for entry in table.range((after + 1)..)? {
            let (_, v) = entry?;
            out.push(Envelope::decode(v.value())?);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(SNAPSHOTS)?;
            let bytes = beck_core::repr::to_bytes(&snapshot.state)?;
            table.insert(snapshot.seq, bytes.as_slice())?;
        }
        tx.commit()?;
        Ok(())
    }

    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SNAPSHOTS)?;
        let mut best: Option<Snapshot> = None;
        for entry in table.range(..=seq)? {
            let (k, v) = entry?;
            let state = beck_core::repr::from_bytes(v.value()).context("decoding a snapshot")?;
            best = Some(Snapshot {
                seq: k.value(),
                state,
            });
        }
        Ok(best)
    }
}

// ---------------------------------------------------------------------------------------------
// PostgreSQL — the durable substrate above rung 0 (§5.3).
// ---------------------------------------------------------------------------------------------

/// The DDL the compiler emits for a `durable` fold (§4.3 stage 4, "emit state artefacts").
pub const DDL: &str = "\
CREATE TABLE IF NOT EXISTS beck_log (
    seq   BIGSERIAL PRIMARY KEY,
    at    BIGINT      NOT NULL,
    actor TEXT        NOT NULL,
    body  BYTEA       NOT NULL
);
-- `seq` is append-only and therefore perfectly correlated with physical order, which is the one
-- case BRIN is built for: a summary per block range instead of a tuple per row. Every read this
-- store performs is `WHERE seq > $1 ORDER BY seq LIMIT $2`, so the index is scanned as a range and
-- never probed as a point. The primary key's btree stays because it enforces uniqueness; BRIN is
-- what the range scans use, and it is kilobytes where the btree is megabytes.
CREATE INDEX IF NOT EXISTS beck_log_seq_brin ON beck_log USING BRIN (seq);
CREATE TABLE IF NOT EXISTS beck_snapshot (
    seq   BIGINT PRIMARY KEY,
    state BYTEA  NOT NULL
);
-- The format the two tables above are written in. Checked on open, because a log read back under a
-- different encoding does not fail — it produces plausible nonsense, and an append-only audit trail
-- may not have that outcome (`beck_core::repr::FORMAT`).
CREATE TABLE IF NOT EXISTS beck_meta (
    id      INT PRIMARY KEY CHECK (id = 1),
    format  INT NOT NULL
);
";

pub struct PgLog {
    client: tokio_postgres::Client,
}

impl PgLog {
    pub async fn connect(url: &str) -> Result<PgLog> {
        let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
            .await
            .context("connecting to the log store")?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!(error = %e, "log store connection closed");
            }
        });
        client
            .batch_execute(DDL)
            .await
            .context("applying the log DDL")?;
        check_format(&client).await?;
        Ok(PgLog { client })
    }

    /// Drop and recreate the log — used by tests and by `beck run --fresh`.
    pub async fn truncate(&self) -> Result<()> {
        self.client
            .batch_execute("TRUNCATE beck_log RESTART IDENTITY; TRUNCATE beck_snapshot;")
            .await?;
        Ok(())
    }
}

/// Stamp the store with the encoding it holds, or refuse a store written in another one.
///
/// The refusal is the point. Before the log carried a format, an older store's `TEXT` bodies would
/// have been read as `BYTEA` and decoded as postcard — producing values, not errors. §3.7 makes
/// replay the only description of a program's history; a history that decodes to something else is
/// the one failure mode this system cannot tolerate.
async fn check_format(client: &tokio_postgres::Client) -> Result<()> {
    let want = beck_core::repr::FORMAT as i32;
    let row = client
        .query_opt("SELECT format FROM beck_meta WHERE id = 1", &[])
        .await?;
    match row {
        Some(r) => {
            let found: i32 = r.get(0);
            if found != want {
                bail!(
                    "this log was written in format {found} and this build reads format {want}. \
                     Replay it through the older build and export, or point at a fresh store — \
                     reading it as-is would decode to something that is not what was written"
                );
            }
        }
        None => {
            client
                .execute(
                    "INSERT INTO beck_meta (id, format) VALUES (1, $1) ON CONFLICT DO NOTHING",
                    &[&want],
                )
                .await?;
        }
    }
    Ok(())
}

#[async_trait]
impl LogStore for PgLog {
    fn kind(&self) -> &'static str {
        "postgres"
    }

    async fn head(&self) -> Result<Seq> {
        let row = self
            .client
            .query_one("SELECT COALESCE(MAX(seq), 0)::BIGINT FROM beck_log", &[])
            .await?;
        Ok(row.get::<_, i64>(0) as u64)
    }

    async fn floor(&self) -> Result<Seq> {
        let row = self
            .client
            .query_one("SELECT COALESCE(MIN(seq), 1)::BIGINT FROM beck_log", &[])
            .await?;
        Ok((row.get::<_, i64>(0) as u64).saturating_sub(1))
    }

    async fn append(&self, batch: &[Pending]) -> Result<Vec<Envelope>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        // One statement for the whole batch, inside one transaction: contiguous `seq`s,
        // all-or-nothing (§3.7).
        let mut sql = String::from("INSERT INTO beck_log (at, actor, body) VALUES ");
        let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
        for (i, p) in batch.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            let base = i * 3;
            sql.push_str(&format!("(${},${},${})", base + 1, base + 2, base + 3));
            params.push(Box::new(p.at.0));
            params.push(Box::new(p.actor.clone()));
            params.push(Box::new(beck_core::repr::to_bytes(&p.body)?));
        }
        // Only `seq` comes back. The rest is already in hand, and re-reading it meant decoding
        // every event the process had just encoded — the same work twice, on the serial path.
        sql.push_str(" RETURNING seq");

        let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
            .iter()
            .map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let rows = self.client.query(&sql, &refs).await?;
        if rows.len() != batch.len() {
            bail!("the log accepted {} of {} events", rows.len(), batch.len());
        }
        let mut out: Vec<Envelope> = rows
            .iter()
            .zip(batch)
            .map(|(r, p)| Envelope {
                seq: r.get::<_, i64>(0) as u64,
                at: p.at,
                actor: p.actor.clone(),
                body: p.body.clone(),
            })
            .collect();
        out.sort_by_key(|e| e.seq);

        // The sequence guarantees uniqueness but not contiguity if another writer exists; a
        // failure here means the single-writer invariant has been broken, which is not something
        // to paper over.
        for pair in out.windows(2) {
            if pair[1].seq != pair[0].seq + 1 {
                bail!("the log assigned non-contiguous seqs: a second writer is appending");
            }
        }
        Ok(out)
    }

    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<Envelope>> {
        let rows = self
            .client
            .query(
                "SELECT seq, at, actor, body FROM beck_log WHERE seq > $1 ORDER BY seq LIMIT $2",
                &[&(after as i64), &(limit as i64)],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| Envelope {
                seq: r.get::<_, i64>(0) as u64,
                at: Instant(r.get::<_, i64>(1)),
                actor: r.get(2),
                body: beck_core::repr::from_bytes(r.get::<_, &[u8]>(3))
                    .expect("an event this store wrote decodes"),
            })
            .collect())
    }

    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        self.client
            .execute(
                "INSERT INTO beck_snapshot (seq, state) VALUES ($1, $2) \
                 ON CONFLICT (seq) DO UPDATE SET state = EXCLUDED.state",
                &[
                    &(snapshot.seq as i64),
                    &beck_core::repr::to_bytes(&snapshot.state)?,
                ],
            )
            .await?;
        Ok(())
    }

    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>> {
        let rows = self
            .client
            .query(
                "SELECT seq, state FROM beck_snapshot WHERE seq <= $1 ORDER BY seq DESC LIMIT 1",
                &[&(seq as i64)],
            )
            .await?;
        match rows.first() {
            None => Ok(None),
            Some(r) => Ok(Some(Snapshot {
                seq: r.get::<_, i64>(0) as u64,
                state: beck_core::repr::from_bytes(r.get::<_, &[u8]>(1))
                    .context("decoding a snapshot")?,
            })),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// SQLite — the substrate that makes rungs 0-2 the same shape as production (§7.8.1)
// ---------------------------------------------------------------------------------------------

/// The same three tables, in SQLite's dialect.
///
/// `INTEGER PRIMARY KEY` is the one spelling SQLite aliases to its own `rowid`, which is what makes
/// it monotonic and gap-free for an append-only table — the property `BIGSERIAL` gives above. Any
/// other integer type would be an ordinary column with a btree beside it.
///
/// There is no BRIN analogue and none is wanted: the `rowid` **is** the physical order, so
/// `WHERE seq > ? ORDER BY seq LIMIT ?` is already a range scan of the table itself.
pub const SQLITE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS beck_log (
    seq   INTEGER PRIMARY KEY,
    at    INTEGER NOT NULL,
    actor TEXT    NOT NULL,
    body  BLOB    NOT NULL
);
CREATE TABLE IF NOT EXISTS beck_snapshot (
    seq   INTEGER PRIMARY KEY,
    state BLOB    NOT NULL
);
CREATE TABLE IF NOT EXISTS beck_meta (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    format  INTEGER NOT NULL
);
";

/// How much a commit promises, which is a *semantic* choice rather than a tuning knob.
///
/// [`03`](../../../../../docs/03-type-and-effect-system.md) §3.7 makes the log the only description of
/// a program's history, so "an accepted event may be lost" is a change to what the system means and
/// not a setting. It is therefore named, defaulted to the strong option, and measured
/// (`docs/67` §67.3) — because the cost of the strong option is 38× and a number that large deserves
/// to be visible rather than discovered.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Durability {
    /// `synchronous = FULL`: every commit is on the platter before it is acknowledged.
    ///
    /// The same promise redb and Postgres make, which is what makes them comparable.
    #[default]
    Fsync,
    /// `synchronous = NORMAL` under WAL: a commit survives a process crash and may be lost in a
    /// power loss. **Not corrupted** — WAL rules that out — but an acknowledged event can vanish.
    ///
    /// Legitimate for a laptop where the log is a scratchpad, and never the default, because a
    /// weaker promise arrived at by not reading a manual is not a decision anybody made.
    Relaxed,
}

/// A log in a single SQLite file.
///
/// [`docs/07`](../../../../../docs/07-dependencies.md) §7.8.1 gives the reason, and it is not speed:
/// SQLite is *also* a read-model engine, so "append and project in one transaction" — the property
/// Postgres gives production — becomes available on a laptop. `redb` cannot offer that at any
/// speed, because it has no query language for a projection to be written in.
///
/// The connection is behind a `Mutex` rather than a pool. One writer is the invariant §3.7 already
/// depends on, and SQLite serialises writers anyway; a pool would add contention to model
/// concurrency the log does not have.
pub struct SqliteLog {
    conn: std::sync::Mutex<rusqlite::Connection>,
    kind: &'static str,
}

impl SqliteLog {
    /// Open a log that makes the same durability promise redb and Postgres do.
    pub fn open(path: &std::path::Path) -> Result<SqliteLog> {
        Self::open_with(path, Durability::default())
    }

    pub fn open_with(path: &std::path::Path, durability: Durability) -> Result<SqliteLog> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).context("creating the log directory")?;
            }
        }
        let conn = rusqlite::Connection::open(path).context("opening the log store")?;
        // The durability is part of the name, because `kind()` is what every measurement and every
        // report labels its number with — and a row that said only "sqlite" would let two very
        // different promises share a line (`docs/67` §67.3).
        let kind = match durability {
            Durability::Fsync => "sqlite",
            Durability::Relaxed => "sqlite-relaxed",
        };
        Self::prepare(conn, kind, durability)
    }

    /// A log that never touches a disk — the same engine and the same SQL, for tests.
    ///
    /// Durability is meaningless here and the strong setting is still passed, so an in-memory run
    /// and an on-disk run differ in exactly one thing: the disk.
    pub fn in_memory() -> Result<SqliteLog> {
        let conn = rusqlite::Connection::open_in_memory().context("opening the log store")?;
        Self::prepare(conn, "sqlite-memory", Durability::Fsync)
    }

    fn prepare(
        conn: rusqlite::Connection,
        kind: &'static str,
        durability: Durability,
    ) -> Result<SqliteLog> {
        // WAL, because a reader must not block the single writer — which is the whole shape of a
        // log that is appended to while it is being replayed. WAL is independent of the durability
        // choice below: it decides who blocks whom, and `synchronous` decides what a commit
        // promises.
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("enabling WAL")?;
        conn.pragma_update(
            None,
            "synchronous",
            match durability {
                Durability::Fsync => "FULL",
                Durability::Relaxed => "NORMAL",
            },
        )
        .context("setting synchronous")?;
        conn.pragma_update(None, "foreign_keys", true).ok();
        conn.execute_batch(SQLITE_DDL)
            .context("applying the log DDL")?;

        // The same refusal `check_format` makes for Postgres, and for the same reason: a log read
        // back under a different encoding does not fail, it produces plausible nonsense.
        let want = beck_core::repr::FORMAT as i64;
        let found: Option<i64> = conn
            .query_row("SELECT format FROM beck_meta WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()
            .context("reading the store's format")?;
        match found {
            Some(found) if found != want => bail!(
                "this log was written in format {found} and this build reads format {want}. \
                 Replay it through the older build and export, or point at a fresh store — \
                 reading it as-is would decode to something that is not what was written"
            ),
            Some(_) => {}
            None => {
                conn.execute("INSERT INTO beck_meta (id, format) VALUES (1, ?1)", [want])
                    .context("stamping the store's format")?;
            }
        }
        Ok(SqliteLog {
            conn: std::sync::Mutex::new(conn),
            kind,
        })
    }

    /// Drop and recreate the log — the counterpart of [`PgLog::truncate`].
    pub fn truncate(&self) -> Result<()> {
        let conn = self.conn.lock().expect("the log mutex is not poisoned");
        conn.execute_batch("DELETE FROM beck_log; DELETE FROM beck_snapshot;")?;
        Ok(())
    }
}

#[async_trait]
impl LogStore for SqliteLog {
    fn kind(&self) -> &'static str {
        self.kind
    }

    async fn head(&self) -> Result<Seq> {
        let conn = self.conn.lock().expect("the log mutex is not poisoned");
        let head: i64 = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM beck_log", [], |r| {
            r.get(0)
        })?;
        Ok(head as u64)
    }

    async fn floor(&self) -> Result<Seq> {
        let conn = self.conn.lock().expect("the log mutex is not poisoned");
        let floor: i64 = conn.query_row("SELECT COALESCE(MIN(seq), 1) FROM beck_log", [], |r| {
            r.get(0)
        })?;
        Ok((floor as u64).saturating_sub(1))
    }

    async fn append(&self, batch: &[Pending]) -> Result<Vec<Envelope>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        // Encode before taking the lock: `to_bytes` can fail, and a transaction opened and then
        // abandoned on an encoding error is a transaction that has to be reasoned about.
        let bodies: Vec<Vec<u8>> = batch
            .iter()
            .map(|p| beck_core::repr::to_bytes(&p.body))
            .collect::<Result<_, _>>()?;

        let mut conn = self.conn.lock().expect("the log mutex is not poisoned");
        let tx = conn
            .transaction()
            .context("opening the append transaction")?;
        // The first `seq` is decided inside the transaction, so a concurrent writer cannot
        // interleave: contiguous, all-or-nothing (§3.7).
        let head: i64 = tx.query_row("SELECT COALESCE(MAX(seq), 0) FROM beck_log", [], |r| {
            r.get(0)
        })?;
        {
            let mut stmt = tx
                .prepare("INSERT INTO beck_log (seq, at, actor, body) VALUES (?1, ?2, ?3, ?4)")
                .context("preparing the append")?;
            for (i, (p, body)) in batch.iter().zip(&bodies).enumerate() {
                stmt.execute(rusqlite::params![
                    head + 1 + i as i64,
                    p.at.0,
                    &p.actor,
                    body
                ])?;
            }
        }
        tx.commit().context("committing the append")?;

        Ok(batch
            .iter()
            .enumerate()
            .map(|(i, p)| Envelope {
                seq: (head + 1 + i as i64) as u64,
                at: p.at,
                actor: p.actor.clone(),
                body: p.body.clone(),
            })
            .collect())
    }

    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<Envelope>> {
        let conn = self.conn.lock().expect("the log mutex is not poisoned");
        let mut stmt = conn.prepare(
            "SELECT seq, at, actor, body FROM beck_log WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![after as i64, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, at, actor, body) = row?;
            out.push(Envelope {
                seq: seq as u64,
                at: Instant(at),
                actor,
                body: beck_core::repr::from_bytes(&body).context("decoding an event")?,
            });
        }
        Ok(out)
    }

    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let state = beck_core::repr::to_bytes(&snapshot.state)?;
        let conn = self.conn.lock().expect("the log mutex is not poisoned");
        conn.execute(
            "INSERT INTO beck_snapshot (seq, state) VALUES (?1, ?2) \
             ON CONFLICT (seq) DO UPDATE SET state = excluded.state",
            rusqlite::params![snapshot.seq as i64, state],
        )?;
        Ok(())
    }

    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>> {
        let conn = self.conn.lock().expect("the log mutex is not poisoned");
        let row: Option<(i64, Vec<u8>)> = conn
            .query_row(
                "SELECT seq, state FROM beck_snapshot WHERE seq <= ?1 ORDER BY seq DESC LIMIT 1",
                [seq as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((seq, state)) => Ok(Some(Snapshot {
                seq: seq as u64,
                state: beck_core::repr::from_bytes(&state).context("decoding a snapshot")?,
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_core::Value;

    fn pending(actor: &str, n: i64) -> Pending {
        Pending {
            at: Instant(n),
            actor: actor.into(),
            body: Value::Int(n),
        }
    }

    async fn contract(store: &dyn LogStore) {
        assert_eq!(store.head().await.unwrap(), 0);
        let batch = vec![pending("alice", 1), pending("alice", 2)];
        let out = store.append(&batch).await.unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].seq, 1);
        assert_eq!(out[1].seq, 2, "a batch lands at contiguous seqs");
        assert_eq!(store.head().await.unwrap(), 2);

        let read = store.read(0, 10).await.unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].event().unwrap(), Value::Int(1));
        assert_eq!(store.read(1, 10).await.unwrap().len(), 1);

        store
            .put_snapshot(&Snapshot {
                seq: 2,
                state: Value::str_("s"),
            })
            .await
            .unwrap();
        let snap = store.snapshot_at_or_before(5).await.unwrap().unwrap();
        assert_eq!(snap.seq, 2);
        assert_eq!(snap.state, Value::str_("s"));
        assert!(store.snapshot_at_or_before(1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn memory_keeps_the_contract() {
        contract(&MemoryLog::new()).await;
    }

    /// The same contract against a real Postgres — the substrate `beck run --store postgres`
    /// uses. No `BECK_PG` is a skip (right on a laptop); `BECK_REQUIRE_PG=1` makes the absence a
    /// failure, so the gate cannot go silently missing from CI (the conformance suite's pattern).
    #[tokio::test]
    async fn postgres_keeps_the_same_contract() {
        let required = std::env::var("BECK_REQUIRE_PG").is_ok_and(|v| v == "1");
        let Ok(url) = std::env::var("BECK_PG") else {
            assert!(
                !required,
                "BECK_REQUIRE_PG=1 is set, so a missing BECK_PG is a failure rather than a skip"
            );
            eprintln!("skipping: set BECK_PG to run the log contract against a real Postgres");
            return;
        };
        // BECK_PG being set is a claim that a server is there; an unreachable one is a failure.
        let store = PgLog::connect(&url)
            .await
            .expect("BECK_PG is set, so Postgres must be reachable");
        store.truncate().await.expect("a fresh log");
        contract(&store).await;
    }

    #[tokio::test]
    async fn redb_keeps_the_same_contract() {
        let dir = std::env::temp_dir().join(format!("beck-rt-log-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let store = RedbLog::open(&dir).unwrap();
        contract(&store).await;
        drop(store);
        let _ = std::fs::remove_file(&dir);
    }

    #[tokio::test]
    async fn redb_survives_reopening() {
        let path = std::env::temp_dir().join(format!("beck-rt-reopen-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let store = RedbLog::open(&path).unwrap();
            store.append(&[pending("a", 7)]).await.unwrap();
        }
        {
            let store = RedbLog::open(&path).unwrap();
            assert_eq!(store.head().await.unwrap(), 1);
            assert_eq!(
                store.read(0, 10).await.unwrap()[0].event().unwrap(),
                Value::Int(7)
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    /// SQLite answers the same contract, on disk and in memory.
    ///
    /// Unlike Postgres this needs no server and therefore **never skips** — which is half of
    /// `docs/07` §7.8.1's argument: the substrate that makes a laptop the same shape as production
    /// is also the one CI can always run.
    #[tokio::test]
    async fn sqlite_keeps_the_same_contract() {
        contract(&SqliteLog::in_memory().unwrap()).await;

        let path = std::env::temp_dir().join(format!("beck-rt-sqlite-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        let store = SqliteLog::open(&path).unwrap();
        contract(&store).await;
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[tokio::test]
    async fn sqlite_survives_reopening() {
        let path =
            std::env::temp_dir().join(format!("beck-rt-sqlite-reopen-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        {
            let store = SqliteLog::open(&path).unwrap();
            store.append(&[pending("a", 7)]).await.unwrap();
            store
                .put_snapshot(&Snapshot {
                    seq: 1,
                    state: Value::Int(70),
                })
                .await
                .unwrap();
        }
        {
            let store = SqliteLog::open(&path).unwrap();
            assert_eq!(store.head().await.unwrap(), 1);
            assert_eq!(
                store.read(0, 10).await.unwrap()[0].event().unwrap(),
                Value::Int(7)
            );
            // The snapshot too: a store that persisted its log and lost its snapshots would replay
            // correctly and slowly, which is the failure nobody notices.
            assert_eq!(
                store.snapshot_at_or_before(5).await.unwrap().unwrap().state,
                Value::Int(70)
            );
            // And appending after a reopen continues the sequence rather than restarting it.
            let out = store.append(&[pending("b", 8)]).await.unwrap();
            assert_eq!(out[0].seq, 2, "a reopened log continues its own sequence");
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    /// The property `docs/07` §7.8.1 is actually about, and the one neither redb nor a file can
    /// offer: **an append and a projection in one transaction**.
    ///
    /// Postgres has it in production. This is the claim that a laptop now has the same shape — not
    /// that the read models are built (they are not, `docs/26` §26.9), but that the substrate
    /// underneath them admits the property. A `LogStore` cannot express it, so the test reaches
    /// past the trait to the connection, which is the only honest way to assert it today.
    #[tokio::test]
    async fn sqlite_can_append_and_project_in_one_transaction() {
        let store = SqliteLog::in_memory().unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute_batch("CREATE TABLE counts (actor TEXT PRIMARY KEY, n INTEGER NOT NULL);")
                .unwrap();
        }

        // One transaction: the event lands and the read model moves, or neither does.
        {
            let mut conn = store.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO beck_log (seq, at, actor, body) VALUES (1, 1, 'ana', ?1)",
                [beck_core::repr::to_bytes(&Value::Int(1)).unwrap()],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO counts (actor, n) VALUES ('ana', 1) \
                 ON CONFLICT (actor) DO UPDATE SET n = n + 1",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(store.head().await.unwrap(), 1);
        {
            let conn = store.conn.lock().unwrap();
            let n: i64 = conn
                .query_row("SELECT n FROM counts WHERE actor = 'ana'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1);
        }

        // And a rolled-back transaction leaves neither behind, which is the half that makes the
        // first half worth anything.
        {
            let mut conn = store.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO beck_log (seq, at, actor, body) VALUES (2, 2, 'ana', ?1)",
                [beck_core::repr::to_bytes(&Value::Int(2)).unwrap()],
            )
            .unwrap();
            tx.execute("UPDATE counts SET n = n + 1 WHERE actor = 'ana'", [])
                .unwrap();
            tx.rollback().unwrap();
        }
        assert_eq!(
            store.head().await.unwrap(),
            1,
            "the rolled-back event is not in the log"
        );
        let conn = store.conn.lock().unwrap();
        let n: i64 = conn
            .query_row("SELECT n FROM counts WHERE actor = 'ana'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "and the projection did not move either");
    }

    /// A SQLite log written under another encoding is refused, the same way redb's is.
    #[test]
    fn a_sqlite_log_written_in_another_format_is_refused_too() {
        let path = std::env::temp_dir().join(format!("beck-sqlite-fmt-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(SQLITE_DDL).unwrap();
            conn.execute(
                "INSERT INTO beck_meta (id, format) VALUES (1, ?1)",
                [beck_core::repr::FORMAT as i64 + 1],
            )
            .unwrap();
        }
        let Err(err) = SqliteLog::open(&path) else {
            panic!("a log in another format has to be refused")
        };
        let text = format!("{err:#}");
        assert!(text.contains("was written in format"), "{text}");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn a_log_written_in_another_format_is_refused_rather_than_misread() {
        // The one failure an append-only history may not have. Postcard is not self-describing, so
        // handing it a Phase 2 log's JSON text does not error — it decodes to *a* `Repr`, and the
        // program's history silently becomes something nobody wrote. The stamp is what turns that
        // into a refusal.
        let dir = std::env::temp_dir().join("beck-format-stamp");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("log.redb");

        // A store this build wrote opens again.
        {
            let store = RedbLog::open(&path).expect("a fresh log opens");
            drop(store);
        }
        RedbLog::open(&path).expect("and opens again");

        // Rewrite the stamp as if an older build had made it.
        {
            let db = redb::Database::create(&path).expect("reopen");
            let tx = db.begin_write().expect("write");
            {
                let mut meta = tx.open_table(META).expect("meta");
                meta.insert("format", beck_core::repr::FORMAT - 1)
                    .expect("stamp");
            }
            tx.commit().expect("commit");
        }
        let said = match RedbLog::open(&path) {
            Ok(_) => panic!("a foreign format must be refused"),
            Err(e) => e.to_string(),
        };
        assert!(said.contains("format"), "{said}");
        assert!(
            said.contains("not what was written"),
            "the message has to say what the risk is, not just that it stopped: {said}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unstamped_log_with_events_in_it_is_refused_too() {
        // The upgrade path from Phase 2, which stamped nothing: an empty unstamped store is one
        // this build just made, and an unstamped store *with events* is somebody's history in the
        // old encoding.
        let dir = std::env::temp_dir().join("beck-format-unstamped");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("log.redb");
        {
            let db = redb::Database::create(&path).expect("create");
            let tx = db.begin_write().expect("write");
            {
                let mut events = tx.open_table(EVENTS).expect("events");
                events
                    .insert(1u64, b"{\"seq\":1}".as_slice())
                    .expect("an old record");
                let _ = tx.open_table(SNAPSHOTS).expect("snapshots");
                let _ = tx.open_table(META).expect("meta");
            }
            tx.commit().expect("commit");
        }
        let said = match RedbLog::open(&path) {
            Ok(_) => panic!("an unstamped log with events must be refused"),
            Err(e) => e.to_string(),
        };
        assert!(said.contains("no format stamp"), "{said}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
