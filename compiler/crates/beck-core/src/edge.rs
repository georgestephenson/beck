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

/// `Session` — who is asking, and what the identity provider said about them.
///
/// The claims are a `Map[Str, Str]` and not a record, because the set is the provider's rather
/// than the program's: a tenant claim one deployment issues is one another has never heard of.
/// They are copied in at the edge for the same reason the actor is — a fold that read them from a
/// token would be a fold that could not replay ([`crate::render`], §3.7).
pub fn session<'a>(actor: &str, claims: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
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
        ]),
    )
}

/// The roster `presence()` produces: actor to how many connections that actor holds.
///
/// Built here beside the other three because it is the same kind of value — something the host
/// hands a pure program — and because the *shape* has to be one definition. A page reads it with
/// `map_len`, `map_keys` and `map_contains`, and a second constructor spelling the pairs
/// differently would be a page that renders one way under `beck test` and another under `beck run`.
pub fn presence<'a>(here: impl IntoIterator<Item = (&'a str, i64)>) -> Value {
    Value::Map(
        here.into_iter()
            .map(|(actor, n)| (Value::str_(actor), Value::Int(n)))
            .collect(),
    )
}

/// The roster of a world with one connection: the viewer's own.
///
/// What `beck test` renders a page in, and what a caller with no connection registry gets. A test
/// asserting on the page one actor sees is asking what that actor sees while looking at it, so a
/// roster that did not contain them would be describing a page nobody is reading.
pub fn presence_of(actor: &str) -> Value {
    presence([(actor, 1)])
}

/// `Proposal` — a command and who proposed it, which is what `validate` is given and the only
/// place a `Session` reaches (§3.5).
pub fn proposal<'a>(
    actor: &str,
    claims: impl IntoIterator<Item = (&'a str, &'a str)>,
    command: Value,
) -> Value {
    Value::data(
        Arc::from("Proposal"),
        None,
        Fields::from_iter([
            (Arc::from("session"), session(actor, claims)),
            (Arc::from("command"), command),
        ]),
    )
}
