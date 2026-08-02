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

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use beck_core::Value;
use redb::ReadableTable;
use serde::{Deserialize, Serialize};

/// Position in the total order. One totally-ordered log per application (§3.7 v1 semantics).
pub type Seq = u64;

/// Wall-clock instant, milliseconds since the Unix epoch, captured at ingress **as data**.
///
/// A fold may read `env.at`; it may not call a clock. The type is deliberately a plain number with
/// no way to obtain "now" from it, so the determinism rule is hard to break by accident.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Instant(pub i64);

/// A durably logged occurrence. The fields are §3.7's, and `actor` is a stable identity — never
/// the live `Session` capability or a token (F5).
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub seq: Seq,
    pub at: Instant,
    pub actor: String,
    /// The event itself.
    ///
    /// A `Value`, not a JSON tree. It was the latter through Phase 2, and the cost was paid on
    /// every append *and* every read: build a `serde_json::Value`, serialise it to text, parse the
    /// text, walk the tree back into a `Value` — four traversals and two allocations per event, at
    /// the one point §3.7 makes serial. [`beck_core::repr`] is the encoding now; the JSON repr
    /// stays for things a person reads.
    pub body: Value,
}

impl Envelope {
    /// The envelope as the `Envelope[Event]` record a fold sees.
    pub fn to_value(&self, event: Value) -> Value {
        Value::Data {
            ty: Arc::from("Envelope"),
            variant: None,
            fields: Arc::new(BTreeMap::from([
                (Arc::from("seq"), Value::Int(self.seq as i64)),
                (Arc::from("at"), Value::Int(self.at.0)),
                (Arc::from("actor"), Value::str_(&self.actor)),
                (Arc::from("body"), event),
            ])),
        }
    }

    /// The event. Kept as a method because every caller had one and the type changed underneath
    /// them; there is nothing to decode any more.
    pub fn event(&self) -> Result<Value> {
        Ok(self.body.clone())
    }

    /// The bytes a store writes.
    fn encode(&self) -> Result<Vec<u8>> {
        let wire = Wire {
            seq: self.seq,
            at: self.at,
            actor: self.actor.clone(),
            body: beck_core::repr::Repr::of(&self.body)?,
        };
        Ok(postcard::to_allocvec(&wire)?)
    }

    fn decode(bytes: &[u8]) -> Result<Envelope> {
        let wire: Wire = postcard::from_bytes(bytes).context("decoding a logged event")?;
        Ok(Envelope {
            seq: wire.seq,
            at: wire.at,
            actor: wire.actor,
            body: wire.body.to_value(),
        })
    }
}

/// The on-disk shape of an [`Envelope`] — a concrete type, so a non-self-describing codec can
/// encode it. See [`beck_core::repr`] for why that matters.
#[derive(Serialize, Deserialize)]
struct Wire {
    seq: Seq,
    at: Instant,
    actor: String,
    body: beck_core::repr::Repr,
}

/// A validated event on its way to the log, before `seq` exists.
#[derive(Clone, Debug)]
pub struct Pending {
    pub at: Instant,
    pub actor: String,
    pub body: Value,
}

/// A snapshot of the durable fold: the accumulator plus the position it was taken at.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub seq: Seq,
    pub state: Value,
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
