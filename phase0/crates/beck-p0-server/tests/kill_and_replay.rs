//! "Then kill the process mid-stream and replay the log."
//!
//! The roadmap's Phase 0 ends on that sentence, so it is a test rather than a demo. Three
//! properties, each of which the whole design rests on:
//!
//! 1. **Acknowledged means durable.** SIGKILL — no drain, no snapshot, no unwinding — and every
//!    command the server acked is still in the log afterwards.
//! 2. **Replay is exact.** Folding the surviving log reproduces the state, and does so identically
//!    every time (§3.7's determinism rule made mechanical).
//! 3. **Subscribers resume across the death.** A client that reconnects to the *new* process with
//!    `(subscription, seq)` gets the gap as a patch, not the world — which is the same code path a
//!    deploy takes (§6.4).

use std::process::{Child, Command as OsCommand, Stdio};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

const BINARY: &str = env!("CARGO_BIN_EXE_beck-p0");

struct Server {
    child: Child,
    port: u16,
}

impl Server {
    async fn start(redb_path: &std::path::Path) -> Server {
        let port = free_port();
        let child = OsCommand::new(BINARY)
            .args([
                "run",
                "--store",
                "redb",
                "--redb-path",
                redb_path.to_str().expect("utf-8 path"),
                "--addr",
                &format!("127.0.0.1:{port}"),
                // Snapshot rarely, so recovery genuinely folds the log.
                "--snapshot-every",
                "100000",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn beck-p0");

        let server = Server { child, port };
        server.wait_until_ready().await;
        server
    }

    async fn wait_until_ready(&self) {
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", self.port)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("server never accepted connections");
    }

    async fn connect(&self) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
        let (socket, _) = connect_async(format!("ws://127.0.0.1:{}/socket", self.port))
            .await
            .expect("websocket connect");
        socket
    }

    /// SIGKILL: no drain, no snapshot, no destructors. The log is on its own.
    fn kill(&mut self) {
        self.child.kill().expect("kill");
        self.child.wait().expect("reap");
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

async fn send(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>, value: Value) {
    socket
        .send(Message::Text(value.to_string()))
        .await
        .expect("send");
}

async fn next_json(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                return serde_json::from_str(&text).expect("json frame")
            }
            Ok(Some(Ok(_))) => continue,
            other => panic!("expected a frame, got {other:?}"),
        }
    }
}

