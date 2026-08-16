//! Outbound HTTP, as a thing that is supplied rather than a thing that is ambient.
//!
//! [`docs/14-review-findings.md`](../../../../../docs/14-review-findings.md) F11 names three
//! resources that cannot be retrofitted — clock, **network** and disk.
//! [`crate::clock`] is the first of them. This is the second, and it arrives with the feature that
//! first needs it rather than after: a program's outbound call goes through this trait, so a
//! simulator, a recorder and a refusal are all the same shape.
//!
//! # What is on the seam
//!
//! One request/response exchange, which is what the `http_fetch` primitive is. Not connection
//! reuse, not a pool, not a redirect policy, not retries — those are decisions of an
//! implementation, and an implementation is what this trait is for.
//!
//! # Transport security is a field, not a mode
//!
//! [`Request::tls`] says whether the exchange is over TLS, and it is a field of the request rather
//! than a property of the client, because a program that calls two peers may reach one of them
//! over a plaintext hop inside a cluster and the other across the internet. What a *name* is
//! verified against is the implementation's business; that the caller asked for TLS is the
//! program's ([`docs/adr/0023`](../../../../../docs/adr/0023-tls-and-the-signature-it-brings.md)).

use std::fmt;
use std::sync::Arc;
use std::sync::OnceLock;

/// One outbound request.
///
/// The **host is not part of the body of the program**: it is the argument of the `net.out(host)`
/// atom the call site performs, which is what becomes a NetworkPolicy peer (§6.5). Everything else
/// here is data a program computed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub host: Arc<str>,
    pub port: u16,
    /// Whether the exchange happens inside a TLS session whose certificate names [`Request::host`].
    ///
    /// A field rather than a scheme in the path, because the host is already the atom the call
    /// site performs and a URL would give a program a second place to write it.
    pub tls: bool,
    pub method: Arc<str>,
    /// Origin-form: `/v1/todos?limit=10`. Sent as written — this seam does not encode, because a
    /// program that built a path is the only thing that knows what in it was data.
    pub path: Arc<str>,
    pub headers: Vec<(Arc<str>, Arc<str>)>,
    pub body: Arc<str>,
}

/// What came back. A status is a *reply*, not a failure — including a 500.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: i64,
    pub headers: Vec<(Arc<str>, Arc<str>)>,
    pub body: Arc<str>,
}

/// Why no reply came back.
///
/// Three of these are the peer's doing and one is the caller's, which is the distinction worth
/// keeping: a program that asked and did not get an answer has a failure to handle, and a program
/// whose *scope* stopped wanting the answer has nothing to handle at all. A fifth that said
/// "something went wrong" would be a `Str` with extra steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Connect or write failed, or nothing is listening.
    Unreachable(String),
    /// The exchange did not finish inside the deadline the implementation was given.
    TimedOut(i64),
    /// Bytes arrived and were not an HTTP response.
    BadResponse(String),
    /// The caller stopped wanting the reply — [`Stop::asked`] became true while the exchange was
    /// in flight.
    ///
    /// Not a failure of the request, and a caller that installed a [`Stop`] is expected to notice
    /// this before it renders anything: `beck-eval` turns it back into the cancellation it came
    /// from. `crate::host::failure_value` renders it as `HttpUnreachable` if one ever reaches a
    /// program, because `HttpError` is a *published* union and a fourth variant would be a wire
    /// change made for a case no program can observe.
    Stopped,
}

/// Whether the caller still wants the reply.
///
/// [`docs/80`](../../../../../docs/80-structured-concurrency-report.md) §80.12 named the gap this
/// closes: cancellation rides the evaluator's step counter, so a `parallel:` child blocked *inside*
/// an outbound call was stopped only when the call came back — a scope whose first child failed
/// still waited out a sibling's ten-second timeout. §80.12 also says where the fix belongs: "a
/// **deadline on the [`net`](crate::net) seam** rather than a change to the scope", because the
/// scope is already right about *when* to stop a child and the thing it cannot reach is a socket.
///
/// A predicate rather than a token or a channel, for the reason the rest of this seam is what it
/// is: the caller already knows the answer — `beck-eval` has the chain of enclosing scopes and
/// their first-failed indices — and anything richer would be a second copy of that state, kept in
/// step by hand.
#[derive(Clone)]
pub struct Stop(Option<Arc<dyn Fn() -> bool + Send + Sync>>);

