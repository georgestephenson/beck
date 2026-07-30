//! HTTP: the first paint, the static assets, the probes, and the websocket upgrade.
//!
//! "First paint is free SSR: evaluate pure `view` against the current accumulator, ship HTML."
//! The document below is therefore not a shell that fetches data — it *is* the data, rendered, and
//! the socket that opens afterwards resumes from the `seq` the render reflects. There is no
//! loading state anywhere in this program because there is nothing to load.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
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

use beck_p0_core::css::stylesheet;
use beck_p0_core::domain::ActorId;
use beck_p0_core::protocol::ScopeSel;

use crate::app::{scope_of, App};

/// The thin client: compiler residue, and the only JavaScript in the system (§5.1).
pub const THIN_CLIENT: &str = include_str!("../../../client/beck-thin.js");

pub async fn serve(
    app: Arc<App>,
    addr: SocketAddr,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
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
        tokio::spawn(async move {
            let service = service_fn(move |req| route(app.clone(), req));
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

async fn route(app: Arc<App>, mut req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    let path = req.uri().path().to_string();
    match (req.method(), path.as_str()) {
        (&Method::GET, "/") => first_paint(&app, &req).await,
        (&Method::GET, "/beck.js") => Ok(asset(THIN_CLIENT, "text/javascript; charset=utf-8")),
        (&Method::GET, "/app.css") => Ok(asset(&stylesheet(), "text/css; charset=utf-8")),
        (&Method::GET, "/healthz") => Ok(text(StatusCode::OK, "ok")),
        // Readiness is "the fold has recovered", which is true by the time this server binds:
        // `App::start` folds the log before `serve` is called. The generated deployment wires its
        // probes here (§6.3).
        (&Method::GET, "/readyz") => Ok(text(StatusCode::OK, "ready")),
        (&Method::GET, "/metrics") => Ok(text(
            StatusCode::OK,
            &app.metrics().render(app.store_kind(), app.head()),
        )),
        (&Method::GET, "/socket") => upgrade(app, &mut req),
        _ => Ok(text(StatusCode::NOT_FOUND, "not found")),
    }
}

async fn first_paint(app: &Arc<App>, req: &Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    let query = req.uri().query().unwrap_or_default();
    let actor = ActorId::new(param(query, "actor").unwrap_or_else(|| "dev".to_string()));
    let sel = match param(query, "scope").as_deref() {
        Some("mine") => ScopeSel::Mine,
        _ => ScopeSel::All,
    };
    let (seq, view) = app.view_now(&scope_of(sel, &actor)).await;

    let body = format!(
        "<!doctype html>\
<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>todos</title><link rel=\"stylesheet\" href=\"/app.css\"></head>\
<body><div id=\"b-root\" data-b-seq=\"{seq}\" data-b-actor=\"{actor}\" data-b-scope=\"{scope}\">{view}</div>\
<script src=\"/beck.js\" defer></script></body></html>",
        seq = seq,
        actor = actor,
        scope = if matches!(sel, ScopeSel::Mine) { "mine" } else { "all" },
        view = view.render(),
    );

    app.metrics().ssr_renders.fetch_add(1, Ordering::Relaxed);
    app.metrics()
        .ssr_bytes
        .fetch_add(body.len() as u64, Ordering::Relaxed);

    let mut response = Response::new(Full::new(Bytes::from(body)));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    // The first paint is a function of the fold's current position, so it is never cacheable —
    // but everything it references is content-stable and cached hard.
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// Websocket upgrade, done by hand: 101, the derived accept key, then hand the raw stream to the
/// subscription loop (§5.1's "connection layer").
fn upgrade(app: Arc<App>, req: &mut Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    let Some(key) = req.headers().get(SEC_WEBSOCKET_KEY).cloned() else {
        return Ok(text(
            StatusCode::BAD_REQUEST,
            "expected a websocket upgrade",
        ));
    };
    let accept = derive_accept_key(key.as_bytes());
    let upgraded = hyper::upgrade::on(req);

    tokio::spawn(async move {
        match upgraded.await {
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

    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    response
        .headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("websocket"));
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    response
        .headers_mut()
        .insert(SEC_WEBSOCKET_ACCEPT, HeaderValue::from_str(&accept)?);
    Ok(response)
}

fn asset(body: &str, content_type: &'static str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
}

fn text(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| percent_decode(v))
    })
}

/// Enough percent-decoding for a dev-mode actor name. Phase 3 replaces this whole path with
/// verified OIDC claims (D6), so it deliberately does not grow into a URL library.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(byte) => {
                    out.push(byte);
                    i += 3;
                }
                Err(_) => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dev_identity_from_the_query() {
        assert_eq!(
            param("actor=alice&scope=mine", "actor").as_deref(),
            Some("alice")
        );
        assert_eq!(param("actor=a%20b", "actor").as_deref(), Some("a b"));
        assert_eq!(param("scope=mine", "actor"), None);
    }
}
