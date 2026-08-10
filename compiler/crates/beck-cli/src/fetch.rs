//! Getting the packages an image is built from.
//!
//! `beck image` assembles the image ([`beck_infra::oci`]); this is the one part of it that touches
//! a network, and it is deliberately the *only* one — everything downstream takes bytes, so the
//! build is a pure function and the suite can drive it from a directory.
//!
//! # Why the compiler does not reuse the program's outbound path
//!
//! [`beck_rt::outbound`](../../beck_rt/outbound/index.html) exists to answer a *running program's*
//! `net.out`: it is quota'd, capped at a body size a program should be allowed, and answerable to
//! an effect row. A compiler fetching a 40 MB package from a repository the user named in no
//! program is none of those things, and borrowing that path would mean either relaxing its limits
//! for everybody or lying to it about who is calling. It gets its own client over the same rustls
//! and the same Mozilla trust store.
//!
//! # The cache mirrors the repository
//!
//! A fetched file is written at `<cache>/<host>/<path>` and read back on the next build, so a
//! second `beck image` on a laptop performs no request at all — which is what makes `--offline` a
//! real mode rather than a failure message. The layout is the repository's own, so an air-gapped
//! build is `rsync` and `--offline` rather than a feature: a directory somebody can populate by
//! hand is a directory somebody can audit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use http_body_util::BodyExt as _;

/// The largest reply this will read. A Wolfi package is single-digit megabytes and the index is
/// smaller; the cap is a runaway backstop, not a budget.
const MAX_BYTES: u64 = 128 * 1024 * 1024;

/// How many redirects to follow. Repositories sit behind CDNs that redirect once.
const MAX_REDIRECTS: usize = 5;

/// A cache directory, and permission to use the network.
pub struct Fetcher {
    cache: PathBuf,
    offline: bool,
    runtime: tokio::runtime::Runtime,
}

impl Fetcher {
    pub fn new(cache: &Path, offline: bool) -> Result<Fetcher> {
        std::fs::create_dir_all(cache)
            .with_context(|| format!("creating the package cache {}", cache.display()))?;
        Ok(Fetcher {
            cache: cache.to_path_buf(),
            offline,
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("beck-fetch")
                .build()
                .context("starting the fetch runtime")?,
        })
    }

    /// Where a URL's bytes live once fetched: the repository's own layout, under the cache root.
    pub fn cached_at(&self, url: &str) -> PathBuf {
        let mut path = self.cache.clone();
        for part in url
            .trim_start_matches("https://")
            .split('/')
            .filter(|p| !p.is_empty())
        {
            // A path component from a URL is somebody else's string; `..` in one would write
            // outside the cache directory, and a repository that can do that can write anywhere.
            path.push(part.replace(['/', '\\'], "_").replace("..", "__"));
        }
        path
    }

    /// The bytes at a URL, from the cache if they are there.
    pub fn get(&self, url: &str) -> Result<Vec<u8>> {
        let path = self.cached_at(url);
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(bytes);
        }
        if self.offline {
            bail!(
                "--offline, and {url} is not in the cache ({}). Run once without it, or point \
                 --cache at a directory that has it",
                path.display()
            );
        }
        let bytes = self
            .runtime
            .block_on(get(url))
            .with_context(|| format!("fetching {url}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        Ok(bytes)
    }
}

/// One HTTPS GET, following redirects.
async fn get(url: &str) -> Result<Vec<u8>> {
    let mut url = url.to_string();
    for _ in 0..MAX_REDIRECTS {
        match once(&url).await? {
            Reply::Body(bytes) => return Ok(bytes),
            Reply::Redirect(to) => url = absolute(&url, &to)?,
        }
    }
    bail!("more than {MAX_REDIRECTS} redirects")
}

enum Reply {
    Body(Vec<u8>),
    Redirect(String),
}

async fn once(url: &str) -> Result<Reply> {
    let (authority, path) = split(url)?;
    let host = authority.split(':').next().unwrap_or(authority);
    let port: u16 = authority
        .rsplit(':')
        .next()
        .filter(|p| *p != host)
        .and_then(|p| p.parse().ok())
        .unwrap_or(443);

    let tcp = tokio::net::TcpStream::connect((host, port))
        .await
        .with_context(|| format!("connecting to {host}:{port}"))?;
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string())
        .with_context(|| format!("{host} is not a server name"))?;
    let stream = tokio_rustls::TlsConnector::from(client_config())
        .connect(name, tcp)
        .await
        .with_context(|| format!("the TLS handshake with {host} failed"))?;

    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .context("the HTTP handshake failed")?;
    let pump = tokio::spawn(async move {
        let _ = conn.await;
    });

