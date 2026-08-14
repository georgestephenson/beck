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
//!
//! # A reset is attempted again; a refusal is not
//!
//! An image resolves to a dozen-odd packages and each one is its own connection, so a build makes
//! more handshakes with one host than anything else in this workspace does. A public repository
//! resets some of them — the failure that motivated this read `the TLS handshake with
//! packages.wolfi.dev failed: Connection reset by peer`, on the eleventh package of a build whose
//! first ten had arrived. One reset ended a build that had already fetched everything else.
//!
//! So a failure is classified rather than reported: a transport that went away, or a repository
//! that answered 429 or 5xx, is attempted again ([`ATTEMPTS`] times, backing off from
//! [`BACKOFF`]); a 404, a certificate that does not verify, or a reply over the size cap is
//! answered once, because a second attempt would fail the same way and only delays the message.
//! The classification is the part worth testing — [`Failure`] is what the tests at the foot of
//! this file drive, with the attempt itself supplied by the test rather than by a network.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use http_body_util::BodyExt as _;

/// The largest reply this will read. A Wolfi package is single-digit megabytes and the index is
/// smaller; the cap is a runaway backstop, not a budget.
const MAX_BYTES: u64 = 128 * 1024 * 1024;

/// How many redirects to follow. Repositories sit behind CDNs that redirect once.
const MAX_REDIRECTS: usize = 5;

/// How many times one request is attempted before the build gives up. Four attempts spend at most
/// [`BACKOFF`] × 7 waiting, which is under four seconds — small beside the download it protects,
/// and short enough that a repository which is genuinely down still fails inside a build's timeout.
const ATTEMPTS: usize = 4;
const _: () = assert!(ATTEMPTS > 0, "a request is attempted at least once");

/// The wait before the second attempt, doubled for each attempt after it.
const BACKOFF: Duration = Duration::from_millis(500);

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

/// One HTTPS GET, following redirects, attempting each hop again if the transport went away.
async fn get(url: &str) -> Result<Vec<u8>> {
    get_with(url, BACKOFF, once).await
}

/// [`get`], with the attempt and the backoff supplied. Production passes [`once`] and [`BACKOFF`];
/// a test passes a closure and [`Duration::ZERO`], which is what makes the retry a tested property
/// rather than a hopeful one.
async fn get_with<F, Fut>(url: &str, backoff: Duration, attempt: F) -> Result<Vec<u8>>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Reply, Failure>>,
{
    let mut url = url.to_string();
    for _ in 0..MAX_REDIRECTS {
        match retrying(&url, backoff, &attempt).await? {
            Reply::Body(bytes) => return Ok(bytes),
            Reply::Redirect(to) => url = absolute(&url, &to)?,
        }
    }
    bail!("more than {MAX_REDIRECTS} redirects")
}

/// One hop, attempted up to [`ATTEMPTS`] times while the failure is one another attempt could
/// survive. The error that escapes is the last one, said plainly: a caller reading it should not
/// have to work out whether the four attempts were four different problems.
async fn retrying<F, Fut>(url: &str, backoff: Duration, attempt: &F) -> Result<Reply>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Reply, Failure>>,
{
    let mut wait = backoff;
    for remaining in (0..ATTEMPTS).rev() {
        match attempt(url.to_string()).await {
            Ok(reply) => return Ok(reply),
            Err(Failure::Permanent(err)) => return Err(err),
            Err(Failure::Transient(err)) if remaining == 0 => {
                return Err(err.context(format!("gave up after {ATTEMPTS} attempts")));
            }
            Err(Failure::Transient(_)) => {
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                wait *= 2;
            }
        }
    }
    unreachable!("ATTEMPTS is not zero, so the loop returns from its last iteration")
}

enum Reply {
    Body(Vec<u8>),
    Redirect(String),
}

/// Whether the same request is worth making again.
enum Failure {
    /// The transport went away, or the repository asked for a later attempt. A reset connection is
    /// this: nothing about the request was wrong, and the next one may well succeed.
    Transient(anyhow::Error),
    /// Another attempt would fail the same way — a package that is not there, a certificate that
    /// does not verify, a reply over the cap.
    Permanent(anyhow::Error),
}

