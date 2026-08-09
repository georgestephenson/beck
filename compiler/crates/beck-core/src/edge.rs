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

/// `Session` — who is asking. Phase 1 carries the actor only; D6's relying party is what would add
/// to it.
pub fn session(actor: &str) -> Value {
    Value::data(
        Arc::from("Session"),
        None,
        Fields::from_iter([(Arc::from("actor"), Value::str_(actor))]),
    )
}

/// `Proposal` — a command and who proposed it, which is what `validate` is given and the only
/// place a `Session` reaches (§3.5).
pub fn proposal(actor: &str, command: Value) -> Value {
    Value::data(
        Arc::from("Proposal"),
        None,
        Fields::from_iter([
            (Arc::from("session"), session(actor)),
            (Arc::from("command"), command),
        ]),
    )
}
