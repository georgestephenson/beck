//! The PostgreSQL wire protocol, served over a program's read models.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.3 asks for "pgwire access
//! for the outside world: `psql`, BI tools, DBeaver see materialized views as ordinary tables — the
//! single cheapest trust-builder for adopting teams", and
//! [`07`](../../../../../docs/07-dependencies.md) §7.2 files it under external-tool compatibility
//! with no alternative listed. [`beck_core::read`] is what the tables are; this is the socket.
//!
//! # Why it is written here rather than taken
//!
//! There are crates that implement this protocol server-side. What they carry is the rest of a
//! database — a type registry, an extended-protocol state machine over prepared statements with
//! parameters, portals that suspend — and a read model has none of those to expose. What is
//! actually needed is the startup exchange, the simple query, the extended query with no
//! parameters, and four type OIDs a driver already knows. That is this file, and it adds no
//! dependency to a workspace whose §7.9 pins everything
//! ([`adr/0020`](../../../../../docs/adr/0020-the-read-model-speaks-pgwire-by-hand.md)).
//!
//! # What it deliberately does not do
//!
//! * **No authentication.** It answers `AuthenticationOk` to everyone, which is why [`serve`]
//!   refuses to bind anywhere but the loopback interface. An unauthenticated read of an
//!   application's whole state must not be reachable from another host, and a flag that turns that
//!   off is a decision with an ADR rather than a convenience.
//! * **No TLS.** The same reason and the same bound.
//! * **No `pg_catalog`.** `psql`'s `\d` sends a join against four catalogue relations and this SQL
//!   has no joins. [`beck_core::read::Schema::CATALOGUE`] is the substitute, and it is a table.
//! * **No writes.** The log is the only way state changes; a read model that accepted an `insert`
//!   would be a second way, which is the property [`01`](../../../../../docs/01-vision-and-premise.md)
//!   §1.1 is about.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Result};
use beck_core::read::{self, Answer, Cardinality, Column, Datum, Schema, SqlError, Table};
use beck_core::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::app::App;

/// The protocol version this speaks: 3.0, which every client since PostgreSQL 7.4 speaks.
const PROTOCOL_3: i32 = 196_608;
const SSL_REQUEST: i32 = 80_877_103;
const GSSENC_REQUEST: i32 = 80_877_104;
const CANCEL_REQUEST: i32 = 80_877_102;

/// The largest message this will read, before it has any reason to trust the sender.
///
/// A startup packet says how long it is and then sends that many bytes; believing an unbounded
/// length is how a listener becomes a memory allocator for whoever connects
/// ([`83`](../../../../../docs/83-the-runtime-edge-report.md) made the same argument about the
/// websocket edge). A query long enough to reach this is a query this SQL cannot parse anyway.
const MAX_MESSAGE: usize = 1 << 20;

/// Serve a program's read models on the PostgreSQL wire protocol.
pub async fn serve(app: Arc<App>, addr: SocketAddr) -> Result<()> {
    serve_on(bind(addr).await?, app).await
}

/// Bind the read-model port, refusing anything but loopback.
///
/// There is no authentication here — see the module docs — and this bound is what stands in for
/// one: a port that answers every question about an application's state belongs on the same host as
/// the process, reached by whatever forwards it there.
///
/// Separate from [`serve`] so a caller can fail the *command* rather than a background task: an
/// address this process will not serve on should be an error the person who typed it sees.
pub async fn bind(addr: SocketAddr) -> Result<TcpListener> {
    if !addr.ip().is_loopback() {
        bail!(
            "{addr} is not a loopback address. The read-model port has no authentication and no \
             transport security, so it is bound to localhost only; forward it (kubectl \
             port-forward, an SSH tunnel, a sidecar) rather than exposing it — \
             docs/adr/0020 is the record that would have to change first"
        );
    }
    Ok(TcpListener::bind(addr).await?)
}

/// Serve on an already-bound listener.
pub async fn serve_on(listener: TcpListener, app: Arc<App>) -> Result<()> {
    let schema = Arc::new(Schema::of(app.runtime().placed(), app.runtime().plan()));
    tracing::info!(
        addr = %listener.local_addr()?,
        tables = schema.tables.len(),
        "read models on pgwire (no authentication; loopback only)"
    );
    loop {
        let (socket, peer) = listener.accept().await?;
        let app = app.clone();
        let schema = schema.clone();
        tokio::spawn(async move {
            if let Err(e) = session(socket, app, schema).await {
                tracing::debug!(peer = %peer, error = %e, "pgwire session ended");
            }
        });
    }
}

