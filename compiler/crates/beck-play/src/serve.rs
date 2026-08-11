//! The other half of the playground: getting it into a browser.
//!
//! Rung A "costs a CDN" ([`docs/17`](../../../../../docs/17-playground.md) §17.1) and that is the
//! literal truth — [`bundle`] is a list of files, and a directory of them on any static host is a
//! working playground. This module exists because two things need it before that is useful: a
//! browser cannot instantiate WebAssembly it fetched over `file://`, and the person developing the
//! playground wants one command. `beck play` is that command.
//!
//! Nothing here is compiled into the module. It is behind `cfg(not(target_arch = "wasm32"))`, so
//! the tab carries no HTTP server, and the crate's two halves cannot quietly become one.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Result;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{CACHE_CONTROL, CONTENT_TYPE};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

/// One file of the playground, and what a browser should be told it is.
pub struct Asset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub body: &'static str,
}

/// Every file the playground is, except the module.
///
/// The three client files are **`beck-rt`'s**, not copies: `beck-patch.js` and `beck-thin.js` are
/// the residue a deployed Beck application serves, and a playground that shipped its own copy
/// would be demonstrating a second client. That is the entire reason this list reaches into
/// another crate's constants.
pub fn bundle() -> Vec<Asset> {
    vec![
        Asset {
            path: "index.html",
            content_type: "text/html; charset=utf-8",
            body: include_str!("../web/index.html"),
        },
        Asset {
            path: "playground.css",
            content_type: "text/css",
            body: include_str!("../web/playground.css"),
        },
        Asset {
            path: "client.css",
            content_type: "text/css",
            body: include_str!("../web/client.css"),
        },
        Asset {
            path: "playground.js",
            content_type: "text/javascript",
            body: include_str!("../web/playground.js"),
        },
        Asset {
            path: "beck-play-worker.js",
            content_type: "text/javascript",
            body: include_str!("../web/beck-play-worker.js"),
        },
        Asset {
            path: "beck-play-port.js",
            content_type: "text/javascript",
            body: include_str!("../web/beck-play-port.js"),
        },
        Asset {
            path: "beck-patch.js",
            content_type: "text/javascript",
            body: beck_rt::PATCH_CLIENT,
        },
        Asset {
            path: "beck-thin.js",
            content_type: "text/javascript",
            body: beck_rt::THIN_CLIENT,
        },
        // Mode B's half of the same rule: a `@render(client)` program's iframe loads the kernel
        // shim a deployment serves, unmodified, and gets its kernel and its bundle through
        // `beck.asset` (docs/103).
        Asset {
            path: "beck-mode-b.js",
            content_type: "text/javascript",
            body: beck_rt::MODE_B_CLIENT,
        },
    ]
}

/// Where `cargo build -p beck-play --release --target wasm32-unknown-unknown` puts the module.
///
/// `BECK_PLAYGROUND` names one explicitly, exactly as `BECK_KERNEL` does for Mode B's kernel and
/// for the same reason: building it needs a target the compiler's own build does not, so `beck`
/// cannot depend on it having been built.
pub fn module_path() -> PathBuf {
    std::env::var_os("BECK_PLAYGROUND").map_or_else(
        || PathBuf::from("target/wasm32-unknown-unknown/release/beck_play.wasm"),
        PathBuf::from,
    )
}

/// The WebAssembly modules the playground is, beside the files it is.
///
/// Two, and the second one is why this is a list rather than a constant: the compiler runs in the
/// worker and Mode B's kernel runs in the client iframe, so a `@render(client)` program in the tab
/// needs both. Neither is compiled into the `beck` binary, because building either needs a target
/// the compiler's own build does not.
pub const MODULES: [(&str, &str); 2] = [
    ("beck-play.wasm", "beck-play"),
    ("beck-kernel.wasm", "beck-wasm"),
];

/// Where each module is looked for on this machine.
fn module_paths() -> [(&'static str, PathBuf, &'static str); 2] {
    [
        (MODULES[0].0, module_path(), "BECK_PLAYGROUND"),
        (MODULES[1].0, beck_rt::http::kernel_path(), "BECK_KERNEL"),
    ]
}

/// The module names [`write()`] writes, for a gate that asserts the page can ask for them.
pub fn written_modules() -> Vec<&'static str> {
    MODULES.iter().map(|(name, _)| *name).collect()
}

/// Write the playground to a directory: the static site §17.1 describes.
///
/// Both modules are copied in beside the rest, so what is written is the whole deployment and not a
/// deployment plus an instruction. The kernel is the *second* one: a `@render(client)` program runs
/// in the client iframe, and it runs there in Mode B's kernel — the same module `beck run` serves
/// on `/beck-kernel.wasm`.
pub fn write(out: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out)?;
    let mut written = Vec::new();
    for asset in bundle() {
        let path = out.join(asset.path);
        std::fs::write(&path, asset.body)?;
        written.push(path);
    }
    for (name, from, env) in module_paths() {
        let crate_name = MODULES
            .iter()
            .find(|(m, _)| *m == name)
            .map(|(_, c)| *c)
            .unwrap_or_default();
        let into = out.join(name);
        std::fs::copy(&from, &into).map_err(|e| {
            anyhow::anyhow!(
                "no module at {}: build it with `cargo build -p {crate_name} --release --target \
                 wasm32-unknown-unknown`, or set {env} ({e})",
                from.display()
            )
        })?;
        written.push(into);
    }
    Ok(written)
}

/// Serve the playground on `addr` until `shutdown` is set.
pub async fn serve(
    addr: SocketAddr,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "the playground is listening");
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return Ok(());
                }
                continue;
            }
        };
        let (stream, _peer) = accepted?;
        stream.set_nodelay(true)?;
        tokio::spawn(async move {
            let service = service_fn(|req| async { route(req) });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                tracing::debug!(error = %e, "connection closed");
            }
        });
    }
}

fn route(req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
    if req.method() != Method::GET {
        return Ok(refuse(StatusCode::METHOD_NOT_ALLOWED, "GET only"));
    }
    let path = req.uri().path();
    let path = if path == "/" { "/index.html" } else { path };
    let name = path.trim_start_matches('/');

    // The two modules, read from disk rather than compiled in: building either needs a target the
    // compiler's own build does not.
    if let Some((_, at, env)) = module_paths().into_iter().find(|(m, _, _)| *m == name) {
        return Ok(match std::fs::read(&at) {
            Ok(module) => body(Bytes::from(module), "application/wasm"),
            Err(e) => {
                tracing::error!(path = %at.display(), error = %e, "no module to serve");
                refuse(
                    StatusCode::NOT_FOUND,
                    &format!(
                        "no module at {}: build it with `cargo build --release --target \
                         wasm32-unknown-unknown`, or set {env}",
                        at.display()
                    ),
                )
            }
        });
    }

    match bundle().into_iter().find(|a| a.path == name) {
        Some(asset) => Ok(body(
            Bytes::from_static(asset.body.as_bytes()),
            asset.content_type,
        )),
        None => Ok(refuse(StatusCode::NOT_FOUND, "no such file")),
    }
}

fn body(bytes: Bytes, content_type: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .header(CONTENT_TYPE, content_type)
        // The module and the page change together and are developed by reloading, so nothing here
        // is cached by the browser. A CDN in front of a *released* playground caches by URL, which
        // is a different decision made by whoever deploys it.
        .header(CACHE_CONTROL, "no-cache")
        .body(Full::new(bytes))
        .expect("a static response builds")
}

fn refuse(status: StatusCode, why: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::from(why.to_string())))
        .expect("a static response builds")
}
