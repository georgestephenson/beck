//! The socket protocol: one connection multiplexing patches down and commands up (§5.1).
//!
//! Resumption is the load-bearing part. A subscriber reconnects with `(subscription, seq)` and the
//! server replays the gap rather than re-rendering the world — which is what makes a deploy, or a
//! dropped train tunnel, cost one small patch instead of a full page.
//!
//! One rule Phase 0 learned the hard way and §18.7 item 5 says to carry forward: **the ack tells
//! you the command landed; the frame tells you where your view stands, and the two are different
//! facts.** A command whose net effect is invisible in a subscriber's own view produces an empty
//! diff and therefore no frame, so a client waiting for "the patch for my command" would wait
//! forever. `up_to_date` is the answer, and it is sent only to a client waiting on its own
//! command — never to idle ones, because a message per idle subscriber per event is precisely the
//! fanout cost this design exists to avoid.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::log::Seq;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "t")]
pub enum ClientMsg {
    /// Subscribe, or resume: `seq` is the last frame this client applied. `seq = 0` means "I have
    /// nothing; send me the world".
    #[serde(rename = "hello")]
    Hello {
        sub: String,
        #[serde(default)]
        seq: Seq,
        /// Dev-mode identity (Phase 1, as Phase 0). D6's OIDC relying party is Phase 3.
        actor: String,
    },
    /// A proposal. `id` is the idempotency key that makes a retry after a reconnect safe (§4.3).
    #[serde(rename = "c")]
    Cmd { id: String, command: Value },
    #[serde(rename = "ping")]
    Ping,
}

impl ClientMsg {
    pub fn parse(text: &str) -> Result<ClientMsg, serde_json::Error> {
        serde_json::from_str(text)
    }
}

/// How a subscription was (re)established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resumption {
    /// First connection: the client has nothing, so the frame carries the whole view.
    Fresh,
    /// The client's `seq` was still reachable: it gets a patch covering exactly the gap.
    Resumed { from: Seq, replayed: u64 },
    /// The gap was unreachable (log truncated, or a `seq` from another lifetime of the app): the
    /// client is reset with a full frame. Honest, and counted separately.
    Reset { from: Seq },
}

impl Resumption {
    pub fn label(&self) -> &'static str {
        match self {
            Resumption::Fresh => "fresh",
            Resumption::Resumed { .. } => "resumed",
            Resumption::Reset { .. } => "reset",
        }
    }
}

pub struct ServerMsg;

impl ServerMsg {
    pub fn welcome(sub: &str, seq: Seq, how: Resumption) -> Value {
        let mut msg = json!({"t": "w", "sub": sub, "q": seq, "how": how.label()});
        if let Resumption::Resumed { replayed, .. } = how {
            msg["replayed"] = json!(replayed);
        }
        msg
    }

    pub fn ack(id: &str, seq: Seq) -> Value {
        json!({"t": "a", "id": id, "q": seq})
    }

    /// "Your view is current as of `seq`, and nothing in it changed."
    pub fn up_to_date(seq: Seq) -> Value {
        json!({"t": "u", "q": seq})
    }

    pub fn nack(id: &str, why: &str) -> Value {
        json!({"t": "n", "id": id, "e": why})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_resume_hello() {
        let msg = ClientMsg::parse(r#"{"t":"hello","sub":"s1","seq":41,"actor":"alice"}"#).unwrap();
        assert_eq!(
            msg,
            ClientMsg::Hello {
                sub: "s1".into(),
                seq: 41,
                actor: "alice".into()
            }
        );
    }

    #[test]
    fn a_command_is_carried_untyped_and_decoded_against_the_programs_union() {
        // The runtime decodes against `union Command` from the source; the protocol itself knows
        // nothing about what commands exist, which is what makes it program-independent.
        let msg = ClientMsg::parse(
            r#"{"t":"c","id":"k1","command":{"c":"Toggle","id":"00000000-0000-0000-0000-000000000001"}}"#,
        )
        .unwrap();
        match msg {
            ClientMsg::Cmd { id, command } => {
                assert_eq!(id, "k1");
                assert_eq!(command["c"], "Toggle");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_welcome_frame_reports_how_resumption_went() {
        let w = ServerMsg::welcome(
            "s",
            7,
            Resumption::Resumed {
                from: 3,
                replayed: 4,
            },
        );
        assert_eq!(w["how"], "resumed");
        assert_eq!(w["replayed"], 4);
        assert_eq!(
            ServerMsg::welcome("s", 7, Resumption::Fresh)["how"],
            "fresh"
        );
    }
}
