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
                    // And the ones already accepted: a subscription is a task of its own, and a
                    // server that only stops *accepting* leaves every open socket up for as long
                    // as the process lives (§5.2's third clause, `App::drain`).
                    app.drain();
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
        // The worker's cache is keyed by the program it is caching, so the id is substituted here
        // rather than fetched by the worker: a worker that had to ask the server which program it
        // was would be asking the one thing it exists to survive the absence of.
        (&Method::GET, "/beck-sw.js") => Ok(asset(
            &crate::SERVICE_WORKER.replace("%WIRE%", app.runtime().wire_id()),
            "text/javascript",
        )),
        // The component's slice, for a browser that renders it (§5.1's Mode B). Derived from the
        // running program rather than read from disk, so a tab can never load a bundle the server
        // is not itself executing.
        (&Method::GET, "/beck-bundle.bpk") => Ok(bytes(
            beck_core::Bundle::of(app.runtime().placed()).to_bytes(),
            "application/octet-stream",
        )),
        (&Method::GET, "/beck-kernel.wasm") => Ok(kernel()),
        (&Method::GET, "/beck.css") => Ok(asset(&crate::css::stylesheet(), "text/css")),
        (&Method::GET, "/beck-devtools.js") => Ok(asset(crate::DEVTOOLS_CLIENT, "text/javascript")),
        // What the panel draws: the program's own signal graph and the dataflow the compiler made
        // of its view. Derived from the running program, so a panel cannot describe a version of
        // the program this process is not executing.
        (&Method::GET, "/beck-signals.json") => Ok(asset(
            crate::signals::document(app.runtime().placed(), app.runtime().plan()),
            "application/json",
        )),
        (&Method::GET, LOGIN_PATH) => login(app, &req).await,
        (&Method::GET, CALLBACK_PATH) => callback(app, &req).await,
        (&Method::GET, LOGOUT_PATH) => Ok(logout()),
        (&Method::GET, "/socket") => upgrade(app, req),
        // Every other GET is the application, at that route. A Beck program has one page and that
        // page is a function of `session.path`, so there is nothing here to match a route against:
        // the *program* decides what `/done` means, and this decides only that `/done` is a page
        // rather than a missing file. Which is what makes a deep link and a reload work — the
        // route is established by the request that renders, and not by a script afterwards.
        (&Method::GET, _) => document(app, req).await,
        _ => Ok(not_found()),
    }
}

/// A path this process answers itself, so a program's routes cannot be shadowed by one.
///
/// The list is derived from the `match` above rather than written twice — `route_is_reserved` is
/// what the gate holds it to. It exists as a function because a program's author needs to know
/// which routes are not theirs, and "read the router" is not an answer.
pub fn reserved_routes() -> &'static [&'static str] {
    &[
        "/healthz",
        "/readyz",
        "/socket",
        "/beck.css",
        "/beck-patch.js",
        "/beck-thin.js",
        "/beck-mode-b.js",
        "/beck-devtools.js",
        "/beck-sw.js",
        "/beck-bundle.bpk",
        "/beck-kernel.wasm",
        "/beck-signals.json",
        LOGIN_PATH,
        CALLBACK_PATH,
        LOGOUT_PATH,
    ]
}

/// Where the login flow lives. Fixed paths rather than configurable ones: they are registered with
/// an identity provider as a redirect URI, and a path an operator can move is a path that stops
/// matching what was registered.
pub const LOGIN_PATH: &str = "/auth/login";
pub const CALLBACK_PATH: &str = "/auth/callback";
pub const LOGOUT_PATH: &str = "/auth/logout";

/// The cookie carrying the credential a verified connection is identified by.
///
/// Under [`crate::oidc`] its value is the **ID token itself** — the issuer's, not one this process
/// made — so every connection re-verifies the issuer's signature rather than a local session's.
const SESSION_COOKIE: &str = "beck_id";
/// The cookie carrying the sealed login transaction, alive only between `/auth/login` and
/// `/auth/callback`.
const TRANSACTION_COOKIE: &str = "beck_login";

