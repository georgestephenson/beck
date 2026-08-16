//! What a *host* answers, as one description rather than one per backend.
//!
//! # Why this is here and not in a backend
//!
//! Four of Beck's primitives cannot be computed. `uuid()` and `now()` are `nondet`, `secret_env`
//! is `env`, and `http_fetch` is `net.out(host)` — each of them is a question whose answer is
//! outside the program, which is exactly what an effect atom *is* (§3.2). A backend does not know
//! the answers; it knows how to ask.
//!
//! The tree-walker asked by calling four methods on `beck_eval::interp::Host`, which is a trait in
//! the evaluator's crate. A second backend that reached the same four answers a second way would
//! be two descriptions of one thing, and the differential between the backends would be comparing
//! them rather than comparing the *program*. So the four live here, on [`Atoms`], and the
//! evaluator's `Host` extends it.
//!
//! # The defaults are the seams
//!
//! Every method has a default, and every default goes through the process seam
//! [`docs/14-review-findings.md`](../../../../../docs/14-review-findings.md) F11 asks for:
//! [`crate::clock`] for the wall clock, [`crate::net`] for the outbound call, the process
//! environment for a secret. A host that wants to answer differently overrides one method; a host
//! that does not still never names a clock or a network stack.
//!
//! # A request is a value, and that conversion is here too
//!
//! `http_fetch` takes a value the program built and answers with a value the program reads, and
//! the translation between those and [`crate::net`]'s `Request`/`Reply` is neither the evaluator's
//! business nor a compiler's. [`request_of`], [`reply_value`] and [`failure_value`] are that
//! translation, in one place, so that two backends making the same call cannot send two different
//! requests.

use std::sync::Arc;

use crate::core::{Fields, Value};
use crate::net::{Failure, Reply, Request, Stop};
use crate::pmap::PMap;

/// The impure capabilities a host supplies, one method per effect atom.
///
/// `Send + Sync` because a compiled backend hands one to a worker that the runtime calls from a
/// sequencer task and a connection task alike, and a host that cannot survive that is not a host
/// for this runtime.
pub trait Atoms: Send + Sync {
    /// Mint an id — `nondet`. Called only where the checker has proved we are not inside a fold.
    fn new_uuid(&self) -> Arc<str> {
        Arc::from(uuid_v7())
    }

    /// Read the wall clock — `nondet`, and forbidden inside a fold for the same reason `uuid()`
    /// is: time is data on the envelope (§3.7).
    fn now_millis(&self) -> i64 {
        crate::clock::process_clock().now_millis()
    }

    /// Read a secret from the process environment — `env`, which no client tier discharges.
    fn secret(&self, name: &str) -> Arc<str> {
        std::env::var(name).unwrap_or_default().into()
    }

    /// Make an outbound request — the runtime half of `net.out(host)`.
    ///
    /// `stop` is how a `parallel:` reaches a child that is blocked in the socket rather than in
    /// the evaluator ([`crate::net::Stop`]). A host that answers without blocking ignores it; one
    /// that talks to a peer watches it, and a caller that cannot be cancelled passes
    /// [`crate::net::Stop::never`].
    fn fetch(&self, request: &Request, stop: &Stop) -> Result<Reply, Failure> {
        crate::net::process_outbound().fetch(request, stop)
    }
}

/// The host every default answers for: the process this is running in.
///
/// A named type rather than an anonymous one because a backend has to be able to say what it was
/// given when nobody gave it anything, and "`ProcessAtoms`" is an answer where a `Box<dyn Atoms>`
/// built out of nothing is not.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessAtoms;

impl Atoms for ProcessAtoms {}

/// A time-ordered id, without pulling a uuid crate into the tree for one call.
///
/// UUIDv7 layout: 48 bits of Unix milliseconds, then version and variant bits, then randomness.
/// The randomness comes from the system, via `getrandom` through the standard library's hash seed
/// — good enough for an id at the edge, and never reached inside a fold, which is where
/// determinism actually matters (§3.7).
pub fn uuid_v7() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let ms = (crate::clock::process_clock().now_millis().max(0) as u64) & 0x0000_FFFF_FFFF_FFFF;
    let rand = || RandomState::new().build_hasher().finish();
    let (a, b) = (rand(), rand());

    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    bytes[6..12].copy_from_slice(&a.to_be_bytes()[..6]);
    bytes[12..].copy_from_slice(&b.to_be_bytes()[..4]);
    bytes[6] = (bytes[6] & 0x0F) | 0x70; // version 7
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // variant 10

    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// An `HttpRequest` the program built, as the request the seam sends.
