//! The outbound HTTP client: `beck_core::net::Outbound`, over hyper.
//!
//! The seam is in `beck-core` because the evaluator needs it and the evaluator must not know what
//! a socket is. The implementation is here because this is the crate that already has an HTTP
//! stack — [`docs/07`](../../../../docs/07-dependencies.md) chose hyper, and the server half of it
//! has been in [`crate::http`] since Phase 1. Nothing new was taken to make a request.
//!
//! # Plaintext, and saying so
//!
//! This speaks HTTP/1.1 over TCP and **not** TLS. docs/07 chooses rustls for that, taking it is a
//! dependency decision rather than a line here, and until it is taken an outbound call is
//! confidential only if the network under it is.
//! `beck-cli/tests/pending_security.rs` asserts the absence, so the day rustls arrives a test goes
//! red and [`docs/43`](../../../../docs/43-threat-model.md) has to be corrected in the same change.
//!
//! # Its own runtime
//!
//! [`beck_core::net::Outbound::fetch`] is synchronous, because the evaluator is: a tree-walker
//! cannot await. Rather than block on whatever runtime happens to be current — which panics on a
//! current-thread runtime and steals a worker on a multi-thread one — this owns a small one of its
//! own, on its own thread. An outbound call therefore cannot stall the runtime serving the page.

use std::sync::Arc;
use std::time::Duration;

use beck_core::net::{Failure, Outbound, Reply, Request};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;

/// How long an exchange may take before it is a [`Failure::TimedOut`].
///
/// A default rather than a policy: a per-call deadline is a language question (§3.6 would have to
/// give it a place in a signature) and this is the number that keeps a wedged peer from wedging a
/// fold. It is elapsed time, which [`beck_core::clock`] deliberately does not cover — a deadline
/// does not enter the log and cannot change what a replay produces.
pub const DEFAULT_TIMEOUT_MS: i64 = 10_000;

/// The most of a reply that will be read.
///
/// A peer that streams for ever is the cheapest denial of service there is, and a runtime that
/// reads until EOF is the one that falls for it. 8 MiB is generous for an API response and
/// bounded, which is the property that matters.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct HttpOutbound {
    runtime: tokio::runtime::Runtime,
    timeout: Duration,
}

impl HttpOutbound {
    pub fn new() -> std::io::Result<HttpOutbound> {
        HttpOutbound::with_timeout(DEFAULT_TIMEOUT_MS)
    }

    pub fn with_timeout(millis: i64) -> std::io::Result<HttpOutbound> {
        Ok(HttpOutbound {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("beck-outbound")
                .build()?,
            timeout: Duration::from_millis(millis.max(1) as u64),
        })
    }

    /// Install this as the process's client, if nothing has installed one.
    ///
    /// Returns whether it was installed. A `beck test` process deliberately does not call this:
    /// `net.out` is auto-stubbed there (§21.3), and a test that reached a real socket would be a
    /// test that depends on somebody else's uptime.
    pub fn install() -> bool {
        match HttpOutbound::new() {
            Ok(client) => beck_core::net::set_process_outbound(Arc::new(client)),
            Err(e) => {
                tracing::warn!(error = %e, "no outbound HTTP client: requests will be refused");
                false
            }
        }
    }
}

impl Outbound for HttpOutbound {
    fn fetch(&self, request: &Request) -> Result<Reply, Failure> {
        let millis = self.timeout.as_millis() as i64;
        self.runtime.block_on(async {
            match tokio::time::timeout(self.timeout, exchange(request)).await {
                Ok(result) => result,
                Err(_) => Err(Failure::TimedOut(millis)),
            }
        })
    }
}

