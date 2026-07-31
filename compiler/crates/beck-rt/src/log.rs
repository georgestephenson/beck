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
//! We do not write a storage engine ([`docs/01-vision-and-premise.md`] §1.5). This is a small log
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub seq: Seq,
    pub at: Instant,
    pub actor: String,
    /// The event, as a lossless [`Value`] encoding.
    pub body: serde_json::Value,
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

    pub fn event(&self) -> Result<Value> {
        beck_core::core::value_from_repr(&self.body).context("decoding a logged event")
    }
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
                body: beck_core::core::value_to_repr(&p.body)?,
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
            let _ = tx.open_table(EVENTS)?;
            let _ = tx.open_table(SNAPSHOTS)?;
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
                    body: beck_core::core::value_to_repr(&p.body)?,
                };
                let bytes = serde_json::to_vec(&env)?;
                table.insert(next, bytes.as_slice())?;
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
            out.push(serde_json::from_slice(v.value())?);
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
            let bytes = serde_json::to_vec(&beck_core::core::value_to_repr(&snapshot.state)?)?;
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
            let repr: serde_json::Value = serde_json::from_slice(v.value())?;
            let state = beck_core::core::value_from_repr(&repr).context("decoding a snapshot")?;
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
    body  TEXT        NOT NULL
);
CREATE TABLE IF NOT EXISTS beck_snapshot (
    seq   BIGINT PRIMARY KEY,
    state TEXT   NOT NULL
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
            params.push(Box::new(
                beck_core::core::value_to_repr(&p.body)?.to_string(),
            ));
        }
        sql.push_str(" RETURNING seq, at, actor, body");

        let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
            .iter()
            .map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        let rows = self.client.query(&sql, &refs).await?;
        let mut out: Vec<Envelope> = rows
            .iter()
            .map(|r| Envelope {
                seq: r.get::<_, i64>(0) as u64,
                at: Instant(r.get::<_, i64>(1)),
                actor: r.get(2),
                body: serde_json::from_str(r.get::<_, &str>(3)).unwrap_or_default(),
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
                body: serde_json::from_str(r.get::<_, &str>(3)).unwrap_or_default(),
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
                    &beck_core::core::value_to_repr(&snapshot.state)?.to_string(),
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
                state: beck_core::core::value_from_repr(&serde_json::from_str(
                    r.get::<_, &str>(1),
                )?)
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
}
