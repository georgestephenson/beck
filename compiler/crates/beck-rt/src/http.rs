//! HTTP: the first paint, the assets, the probes, and the websocket upgrade.
//!
//! "First paint is free SSR: evaluate pure `view` against the current accumulator, ship HTML."
//! The document below is therefore not a shell that fetches data — it *is* the data, rendered, and
//! the socket that opens afterwards resumes from the `seq` the render reflects. There is no
//! loading state anywhere in a Beck program because there is nothing to load.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    HeaderValue, CACHE_CONTROL, CONNECTION, CONTENT_TYPE, ORIGIN, SEC_WEBSOCKET_ACCEPT,
    SEC_WEBSOCKET_KEY, UPGRADE,
};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
use tokio_tungstenite::WebSocketStream;

use crate::app::App;
use crate::dash::Dashboard;

pub async fn serve(
    app: Arc<App>,
    addr: SocketAddr,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    serve_with_dashboard(app, addr, shutdown, None).await
}

/// Serve, optionally with the dashboard mounted under `/_beck`.
///
/// Optional because the dashboard needs the infrastructure graph, which `beck-rt` cannot build —
/// it does not depend on `beck-infra`, and should not: the runtime does not know what Kubernetes
/// is. Whoever assembles the process knows both, and passes one in.
pub async fn serve_with_dashboard(
    app: Arc<App>,
    addr: SocketAddr,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    dashboard: Option<Arc<Dashboard>>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, store = app.store_kind(), "listening");

    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("draining: no longer accepting connections");
                    return Ok(());
                }
                continue;
            }
        };

        let (stream, _peer) = accepted?;
        stream.set_nodelay(true)?;
        let app = app.clone();
        let dashboard = dashboard.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req| route(app.clone(), dashboard.clone(), req));
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await
            {
                tracing::debug!(error = %e, "connection closed");
            }
        });
    }
}

/// The port the local listener actually bound, for tests that ask for port 0.
pub async fn bind(addr: SocketAddr) -> Result<TcpListener> {
    Ok(TcpListener::bind(addr).await?)
}

async fn route(
    app: Arc<App>,
    dashboard: Option<Arc<Dashboard>>,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>> {
    let path = req.uri().path().to_string();
    if req.method() == Method::GET {
        if let Some(d) = &dashboard {
            if let Some((content_type, body)) = d.route(&path, &app) {
                return Ok(asset(&body, content_type));
            }
        }
    }
    match (req.method(), path.as_str()) {
        (&Method::GET, "/healthz") | (&Method::GET, "/readyz") => Ok(text("ok")),
        (&Method::GET, "/beck-patch.js") => Ok(asset(crate::PATCH_CLIENT, "text/javascript")),
        (&Method::GET, "/beck-thin.js") => Ok(asset(crate::THIN_CLIENT, "text/javascript")),
        (&Method::GET, "/beck-mode-b.js") => Ok(asset(crate::MODE_B_CLIENT, "text/javascript")),
        // The component's slice, for a browser that renders it (§5.1's Mode B). Derived from the
        // running program rather than read from disk, so a tab can never load a bundle the server
        // is not itself executing.
        (&Method::GET, "/beck-bundle.bpk") => Ok(bytes(
            beck_core::Bundle::of(app.runtime().placed()).to_bytes(),
            "application/octet-stream",
        )),
        (&Method::GET, "/beck-kernel.wasm") => Ok(kernel()),
        (&Method::GET, "/beck.css") => Ok(asset(&crate::css::stylesheet(), "text/css")),
        (&Method::GET, "/socket") => upgrade(app, req),
        (&Method::GET, "/") => document(app, req).await,
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"not found")))?),
    }
}

/// What this request *claims* to be — `?actor=alice`, or nothing.
///
/// A claim, not an actor. [`crate::identity`] is what turns one into the other, and this function
/// deliberately does no defaulting: "nobody said" and "somebody said `dev`" are different facts,
/// and the provider is what decides whether the first is acceptable.
fn claimed_actor(req: &Request<Incoming>) -> String {
    req.uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find_map(|kv| kv.strip_prefix("actor=").map(|v| v.to_string()))
        })
        .unwrap_or_default()
}

