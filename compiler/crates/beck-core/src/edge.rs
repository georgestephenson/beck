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

/// An awareness roster: each actor's contribution, keyed by actor.
///
/// [`presence`] with a payload. The keys are the same names and carry the same warning — they are
/// what the client said it was, bounded by the registry rather than trusted (`docs/82` §82.5).
pub fn awareness<'a>(here: impl IntoIterator<Item = (&'a str, Value)>) -> Value {
    Value::Map(
        here.into_iter()
            .map(|(actor, v)| (Value::str_(actor), v))
            .collect(),
    )
}

/// The roster of a world with one connection, for [`presence_of`]'s reason: a caller with no
/// registry is rendering the page one actor sees while looking at it, and a roster without them in
/// it would describe a page nobody is reading.
pub fn awareness_of(actor: &str, mine: Value) -> Value {
    awareness([(actor, mine)])
}

/// No roster at all — what a page that reads none is handed.
pub fn no_awareness() -> Value {
    Value::Map(Default::default())
}

/// `Freshness` — whether the page about to be rendered is of the confirmed state or of a guess.
///
/// §3.7: "`Signal[T]` carries a freshness dimension (`confirmed | pending(n)`) that UI code can
/// render (\"saving…\") — staleness is typed, not pretended away."
///
/// `n` is how many of this client's own commands are folded into the state being rendered and not
/// yet reflected in what the server has confirmed. Zero is `Confirmed` rather than `Pending(0)`,
/// which is the one decision in this function: a page asking "is this a guess" would otherwise
/// have to know that one of the two variants sometimes means the other.
///
/// Built here, beside the other four, because a Mode B client is the only thing that ever produces
/// a non-`Confirmed` value and the server produces the confirmed one — two constructors would be
/// two spellings of a union the same `view` matches on.
pub fn freshness(pending: usize) -> Value {
    match pending {
        0 => Value::data(
            Arc::from("Freshness"),
            Some(Arc::from("Confirmed")),
            Fields::new(),
        ),
        n => Value::data(
            Arc::from("Freshness"),
            Some(Arc::from("Pending")),
            Fields::from_iter([(Arc::from("n"), Value::Int(n as i64))]),
        ),
    }
}

/// The freshness of a page nothing is in flight for.
///
/// Every renderer that is not a Mode B client passes through here: the server (which holds the
/// confirmed state by definition — a guess is the client's, and the log is the server's), `beck
/// test`, a read model, a benchmark. It is a function rather than a constant so that the *reason*
/// has one place to be written down.
pub fn confirmed() -> Value {
    freshness(0)
}

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
