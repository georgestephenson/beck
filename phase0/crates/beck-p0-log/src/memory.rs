//! In-memory log. No durability, same total order — the substrate against which the others are
//! differentially tested, and the one the fanout benchmark uses so that "per-idle-session memory"
//! measures sessions rather than page cache.

use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;

use beck_p0_core::envelope::{EventEnvelope, Seq};

use crate::{LogStore, PendingEvent, Snapshot};

#[derive(Default)]
pub struct MemoryLog {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    log: Vec<EventEnvelope>,
    snapshots: Vec<Snapshot>,
}

impl MemoryLog {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LogStore for MemoryLog {
    fn kind(&self) -> &'static str {
        "memory"
    }

    async fn head(&self) -> Result<Seq> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.log.last().map(|e| e.seq).unwrap_or(0))
    }

    async fn floor(&self) -> Result<Seq> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.log.first().map(|e| e.seq).unwrap_or(0))
    }

    async fn append(&self, batch: &[PendingEvent]) -> Result<Vec<EventEnvelope>> {
        let mut inner = self.inner.lock().unwrap();
        let mut seq = inner.log.last().map(|e| e.seq).unwrap_or(0);
        let stamped: Vec<_> = batch
            .iter()
            .map(|p| {
                seq += 1;
                p.stamp(seq)
            })
            .collect();
        inner.log.extend(stamped.iter().cloned());
        Ok(stamped)
    }

    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<EventEnvelope>> {
        let inner = self.inner.lock().unwrap();
        let start = inner.log.partition_point(|e| e.seq <= after);
        Ok(inner
            .log
            .get(start..)
            .unwrap_or_default()
            .iter()
            .take(limit)
            .cloned()
            .collect())
    }

    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.snapshots.push(snapshot.clone());
        Ok(())
    }

    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .snapshots
            .iter()
            .filter(|s| s.seq <= seq)
            .max_by_key(|s| s.seq)
            .cloned())
    }
}