impl Stop {
    /// Nobody is going to stop this call, so an implementation need not watch for it.
    ///
    /// [`Stop::watched`] is false for this one, which is what lets a client take the path with no
    /// timer in it — the ordinary case, since only a child of a `parallel:` can be cancelled.
    pub fn never() -> Stop {
        Stop(None)
    }

    pub fn when(f: impl Fn() -> bool + Send + Sync + 'static) -> Stop {
        Stop(Some(Arc::new(f)))
    }

    /// Whether the caller has stopped wanting the reply. Called repeatedly, so it is cheap.
    pub fn asked(&self) -> bool {
        self.0.as_ref().is_some_and(|f| f())
    }

    /// Whether anybody could ever ask.
    pub fn watched(&self) -> bool {
        self.0.is_some()
    }
}

impl fmt::Debug for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.watched() {
            "Stop(watched)"
        } else {
            "Stop(never)"
        })
    }
}

impl Default for Stop {
    fn default() -> Stop {
        Stop::never()
    }
}

/// Somewhere for an outbound request to go.
pub trait Outbound: Send + Sync + fmt::Debug {
    /// Make the exchange, watching `stop` while it is in flight.
    ///
    /// `stop` is a parameter rather than a default-implemented second method on purpose: an
    /// implementation that ignores it is one a `parallel:` cannot cancel, and a seam whose
    /// implementations can opt out of a property by not mentioning it is
    /// [`docs/82`](../../../../../docs/82-the-edge-report.md) §82.10's gate that cannot fail. Every
    /// implementation has to say what it does about this, even if what it does is nothing because
    /// it never blocks.
    fn fetch(&self, request: &Request, stop: &Stop) -> Result<Reply, Failure>;
}

/// The default: every request fails, and says why in a sentence a program can print.
///
/// A process that has not installed a client has not *decided* to make outbound calls, and
/// `beck test` is the ordinary case — a `net.out` atom is auto-stubbed there (§21.3), so a test
/// that reaches this has one the harness could not stub.
#[derive(Clone, Copy, Debug, Default)]
pub struct Refusing;

impl Outbound for Refusing {
    /// Nothing to watch: this answers before it returns.
    fn fetch(&self, request: &Request, _stop: &Stop) -> Result<Reply, Failure> {
        Err(Failure::Unreachable(format!(
            "no outbound HTTP client is installed in this process, so `{}` was not called",
            request.host
        )))
    }
}

/// A canned client: replies decided in advance, requests kept.
///
/// The seam's second implementation, and a seam with one implementation is an abstraction nobody
/// has checked. It is also what a Rust-level test uses when it wants to assert what a program
/// *sent*, which a stub in Beck cannot see.
#[derive(Debug, Default)]
pub struct Canned {
    replies: std::sync::Mutex<Vec<Result<Reply, Failure>>>,
    sent: std::sync::Mutex<Vec<Request>>,
}