// -------------------------------------------------------------------------------------------
// One connection
// -------------------------------------------------------------------------------------------

/// A parsed statement, held between `Parse` and `Execute`.
#[derive(Clone, Default)]
struct Statement {
    sql: String,
    columns: Vec<Column>,
}

/// A bound statement, and the format its caller wants each column in.
#[derive(Clone, Default)]
struct Portal {
    statement: Statement,
    /// Empty for all-text, one entry for all-of-that, or one per column.
    formats: Vec<i16>,
}

async fn session(mut socket: TcpStream, app: Arc<App>, schema: Arc<Schema>) -> Result<()> {
    socket.set_nodelay(true)?;
    if !startup(&mut socket).await? {
        return Ok(());
    }

    // The connection is a reader of the shared dataflow for as long as it is open: the same reader
    // set a subscription joins, for the same reason (`beck_core::engine::Reader`).
    let reader = app.shared_dataflow().reader();

    let mut out = Vec::new();
    authentication_ok(&mut out);
    parameter_status(&mut out, "server_version", "15.0 (beck)");
    parameter_status(&mut out, "server_encoding", "UTF8");
    parameter_status(&mut out, "client_encoding", "UTF8");
    parameter_status(&mut out, "DateStyle", "ISO, MDY");
    parameter_status(&mut out, "TimeZone", "UTC");
    parameter_status(&mut out, "integer_datetimes", "on");
    parameter_status(&mut out, "standard_conforming_strings", "on");
    // A cancel request is refused rather than honoured, so the key is a constant rather than a
    // secret: there is nothing to cancel that is not already `O(rows)`.
    message(&mut out, b'K', |b| {
        b.extend_from_slice(&0i32.to_be_bytes());
        b.extend_from_slice(&0i32.to_be_bytes());
    });
    ready(&mut out);
    socket.write_all(&out).await?;

    let mut statements: std::collections::HashMap<String, Statement> = Default::default();
    let mut portals: std::collections::HashMap<String, Portal> = Default::default();
    // Between an error and the next `Sync`, the extended protocol says every message is skipped.
    let mut failed = false;

    loop {
        let mut tag = [0u8; 1];
        if socket.read_exact(&mut tag).await.is_err() {
            return Ok(());
        }
        let body = read_body(&mut socket).await?;
        let mut out = Vec::new();
        match tag[0] {
            b'X' => return Ok(()),
            b'S' => {
                failed = false;
                ready(&mut out);
            }
            _ if failed => {}
            b'Q' => {
                let sql = cstr(&body, &mut 0)?;
                simple_query(&app, &schema, &reader, &sql, &mut out).await;
                ready(&mut out);
            }
            b'P' => {
                let mut i = 0;
                let name = cstr(&body, &mut i)?;
                let sql = cstr(&body, &mut i)?;
                match schema.describe(&sql) {
                    Ok(columns) => {
                        statements.insert(name, Statement { sql, columns });
                        message(&mut out, b'1', |_| {});
                    }
                    Err(e) => {
                        error_response(&mut out, &e);
                        failed = true;
                    }
                }
            }
            b'B' => {
                let mut i = 0;
                let portal = cstr(&body, &mut i)?;
                let statement = cstr(&body, &mut i)?;
                // Parameter formats and parameters: this SQL has no placeholders, so they are read
                // to advance past them rather than used.
                let formats = i16s(&body, &mut i)?;
                let _ = formats;
                let params = i16_count(&body, &mut i)?;
                for _ in 0..params {
                    let len = i32_at(&body, &mut i)?;
                    if len > 0 {
                        i += len as usize;
                    }
                }
                let results = i16s(&body, &mut i)?;
                match statements.get(&statement) {
                    Some(s) => {
                        portals.insert(
                            portal,
                            Portal {
                                statement: s.clone(),
                                formats: results,
                            },
                        );
                        message(&mut out, b'2', |_| {});
                    }
                    None => {
                        error_response(
                            &mut out,
                            &SqlError {
                                message: format!("there is no prepared statement \"{statement}\""),
                                code: "26000",
                            },
                        );
                        failed = true;
                    }
                }
            }
            b'D' => {
                let mut i = 0;
                let what = *body.first().unwrap_or(&b'S');
                i += 1;
                let name = cstr(&body, &mut i)?;
                let (columns, formats) = match what {
                    b'P' => match portals.get(&name) {
                        Some(p) => (p.statement.columns.clone(), p.formats.clone()),
                        None => (Vec::new(), Vec::new()),
                    },
                    _ => {
                        // A statement description says what the *statement* takes and returns; the
                        // formats are a property of a portal, so this is always text here, exactly
                        // as a real server answers it.
                        message(&mut out, b't', |b| {
                            b.extend_from_slice(&0i16.to_be_bytes());
                        });
                        match statements.get(&name) {
                            Some(s) => (s.columns.clone(), Vec::new()),
                            None => (Vec::new(), Vec::new()),
                        }
                    }
                };
                if columns.is_empty() {
                    message(&mut out, b'n', |_| {});
                } else {
                    row_description(&mut out, &columns, &formats);
                }
            }
            b'E' => {
                let mut i = 0;
                let name = cstr(&body, &mut i)?;
                let max = i32_at(&body, &mut i)?.max(0) as usize;
                match portals.get(&name).cloned() {
                    Some(p) => match run(&app, &schema, &reader, &p.statement.sql).await {
                        Ok(answer) => {
                            let total = answer.rows.len();
                            let take = if max == 0 { total } else { max.min(total) };
                            for row in answer.rows.iter().take(take) {
                                data_row(&mut out, row, &p.formats);
                            }
                            if take < total {
                                message(&mut out, b's', |_| {});
                            } else {
                                command_complete(&mut out, &answer.tag);
                            }
                        }
                        Err(e) => {
                            error_response(&mut out, &e);
                            failed = true;
                        }
                    },
                    None => {
                        error_response(
                            &mut out,
                            &SqlError {
                                message: format!("there is no portal \"{name}\""),
                                code: "34000",
                            },
                        );
                        failed = true;
                    }
                }
            }
            b'C' => {
                let mut i = 0;
                let what = *body.first().unwrap_or(&b'S');
                i += 1;
                let name = cstr(&body, &mut i)?;
                if what == b'P' {
                    portals.remove(&name);
                } else {
                    statements.remove(&name);
                }
                message(&mut out, b'3', |_| {});
            }
            b'H' => {}
            other => {
                error_response(
                    &mut out,
                    &SqlError {
                        message: format!("message type '{}' is not supported", other as char),
                        code: "08P01",
                    },
                );
                failed = true;
            }
        }
        if !out.is_empty() {
            socket.write_all(&out).await?;
        }
    }
}

