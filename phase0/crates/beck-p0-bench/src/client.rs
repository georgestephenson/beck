//! A websocket client that speaks the thin client's protocol — the same frames `beck-thin.js`
//! sends, so the numbers measured here are the numbers a browser would see, minus the DOM.

use std::time::Duration;

use anyhow::{bail, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use beck_p0_core::domain::Command;
use beck_p0_core::envelope::Seq;

/// One completed interaction: click → command → event → fold → patch → (DOM).
pub struct Interaction {
    pub millis: f64,
    pub bytes: usize,
    pub ops: usize,
    pub seq: Seq,
}

pub struct Client {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// One-way network delay to simulate. Applied on both legs, so an interaction pays it twice —
    /// which is exactly what "click → … → DOM on a realistic RTT" means in Mode A.
    one_way: Duration,
}

impl Client {
    pub async fn connect(url: &str, rtt: Duration) -> Result<Client> {
        let (socket, _) = connect_async(url).await?;
        Ok(Client {
            socket,
            one_way: rtt / 2,
        })
    }

    /// Subscribe or resume. Returns the seq the server reports and how it treated the resumption.
    pub async fn hello(
        &mut self,
        sub: &str,
        seq: Seq,
        actor: &str,
        scope: &str,
    ) -> Result<(Seq, String)> {
        let hello = serde_json::json!({"t": "hello", "sub": sub, "seq": seq, "actor": actor, "scope": scope});
        self.socket.send(Message::Text(hello.to_string())).await?;
        loop {
            let value = self.next_message().await?;
            if value["t"] == "w" {
                return Ok((
                    value["q"].as_u64().unwrap_or(0),
                    value["how"].as_str().unwrap_or("?").to_string(),
                ));
            }
        }
    }

    pub async fn send(&mut self, command: &Command) -> Result<()> {
        let frame = serde_json::json!({
            "t": "c",
            "id": uuid::Uuid::new_v4(),
            "command": command,
        });
        if !self.one_way.is_zero() {
            tokio::time::sleep(self.one_way).await;
        }
        self.socket.send(Message::Text(frame.to_string())).await?;
        Ok(())
    }

    /// Wait for the next patch frame, returning `(seq, bytes, ops)`.
    pub async fn next_patch(&mut self) -> Result<(Seq, usize, usize)> {
        loop {
            let (value, bytes) = self.next_message_sized().await?;
            if value["t"] == "p" {
                if !self.one_way.is_zero() {
                    tokio::time::sleep(self.one_way).await;
                }
                return Ok((
                    value["q"].as_u64().unwrap_or(0),
                    bytes,
                    value["o"].as_array().map_or(0, |ops| ops.len()),
                ));
            }
        }
    }

    /// Send a command and wait until this client's view reflects it — the honest definition of
    /// "the interaction finished" in Mode A.
    ///
    /// A command completes when a patch brings the view to at least the acked seq, *or* when the
    /// server says the view is already current at that seq (`{"t":"u"}`). Waiting only for a patch
    /// deadlocks the moment a command's net effect is invisible, which is exactly how this
    /// benchmark found that hole in the protocol.
    pub async fn interact(&mut self, command: &Command) -> Result<Interaction> {
        let started = std::time::Instant::now();
        self.send(command).await?;

        let mut committed: Option<Seq> = None;
        loop {
            let (value, bytes) = self.next_message_sized().await?;
            match value["t"].as_str() {
                Some("a") => committed = Some(value["q"].as_u64().unwrap_or(0)),
                Some("n") => bail!("command rejected: {}", value["e"]),
                Some("p") | Some("u") => {
                    let at = value["q"].as_u64().unwrap_or(0);
                    if committed.is_some_and(|target| at >= target) {
                        if !self.one_way.is_zero() {
                            tokio::time::sleep(self.one_way).await;
                        }
                        return Ok(Interaction {
                            millis: started.elapsed().as_secs_f64() * 1000.0,
                            bytes: if value["t"] == "p" { bytes } else { 0 },
                            ops: value["o"].as_array().map_or(0, |ops| ops.len()),
                            seq: at,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    pub async fn next_message(&mut self) -> Result<serde_json::Value> {
        Ok(self.next_message_sized().await?.0)
    }

    async fn next_message_sized(&mut self) -> Result<(serde_json::Value, usize)> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let len = text.len();
                    return Ok((serde_json::from_str(&text)?, len));
                }
                Some(Ok(Message::Ping(payload))) => {
                    self.socket.send(Message::Pong(payload)).await?;
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => bail!("socket error: {e}"),
                None => bail!("socket closed"),
            }
        }
    }

    pub async fn close(mut self) -> Result<()> {
        self.socket.close(None).await?;
        Ok(())
    }
}

/// A minimal HTTP/1.1 GET, so the harness can measure first paint without pulling in an HTTP
/// client. Returns `(status, body, time to first byte, total time)`.
pub async fn http_get(host: &str, path: &str) -> Result<(u16, Vec<u8>, Duration, Duration)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let started = std::time::Instant::now();
    let mut stream = TcpStream::connect(host).await?;
    stream.set_nodelay(true)?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;

    let mut raw = Vec::new();
    let mut first_byte = None;
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if first_byte.is_none() {
            first_byte = Some(started.elapsed());
        }
        raw.extend_from_slice(&chunk[..read]);
    }
    let total = started.elapsed();

    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("no header terminator in response"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);

    Ok((
        status,
        raw[split + 4..].to_vec(),
        first_byte.unwrap_or(total),
        total,
    ))
}

/// Read one gauge out of the Prometheus exposition.
pub async fn metric(host: &str, name: &str) -> Result<f64> {
    let (_, body, _, _) = http_get(host, "/metrics").await?;
    let body = String::from_utf8_lossy(&body);
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix(name) {
            if let Some(value) = rest.split_whitespace().last() {
                return Ok(value.parse()?);
            }
        }
    }
    bail!("metric {name} not found")
}