async fn exchange(request: &Request) -> Result<Reply, Failure> {
    let authority = format!("{}:{}", request.host, request.port);
    let stream = tokio::net::TcpStream::connect(&authority)
        .await
        .map_err(|e| Failure::Unreachable(e.to_string()))?;
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(|e| Failure::BadResponse(e.to_string()))?;
    // The connection is a future that has to be polled for the exchange to progress. It ends when
    // the response is done; nothing here reuses it, because a pool is a policy and this is a seam.
    let pump = tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut builder = hyper::Request::builder()
        .method(request.method.as_ref())
        .uri(request.path.as_ref())
        // HTTP/1.1 requires it, and it is the name the peer is asked about rather than the address
        // it was reached at.
        .header(hyper::header::HOST, authority.as_str());
    for (name, value) in &request.headers {
        builder = builder.header(name.as_ref(), value.as_ref());
    }
    let outgoing = builder
        .body(Full::new(Bytes::from(request.body.as_bytes().to_vec())))
        .map_err(|e| Failure::BadResponse(e.to_string()))?;

    let response = sender
        .send_request(outgoing)
        .await
        .map_err(|e| Failure::Unreachable(e.to_string()))?;
    let status = response.status().as_u16() as i64;
    let headers: Vec<(Arc<str>, Arc<str>)> = response
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (Arc::from(k.as_str()), Arc::from(v)))
        })
        .collect();
    let body = Limited::new(response.into_body(), MAX_BODY_BYTES)
        .collect()
        .await
        .map_err(|_| {
            Failure::BadResponse(format!("the reply is longer than {MAX_BODY_BYTES} bytes"))
        })?
        .to_bytes();
    pump.abort();
    Ok(Reply {
        status,
        headers,
        body: Arc::from(String::from_utf8_lossy(&body).as_ref()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    /// A server that answers once, and says what it was asked.
    ///
    /// Loopback, on a port the OS picks: this test makes a real HTTP request over a real socket
    /// and reaches nothing outside the process, which is the only way to test a client without
    /// making the suite depend on somebody else's uptime.
    async fn echo_once(reply: &'static str) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("a connection");
            let service = hyper::service::service_fn(
                move |req: hyper::Request<hyper::body::Incoming>| async move {
                    let method = req.method().to_string();
                    let path = req.uri().path().to_string();
                    let hdr = req
                        .headers()
                        .get("x-beck")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let body = req.into_body().collect().await.expect("a body").to_bytes();
                    let seen = format!(
                        "{method} {path} x-beck={hdr} body={}",
                        String::from_utf8_lossy(&body)
                    );
                    Ok::<_, std::convert::Infallible>(
                        hyper::Response::builder()
                            .status(if reply == "seen" { 200 } else { 503 })
                            .header("x-reply", "yes")
                            .body(Full::new(Bytes::from(seen)))
                            .expect("a response"),
                    )
                },
            );
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                .await;
        });
        addr
    }

    fn request(port: u16, path: &str) -> Request {
        Request {
            host: Arc::from("127.0.0.1"),
            port,
            method: Arc::from("POST"),
            path: Arc::from(path),
            headers: vec![(Arc::from("x-beck"), Arc::from("1"))],
            body: Arc::from("hello"),
        }
    }

    #[test]
    fn a_request_reaches_a_real_server_and_the_reply_comes_back() {
        let client = HttpOutbound::new().expect("a client");
        let addr = client.runtime.block_on(echo_once("seen"));
        let reply = client
            .fetch(&request(addr.port(), "/v1/things?x=1"))
            .expect("a reply");
        assert_eq!(reply.status, 200);
        assert_eq!(
            reply.body.as_ref(),
            "POST /v1/things x-beck=1 body=hello",
            "the server saw the method, the path, the header and the body"
        );
        assert!(
            reply
                .headers
                .iter()
                .any(|(k, v)| k.as_ref() == "x-reply" && v.as_ref() == "yes"),
            "{:?}",
            reply.headers
        );
    }

    #[test]
    fn a_status_is_a_reply_and_not_a_failure() {
        let client = HttpOutbound::new().expect("a client");
        let addr = client.runtime.block_on(echo_once("no"));
        let reply = client
            .fetch(&request(addr.port(), "/gone"))
            .expect("a reply");
        assert_eq!(reply.status, 503);
        assert!(reply.body.contains("/gone"), "the body survives a 503");
    }

    #[test]
    fn nothing_listening_is_unreachable_rather_than_a_panic() {
        let client = HttpOutbound::new().expect("a client");
        // Port 1 on loopback: privileged, and nothing in this process is bound to it.
        match client.fetch(&request(1, "/")) {
            Err(Failure::Unreachable(_)) => {}
            other => panic!("expected unreachable, got {other:?}"),
        }
    }
}