/// The startup exchange. Answers whether the connection continues.
async fn startup(socket: &mut TcpStream) -> Result<bool> {
    loop {
        let mut len = [0u8; 4];
        if socket.read_exact(&mut len).await.is_err() {
            return Ok(false);
        }
        let len = i32::from_be_bytes(len);
        if !(8..=MAX_MESSAGE as i32).contains(&len) {
            bail!("a startup packet of {len} bytes");
        }
        let mut body = vec![0u8; len as usize - 4];
        socket.read_exact(&mut body).await?;
        let version = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
        match version {
            // "No" to both, in the one-byte form the protocol reserves for the answer, and the
            // client retries in plaintext. A refusal is not a failure here — `sslmode=prefer`, the
            // default in most drivers, is exactly this exchange.
            SSL_REQUEST | GSSENC_REQUEST => {
                socket.write_all(b"N").await?;
            }
            CANCEL_REQUEST => return Ok(false),
            PROTOCOL_3 => return Ok(true),
            other => {
                let mut out = Vec::new();
                error_response(
                    &mut out,
                    &SqlError {
                        message: format!(
                            "this read model speaks protocol 3.0; the client asked for {}.{}",
                            other >> 16,
                            other & 0xffff
                        ),
                        code: "0A000",
                    },
                );
                socket.write_all(&out).await?;
                return Ok(false);
            }
        }
    }
}

async fn simple_query(
    app: &Arc<App>,
    schema: &Schema,
    reader: &beck_core::engine::Reader,
    sql: &str,
    out: &mut Vec<u8>,
) {
    if sql.trim().is_empty() {
        message(out, b'I', |_| {});
        return;
    }
    match run(app, schema, reader, sql).await {
        Ok(answer) => {
            if !answer.columns.is_empty() {
                row_description(out, &answer.columns, &[]);
                for row in &answer.rows {
                    data_row(out, row, &[]);
                }
            }
            command_complete(out, &answer.tag);
        }
        Err(e) => error_response(out, &e),
    }
}

