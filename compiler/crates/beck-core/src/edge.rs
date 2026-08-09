//! The three values a host hands a pure program at the edge.
//!
//! A Beck program never constructs an `Envelope`, a `Session` or a `Proposal` — it receives them.
//! §3.7's replay rule is the reason: "`env.at` and `env.actor` are read as *data* and never from a
//! clock", which is only true if something outside the program supplies them.
//!
//! They are built here rather than in the runtime because a Mode B client builds them too. A
//! browser that applies a command speculatively folds it under an envelope of its own, and an
//! envelope with a differently-spelled field is a fold that fails in the browser and succeeds on
//! the server — the exact class of divergence Mode B has to be free of, since its whole claim is
//! that the client runs *the same* fold ([`crate::render`]).

use std::sync::Arc;

use crate::core::{Fields, Value};

/// `Envelope[Event]` — the record a fold sees.
pub fn envelope(seq: u64, at: i64, actor: &str, event: Value) -> Value {
    Value::data(
        Arc::from("Envelope"),
        None,
        Fields::from_iter([
            (Arc::from("seq"), Value::Int(seq as i64)),
            (Arc::from("at"), Value::Int(at)),
            (Arc::from("actor"), Value::str_(actor)),
            (Arc::from("body"), event),
        ]),
    )
}

/// `Session` — who is asking, what the identity provider said about them, and where they are.
///
/// The claims are a `Map[Str, Str]` and not a record, because the set is the provider's rather
/// than the program's: a tenant claim one deployment issues is one another has never heard of.
/// They are copied in at the edge for the same reason the actor is — a fold that read them from a
/// token would be a fold that could not replay ([`crate::render`], §3.7).
///
/// `path` is the route, and it is built here for the same reason the other two are: a Mode B
/// client renders the same view against a `Session` of its own, and a route the browser spelled
/// differently than the server is a page that differs from the one it is hydrating.
pub fn session<'a>(
    actor: &str,
    claims: impl IntoIterator<Item = (&'a str, &'a str)>,
    path: &str,
) -> Value {
    Value::data(
        Arc::from("Session"),
        None,
        Fields::from_iter([
            (Arc::from("actor"), Value::str_(actor)),
            (
                Arc::from("claims"),
                Value::Map(
                    claims
                        .into_iter()
                        .map(|(k, v)| (Value::str_(k), Value::str_(v)))
                        .collect(),
                ),
            ),
            (Arc::from("path"), Value::str_(path)),
        ]),
    )
}

/// The route a connection has not stated: the application's own root.
///
/// One constant rather than an empty string, because "the client did not say" and "the client is
/// at the root" are the same page and a program matching on `session.path` should not have to
/// spell both. Every caller that has no route — `beck test`, the differential harness, a
/// benchmark, a read model — passes through here rather than writing `"/"` again.
pub const ROOT: &str = "/";

/// `Proposal` — a command and who proposed it, which is what `validate` is given and the only
/// place a `Session` reaches (§3.5).
pub fn proposal<'a>(
    actor: &str,
    claims: impl IntoIterator<Item = (&'a str, &'a str)>,
    path: &str,
    command: Value,
) -> Value {
    Value::data(
        Arc::from("Proposal"),
        None,
        Fields::from_iter([
            (Arc::from("session"), session(actor, claims, path)),
            (Arc::from("command"), command),
        ]),
    )
}
