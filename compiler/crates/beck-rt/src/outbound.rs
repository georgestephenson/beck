//! The outbound HTTP client: `beck_core::net::Outbound`, over hyper.
//!
//! The seam is in `beck-core` because the evaluator needs it and the evaluator must not know what
//! a socket is. The implementation is here because this is the crate that already has an HTTP
//! stack — [`docs/07`](../../../../../docs/07-dependencies.md) chose hyper, and the server half of it
//! has been in [`crate::http`] since Phase 1. Nothing new was taken to make a request.
//!
//! # TLS, when the request asked for it
//!
//! [`beck_core::net::Request::tls`] decides, and the certificate is verified against
//! [`beck_core::net::Request::host`] — the string the *call site* wrote, which is also the atom the
//! call performs and therefore the peer in the cluster's egress rule
//! ([`adr/0013`](../../../../../docs/adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md)).
//! There is deliberately no way to reach a peer under a name the deployment was not told about:
//! no SNI override, no `danger_accept_invalid_certs`, no pinning.
//!
//! The trust anchors are Mozilla's, compiled in as data
//! ([`adr/0023`](../../../../../docs/adr/0023-tls-and-the-signature-it-brings.md)) rather than read
//! from the container's filesystem — §6.2's images execute nothing at build time, so a
//! `ca-certificates` package would be one more thing whose version the SBOM cannot state.
//!
//! # Its own runtime
//!
//! [`beck_core::net::Outbound::fetch`] is synchronous, because the evaluator is: a tree-walker
//! cannot await. Rather than block on whatever runtime happens to be current — which panics on a
//! current-thread runtime and steals a worker on a multi-thread one — this owns a small one of its
//! own, on its own thread. An outbound call therefore cannot stall the runtime serving the page.

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use beck_core::net::{Failure, Outbound, Reply, Request, Stop};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;

/// How long an exchange may take before it is a [`Failure::TimedOut`].
///
/// A default rather than a policy: a per-call deadline is a language question (§3.6 would have to
/// give it a place in a signature) and this is the number that keeps a wedged peer from wedging a
/// fold. It is elapsed time, which [`beck_core::clock`] deliberately does not cover — a deadline
/// does not enter the log and cannot change what a replay produces.
pub const DEFAULT_TIMEOUT_MS: i64 = 10_000;

/// How often a watched exchange asks whether the caller still wants the reply.
///
/// Only a request made by a child of a `parallel:` is watched at all, so this is not a cost every
/// outbound call pays. The number is chosen against what cancellation is *for*: a sibling that
/// failed should not leave a scope waiting, and the difference between 5 ms and 50 ms of extra
/// waiting is invisible beside the ten-second timeout it replaces.
const POLL_MS: Duration = Duration::from_millis(5);

/// The most of a reply that will be read.
///
/// A peer that streams for ever is the cheapest denial of service there is, and a runtime that
/// reads until EOF is the one that falls for it. 8 MiB is generous for an API response and
/// bounded, which is the property that matters.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The client half of a TLS session, built once.
///
/// Mozilla's trust anchors and nothing else: no filesystem store, no environment variable naming
/// one, and no way for a program to add to it. What a Beck program may reach is decided by the
/// hosts it writes (§6.5's egress rule); *who* may answer to one of those names is decided here,
/// and neither is a runtime knob.
fn client_config() -> &'static Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(config_for(roots))
    })
}

/// The provider is named rather than defaulted: rustls picks one for you only when exactly one is
/// compiled in, and "exactly one" is a property of somebody else's feature unification.
fn config_for(roots: tokio_rustls::rustls::RootCertStore) -> ClientConfig {
    let mut config = ClientConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the default protocol versions are supported by the provider")
    .with_root_certificates(roots)
    .with_no_client_auth();
    // The exchange is one request over HTTP/1.1 (`beck_core::net::Outbound`), so say so: a peer
    // that negotiates h2 against a client that cannot speak it is a hang rather than an error.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

#[derive(Debug)]
pub struct HttpOutbound {
    runtime: tokio::runtime::Runtime,
    timeout: Duration,
    /// Whose certificates are believed. `None` is Mozilla's set, which is every deployment;
    /// `Some` is a test that made its own certificate authority a moment ago.
    roots: Option<Arc<ClientConfig>>,
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
            roots: None,
        })
    }

    /// A client that believes one certificate authority instead of Mozilla's.
    ///
    /// `#[cfg(test)]`, and that is the point: a real TLS handshake is the only way to check that
    /// this module speaks TLS rather than that it compiles against a TLS library, and it needs a
    /// certificate somebody trusts. Shipping the knob would put "trust this instead" one call away
    /// from every deployment, so it is not shipped.
    #[cfg(test)]
    fn trusting(roots: tokio_rustls::rustls::RootCertStore) -> std::io::Result<HttpOutbound> {
        Ok(HttpOutbound {
            roots: Some(Arc::new(config_for(roots))),
            ..HttpOutbound::new()?
        })
    }

    fn tls(&self) -> Arc<ClientConfig> {
        self.roots
            .clone()
            .unwrap_or_else(|| Arc::clone(client_config()))
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
    fn fetch(&self, request: &Request, stop: &Stop) -> Result<Reply, Failure> {
        let millis = self.timeout.as_millis() as i64;
        let tls = self.tls();
        self.runtime.block_on(async {
            // The ordinary case has no watcher, and pays for none: only a child of a `parallel:`
            // can be cancelled, so a request from anywhere else takes the path this always had.
            if !stop.watched() {
                return match tokio::time::timeout(self.timeout, exchange(request, tls)).await {
                    Ok(result) => result,
                    Err(_) => Err(Failure::TimedOut(millis)),
                };
            }
            tokio::select! {
                // Biased so that a reply already in hand wins a stop that arrived in the same
                // tick: answering is never worse than not answering, and an arbitrary choice here
                // would make a cancelled scope's *timing* decide whether a peer was called for
                // nothing.
                biased;
                result = tokio::time::timeout(self.timeout, exchange(request, tls)) => match result {
                    Ok(result) => result,
                    Err(_) => Err(Failure::TimedOut(millis)),
                },
                () = watch(stop) => Err(Failure::Stopped),
            }
        })
    }
}