/// Answer one query against a consistent snapshot of the program's state.
///
/// The whole query runs under the accumulator's read lock. That is what makes it a *snapshot*: the
/// sequencer takes the write lock to commit, so while this runs nothing can move the state, and
/// therefore nothing can advance the shared dataflow past the version the base tables were read at.
/// Two tables in one query cannot disagree about which events have happened.
///
/// The cost is stated rather than hidden: a scan of a large table delays the next commit by the
/// length of the scan. The alternative — copy the accumulator and let the arrangements move — is
/// cheaper for the writer and gives a query that sees two versions at once, which is the wrong
/// trade for a read model whose selling point is that it cannot disagree with the page.
async fn run(
    app: &Arc<App>,
    schema: &Schema,
    reader: &beck_core::engine::Reader,
    sql: &str,
) -> Result<Answer, SqlError> {
    app.read_snapshot(|state, version| {
        let rows = Snapshot {
            reader,
            state,
            version,
        };
        schema.run(sql, &rows)
    })
    .await
}

/// Where a table's rows come from at one version.
struct Snapshot<'a> {
    reader: &'a beck_core::engine::Reader,
    state: &'a Value,
    version: u64,
}

impl read::Rows for Snapshot<'_> {
    fn scan(&self, table: &Table) -> Result<Vec<Value>, SqlError> {
        let values = match &table.source {
            read::Source::Catalogue => return Ok(Vec::new()),
            read::Source::State(path) => {
                let at = read::at_path(self.state, path).ok_or_else(|| SqlError {
                    message: format!(
                        "\"{}\" is not in this state — the accumulator has no such field",
                        table.name
                    ),
                    code: "42P01",
                })?;
                match table.cardinality {
                    Cardinality::Many => read::elements(&at),
                    Cardinality::One => vec![at],
                }
            }
            read::Source::View(op) => {
                let vals = self
                    .reader
                    .read(self.state, self.version, *op)
                    .map_err(|e| SqlError {
                        message: format!("the view this table reads could not be maintained: {e}"),
                        code: "58000",
                    })?;
                match table.cardinality {
                    Cardinality::One => vals.into_iter().take(1).collect(),
                    // A maintained arrangement answers its entries; a pointwise operator answers
                    // one value, which for a collection-shaped table is the collection.
                    Cardinality::Many => match vals.as_slice() {
                        [one @ (Value::List(_) | Value::Map(_))] => read::elements(one),
                        _ => vals,
                    },
                }
            }
        };
        Ok(values)
    }
}

// -------------------------------------------------------------------------------------------
// Messages
// -------------------------------------------------------------------------------------------

/// Frame a message: a tag, a length that counts itself, and a body.
fn message(out: &mut Vec<u8>, tag: u8, body: impl FnOnce(&mut Vec<u8>)) {
    out.push(tag);
    let at = out.len();
    out.extend_from_slice(&0i32.to_be_bytes());
    body(out);
    let len = (out.len() - at) as i32;
    out[at..at + 4].copy_from_slice(&len.to_be_bytes());
}

fn authentication_ok(out: &mut Vec<u8>) {
    message(out, b'R', |b| {
        b.extend_from_slice(&0i32.to_be_bytes());
    });
}

fn parameter_status(out: &mut Vec<u8>, key: &str, value: &str) {
    message(out, b'S', |b| {
        put_cstr(b, key);
        put_cstr(b, value);
    });
}

fn ready(out: &mut Vec<u8>) {
    // Always 'I': idle, never in a transaction. A read model has nothing to be in one for, and a
    // driver that opened one was answered `BEGIN` and told nothing changed.
    message(out, b'Z', |b| b.push(b'I'));
}

fn command_complete(out: &mut Vec<u8>, tag: &str) {
    message(out, b'C', |b| put_cstr(b, tag));
}

fn row_description(out: &mut Vec<u8>, columns: &[Column], formats: &[i16]) {
    message(out, b'T', |b| {
        b.extend_from_slice(&(columns.len() as i16).to_be_bytes());
        for (i, c) in columns.iter().enumerate() {
            put_cstr(b, &c.name);
            // No table and no attribute number: these columns are not from a table a catalogue
            // knows about, and zero is what the protocol reserves for saying so.
            b.extend_from_slice(&0i32.to_be_bytes());
            b.extend_from_slice(&0i16.to_be_bytes());
            b.extend_from_slice(&c.ty.oid().to_be_bytes());
            b.extend_from_slice(&c.ty.width().to_be_bytes());
            b.extend_from_slice(&(-1i32).to_be_bytes());
            b.extend_from_slice(&format_of(formats, i).to_be_bytes());
        }
    });
}

fn format_of(formats: &[i16], i: usize) -> i16 {
    match formats.len() {
        0 => 0,
        1 => formats[0],
        _ => formats.get(i).copied().unwrap_or(0),
    }
}