    let request = hyper::Request::builder()
        .method("GET")
        .uri(path)
        .header(hyper::header::HOST, authority)
        .header(
            hyper::header::USER_AGENT,
            concat!("beck/", env!("CARGO_PKG_VERSION")),
        )
        .body(http_body_util::Empty::<hyper::body::Bytes>::new())
        .context("building the request")?;
    let response = sender.send_request(request).await.context("no reply")?;
    let status = response.status();
    if status.is_redirection() {
        let location = response
            .headers()
            .get(hyper::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("a {status} with no Location header"))?;
        pump.abort();
        return Ok(Reply::Redirect(location));
    }
    if !status.is_success() {
        pump.abort();
        bail!("the repository answered {status}");
    }
    let body = http_body_util::Limited::new(response.into_body(), MAX_BYTES as usize)
        .collect()
        .await
        .map_err(|_| anyhow!("the reply is longer than {MAX_BYTES} bytes"))?
        .to_bytes();
    pump.abort();
    Ok(Reply::Body(body.to_vec()))
}

/// Mozilla's trust store, and the provider named rather than defaulted — rustls picks one for you
/// only when exactly one is compiled in, and "exactly one" is a property of feature unification.
fn client_config() -> Arc<tokio_rustls::rustls::ClientConfig> {
    use std::sync::OnceLock;
    static CONFIG: OnceLock<Arc<tokio_rustls::rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = tokio_rustls::rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let mut config = tokio_rustls::rustls::ClientConfig::builder_with_provider(Arc::new(
                tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("the default protocol versions are supported by the provider")
            .with_root_certificates(roots)
            .with_no_client_auth();
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            Arc::new(config)
        })
        .clone()
}

/// `https://host/path` into `("host", "/path")`. Only `https`: a package repository reached over
/// plaintext is a package repository somebody else can rewrite.
fn split(url: &str) -> Result<(&str, &str)> {
    let rest = url
        .strip_prefix("https://")
        .with_context(|| format!("{url} is not an https URL"))?;
    Ok(match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    })
}

/// Resolve a `Location` against the URL it was returned for.
fn absolute(from: &str, location: &str) -> Result<String> {
    if location.starts_with("https://") {
        return Ok(location.to_string());
    }
    let (authority, _) = split(from)?;
    if let Some(path) = location.strip_prefix('/') {
        return Ok(format!("https://{authority}/{path}"));
    }
    let base = from.rsplit_once('/').map(|(b, _)| b).unwrap_or(from);
    Ok(format!("{base}/{location}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_splits_into_an_authority_and_a_path() {
        assert_eq!(
            split("https://packages.wolfi.dev/os/x86_64/tzdata-1.apk").expect("splits"),
            ("packages.wolfi.dev", "/os/x86_64/tzdata-1.apk")
        );
        assert_eq!(split("http://example.com/x").ok(), None);
    }

    #[test]
    fn a_relative_redirect_resolves_against_the_url_it_came_from() {
        let from = "https://packages.wolfi.dev/os/x86_64/tzdata-1.apk";
        assert_eq!(
            absolute(from, "/cdn/tzdata-1.apk").expect("absolute"),
            "https://packages.wolfi.dev/cdn/tzdata-1.apk"
        );
        assert_eq!(
            absolute(from, "https://cdn.example/tzdata-1.apk").expect("absolute"),
            "https://cdn.example/tzdata-1.apk"
        );
        assert_eq!(
            absolute(from, "other.apk").expect("absolute"),
            "https://packages.wolfi.dev/os/x86_64/other.apk"
        );
    }

    #[test]
    fn the_cache_mirrors_the_repository_and_stays_inside_itself() {
        let dir = std::env::temp_dir().join("beck-fetch-test");
        let f = Fetcher::new(&dir, true).expect("a fetcher");
        assert_eq!(
            f.cached_at("https://a.example/os/x86_64/tzdata-1.apk"),
            dir.join("a.example/os/x86_64/tzdata-1.apk")
        );
        assert_ne!(
            f.cached_at("https://a.example/os/x86_64/tzdata-1.apk"),
            f.cached_at("https://b.example/os/x86_64/tzdata-1.apk"),
            "two repositories must not share a cache entry"
        );
        // A repository that can put `..` in a path is a repository that can write anywhere.
        let escaped = f.cached_at("https://a.example/../../etc/passwd");
        assert!(escaped.starts_with(&dir), "{}", escaped.display());
    }

    #[test]
    fn offline_says_what_is_missing_rather_than_reaching_the_network() {
        let dir = std::env::temp_dir().join("beck-fetch-test-offline");
        let f = Fetcher::new(&dir, true).expect("a fetcher");
        let err = f
            .get("https://packages.wolfi.dev/os/x86_64/APKINDEX.tar.gz")
            .expect_err("offline");
        assert!(err.to_string().contains("--offline"), "{err}");
    }
}