/// Resolve when the caller stops wanting the reply.
///
/// A poll rather than a notification, because [`Stop`] is a predicate over state the *caller*
/// already keeps — the chain of enclosing scopes and their first-failed indices — and a
/// notification would be a second copy of it. [`POLL_MS`] is what that costs.
async fn watch(stop: &Stop) {
    loop {
        if stop.asked() {
            return;
        }
        tokio::time::sleep(POLL_MS).await;
    }
}

async fn exchange(request: &Request, tls: Arc<ClientConfig>) -> Result<Reply, Failure> {
    let authority = format!("{}:{}", request.host, request.port);
    let stream = tokio::net::TcpStream::connect(&authority)
        .await
        .map_err(|e| Failure::Unreachable(e.to_string()))?;
    if !request.tls {
        return speak_http(stream, request, &authority).await;
    }
    // The name checked is `request.host` — the literal the call site wrote, which is the atom the
    // call performs. There is no other string here it *could* be checked against, which is what
    // makes "every host a program can reach is one the program named" hold on the inbound half too.
    let name = ServerName::try_from(request.host.to_string()).map_err(|_| {
        Failure::Unreachable(format!(
            "`{}` is not a name a certificate can be checked against",
            request.host
        ))
    })?;
    let stream = TlsConnector::from(tls)
        .connect(name, stream)
        .await
        .map_err(|e| {
            Failure::Unreachable(format!(
                "the TLS handshake with `{}` failed: {e}",
                request.host
            ))
        })?;
    speak_http(stream, request, &authority).await
}

