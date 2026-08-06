//! What an untrusted client can do to a running Beck app, over a real socket.
//!
//! `docs/42` §42.6 lists what "an untrusted client can do to a running Beck app today", and two of
//! its bullets were about the websocket upgrade: the limits were tungstenite's defaults because the
//! handshake passed `None`, and nothing inspected `Origin`, so a page on any host could open a
//! socket. `docs/83` is both, closed.
//!
//! This is the **first test in the project to drive `beck-rt`'s HTTP edge**. Every other harness
//! that touches a session goes through `beck_rt::session::run` over an in-memory duplex — which is
//! what the `Socket` trait was written for and is right for testing a subscription — and that means
//! nothing has ever exercised the handshake in front of it. A refusal wired into `upgrade` and
//! tested only as a pure function is a refusal one refactor away from never being called.
//!
//! So the client here is a `TcpStream` and a handful of literal bytes, which is what a browser
//! sends. No HTTP client library, because the thing under test is the handshake and a library that
//! manages handshakes for you is the wrong instrument.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

mod support;

/// Start the real server on an ephemeral port and give back its address.
async fn serve() -> (SocketAddr, tokio::sync::watch::Sender<bool>) {
    let app = beck_rt::App::start(
        support::todo_runtime(),
        Arc::new(beck_rt::log::MemoryLog::new()),
        beck_rt::AppConfig::default(),
    )
    .await
    .expect("the example starts");
    let listener = beck_rt::http::bind("127.0.0.1:0".parse().expect("a literal address"))
        .await
        .expect("an ephemeral port");
    let addr = listener.local_addr().expect("a bound address");
    drop(listener); // bound only to learn a free port; `serve` binds it for real

    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = beck_rt::http::serve(app, addr, rx).await;
    });
    // The listener is bound inside the task, so wait for it rather than racing it.
    for _ in 0..200 {
        if TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (addr, tx)
}

/// Send a raw upgrade request and give back the status line.
async fn upgrade_with(addr: SocketAddr, origin: Option<&str>) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("the server is up");
    let origin = origin
        .map(|o| format!("Origin: {o}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "GET /socket HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         {origin}\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .expect("the request is written");
    let mut buf = vec![0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("the server answers")
        .expect("a readable response");
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The page this server rendered gets its socket; a page on any other host does not.
///
/// Both halves in one test on purpose: "cross-origin is refused" is worth nothing without
/// "same-origin still works" beside it, and a check that refuses everything would pass the first
/// assertion alone.
#[tokio::test]
async fn a_socket_opens_for_the_servers_own_page_and_not_for_another_hosts() {
    let (addr, shutdown) = serve().await;

    assert!(
        upgrade_with(addr, Some(&format!("http://{addr}")))
            .await
            .contains("101"),
        "the server's own page must still get a socket"
    );
    assert!(
        upgrade_with(addr, None).await.contains("101"),
        "a client that is not a browser sends no Origin and is not what this defends against"
    );
    assert!(
        upgrade_with(addr, Some("https://evil.example"))
            .await
            .contains("403"),
        "a page on another host must not open a socket"
    );
    assert!(
        upgrade_with(addr, Some("null")).await.contains("403"),
        "a sandboxed iframe has no authority and matches no host"
    );

    let _ = shutdown.send(true);
}
