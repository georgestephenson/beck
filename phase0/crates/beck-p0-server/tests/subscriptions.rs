//! Subscription properties, over real websocket framing on an in-memory duplex.
//!
//! Two of these exist because the benchmark harness deadlocked against an earlier version of this
//! server, and a hang is exactly the kind of bug that a measurement finds and a unit test should
//! have: a patch stream carries *states*, not events, so "one command, one patch" is false, and
//! anything that assumes it waits forever.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::DuplexStream;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use beck_p0_log::MemoryLog;
use beck_p0_server::app::{App, AppConfig};
use beck_p0_server::Metrics;

type Socket = WebSocketStream<DuplexStream>;

async fn app() -> Arc<App> {
    App::start(
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
        Arc::new(Metrics::default()),
    )
    .await
    .unwrap()
}

/// Attach a subscription to the app and hand back the client end.
async fn subscribe(app: &Arc<App>, sub: &str, actor: &str, scope: &str) -> Socket {
    let (server_side, client_side) = tokio::io::duplex(64 * 1024);
    let app = app.clone();
    tokio::spawn(async move {
        let socket = WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
        let _ = beck_p0_server::session::run(app, socket).await;
    });

    let mut socket = WebSocketStream::from_raw_socket(client_side, Role::Client, None).await;
    send(
        &mut socket,
        json!({"t": "hello", "sub": sub, "seq": 0, "actor": actor, "scope": scope}),
    )
    .await;
    // Welcome, then the frame that establishes the view.
    assert_eq!(next(&mut socket).await["t"], "w");
    assert_eq!(next(&mut socket).await["t"], "p");
    socket
}

async fn send(socket: &mut Socket, value: Value) {
    socket
        .send(Message::Text(value.to_string()))
        .await
        .expect("send");
}

async fn next(socket: &mut Socket) -> Value {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => return serde_json::from_str(&text).unwrap(),
            Ok(Some(Ok(_))) => continue,
            other => panic!("expected a frame within 5s, got {other:?}"),
        }
    }
}

async fn quiet_for(socket: &mut Socket, duration: Duration) -> Option<Value> {
    match tokio::time::timeout(duration, socket.next()).await {
        Err(_) => None,
        Ok(Some(Ok(Message::Text(text)))) => Some(serde_json::from_str(&text).unwrap()),
        Ok(other) => panic!("unexpected {other:?}"),
    }
}

fn add(id: u128, text: &str) -> Value {
    json!({
        "t": "c",
        "id": uuid::Uuid::new_v4(),
        "command": {"c": "add", "id": uuid::Uuid::from_u128(id), "text": text}
    })
}

fn delete(id: u128) -> Value {
    json!({
        "t": "c",
        "id": uuid::Uuid::new_v4(),
        "command": {"c": "delete", "id": uuid::Uuid::from_u128(id)}
    })
}

#[tokio::test]
async fn a_burst_of_commands_always_leaves_the_client_knowing_where_it_is() {
    let app = app().await;
    let mut socket = subscribe(&app, "burst", "alice", "all").await;

    // Sent without waiting, so the sequencer is free to group them — which is when an add and its
    // matching delete can collapse into an empty diff and no patch at all.
    for i in 0..20u128 {
        send(&mut socket, add(i, &format!("todo {i}"))).await;
        send(&mut socket, delete(i)).await;
    }

    let mut acked = 0u64;
    let mut known = 0u64;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && (acked < 40 || known < acked) {
        let message = next(&mut socket).await;
        match message["t"].as_str() {
            Some("a") => acked = acked.max(message["q"].as_u64().unwrap()),
            // Either kind of frame tells the client where its view now stands.
            Some("p") | Some("u") => known = known.max(message["q"].as_u64().unwrap()),
            _ => {}
        }
    }

    assert_eq!(acked, 40, "every command should have been acked");
    assert!(
        known >= acked,
        "the client must learn its view is current at {acked}; it only knows {known}"
    );
    assert!(app.head() >= 40);
}

#[tokio::test]
async fn an_idle_per_session_subscriber_pays_nothing_for_other_peoples_events() {
    let app = app().await;
    let mut alice = subscribe(&app, "alice", "alice", "mine").await;
    let mut bob = subscribe(&app, "bob", "bob", "mine").await;

    // Bob adds. Alice's view is `todos.filter(owner == alice)`, so nothing about it changed, and
    // she is not waiting on anything — so she must receive no frame at all. That is the whole
    // fanout argument: N idle subscribers cost N cheap diffs, not N messages.
    send(&mut bob, add(1, "bob's todo")).await;
    let patch = next(&mut bob).await["t"] == "a";
    assert!(patch, "bob gets his ack first");
    assert_eq!(next(&mut bob).await["t"], "p", "bob sees his own todo");

    assert!(
        quiet_for(&mut alice, Duration::from_millis(500))
            .await
            .is_none(),
        "an idle subscriber of a per-session view must not be woken with a message"
    );

    // And the filter really is server-side: the unfiltered state never reaches her.
    send(&mut alice, add(2, "alice's todo")).await;
    let mut saw = String::new();
    for _ in 0..3 {
        let message = next(&mut alice).await;
        if message["t"] == "p" {
            saw = message.to_string();
            break;
        }
    }
    assert!(saw.contains("alice's todo"));
    assert!(!saw.contains("bob's todo"));
}