async fn document(app: Arc<App>, req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    // The server-rendered document is a *view*, so it is behind the same question the socket is:
    // rendering a page for whoever asked would leak exactly what a per-session view exists to keep
    // separate. Under `DevIdentity` a claim of `dev` is what an unauthenticated laptop gets, and
    // that default lives in one place.
    let claimed = claimed_actor(&req);
    let claimed = if claimed.is_empty() && !app.identity().verifies() {
        "dev".to_string()
    } else {
        claimed
    };
    let actor = match app.identity().verify(&claimed) {
        Ok(a) => a.name().to_string(),
        Err(why) => {
            tracing::warn!(
                reason = why.reason(),
                "identity refused for a document request"
            );
            crate::telemetry::telemetry().unauthenticated.incr();
            return Ok(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Full::new(Bytes::from_static(b"unauthenticated")))?);
        }
    };
    let seq = app.head();
    let body = app.render(&actor).await?.render();

    // The document carries the position it reflects, so the first socket message either finds
    // nothing to do or is exactly the gap. That is what made hydration free in Phase 0.
    //
    // `#b-root` is the subscription's frame: a patch path is child indices *from it*, so it has to
    // be an element of its own rather than the body — whose other children are the two script tags
    // below, which an insertion at the frame's root would otherwise be counted against.
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><link rel=\"stylesheet\" href=\"/beck.css\">\
         </head><body>\
         <div id=\"b-root\" data-b-seq=\"{seq}\" data-b-actor=\"{actor}\">{body}</div>\
         <script src=\"/beck-patch.js\" defer></script>\
         <script src=\"{client}\" defer></script></body></html>",
        title = app.runtime().placed().program.name,
        // Which residue this page needs is the component's rendering mode, and the server is the
        // one that knows it: a Mode B document that loaded the thin client would sit waiting for
        // DOM patches the server is never going to send.
        client = match app.runtime().placed().render.mode {
            beck_core::render::Mode::Server => "/beck-thin.js",
            beck_core::render::Mode::Client => "/beck-mode-b.js",
        },
    );
    Ok(Response::builder()
        .header(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )
        .header(CACHE_CONTROL, HeaderValue::from_static("no-store"))
        .body(Full::new(Bytes::from(html)))?)
}

/// Is this upgrade coming from a page the server itself served?
///
/// `Origin` is a header a **browser** sets and a script cannot forge, so it answers exactly one
/// question: is the page asking for this socket the one this server rendered? The check is
/// `Origin`'s authority against `Host`, and it is what stops a page on any other host opening a
/// socket to a Beck app with whatever ambient credentials the visitor's browser carries
/// ([`docs/42`](../../../../../docs/42-security-assurance.md) §42.6, third bullet).
///
/// Three decisions, because each could have gone the other way:
///
/// * **An absent `Origin` is allowed.** Non-browser clients do not send one — `beck test`, a
///   script, a load generator — and the attack this defends against needs a browser, which always
///   sends one. Refusing an absent header would break every non-browser client for no security
///   gain, since an attacker running their own client is not subject to a browser's rules anyway.
/// * **The scheme is not compared.** Behind a TLS-terminating gateway — which is what
///   [`docs/06`](../../../../../docs/06-kubernetes-and-packaging.md) §6.5's HTTPRoute is — the page
///   is `https://app.example` and the request arriving here is plain HTTP. Comparing schemes would
///   refuse every deployment this project generates.
/// * **There is no allowlist.** A Beck app serves its own page (§5.2's first paint), so same-origin
///   is not a policy choice but a description. A deployment that genuinely needs a cross-origin
///   client has nothing to configure yet, and §42.6 is where that is recorded.
///
/// Takes the headers rather than the request so it can be tested as what it is — a function of two
/// strings — rather than through a socket.
pub(crate) fn same_origin(headers: &hyper::HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    // `Origin: null` — a sandboxed iframe or a `file://` page — has no authority and matches
    // nothing, which is the answer it should get.
    let authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    headers
        .get(hyper::header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|host| host == authority)
}