///
/// The host is not read out of the value: it is the argument of the `net.out(host)` atom the call
/// site performs, so it arrives separately and cannot be computed (§6.5).
pub fn request_of(host: &str, v: &Value) -> Result<Request, String> {
    let field = |name: &str| v.field(name).cloned().unwrap_or(Value::Unit);
    let text = |name: &str| -> Arc<str> {
        match field(name) {
            Value::Str(s) => Arc::from(s.as_str()),
            _ => Arc::from(""),
        }
    };
    let port = match field("port") {
        Value::Int(p) if (1..=65_535).contains(&p) => p as u16,
        Value::Int(0) | Value::Unit => 80,
        Value::Int(p) => return Err(format!("`{p}` is not a port an outbound call can use")),
        _ => 80,
    };
    let mut headers: Vec<(Arc<str>, Arc<str>)> = match field("headers") {
        Value::Map(m) => m
            .iter()
            .filter_map(|(k, val)| match (k, val) {
                (Value::Str(k), Value::Str(v)) => {
                    Some((Arc::from(k.as_str()), Arc::from(v.as_str())))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    // The secret half, unwrapped here and nowhere else. §3.5 gives a program no way to read a
    // `secret[Str]`; this is the edge, past every tier the checker places, so the credential
    // becomes bytes exactly where it becomes a request and never becomes a value the program
    // could have put somewhere else.
    if let Value::Map(m) = field("secrets") {
        for (k, val) in m.iter() {
            if let (Value::Str(name), Some(Value::Str(secret))) = (k, val.field("value")) {
                headers.push((Arc::from(name.as_str()), Arc::from(secret.as_str())));
            }
        }
    }
    let method = text("method");
    Ok(Request {
        host: Arc::from(host),
        port,
        // Plaintext unless the program said otherwise, which is the same default `lib/http.beck`
        // writes: `over_tls` is a call somebody makes, so a request that crosses the internet
        // without one is a thing in the source rather than a thing in the runtime.
        tls: field("tls").as_bool().unwrap_or(false),
        method: if method.is_empty() {
            Arc::from("GET")
        } else {
            method
        },
        path: {
            let p = text("path");
            if p.is_empty() {
                Arc::from("/")
            } else {
                p
            }
        },
        headers,
        body: text("body"),
    })
}

/// What came back, as the `HttpResponse` the program reads.
pub fn reply_value(reply: &Reply) -> Value {
    let headers = reply.headers.iter().fold(PMap::new(), |m, (k, v)| {
        m.insert(Value::str_(k), Value::str_(v))
    });
    Value::data(
        Arc::from("HttpResponse"),
        None,
        Fields::from_iter([
            (Arc::from("status"), Value::Int(reply.status)),
            (Arc::from("headers"), Value::Map(headers)),
            (Arc::from("body"), Value::str_(&reply.body)),
        ]),
    )
}

/// The seam's [`Failure`] as the `HttpError` the call raises.
///
/// The host is put back in here rather than carried through the failure, because the seam's
/// implementation was told which host it was calling and there is no case where the two differ.
pub fn failure_value(host: &str, f: &Failure) -> Value {
    let (variant, fields): (&str, Vec<(Arc<str>, Value)>) = match f {
        Failure::Unreachable(why) => (
            "HttpUnreachable",
            vec![
                (Arc::from("host"), Value::str_(host)),
                (Arc::from("why"), Value::str_(why)),
            ],
        ),
        Failure::TimedOut(ms) => (
            "HttpTimedOut",
            vec![
                (Arc::from("host"), Value::str_(host)),
                (Arc::from("millis"), Value::Int(*ms)),
            ],
        ),
        Failure::BadResponse(why) => (
            "HttpBadResponse",
            vec![(Arc::from("why"), Value::str_(why))],
        ),
        // The seam's fourth case, rendered as the third. `HttpError` is a published union and a
        // program cannot observe this one — `beck-eval` turns a stopped fetch back into the
        // cancellation it came from — so a fourth variant would be a wire change bought for
        // nothing (`beck check --wire-compat`).
        Failure::Stopped => (
            "HttpUnreachable",
            vec![
                (Arc::from("host"), Value::str_(host)),
                (
                    Arc::from("why"),
                    Value::str_("the caller stopped waiting for this reply"),
                ),
            ],
        ),
    };
    Value::data(
        Arc::from("HttpError"),
        Some(Arc::from(variant)),
        fields.into_iter().collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_ids_are_v7_shaped_and_distinct() {
        let a = uuid_v7();
        let b = uuid_v7();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'7', "version nibble: {a}");
        assert!(
            matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "variant: {a}"
        );
    }

    /// The port rule is the one place this conversion can refuse, and it refuses by naming the
    /// number rather than by defaulting quietly.
    #[test]
    fn a_port_no_call_can_use_is_refused_by_number() {
        let v = Value::data(
            Arc::from("HttpRequest"),
            None,
            Fields::from_iter([(Arc::from("port"), Value::Int(70_000))]),
        );
        let why = request_of("example.com", &v).expect_err("70000 is not a port");
        assert!(why.contains("70000"), "{why}");
    }

    #[test]
    fn an_absent_field_takes_the_default_the_library_writes() {
        let v = Value::data(Arc::from("HttpRequest"), None, Fields::default());
        let r = request_of("example.com", &v).expect("defaults");
        assert_eq!(&*r.method, "GET");
        assert_eq!(&*r.path, "/");
        assert_eq!(r.port, 80);
        assert!(!r.tls);
    }
}