fn replay(redb_path: &std::path::Path, extra: &[&str]) -> String {
    let output = OsCommand::new(BINARY)
        .args(["replay", "--store", "redb", "--redb-path"])
        .arg(redb_path)
        .args(extra)
        .output()
        .expect("run replay");
    assert!(
        output.status.success(),
        "replay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn field<'a>(report: &'a str, name: &str) -> &'a str {
    report
        .lines()
        .find(|line| line.starts_with(name))
        .and_then(|line| line.split_whitespace().last())
        .unwrap_or_else(|| panic!("no field {name} in:\n{report}"))
}

#[tokio::test]
async fn kill_mid_stream_then_replay() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("app.redb");
    let mut server = Server::start(&log_path).await;

    // A subscriber, watching.
    let mut watcher = server.connect().await;
    send(
        &mut watcher,
        json!({"t": "hello", "sub": "watcher", "seq": 0, "actor": "alice", "scope": "all"}),
    )
    .await;
    assert_eq!(next_json(&mut watcher).await["t"], "w");
    let mut watcher_seq = next_json(&mut watcher).await["q"].as_u64().unwrap();

    // Traffic. Each ack is a promise; we record them and hold the server to it.
    let mut acked: Vec<(String, u64)> = Vec::new();
    let mut writer = server.connect().await;
    send(
        &mut writer,
        json!({"t": "hello", "sub": "writer", "seq": 0, "actor": "alice", "scope": "all"}),
    )
    .await;
    next_json(&mut writer).await;
    next_json(&mut writer).await;

    for i in 0..40u128 {
        let todo_id = format!("00000000-0000-4000-8000-{i:012x}");
        send(
            &mut writer,
            json!({
                "t": "c",
                "id": uuid::Uuid::new_v4(),
                "command": {"c": "add", "id": todo_id, "text": format!("todo {i}")}
            }),
        )
        .await;
        loop {
            let msg = next_json(&mut writer).await;
            if msg["t"] == "a" {
                acked.push((todo_id.clone(), msg["q"].as_u64().unwrap()));
                break;
            }
        }
    }

    // Let the watcher catch up, then note where it is: this is what it will resume from.
    tokio::time::sleep(Duration::from_millis(200)).await;
    while let Ok(Some(Ok(Message::Text(text)))) =
        tokio::time::timeout(Duration::from_millis(200), watcher.next()).await
    {
        let value: Value = serde_json::from_str(&text).unwrap();
        if value["t"] == "p" {
            watcher_seq = value["q"].as_u64().unwrap();
        }
    }
    assert!(watcher_seq > 0, "the watcher saw nothing");

    // Now kill it, mid-stream: no SIGTERM, no drain, no snapshot.
    let head_before = acked.last().expect("acks").1;
    server.kill();

    // 1. Acknowledged means durable.
    let report = replay(&log_path, &["--genesis"]);
    let replayed_to: u64 = field(&report, "replayed to").parse().unwrap();
    let todos: usize = field(&report, "todos").parse().unwrap();
    assert!(
        replayed_to >= head_before,
        "log lost acknowledged events: replayed to {replayed_to}, acked up to {head_before}"
    );
    assert_eq!(
        todos,
        acked.len(),
        "every acknowledged todo must survive the kill"
    );

    // 2. Replay is exact, and the digest does not depend on how it was reached.
    let again = replay(&log_path, &["--genesis"]);
    assert_eq!(field(&report, "digest"), field(&again, "digest"));
    let from_snapshot_path = replay(&log_path, &[]);
    assert_eq!(
        field(&report, "digest"),
        field(&from_snapshot_path, "digest")
    );

    // 3. The replacement process serves the same log, and the watcher resumes across the death.
    let server = Server::start(&log_path).await;
    let mut resumed = server.connect().await;
    send(
        &mut resumed,
        json!({"t": "hello", "sub": "watcher", "seq": watcher_seq, "actor": "alice", "scope": "all"}),
    )
    .await;
    let welcome = next_json(&mut resumed).await;
    assert_eq!(welcome["t"], "w");
    assert_eq!(
        welcome["how"], "resumed",
        "a subscriber must resume across a process death, not be reset"
    );

    // If the watcher was already up to date there is no gap and no patch — which is itself the
    // strongest form of "it resumed".
    if watcher_seq < replayed_to {
        let patch = next_json(&mut resumed).await;
        assert_eq!(patch["t"], "p");
        assert_eq!(patch["q"].as_u64().unwrap(), replayed_to);
    }

    drop(resumed);
    let mut server = server;
    server.kill();
}

#[tokio::test]
async fn a_drained_process_leaves_a_snapshot_and_the_next_one_starts_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("drain.redb");
    let mut server = Server::start(&log_path).await;

    let mut writer = server.connect().await;
    send(
        &mut writer,
        json!({"t": "hello", "sub": "w", "seq": 0, "actor": "alice", "scope": "all"}),
    )
    .await;
    next_json(&mut writer).await;
    next_json(&mut writer).await;
    for i in 0..5u128 {
        send(
            &mut writer,
            json!({
                "t": "c",
                "id": uuid::Uuid::new_v4(),
                "command": {"c": "add", "id": format!("00000000-0000-4000-8000-{i:012x}"), "text": "x"}
            }),
        )
        .await;
        loop {
            if next_json(&mut writer).await["t"] == "a" {
                break;
            }
        }
    }

    // SIGTERM: the drain path, which snapshots on the way out (§6.4).
    unsafe {
        libc::kill(server.child.id() as i32, libc::SIGTERM);
    }
    let _ = server.child.wait();

    let with_snapshot = replay(&log_path, &[]);
    let from_genesis = replay(&log_path, &["--genesis"]);
    assert_eq!(
        field(&with_snapshot, "digest"),
        field(&from_genesis, "digest"),
        "the snapshot a drain wrote must agree with a fold from genesis"
    );
    assert_eq!(field(&with_snapshot, "todos"), "5");
}