impl Failure {
    /// An I/O failure on the way to or through the transport, classified by its kind. Everything
    /// here is worth another attempt except the two kinds that carry a rustls verdict:
    /// `InvalidData` is what a certificate that does not verify arrives as, and `InvalidInput` a
    /// request rustls refused to make. Retrying either would turn one honest error into four slow
    /// ones.
    fn from_io(err: std::io::Error, context: String) -> Failure {
        let permanent = matches!(
            err.kind(),
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
        );
        let err = anyhow::Error::new(err).context(context);
        if permanent {
            Failure::Permanent(err)
        } else {
            Failure::Transient(err)
        }
    }
}

/// Whether an HTTP status is one the repository is asking us to come back for. 408 and 425 are the
/// server saying the *request* did not land; 429 and every 5xx are it saying it cannot answer now.
/// Anything else — a 404 above all — is an answer, and an answer does not improve with repetition.
fn status_is_transient(status: hyper::StatusCode) -> bool {
    status.is_server_error()
        || matches!(
            status,
            hyper::StatusCode::REQUEST_TIMEOUT
                | hyper::StatusCode::TOO_EARLY
                | hyper::StatusCode::TOO_MANY_REQUESTS
        )
}

/// One attempt at one URL. Every failure it can produce is classified on the way out: this is the
/// function whose errors [`retrying`] reads.
async fn once(url: String) -> Result<Reply, Failure> {
    let (authority, path) = split(&url).map_err(Failure::Permanent)?;
    let host = authority.split(':').next().unwrap_or(authority);
    let port: u16 = authority
        .rsplit(':')
        .next()
        .filter(|p| *p != host)
        .and_then(|p| p.parse().ok())
        .unwrap_or(443);

    let tcp = tokio::net::TcpStream::connect((host, port))
        .await
        .map_err(|e| Failure::from_io(e, format!("connecting to {host}:{port}")))?;
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| Failure::Permanent(anyhow!("{host} is not a server name: {e}")))?;
    // The failure that motivated the retry arrives here, as a `ConnectionReset` from the peer.
    let stream = tokio_rustls::TlsConnector::from(client_config())
        .connect(name, tcp)
        .await
        .map_err(|e| Failure::from_io(e, format!("the TLS handshake with {host} failed")))?;

    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(|e| Failure::Transient(anyhow!("the HTTP handshake failed: {e}")))?;
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
        .map_err(|e| Failure::Permanent(anyhow!("building the request: {e}")))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| Failure::Transient(anyhow!("no reply: {e}")))?;
    let status = response.status();
    if status.is_redirection() {
        let location = response
            .headers()
            .get(hyper::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        pump.abort();
        return match location {
            Some(location) => Ok(Reply::Redirect(location)),
            None => Err(Failure::Permanent(anyhow!(
                "a {status} with no Location header"
            ))),
        };
    }
    if !status.is_success() {
        pump.abort();
        let err = anyhow!("the repository answered {status}");
        return Err(if status_is_transient(status) {
            Failure::Transient(err)
        } else {
            Failure::Permanent(err)
        });
    }
    let body = http_body_util::Limited::new(response.into_body(), MAX_BYTES as usize)
        .collect()
        .await;
    pump.abort();
    // A body that stops early and a body that runs past the cap arrive at the same place, and
    // saying the wrong one is not cosmetic: "longer than 128 MiB" for a connection that dropped
    // sends a reader looking for a package that grew, and would answer a reset with no retry.
    let body = body.map_err(|e| {
        if e.downcast_ref::<http_body_util::LengthLimitError>()
            .is_some()
        {
            Failure::Permanent(anyhow!("the reply is longer than {MAX_BYTES} bytes"))
        } else {
            Failure::Transient(anyhow!("the reply stopped early: {e}"))
        }
    })?;
    Ok(Reply::Body(body.to_bytes().to_vec()))
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

    /// Drive [`get_with`] over a script of replies, with no waiting and no network. Returns what
    /// the fetch answered and how many attempts it took to answer it.
    fn fetch(script: Vec<Result<Reply, Failure>>) -> (Result<Vec<u8>>, usize) {
        let script = std::cell::RefCell::new(std::collections::VecDeque::from(script));
        let attempts = std::cell::Cell::new(0);
        let out = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(get_with(
                "https://a.example/os/x86_64/tzdata-1.apk",
                Duration::ZERO,
                |_url| {
                    attempts.set(attempts.get() + 1);
                    let next = script
                        .borrow_mut()
                        .pop_front()
                        .unwrap_or_else(|| panic!("attempt {} was not scripted", attempts.get()));
                    std::future::ready(next)
                },
            ));
        (out, attempts.get())
    }

    fn reset() -> Result<Reply, Failure> {
        Err(Failure::from_io(
            std::io::Error::from(std::io::ErrorKind::ConnectionReset),
            "the TLS handshake with a.example failed".into(),
        ))
    }

    /// The CI failure this retry exists for: a handshake reset by the peer, part-way through a
    /// build whose earlier packages had arrived. Without the retry the first reset is the answer.
    #[test]
    fn a_handshake_the_peer_reset_is_attempted_again() {
        let (out, attempts) = fetch(vec![reset(), reset(), Ok(Reply::Body(b"apk".to_vec()))]);
        assert_eq!(out.expect("the third attempt succeeded"), b"apk");
        assert_eq!(attempts, 3);
    }

    /// The bound, and the message. Four resets are one failure reported once, not four stacked up.
    #[test]
    fn a_repository_that_stays_down_fails_after_a_bounded_number_of_attempts() {
        let (out, attempts) = fetch((0..ATTEMPTS).map(|_| reset()).collect());
        let err = format!("{:#}", out.expect_err("every attempt was reset"));
        assert_eq!(attempts, ATTEMPTS);
        assert!(err.contains("4 attempts"), "{err}");
        assert!(err.contains("the TLS handshake"), "{err}");
    }

    /// The other half of the classification, and the half that keeps the retry honest: a package
    /// that is not there is not there four times, and a certificate that does not verify is an
    /// answer rather than a hiccup. Retrying either would slow down every real error.
    #[test]
    fn an_answer_the_repository_meant_is_not_attempted_again() {
        for (what, failure) in [
            (
                "a 404",
                Failure::Permanent(anyhow!("the repository answered 404 Not Found")),
            ),
            (
                "a certificate that does not verify",
                Failure::from_io(
                    std::io::Error::from(std::io::ErrorKind::InvalidData),
                    "the TLS handshake with a.example failed".into(),
                ),
            ),
        ] {
            let (out, attempts) = fetch(vec![Err(failure)]);
            out.expect_err(what);
            assert_eq!(attempts, 1, "{what} was attempted again");
        }
    }

    /// A 429 or a 503 is the repository asking for a later attempt, and a build that gives up on
    /// one has not been refused anything.
    #[test]
    fn a_status_that_asks_for_a_later_attempt_gets_one() {
        for status in [
            hyper::StatusCode::TOO_MANY_REQUESTS,
            hyper::StatusCode::SERVICE_UNAVAILABLE,
            hyper::StatusCode::BAD_GATEWAY,
        ] {
            assert!(status_is_transient(status), "{status}");
        }
        for status in [
            hyper::StatusCode::NOT_FOUND,
            hyper::StatusCode::FORBIDDEN,
            hyper::StatusCode::UNAUTHORIZED,
        ] {
            assert!(!status_is_transient(status), "{status}");
        }
    }

    /// Attempts are counted per hop, so a redirect does not spend the budget the package it
    /// redirects to will need — and a reset on the second hop is retried as the first would be.
    #[test]
    fn each_hop_of_a_redirect_gets_its_own_attempts() {
        let (out, attempts) = fetch(vec![
            reset(),
            Ok(Reply::Redirect("https://cdn.example/tzdata-1.apk".into())),
            reset(),
            reset(),
            Ok(Reply::Body(b"apk".to_vec())),
        ]);
        assert_eq!(out.expect("the fetch followed the redirect"), b"apk");
        assert_eq!(attempts, 5);
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