/// The HTTP/1.1 half, over whichever stream the caller established.
///
/// Generic so that the plaintext and TLS paths are the *same* exchange rather than two written
/// out: a difference between them would be a difference nothing in the suite could see, since the
/// tests that assert what a request looks like run over the plaintext one.
async fn speak_http<S>(stream: S, request: &Request, authority: &str) -> Result<Reply, Failure>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
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
        .header(hyper::header::HOST, authority);
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
            tls: false,
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
            .fetch(&request(addr.port(), "/v1/things?x=1"), &Stop::never())
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
            .fetch(&request(addr.port(), "/gone"), &Stop::never())
            .expect("a reply");
        assert_eq!(reply.status, 503);
        assert!(reply.body.contains("/gone"), "the body survives a 503");
    }

    /// The client lets go of a peer that never answers, when the caller stops wanting the reply.
    ///
    /// A real socket that accepts and then says nothing, which is the shape a hung peer has: the
    /// exchange is genuinely in flight, so this is `Stop` reaching *into* the await rather than a
    /// check made before or after it. The alternative is the ten-second timeout, and a `parallel:`
    /// whose first child failed spending all of it.
    #[test]
    fn a_watched_request_is_given_up_when_the_caller_stops_wanting_it() {
        let client = HttpOutbound::new().expect("a client");
        let addr = client.runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("a port");
            let addr = listener.local_addr().expect("an address");
            tokio::spawn(async move {
                // Accept and hold: no bytes, no close. Dropped when the runtime is.
                let held = listener.accept().await;
                std::future::pending::<()>().await;
                drop(held);
            });
            addr
        });

        let asked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = {
            let asked = Arc::clone(&asked);
            Stop::when(move || asked.load(std::sync::atomic::Ordering::SeqCst))
        };
        // Set from another thread while the call is in flight, which is what a sibling failing in
        // a `parallel:` looks like from here.
        let flip = {
            let asked = Arc::clone(&asked);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                asked.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        };

        let out = client.fetch(&request(addr.port(), "/hangs"), &stop);
        flip.join().expect("the flipping thread");
        assert!(
            matches!(out, Err(Failure::Stopped)),
            "the client should give up on a stopped request rather than wait out its timeout, and \
             it answered {out:?}"
        );
    }

    #[test]
    fn nothing_listening_is_unreachable_rather_than_a_panic() {
        let client = HttpOutbound::new().expect("a client");
        // Port 1 on loopback: privileged, and nothing in this process is bound to it.
        match client.fetch(&request(1, "/"), &Stop::never()) {
            Err(Failure::Unreachable(_)) => {}
            other => panic!("expected unreachable, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------------------------ TLS
    //
    // A real handshake against a real server, because the thing worth checking is that this module
    // speaks TLS rather than that it compiles against a TLS library. The certificate is made here,
    // a moment before it is used, and trusted by *this client only* — `HttpOutbound::trusting` is
    // `#[cfg(test)]` so the knob does not exist in a deployment.

    /// A certificate authority, and the trust store that believes exactly it.
    fn certificate_authority() -> (
        rcgen::Issuer<'static, rcgen::KeyPair>,
        tokio_rustls::rustls::RootCertStore,
    ) {
        let mut params = rcgen::CertificateParams::new(Vec::new()).expect("no names on a CA");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let key = rcgen::KeyPair::generate().expect("a key pair");
        let ca = params.self_signed(&key).expect("a self-signed CA");
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots
            .add(ca.der().clone())
            .expect("the CA is a certificate");
        (rcgen::Issuer::new(params, key), roots)
    }

    /// A TLS server on loopback that answers one request, presenting a certificate for whatever
    /// name it is told to claim.
    async fn tls_echo_once(
        issuer: &rcgen::Issuer<'static, rcgen::KeyPair>,
        claims: &str,
    ) -> SocketAddr {
        let params =
            rcgen::CertificateParams::new(vec![claims.to_string()]).expect("a subject alt name");
        let key = rcgen::KeyPair::generate().expect("a key pair");
        let leaf = params.signed_by(&key, issuer).expect("issued by the CA");

        let config = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
            tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("the default protocol versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            tokio_rustls::rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
        )
        .expect("a server configuration");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
            let (stream, _) = listener.accept().await.expect("a connection");
            let Ok(stream) = acceptor.accept(stream).await else {
                return;
            };
            let service = hyper::service::service_fn(
                |_req: hyper::Request<hyper::body::Incoming>| async move {
                    Ok::<_, std::convert::Infallible>(hyper::Response::new(Full::new(
                        Bytes::from_static(b"ok, privately"),
                    )))
                },
            );
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                .await;
        });
        addr
    }

    fn secure_request(host: &str, port: u16) -> Request {
        Request {
            host: Arc::from(host),
            port,
            tls: true,
            method: Arc::from("GET"),
            path: Arc::from("/"),
            headers: Vec::new(),
            body: Arc::from(""),
        }
    }

    /// A real handshake, with the name verified.
    ///
    /// The host a request carries is both the address it is reached at and the name the
    /// certificate must answer for, so the certificate is issued for `127.0.0.1` — an IP
    /// subject-alt-name, which is a name a certificate can carry. That the verification is real
    /// and not a formality is the second half: the same trust anchor, a certificate for another
    /// name, refused.
    #[test]
    fn tls_verifies_the_name_the_call_site_wrote() {
        let (issuer, roots) = certificate_authority();
        let client = HttpOutbound::trusting(roots).expect("a client");

        let addr = client.runtime.block_on(tls_echo_once(&issuer, "127.0.0.1"));
        let reply = client
            .fetch(&secure_request("127.0.0.1", addr.port()), &Stop::never())
            .expect("the handshake completes and the reply comes back");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body.as_ref(), "ok, privately");

        // Same trust anchor, a certificate for somebody else: refused.
        let (issuer, roots) = certificate_authority();
        let client = HttpOutbound::trusting(roots).expect("a client");
        let addr = client
            .runtime
            .block_on(tls_echo_once(&issuer, "elsewhere.test"));
        match client.fetch(&secure_request("127.0.0.1", addr.port()), &Stop::never()) {
            Err(Failure::Unreachable(why)) => assert!(why.contains("handshake"), "{why}"),
            other => panic!("a certificate for another name was accepted: {other:?}"),
        }
    }

    /// And a peer that is not speaking TLS at all is a failure rather than a hang or a plaintext
    /// request sent in the clear — which is the mode confusion this field exists to prevent.
    #[test]
    fn a_plaintext_peer_does_not_answer_a_request_that_asked_for_tls() {
        let client = HttpOutbound::new().expect("a client");
        let addr = client.runtime.block_on(echo_once("seen"));
        match client.fetch(&secure_request("127.0.0.1", addr.port()), &Stop::never()) {
            Err(Failure::Unreachable(why)) => assert!(why.contains("handshake"), "{why}"),
            other => panic!("a plaintext peer answered a TLS request: {other:?}"),
        }
    }
}