/// What this request *claims* to be: the session cookie, then a bearer credential, then
/// `?actor=alice`, then nothing.
///
/// A claim, not an actor. [`crate::identity`] is what turns one into the other, and this function
/// deliberately does no defaulting: "nobody said" and "somebody said `dev`" are different facts,
/// and the provider is what decides whether the first is acceptable.
///
/// The cookie comes first because it is the one a **browser** sends by itself, and the query
/// parameter comes last because a credential in a URL is a credential in a log file — it stays
/// because it is what `beck run` on a laptop has always used, and under a verifying provider it
/// carries a token nobody would put there.
fn claimed_actor(headers: &hyper::HeaderMap, query: Option<&str>) -> String {
    if let Some(cookie) = cookie(headers, SESSION_COOKIE) {
        return cookie;
    }
    if let Some(bearer) = headers
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return bearer.trim().to_string();
    }
    query
        .and_then(|q| {
            crate::oidc::query_params(q)
                .into_iter()
                .find(|(k, _)| k == "actor")
                .map(|(_, v)| v)
        })
        .unwrap_or_default()
}

/// One cookie's value out of a `Cookie` header, or nothing.
fn cookie(headers: &hyper::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(hyper::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// A cookie a script cannot read and a cross-site request does not send.
///
/// `HttpOnly` because the value is a credential and §5.1's thin client has no reason to read it.
/// `SameSite=Lax` rather than `Strict` because the identity provider redirects the browser back to
/// `/auth/callback` and `Strict` would withhold the transaction cookie on exactly that navigation.
/// `Secure` is **not** set: §6.5's gateway terminates TLS in front of a plaintext hop, so setting
/// it would make the cookie unusable in the deployment this project generates — the same reason
/// `same_origin` does not compare schemes, and it is recorded in `docs/95` §95.6 rather than left
/// to be discovered.
fn set_cookie(name: &str, value: &str, max_age: i64) -> String {
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}")
}

/// Send the browser somewhere, setting or clearing cookies on the way.
fn redirect(to: &str, cookies: &[String]) -> Response<Full<Bytes>> {
    let mut builder = Response::builder()
        .status(StatusCode::FOUND)
        .header(hyper::header::LOCATION, to)
        .header(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    for cookie in cookies {
        builder = builder.header(hyper::header::SET_COOKIE, cookie);
    }
    builder
        .body(Full::new(Bytes::new()))
        .expect("a redirect builds")
}

/// `/auth/login` — begin the authorization-code flow.
async fn login(app: Arc<App>, req: &Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    let Some(party) = app.identity().login() else {
        return Ok(not_found());
    };
    let return_to = req
        .uri()
        .query()
        .and_then(|q| {
            crate::oidc::query_params(q)
                .into_iter()
                .find(|(k, _)| k == "next")
                .map(|(_, v)| v)
        })
        .unwrap_or_else(|| "/".to_string());
    match party.begin_login(&return_to) {
        Ok(begun) => Ok(redirect(
            &begun.url,
            &[set_cookie(
                TRANSACTION_COOKIE,
                &begun.transaction,
                crate::oidc::LOGIN_WINDOW_MS / 1_000,
            )],
        )),
        Err(why) => {
            tracing::warn!(why, "a login could not be started");
            Ok(unavailable())
        }
    }
}

/// `/auth/callback` — the browser is back from the identity provider.
async fn callback(app: Arc<App>, req: &Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    if app.identity().login().is_none() {
        return Ok(not_found());
    }
    let transaction = cookie(req.headers(), TRANSACTION_COOKIE).unwrap_or_default();
    let query = req.uri().query().unwrap_or_default().to_string();
    // Synchronous, and `beck_core::net::Outbound` is deliberately synchronous too (a tree-walker
    // cannot await), so the token exchange goes on a blocking thread rather than on a worker
    // serving pages.
    let holder = app.clone();
    let completed = tokio::task::spawn_blocking(move || match holder.identity().login() {
        Some(party) => party.complete_login(&query, &transaction),
        None => Err("this process has no relying party".to_string()),
    })
    .await;

    match completed {
        Ok(Ok(done)) => {
            tracing::info!(actor = %done.verified.subject, "a login completed");
            let seconds =
                (done.verified.expires_at_millis - app.clock().now_millis()).max(0) / 1_000;
            Ok(redirect(
                &done.return_to,
                &[
                    set_cookie(SESSION_COOKIE, &done.id_token, seconds),
                    // The transaction is spent. Clearing it is not tidiness: a state and a PKCE
                    // verifier that outlive their one use are a replayable login.
                    set_cookie(TRANSACTION_COOKIE, "", 0),
                ],
            ))
        }
        Ok(Err(why)) => {
            // Specific to the operator, and the browser is told it was refused and nothing else.
            tracing::warn!(why, "a login did not complete");
            crate::telemetry::telemetry().unauthenticated.incr();
            Ok(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(
                    hyper::header::SET_COOKIE,
                    set_cookie(TRANSACTION_COOKIE, "", 0),
                )
                .body(Full::new(Bytes::from_static(b"unauthenticated")))?)
        }
        Err(e) => {
            tracing::warn!(error = %e, "the token exchange panicked");
            Ok(unavailable())
        }
    }
}

/// `/auth/logout` — forget the credential.
///
/// Local only: it clears this app's cookie and does not call the issuer's end-session endpoint, so
/// the browser is still signed in to the identity provider and `/auth/login` will complete without
/// another password. That is the ordinary meaning of "log out of this app" and `docs/95` §95.6 says
/// so rather than leaving somebody to find out.
fn logout() -> Response<Full<Bytes>> {
    redirect(
        "/",
        &[
            set_cookie(SESSION_COOKIE, "", 0),
            set_cookie(TRANSACTION_COOKIE, "", 0),
        ],
    )
}

fn not_found() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Full::new(Bytes::from_static(b"not found")))
        .expect("static response builds")
}