/// What a client may send, in numbers this project chose.
///
/// [`docs/42`](../../../../../docs/42-security-assurance.md) §42.6's second bullet: the upgrade
/// passed `None`, so the limits were tungstenite's defaults — 64 MiB a message and 16 MiB a frame.
/// Bounded, but by somebody else's judgement. These are the arguments for these numbers:
///
/// * **256 KiB a message, and the same a frame.** A client sends two things: a `hello` naming a
///   subscription and an actor, and a `Cmd` carrying one value of the program's own `union
///   Command`. The largest field either can hold is text a person typed into a form, and 256 KiB
///   is around a hundred pages of it. Nothing legitimate approaches it and 64 MiB is 256× further
///   away.
/// * **8 KiB of read buffer**, down from 128 KiB. It is **eagerly allocated per connection**, and
///   §5.3 makes per-subscriber memory a number this project reports rather than hopes about — the
///   library's own default is tuned for high read load, and a Beck client sends a few hundred
///   bytes when somebody clicks something.
/// * **8 MiB of write buffer at most**, down from unbounded. It only grows past
///   `write_buffer_size` when writes are failing, so this is backpressure against a client that
///   has stopped reading rather than a limit on what a healthy one is sent.
///
/// Outgoing patches are unaffected: `max_message_size` and `max_frame_size` bound what is *read*.
fn socket_limits() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(8 * 1024)
        .max_write_buffer_size(8 << 20)
        .max_message_size(Some(256 << 10))
        .max_frame_size(Some(256 << 10))
}

fn upgrade(app: Arc<App>, mut req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    if !same_origin(req.headers()) {
        // Coarse to the caller on purpose, and the same shape `docs/48` §48.3 chose for a refused
        // identity: a cross-origin page learns that it was refused and nothing about why.
        return Ok(Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Full::new(Bytes::from_static(b"forbidden")))?);
    }
    let key = req
        .headers()
        .get(SEC_WEBSOCKET_KEY)
        .map(|k| derive_accept_key(k.as_bytes()));
    let Some(accept) = key else {
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Full::new(Bytes::from_static(
                b"expected a websocket upgrade",
            )))?);
    };

    tokio::spawn(async move {
        match hyper::upgrade::on(&mut req).await {
            Ok(upgraded) => {
                let socket = WebSocketStream::from_raw_socket(
                    TokioIo::new(upgraded),
                    Role::Server,
                    Some(socket_limits()),
                )
                .await;
                if let Err(e) = crate::session::run(app, socket).await {
                    tracing::debug!(error = %e, "subscription ended");
                }
            }
            Err(e) => tracing::debug!(error = %e, "upgrade failed"),
        }
    });

    Ok(Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(CONNECTION, HeaderValue::from_static("Upgrade"))
        .header(UPGRADE, HeaderValue::from_static("websocket"))
        .header(SEC_WEBSOCKET_ACCEPT, accept)
        .body(Full::new(Bytes::new()))?)
}

fn text(s: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
        .body(Full::new(Bytes::from(s.to_string())))
        .expect("static response builds")
}

fn asset(body: &str, mime: &'static str) -> Response<Full<Bytes>> {
    Response::builder()
        .header(CONTENT_TYPE, HeaderValue::from_static(mime))
        .header(
            CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=60"),
        )
        .body(Full::new(Bytes::from(body.to_string())))
        .expect("static response builds")
}

fn bytes(body: Vec<u8>, mime: &'static str) -> Response<Full<Bytes>> {
    Response::builder()
        .header(CONTENT_TYPE, HeaderValue::from_static(mime))
        .header(
            CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=60"),
        )
        .body(Full::new(Bytes::from(body)))
        .expect("static response builds")
}

/// Where the Mode B kernel is looked for.
///
/// `BECK_KERNEL` names the module; without it, the path `cargo build -p beck-wasm --release
/// --target wasm32-unknown-unknown` writes to, relative to the working directory. The kernel is a
/// *build artefact of this workspace* rather than something compiled into the binary, because
/// building it needs a target the compiler's own build does not: making `beck` depend on a wasm
/// toolchain to serve a Mode A page would be the wrong trade.
pub fn kernel_path() -> std::path::PathBuf {
    std::env::var_os("BECK_KERNEL").map_or_else(
        || std::path::PathBuf::from("target/wasm32-unknown-unknown/release/beck_wasm.wasm"),
        std::path::PathBuf::from,
    )
}

/// The kernel, or a refusal that says what to do about it.
fn kernel() -> Response<Full<Bytes>> {
    let path = kernel_path();
    match std::fs::read(&path) {
        Ok(module) => bytes(module, "application/wasm"),
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "no Mode B kernel to serve");
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from(format!(
                    "no kernel at {}: build it with `cargo build -p beck-wasm --release \
                     --target wasm32-unknown-unknown`, or set BECK_KERNEL",
                    path.display()
                ))))
                .expect("static response builds")
        }
    }
}

