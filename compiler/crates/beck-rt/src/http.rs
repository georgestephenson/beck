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
    HeaderValue, CACHE_CONTROL, CONNECTION, CONTENT_TYPE, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY,
    UPGRADE,
};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
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
        (&Method::GET, "/beck-thin.js") => Ok(asset(crate::THIN_CLIENT, "text/javascript")),
        (&Method::GET, "/beck.css") => Ok(asset(&crate::css::stylesheet(), "text/css")),
        (&Method::GET, "/socket") => upgrade(app, req),
        (&Method::GET, "/") => document(app, req).await,
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"not found")))?),
    }
}

/// Which actor this request is for. Dev-mode identity, exactly as Phase 0 had it: `?actor=alice`.
fn actor_of(req: &Request<Incoming>) -> String {
    req.uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find_map(|kv| kv.strip_prefix("actor=").map(|v| v.to_string()))
        })
        .unwrap_or_else(|| "dev".to_string())
}

async fn document(app: Arc<App>, req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    let actor = actor_of(&req);
    let seq = app.head();
    let body = app.render(&actor).await?.render();

    // The document carries the position it reflects, so the first socket message either finds
    // nothing to do or is exactly the gap. That is what made hydration free in Phase 0.
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><link rel=\"stylesheet\" href=\"/beck.css\">\
         </head><body data-b-seq=\"{seq}\" data-b-actor=\"{actor}\">{body}\
         <script src=\"/beck-thin.js\" defer></script></body></html>",
        title = app.runtime().placed().program.name,
    );
    Ok(Response::builder()
        .header(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )
        .header(CACHE_CONTROL, HeaderValue::from_static("no-store"))
        .body(Full::new(Bytes::from(html)))?)
}

fn upgrade(app: Arc<App>, mut req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
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
                let socket =
                    WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None)
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
