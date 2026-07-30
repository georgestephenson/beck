//! The socket protocol: one connection multiplexing patches down and commands up (§5.1).
//!
//! Resumption is the load-bearing part. A subscriber reconnects with `(subscription, seq)` and the
//! server replays the gap rather than re-rendering the world — which is what makes a deploy, or a
//! dropped train tunnel, cost one small patch instead of a full page (§5.1, R5).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::domain::Command;
use crate::envelope::Seq;
use crate::patch::SubId;

/// Which slice of the fold the subscriber is asking for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeSel {
    /// The sketch's broadcast view: every todo.
    #[default]
    All,
    /// §3.8's per-session view — `todos.map(filter_by(session.user))`.
    Mine,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "t")]
pub enum ClientMsg {
    /// Subscribe, or resume: `seq` is the last frame this client applied. `seq = 0` means "I have
    /// nothing; send me the world".
    #[serde(rename = "hello")]
    Hello {
        sub: SubId,
        #[serde(default)]
        seq: Seq,
        /// Dev-mode identity (Phase 0). Phase 3 replaces this with verified OIDC claims (D6).
        actor: String,
        #[serde(default)]
        scope: ScopeSel,
    },
    /// A proposal. `id` is the idempotency key that makes a retry after a reconnect safe (§4.3).
    ///
    /// The command is nested rather than flattened: a command envelope has its own id, and so does
    /// every entity the todo commands name. Flattening would collide them.
    #[serde(rename = "c")]
    Cmd { id: Uuid, command: Command },
    #[serde(rename = "ping")]
    Ping,
}

impl ClientMsg {
    pub fn parse(text: &str) -> Result<ClientMsg, serde_json::Error> {
        serde_json::from_str(text)
    }
}

/// How a subscription was (re)established — reported to the client and counted as a metric,
/// because "did resumption actually replay the gap" is a Phase 0 exit criterion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resumption {
    /// First connection: the client has nothing, so the frame carries the whole view.
    Fresh,
    /// The client's `seq` was still reachable: it gets a patch covering exactly the gap.
    Resumed { from: Seq, replayed: u64 },
    /// The gap was unreachable (log truncated, or `seq` from another lifetime of the app): the
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

/// Server → client frames. Encoded by hand: these ride on every interaction, and the shapes are
/// small enough that a derived encoder would only obscure what goes on the wire.
pub struct ServerMsg;

impl ServerMsg {
    pub fn welcome(sub: &str, seq: Seq, how: Resumption) -> Value {
        let mut msg = json!({"t": "w", "sub": sub, "q": seq, "how": how.label()});
        if let Resumption::Resumed { replayed, .. } = how {
            msg["replayed"] = json!(replayed);
        }
        msg
    }

    pub fn ack(id: Uuid, seq: Seq) -> Value {
        json!({"t": "a", "id": id, "q": seq})
    }

    /// "Your view is current as of `seq`, and nothing in it changed."
    ///
    /// A patch stream carries *states*, not events: a command whose net effect is invisible in a
    /// subscriber's own view — an add and a delete coalesced into one wake, or a change filtered
    /// out by a per-session view — produces an empty diff and therefore no frame. Without this
    /// notice, such a client waits forever for a patch that will never come, and its `seq` stops
    /// advancing, so its next reconnect replays a gap that contains nothing.
    ///
    /// Sent only to a subscriber that is waiting on one of its own commands, never to idle ones —
    /// otherwise every idle subscriber would pay a message per event, which is precisely the
    /// fanout cost this design exists to avoid.
    pub fn up_to_date(seq: Seq) -> Value {
        json!({"t": "u", "q": seq})
    }

    pub fn nack(id: Uuid, why: &str) -> Value {
        json!({"t": "n", "id": id, "e": why})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Id;

    #[test]
    fn parses_a_resume_hello() {
        let msg =
            ClientMsg::parse(r#"{"t":"hello","sub":"s1","seq":41,"actor":"alice","scope":"mine"}"#)
                .unwrap();
        assert_eq!(
            msg,
            ClientMsg::Hello {
                sub: "s1".into(),
                seq: 41,
                actor: "alice".into(),
                scope: ScopeSel::Mine,
            }
        );
    }

    #[test]
    fn parses_the_commands_the_view_emits() {
        // `command` is exactly the payload `data-b-click` carries, with the thin client's two
        // substitutions already applied.
        let msg = ClientMsg::parse(
            r#"{"t":"c","id":"6ba7b810-9dad-11d1-80b4-00c04fd430c8",
                "command":{"c":"toggle","id":"00000000-0000-0000-0000-000000000001"}}"#,
        )
        .unwrap();
        match msg {
            ClientMsg::Cmd { command, .. } => assert_eq!(
                command,
                Command::Toggle {
                    id: Id::from_u128(1)
                }
            ),
            other => panic!("unexpected {other:?}"),
        }

        let add = ClientMsg::parse(
            r#"{"t":"c","id":"6ba7b810-9dad-11d1-80b4-00c04fd430c8",
                "command":{"c":"add","id":"00000000-0000-0000-0000-000000000002","text":"buy milk"}}"#,
        )
        .unwrap();
        assert!(matches!(
            add,
            ClientMsg::Cmd {
                command: Command::Add { .. },
                ..
            }
        ));
    }
}
