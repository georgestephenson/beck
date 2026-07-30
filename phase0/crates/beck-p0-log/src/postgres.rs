//! PostgreSQL log store — the v1 durable substrate (§5.3, §7.4).
//!
//! "Boring, transactional, operable everywhere, PITR for free." The schema below is what stage 8
//! of the compiler will emit as log-store DDL (§4.3), written out here by hand.
//!
//! Note what the schema does *not* have: no `updated_at`, no mutable columns, no primary-key
//! churn. State changes are events by construction, which is why the generated grants
//! (`deploy/postgres/grants.sql`) can be INSERT-only on the log — §6.5's "no generic
//! UPDATE/DELETE exists anywhere".

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, NoTls, Statement};

use beck_p0_core::domain::{ActorId, TodoState};
use beck_p0_core::envelope::{Envelope, EventEnvelope, Instant, Seq};

use crate::{LogStore, PendingEvent, Snapshot};

/// The generated log-store DDL. Idempotent, so it doubles as the migration for a fresh database.
pub const DDL: &str = "\
create table if not exists beck_log (
    seq   bigint primary key,
    at    bigint not null,
    actor text   not null,
    body  bytea  not null
);
create table if not exists beck_snapshot (
    seq   bigint primary key,
    at    bigint not null,
    state bytea  not null
);";

pub struct PgLog {
    client: Client,
    /// Appends are serialised: `seq` has exactly one assigner (§3.7). The lock is held across the
    /// insert so a reader can never observe a gap that later fills in.
    writer: Mutex<Seq>,
    insert_1: Statement,
    read: Statement,
    put_snapshot: Statement,
    snapshot_at: Statement,
}

impl PgLog {
    /// Connect, apply the DDL, and adopt the existing total order.
    pub async fn connect(url: &str) -> Result<Self> {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .with_context(|| format!("connecting to {}", redact(url)))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!(error = %e, "postgres connection closed");
            }
        });

        client
            .batch_execute(DDL)
            .await
            .context("applying log DDL")?;

        let head: i64 = client
            .query_one("select coalesce(max(seq), 0) from beck_log", &[])
            .await?
            .get(0);

        let insert_1 = client
            .prepare("insert into beck_log (seq, at, actor, body) values ($1, $2, $3, $4)")
            .await?;
        let read = client
            .prepare(
                "select seq, at, actor, body from beck_log where seq > $1 order by seq limit $2",
            )
            .await?;
        let put_snapshot = client
            .prepare(
                "insert into beck_snapshot (seq, at, state) values ($1, $2, $3)
                 on conflict (seq) do nothing",
            )
            .await?;
        let snapshot_at = client
            .prepare(
                "select seq, state from beck_snapshot where seq <= $1 order by seq desc limit 1",
            )
            .await?;

        Ok(Self {
            client,
            writer: Mutex::new(head as Seq),
            insert_1,
            read,
            put_snapshot,
            snapshot_at,
        })
    }

    /// Drop every table. Used by tests and benchmarks, never by the server.
    pub async fn truncate(&self) -> Result<()> {
        self.client
            .batch_execute("truncate beck_log; truncate beck_snapshot;")
            .await?;
        *self.writer.lock().await = 0;
        Ok(())
    }
}

#[async_trait]
impl LogStore for PgLog {
    fn kind(&self) -> &'static str {
        "postgres"
    }

    async fn head(&self) -> Result<Seq> {
        Ok(*self.writer.lock().await)
    }

    async fn floor(&self) -> Result<Seq> {
        let row = self
            .client
            .query_one("select coalesce(min(seq), 0) from beck_log", &[])
            .await?;
        let floor: i64 = row.get(0);
        Ok(floor as Seq)
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

        // One statement, so the batch lands atomically at contiguous seqs without the round trips
        // of an explicit transaction. Group commit is what makes the events/s number respectable:
        // the ingress task drains everything queued and appends it in a single statement.
        let bodies: Vec<Vec<u8>> = stamped
            .iter()
            .map(|e| postcard::to_allocvec(&e.body).expect("event is serialisable"))
            .collect();
        let seqs: Vec<i64> = stamped.iter().map(|e| e.seq as i64).collect();
        let ats: Vec<i64> = stamped.iter().map(|e| e.at.millis()).collect();

        if stamped.len() == 1 {
            self.client
                .execute(
                    &self.insert_1,
                    &[&seqs[0], &ats[0], &stamped[0].actor.0, &bodies[0]],
                )
                .await?;
        } else {
            let mut sql = String::from("insert into beck_log (seq, at, actor, body) values ");
            let mut params: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(stamped.len() * 4);
            for (i, env) in stamped.iter().enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                let base = i * 4;
                sql.push_str(&format!(
                    "(${},${},${},${})",
                    base + 1,
                    base + 2,
                    base + 3,
                    base + 4
                ));
                params.push(&seqs[i]);
                params.push(&ats[i]);
                params.push(&env.actor.0);
                params.push(&bodies[i]);
            }
            self.client.execute(sql.as_str(), &params).await?;
        }

        *head += stamped.len() as Seq;
        Ok(stamped)
    }

    async fn read(&self, after: Seq, limit: usize) -> Result<Vec<EventEnvelope>> {
        let rows = self
            .client
            .query(&self.read, &[&(after as i64), &(limit as i64)])
            .await?;
        rows.iter()
            .map(|row| {
                let seq: i64 = row.get(0);
                let at: i64 = row.get(1);
                let actor: String = row.get(2);
                let body: Vec<u8> = row.get(3);
                Ok(Envelope::new(
                    seq as Seq,
                    Instant(at),
                    ActorId(actor),
                    postcard::from_bytes(&body).context("decoding logged event")?,
                ))
            })
            .collect()
    }

    async fn put_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let state = postcard::to_allocvec(&snapshot.state)?;
        self.client
            .execute(&self.put_snapshot, &[&(snapshot.seq as i64), &0i64, &state])
            .await?;
        Ok(())
    }

    async fn snapshot_at_or_before(&self, seq: Seq) -> Result<Option<Snapshot>> {
        let rows = self
            .client
            .query(&self.snapshot_at, &[&(seq as i64)])
            .await?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let seq: i64 = row.get(0);
        let state: Vec<u8> = row.get(1);
        let state: TodoState = postcard::from_bytes(&state).context("decoding snapshot")?;
        Ok(Some(Snapshot {
            seq: seq as Seq,
            state,
        }))
    }
}

fn redact(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            format!("{}://…@{}", &url[..scheme], &url[at + 1..])
        }
        _ => url.to_string(),
    }
}