fn unavailable() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(Full::new(Bytes::from_static(b"identity is unavailable")))
        .expect("static response builds")
}

async fn document(app: Arc<App>, req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    // The server-rendered document is a *view*, so it is behind the same question the socket is:
    // rendering a page for whoever asked would leak exactly what a per-session view exists to keep
    // separate. Under `DevIdentity` a claim of `dev` is what an unauthenticated laptop gets, and
    // that default lives in one place.
    let claimed = claimed_actor(req.headers(), req.uri().query());
    let claimed = if claimed.is_empty() && !app.identity().verifies() {
        "dev".to_string()
    } else {
        claimed
    };
    let actor = match app.identity().verify(&claimed) {
        Ok(a) => a,
        Err(why) => {
            tracing::warn!(
                reason = why.reason(),
                "identity refused for a document request"
            );
            crate::telemetry::telemetry().unauthenticated.incr();
            // A provider that can run a login flow sends the browser to it rather than answering
            // 401: a person who is not signed in has somewhere to go, and a person whose token has
            // expired has the same somewhere.
            if app.identity().login().is_some() {
                return Ok(redirect(LOGIN_PATH, &[set_cookie(SESSION_COOKIE, "", 0)]));
            }
            return Ok(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Full::new(Bytes::from_static(b"unauthenticated")))?);
        }
    };
    let seq = app.head();
    // The route this document is *of*. First paint is a render of the page at this path, so a deep
    // link and a reload produce the page the client would have navigated to rather than the root's
    // page followed by a correction — which is the whole difference between a router and a
    // redirect.
    let who = crate::program::At {
        who: actor,
        path: std::sync::Arc::from(req.uri().path()),
    };
    let body = app.render(&who).await?.render();
    let actor = &who.who;
    // The claims go into the document because Mode B's client renders the same view against the
    // same `Session`, and it has no provider to ask: a client left to fill in a blank map would
    // show a different page than the one it is hydrating. They are what the server already
    // verified and already rendered against, so the document is not telling the browser anything
    // the page it carries does not — and the browser's copy is advice, exactly as its `validate`
    // is: the server verifies the token again on the socket and every command goes through the
    // chokepoint there (§3.5).
    let claims = serde_json::to_string(
        &actor
            .claims()
            .iter()
            .map(|(k, v)| (k.as_ref(), v.as_ref()))
            .collect::<std::collections::BTreeMap<&str, &str>>(),
    )
    .unwrap_or_else(|_| "{}".into());
    let actor = beck_core::html::escape_attr(actor.name());
    let claims = beck_core::html::escape_attr(&claims);

    let html = shell(
        &app.runtime().placed().program.name,
        seq,
        &actor,
        &claims,
        &body,
        // Which residue this page needs is the component's rendering mode, and the server is the
        // one that knows it: a Mode B document that loaded the thin client would sit waiting for
        // DOM patches the server is never going to send.
        match app.runtime().placed().render.mode {
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

/// The document around a rendered page: what the browser needs in order to become a client of it.
///
/// A function rather than a `format!` inside [`document`] so that the attributes it writes can be
/// held against the attributes the served JavaScript reads, in both directions
/// (`the_document_carries_every_attribute_the_residue_reads_off_it`). An attribute a client reads
/// and this does not write is a page that never connects, and nothing else in this workspace would
/// say so.
///
/// `seq` is the position the page reflects, so the first socket message either finds nothing to do
/// or is exactly the gap — that is what made hydration free in Phase 0. `actor` and `claims` are
/// already escaped: they are the identity provider's strings, and the caller is where the value to
/// escape exists.
///
/// `#b-root` is the subscription's frame: a patch path is child indices *from it*, so it has to be
/// an element of its own rather than the body — whose other children are the two script tags below,
/// which an insertion at the frame's root would otherwise be counted against.
fn shell(title: &str, seq: u64, actor: &str, claims: &str, body: &str, client: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><link rel=\"stylesheet\" href=\"/beck.css\">\
         </head><body>\
         <div id=\"b-root\" data-b-seq=\"{seq}\" data-b-actor=\"{actor}\" \
         data-b-claims=\"{claims}\">{body}</div>\
         <script src=\"/beck-patch.js\" defer></script>\
         <script src=\"{client}\" defer></script></body></html>"
    )
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

    // A browser's credential is in a cookie, which the `hello` frame cannot see: the document may
    // not contain the token, because a script that reads the document reads the token. So the
    // *upgrade* is where a cookie-carrying connection is identified, and the frame's `actor` is
    // then not consulted at all (`crate::session::run_as`).
    //
    // A connection with no cookie is not refused here — `beck test`, a script and the corpus
    // harnesses all connect without one — it is passed on as `None` and `session::run_as` asks the
    // provider about the `hello` frame's claim exactly as before.
    let verified = match cookie(req.headers(), SESSION_COOKIE) {
        Some(claim) => match app.identity().verify(&claim) {
            Ok(actor) => Some(actor),
            Err(why) => {
                tracing::warn!(reason = why.reason(), "identity refused at the upgrade");
                crate::telemetry::telemetry().unauthenticated.incr();
                return Ok(Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(hyper::header::SET_COOKIE, set_cookie(SESSION_COOKIE, "", 0))
                    .body(Full::new(Bytes::from_static(b"unauthenticated")))?);
            }
        },
        None => None,
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
                if let Err(e) = crate::session::run_as(app, socket, verified).await {
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
    /// because no test in this workspace runs JavaScript (`docs/94` §94.15).
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

    /// The other direction, which is the one that bites: **every attribute a client reads is one
    /// the document writes.**
    ///
    /// The test above says something reads each attribute somebody thought to list; it cannot fail
    /// when a client starts reading an attribute the server never writes. That failure is silent —
    /// `dataset.bClaims` on a document without it is `undefined`, so the Mode B kernel would build
    /// a `Session` with no claims and refuse commands the server accepts — and it is exactly the
    /// shape of the defect Mode B's own `#b-root` finding was (`docs/94` §94.15).
    ///
    /// So the list is derived from the residue rather than written down: whatever `dataset.bFoo`
    /// the shipped JavaScript reads, `data-b-foo` has to be in the shell.
    #[test]
    fn the_document_carries_every_attribute_the_residue_reads_off_it() {
        let page = super::shell("t", 7, "ana", "{}", "<p>hi</p>", "/beck-thin.js");
        let mut checked = 0;
        for client in [crate::THIN_CLIENT, crate::MODE_B_CLIENT] {
            for (i, _) in client.match_indices("dataset.b") {
                let name: String = client[i + "dataset.".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                // `bActor` is `data-b-actor`; a second capital would be a second dash.
                let mut attr = String::from("data-");
                for c in name.chars() {
                    if c.is_ascii_uppercase() {
                        attr.push('-');
                        attr.push(c.to_ascii_lowercase());
                    } else {
                        attr.push(c);
                    }
                }
                assert!(
                    page.contains(&format!("{attr}=")),
                    "the residue reads `dataset.{name}` and the document has no `{attr}`"
                );
                checked += 1;
            }
        }
        // A rename that made the loop match nothing would otherwise pass in silence.
        assert!(checked >= 3, "only {checked} attribute reads were found");
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
