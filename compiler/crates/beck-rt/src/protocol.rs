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
    /// Subscribe, or resume.
    ///
    /// `seq` is **what this client holds**, and its absence is meaningful: `None` is "I have
    /// nothing, send me the world" and `Some(n)` is "I hold the frame as of `n`" — including
    /// `Some(0)`, which is what a browser says when the document it is running in was rendered
    /// from an empty log. Those were the same message until a browser ran the thin client and the
    /// first paint rebuilt the page it had just been served (`docs/94` §94.7): position zero and
    /// nothing-at-all are different facts, and a protocol that spells them the same way cannot
    /// keep §5.1's "first paint is free" promise.
    #[serde(rename = "hello")]
    Hello {
        sub: String,
        #[serde(default)]
        seq: Option<Seq>,
        /// What the client says it is. **A claim, not an actor**: `beck_rt::identity` is what
        /// turns it into one, and under `DevIdentity` the two are the same value — which is a
        /// choice an operator makes rather than a property of the protocol (`docs/48`).
        #[serde(default)]
        actor: String,
        /// Where this client is, as a route. Absent means the application's root.
        ///
        /// On the `hello` rather than only in a [`ClientMsg::Nav`] because a subscription is
        /// re-established after every disconnection, and a client whose route were established by
        /// a separate frame would render one page for as long as the two frames were in flight —
        /// and would render the wrong page for as long as it took the second one to be *re*sent
        /// after a reload with the network down.
        #[serde(default = "root")]
        path: String,
    },
    /// A proposal. `id` is the idempotency key that makes a retry after a reconnect safe (§4.3).
    #[serde(rename = "c")]
    Cmd { id: String, command: Value },
    /// The client is somewhere else now.
    ///
    /// It travels on the same socket as the commands, which is the whole of the ordering argument:
    /// a command proposed from a page is preceded by the navigation that produced that page, so
    /// the `Session` the server hands `validate` is the one the client's own copy of `validate`
    /// saw. Nothing had to be added to the command frame to make that true.
    #[serde(rename = "g")]
    Nav { path: String },
    #[serde(rename = "ping")]
    Ping,
}

fn root() -> String {
    beck_core::edge::ROOT.to_string()
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

    /// The connection is over before it began: the identity was refused.
    ///
    /// Coarse on purpose (`identity::Rejected::message`) — a client learns it was refused and not
    /// which of the three ways, because the difference is useful to an attacker and to nobody
    /// else. The operator gets the distinction, in the log.
    pub fn error(why: &str) -> Value {
        json!({"t": "e", "e": why})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absence and zero are different messages, and the server reads them differently.
    #[test]
    fn a_hello_without_a_position_holds_nothing_and_one_with_zero_holds_the_first_page() {
        let nothing = ClientMsg::parse(r#"{"t":"hello","sub":"s1","actor":"ana"}"#).unwrap();
        assert!(matches!(nothing, ClientMsg::Hello { seq: None, .. }));
        let painted =
            ClientMsg::parse(r#"{"t":"hello","sub":"s1","seq":0,"actor":"ana"}"#).unwrap();
        assert!(matches!(painted, ClientMsg::Hello { seq: Some(0), .. }));
    }

    #[test]
    fn parses_a_resume_hello() {
        let msg = ClientMsg::parse(r#"{"t":"hello","sub":"s1","seq":41,"actor":"alice"}"#).unwrap();
        assert_eq!(
            msg,
            ClientMsg::Hello {
                sub: "s1".into(),
                seq: Some(41),
                actor: "alice".into(),
                path: "/".into(),
            }
        );
    }

    /// A client that says where it is, and one that does not.
    ///
    /// The default is the application's root rather than an empty string, because a program
    /// matching on `session.path` should not have to spell "the client did not say" and "the client
    /// is at the root" as two different pages — and every client that predates the router sends no
    /// `path` at all.
    #[test]
    fn a_hello_carries_the_route_and_defaults_to_the_root() {
        let deep =
            ClientMsg::parse(r#"{"t":"hello","sub":"s1","actor":"ana","path":"/done"}"#).unwrap();
        assert!(matches!(deep, ClientMsg::Hello { ref path, .. } if path == "/done"));
        let silent = ClientMsg::parse(r#"{"t":"hello","sub":"s1","actor":"ana"}"#).unwrap();
        assert!(matches!(silent, ClientMsg::Hello { ref path, .. } if path == "/"));
    }

    /// A navigation is its own frame, on the same socket as the commands — which is what makes the
    /// `Session` the server hands `validate` the one the client's own `validate` saw.
    #[test]
    fn parses_a_navigation() {
        assert_eq!(
            ClientMsg::parse(r#"{"t":"g","path":"/done"}"#).unwrap(),
            ClientMsg::Nav {
                path: "/done".into()
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