#[cfg(test)]
mod tests {
    /// The frame root the served JavaScript looks for, and the attributes it reads off it.
    ///
    /// Both clients open with `document.getElementById("b-root")` and give up if it is missing, so
    /// a document without it is a page that never connects — and nothing would have said so,
    /// because no test in this workspace runs JavaScript (`docs/94` §94.8).
    #[test]
    fn the_document_carries_the_frame_root_the_residue_looks_for() {
        for client in [crate::THIN_CLIENT, crate::MODE_B_CLIENT] {
            assert!(
                client.contains(r#"getElementById("b-root")"#),
                "a client that does not look for the frame root"
            );
        }
        for read in ["dataset.bActor", "dataset.bSeq"] {
            assert!(
                crate::THIN_CLIENT.contains(read) || crate::MODE_B_CLIENT.contains(read),
                "nothing reads {read}"
            );
        }
    }

    use super::*;
    use hyper::HeaderMap;

    fn headers(pairs: &[(hyper::header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(k.clone(), HeaderValue::from_str(v).expect("a legal header"));
        }
        h
    }

    #[test]
    fn a_page_this_server_served_may_open_a_socket() {
        assert!(same_origin(&headers(&[
            (ORIGIN, "http://app.example"),
            (hyper::header::HOST, "app.example"),
        ])));
        // …including on a port, which is what `beck run` on a laptop looks like.
        assert!(same_origin(&headers(&[
            (ORIGIN, "http://localhost:8080"),
            (hyper::header::HOST, "localhost:8080"),
        ])));
    }

    #[test]
    fn a_page_on_another_host_may_not() {
        assert!(!same_origin(&headers(&[
            (ORIGIN, "https://evil.example"),
            (hyper::header::HOST, "app.example"),
        ])));
        // A different port is a different origin, which is the browser's rule and not ours.
        assert!(!same_origin(&headers(&[
            (ORIGIN, "http://app.example:9999"),
            (hyper::header::HOST, "app.example:8080"),
        ])));
        // A prefix is not an authority: `app.example.evil.test` must not pass as `app.example`.
        assert!(!same_origin(&headers(&[
            (ORIGIN, "https://app.example.evil.test"),
            (hyper::header::HOST, "app.example"),
        ])));
    }

    /// `Origin: null` is what a sandboxed iframe and a `file://` page send, and it is a value with
    /// no authority — so it matches no host and is refused rather than treated as absent.
    #[test]
    fn a_null_origin_is_refused_rather_than_ignored() {
        assert!(!same_origin(&headers(&[
            (ORIGIN, "null"),
            (hyper::header::HOST, "app.example"),
        ])));
    }

    /// The decision that lets every non-browser client keep working.
    ///
    /// `beck test`, a script and a load generator send no `Origin`; the attack this defends against
    /// needs a browser, and a browser always sends one.
    #[test]
    fn a_client_that_is_not_a_browser_sends_no_origin_and_is_allowed() {
        assert!(same_origin(&headers(&[(
            hyper::header::HOST,
            "app.example"
        )])));
        assert!(same_origin(&HeaderMap::new()));
    }

    /// The scheme is deliberately not compared: behind a TLS-terminating gateway the page is
    /// `https://` and the request arriving here is not.
    #[test]
    fn the_scheme_is_not_part_of_the_comparison() {
        assert!(same_origin(&headers(&[
            (ORIGIN, "https://app.example"),
            (hyper::header::HOST, "app.example"),
        ])));
    }

    /// The numbers, asserted so that changing one is a decision rather than an edit.
    ///
    /// `docs/83` §83.2 is the argument for each; this is what stops the file drifting back to
    /// somebody else's defaults without the argument moving too.
    #[test]
    fn the_socket_limits_are_the_numbers_this_project_chose() {
        let c = socket_limits();
        assert_eq!(c.max_message_size, Some(256 << 10));
        assert_eq!(c.max_frame_size, Some(256 << 10));
        assert_eq!(c.read_buffer_size, 8 * 1024);
        assert_eq!(c.max_write_buffer_size, 8 << 20);
        // …and every one of them is tighter than the library's, which is the point.
        let d = WebSocketConfig::default();
        assert!(c.max_message_size < d.max_message_size);
        assert!(c.max_frame_size < d.max_frame_size);
        assert!(c.read_buffer_size < d.read_buffer_size);
        assert!(c.max_write_buffer_size < d.max_write_buffer_size);
    }
}