fn data_row(out: &mut Vec<u8>, row: &[Option<Datum>], formats: &[i16]) {
    message(out, b'D', |b| {
        b.extend_from_slice(&(row.len() as i16).to_be_bytes());
        for (i, cell) in row.iter().enumerate() {
            match cell {
                None => b.extend_from_slice(&(-1i32).to_be_bytes()),
                Some(d) => {
                    let bytes = if format_of(formats, i) == 1 {
                        binary(d)
                    } else {
                        d.text().into_bytes()
                    };
                    b.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    b.extend_from_slice(&bytes);
                }
            }
        }
    });
}

/// The binary form of a datum, which is what every Rust and Java driver asks for.
fn binary(d: &Datum) -> Vec<u8> {
    match d {
        Datum::Boolean(v) => vec![u8::from(*v)],
        Datum::Bigint(v) => v.to_be_bytes().to_vec(),
        Datum::Double(v) => v.to_bits().to_be_bytes().to_vec(),
        Datum::Text(s) => s.as_bytes().to_vec(),
    }
}

fn error_response(out: &mut Vec<u8>, e: &SqlError) {
    message(out, b'E', |b| {
        b.push(b'S');
        put_cstr(b, "ERROR");
        b.push(b'V');
        put_cstr(b, "ERROR");
        b.push(b'C');
        put_cstr(b, e.code);
        b.push(b'M');
        put_cstr(b, &e.message);
        b.push(0);
    });
}

fn put_cstr(out: &mut Vec<u8>, s: &str) {
    // A NUL inside a string would end it early, which is a way to smuggle a second field into a
    // message. Beck strings are UTF-8 and may contain one, so it is replaced rather than trusted.
    for byte in s.bytes() {
        out.push(if byte == 0 { b' ' } else { byte });
    }
    out.push(0);
}

// -------------------------------------------------------------------------------------------
// Reading
// -------------------------------------------------------------------------------------------

async fn read_body(socket: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    socket.read_exact(&mut len).await?;
    let len = i32::from_be_bytes(len);
    if !(4..=MAX_MESSAGE as i32).contains(&len) {
        bail!("a message of {len} bytes");
    }
    let mut body = vec![0u8; len as usize - 4];
    socket.read_exact(&mut body).await?;
    Ok(body)
}

fn cstr(body: &[u8], i: &mut usize) -> Result<String> {
    let start = *i;
    while *i < body.len() && body[*i] != 0 {
        *i += 1;
    }
    if *i >= body.len() {
        bail!("a string in a message is not terminated");
    }
    let s = String::from_utf8_lossy(&body[start..*i]).into_owned();
    *i += 1;
    Ok(s)
}

fn i32_at(body: &[u8], i: &mut usize) -> Result<i32> {
    if *i + 4 > body.len() {
        bail!("a message ends inside a number");
    }
    let v = i32::from_be_bytes([body[*i], body[*i + 1], body[*i + 2], body[*i + 3]]);
    *i += 4;
    Ok(v)
}

fn i16_count(body: &[u8], i: &mut usize) -> Result<usize> {
    if *i + 2 > body.len() {
        bail!("a message ends inside a count");
    }
    let v = i16::from_be_bytes([body[*i], body[*i + 1]]);
    *i += 2;
    Ok(v.max(0) as usize)
}

fn i16s(body: &[u8], i: &mut usize) -> Result<Vec<i16>> {
    let n = i16_count(body, i)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if *i + 2 > body.len() {
            bail!("a message ends inside a format code");
        }
        out.push(i16::from_be_bytes([body[*i], body[*i + 1]]));
        *i += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_carries_its_own_length() {
        let mut out = Vec::new();
        command_complete(&mut out, "SELECT 2");
        assert_eq!(out[0], b'C');
        let len = i32::from_be_bytes([out[1], out[2], out[3], out[4]]) as usize;
        assert_eq!(len, out.len() - 1);
        assert_eq!(&out[5..out.len() - 1], b"SELECT 2");
        assert_eq!(out[out.len() - 1], 0);
    }

    #[test]
    fn a_nul_inside_a_string_cannot_end_it() {
        let mut out = Vec::new();
        put_cstr(&mut out, "a\0b");
        assert_eq!(out, b"a b\0");
    }

    #[test]
    fn binary_is_big_endian_and_text_is_not() {
        assert_eq!(binary(&Datum::Bigint(1)), vec![0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(Datum::Bigint(1).text(), "1");
        assert_eq!(binary(&Datum::Boolean(true)), vec![1]);
        assert_eq!(Datum::Boolean(true).text(), "t");
    }

    #[test]
    fn one_format_code_covers_every_column() {
        assert_eq!(format_of(&[], 3), 0);
        assert_eq!(format_of(&[1], 3), 1);
        assert_eq!(format_of(&[0, 1], 1), 1);
    }
}
