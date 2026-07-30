//! Embedded log store on redb — rung 0 of the parity ladder (§6.6).
//!
//! `beck run` must need no server, no container and no cluster, and the log file must still
//! replay. That is the whole reason this store exists: the dev rung is not a toy mode with
//! different semantics, it is the same total order in a file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use tokio::sync::Mutex;

use beck_p0_core::domain::TodoState;
use beck_p0_core::envelope::{EventEnvelope, Seq};

use crate::{LogStore, PendingEvent, Snapshot};

const LOG: TableDefinition<u64, &[u8]> = TableDefinition::new("beck_log");
const SNAPSHOT: TableDefinition<u64, &[u8]> = TableDefinition::new("beck_snapshot");

pub struct RedbLog {
    db: Arc<Database>,
    path: PathBuf,
    writer: Mutex<Seq>,
}

impl RedbLog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let db = Database::create(&path).with_context(|| format!("opening {}", path.display()))?;

        // Create both tables up front so read transactions never race a missing table.
        let txn = db.begin_write()?;
        {
            txn.open_table(LOG)?;
            txn.open_table(SNAPSHOT)?;
        }
        txn.commit()?;

        let head = {
            let txn = db.begin_read()?;
            let table = txn.open_table(LOG)?;
            let head = table.last()?.map(|(k, _)| k.value()).unwrap_or(0);
            head
        };

        Ok(Self {
            db: Arc::new(db),
            path,
            writer: Mutex::new(head),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl LogStore for RedbLog {
    fn kind(&self) -> &'static str {
        "redb"
    }

    async fn head(&self) -> Result<Seq> {
        Ok(*self.writer.lock().await)
    }

    async fn floor(&self) -> Result<Seq> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(LOG)?;
            let floor = table.first()?.map(|(k, _)| k.value()).unwrap_or(0);
            Ok(floor)
        })
        .await?
    }

    async fn append(&self, batch: &[PendingEvent]) -> Result<Vec<EventEnvelope>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let mut head = self.writer.lock().await;
        let stamped: Vec<EventEnvelope> = batch
            .iter()
            .enumerate()
            .map(|(i, p)| p.stamp(*head + 1 + i as Seq))
            .collect();

        let db = self.db.clone();
        let to_write: Vec<(u64, Vec<u8>)> = stamped
            .iter()
            .map(|env| {
                (
                    env.seq,
                    postcard::to_allocvec(env).expect("envelope is serialisable"),
                )
            })
            .collect();

        // One write transaction per batch: redb commits are durable, so this is the fsync — and
        // the reason the ingress task batches rather than appending event by event.
        tokio::task::spawn_blocking(move || -> Result<()> {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(LOG)?;
                for (seq, bytes) in &to_write {
                    table.insert(*seq, bytes.as_slice())?;
                }
            }
            txn.commit()?;
            Ok(())
        })
        .await??;

        *head += stamped.len() as Seq;
        Ok(stamped)
    }

    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<EventEnvelope>> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(LOG)?;
            let mut out = Vec::new();
            for entry in table.range(after.saturating_add(1)..)? {
                let (_, value) = entry?;
                out.push(postcard::from_bytes(value.value()).context("decoding logged envelope")?);
                if out.len() >= limit {
                    break;
                }
            }
            Ok(out)
        })
        .await?
    }

    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let db = self.db.clone();
        let seq = snapshot.seq;
        let bytes = postcard::to_allocvec(&snapshot.state)?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let txn = db.begin_write()?;
            {
                let mut table = txn.open_table(SNAPSHOT)?;
                table.insert(seq, bytes.as_slice())?;
            }
            txn.commit()?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let txn = db.begin_read()?;
            let table = txn.open_table(SNAPSHOT)?;
            if table.is_empty()? {
                return Ok(None);
            }
            let Some(entry) = table.range(..=seq)?.next_back() else {
                return Ok(None);
            };
            let (k, v) = entry?;
            let state: TodoState = postcard::from_bytes(v.value()).context("decoding snapshot")?;
            Ok(Some(Snapshot {
                seq: k.value(),
                state,
            }))
        })
        .await?
    }
}