impl Canned {
    /// Replies are handed out in order; a request past the end gets [`Failure::Unreachable`].
    pub fn new(replies: Vec<Result<Reply, Failure>>) -> Canned {
        Canned {
            replies: std::sync::Mutex::new(replies.into_iter().rev().collect()),
            sent: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// One 200 with this body, once.
    pub fn ok(body: &str) -> Canned {
        Canned::new(vec![Ok(Reply {
            status: 200,
            headers: vec![(Arc::from("content-type"), Arc::from("application/json"))],
            body: Arc::from(body),
        })])
    }

    pub fn sent(&self) -> Vec<Request> {
        self.sent.lock().expect("not poisoned").clone()
    }
}

impl Outbound for Canned {
    /// Nothing to watch: a canned reply is already in hand.
    fn fetch(&self, request: &Request, _stop: &Stop) -> Result<Reply, Failure> {
        self.sent
            .lock()
            .expect("not poisoned")
            .push(request.clone());
        self.replies
            .lock()
            .expect("not poisoned")
            .pop()
            .unwrap_or_else(|| {
                Err(Failure::Unreachable(
                    "this canned client has no reply left".into(),
                ))
            })
    }
}

static PROCESS: OnceLock<Arc<dyn Outbound>> = OnceLock::new();

/// The process's outbound client — read by the evaluator, which has no other way to reach one.
pub fn process_outbound() -> &'static Arc<dyn Outbound> {
    PROCESS.get_or_init(|| Arc::new(Refusing))
}

/// Install it. Returns `false` if one has already been read or installed.
///
/// Once, at startup, before anything reads it — the same discipline and the same reason as
/// [`crate::clock::set_process_clock`]: a test binary running two tests in one process must not
/// abort on the second.
pub fn set_process_outbound(client: Arc<dyn Outbound>) -> bool {
    PROCESS.set(client).is_ok()
}

/// Is this a host the `net.out(host)` atom can name?
///
/// The atom is written in a `uses` clause as bare tokens — `net.out(payments.example.com)` — so a
/// host a program *calls* has to be one a program can *declare*. Dotted ASCII labels, which is
/// what a DNS name is and what a NetworkPolicy peer is written as. No port: a port is transport,
/// and the policy peer is the name.
pub fn is_nameable_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_client_refuses_and_names_the_host() {
        let r = Request {
            host: Arc::from("api.example.com"),
            port: 80,
            tls: false,
            method: Arc::from("GET"),
            path: Arc::from("/"),
            headers: Vec::new(),
            body: Arc::from(""),
        };
        match Refusing.fetch(&r, &Stop::never()) {
            Err(Failure::Unreachable(why)) => assert!(why.contains("api.example.com"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_canned_client_hands_out_its_replies_in_order_and_keeps_what_was_sent() {
        let c = Canned::new(vec![
            Ok(Reply {
                status: 204,
                headers: Vec::new(),
                body: Arc::from(""),
            }),
            Err(Failure::TimedOut(1_000)),
        ]);
        let req = |path: &str| Request {
            host: Arc::from("h.example.com"),
            port: 80,
            tls: false,
            method: Arc::from("GET"),
            path: Arc::from(path),
            headers: Vec::new(),
            body: Arc::from(""),
        };
        assert_eq!(
            c.fetch(&req("/a"), &Stop::never()).map(|r| r.status),
            Ok(204)
        );
        assert_eq!(
            c.fetch(&req("/b"), &Stop::never()),
            Err(Failure::TimedOut(1_000))
        );
        assert!(matches!(
            c.fetch(&req("/c"), &Stop::never()),
            Err(Failure::Unreachable(_))
        ));
        let sent: Vec<Arc<str>> = c.sent().into_iter().map(|r| r.path).collect();
        assert_eq!(
            sent,
            vec![Arc::from("/a"), Arc::from("/b"), Arc::from("/c")]
        );
    }

    #[test]
    fn a_host_is_nameable_when_a_uses_clause_could_have_written_it() {
        assert!(is_nameable_host("payments.example.com"));
        assert!(is_nameable_host("localhost"));
        assert!(is_nameable_host("api-2.example.com"));
        // A port is transport, and the policy peer is the name.
        assert!(!is_nameable_host("api.example.com:8080"));
        assert!(!is_nameable_host("api..example.com"));
        assert!(!is_nameable_host("-api.example.com"));
        assert!(!is_nameable_host("https://api.example.com"));
        assert!(!is_nameable_host(""));
        assert!(!is_nameable_host("héllo.example.com"));
    }
}
