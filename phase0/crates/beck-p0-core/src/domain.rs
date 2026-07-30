//! The todo domain, hand-lowered from the sketch in `docs/00-original-idea.md`.
//!
//! Two vocabularies, one trust boundary: clients propose `Command`s, the server's `validate`
//! decides which become `Event`s, and only events reach the fold. The fold is replay-pure — it
//! reads `env.at` and `env.actor` as data and never calls a clock, a random number generator, or
//! I/O (§3.7).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::envelope::Envelope;

/// `(type Id Uuid)` — minted by the client, because a browser here is a replica, not a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Id(pub Uuid);

impl Id {
    pub fn nil() -> Self {
        Id(Uuid::nil())
    }

    /// Deterministic id for tests and benchmarks — never used on the ingress path.
    pub fn from_u128(n: u128) -> Self {
        Id(Uuid::from_u128(n))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for Id {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Id(Uuid::parse_str(s)?))
    }
}

/// A stable authenticated identity. Never a token, never a live capability (F5).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ActorId(pub String);

impl ActorId {
    pub fn new(s: impl Into<String>) -> Self {
        ActorId(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The capability held by `validate`, the sole consumer of ingress.
///
/// Phase 0 uses dev-mode identity (the actor is asserted by the connecting client); the OIDC
/// relying-party runtime that mints this for real arrives in Phase 3 (`docs/10-decisions.md` D6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub actor: ActorId,
}

impl Session {
    pub fn new(actor: ActorId) -> Self {
        Self { actor }
    }
}

/// `(type Todo {id, text, done})`, plus the owner the fold derives from `env.actor`.
///
/// The sketch's todo has no owner because the sketch is deliberately auth-free; §3.8 says
/// per-session views are the norm, and the fanout exit criterion measures exactly such a view, so
/// Phase 0 carries the owner. It is data by the time the fold sees it, so determinism is intact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub id: Id,
    pub text: String,
    pub done: bool,
    pub owner: ActorId,
}

/// What clients may ASK.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "c", rename_all = "lowercase")]
pub enum Command {
    Add { id: Id, text: String },
    Toggle { id: Id },
    Delete { id: Id },
}

/// What the server RECORDS. Past tense, immutable, the only input to the fold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Added { id: Id, text: String },
    Toggled { id: Id },
    Deleted { id: Id },
}

/// Why a command produced no events. Rejections are not logged (F3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    /// `(if (blank? text) None ...)` — the sketch's own rule.
    BlankText,
    /// A client-minted id that is already in use. First-writer wins; never an overwrite (F2).
    IdTaken,
    NoSuchTodo,
    /// The command references an entity this actor does not own (F2).
    NotOwner,
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Rejection::BlankText => "blank text",
            Rejection::IdTaken => "id already taken",
            Rejection::NoSuchTodo => "no such todo",
            Rejection::NotOwner => "not the owner",
        };
        f.write_str(s)
    }
}

/// The accumulator of the durable fold: `(def todos (durable (fold apply-event {} events)))`.
///
/// `BTreeMap` rather than `HashMap` on purpose — iteration order is part of the rendered view, and
/// replay must reproduce the patch stream bit for bit, not merely the set of todos.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoState {
    pub todos: BTreeMap<Id, Todo>,
}

impl TodoState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Content hash of the accumulator — the replay-determinism oracle (§4.8).
    pub fn digest(&self) -> [u8; 32] {
        let bytes = postcard::to_allocvec(self).expect("state is serialisable");
        *blake3::hash(&bytes).as_bytes()
    }

    pub fn len(&self) -> usize {
        self.todos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.todos.is_empty()
    }
}

/// `apply-event` — pure, unplaced, replay-pure. The compiler will compile this twice (server fold,
/// and, in Mode B, the client's speculative fold); Phase 0 compiles it once because v0.1 is Mode A.
///
/// Written as an in-place update of an owned accumulator: that is the shape the compiler's linear
/// analysis produces for a fold whose previous state is dead, and it keeps the fold O(log n) per
/// event instead of O(n).
pub fn apply_event(state: &mut TodoState, env: &Envelope<Event>) {
    match &env.body {
        Event::Added { id, text } => {
            state.todos.entry(*id).or_insert_with(|| Todo {
                id: *id,
                text: text.clone(),
                done: false,
                owner: env.actor.clone(),
            });
        }
        Event::Toggled { id } => {
            if let Some(t) = state.todos.get_mut(id) {
                t.done = !t.done;
            }
        }
        Event::Deleted { id } => {
            state.todos.remove(id);
        }
    }
}

/// The sketch's literal reading: `(Map Id Todo) -> Event -> (Map Id Todo)`.
///
/// Kept as the oracle for [`apply_event`]; property tests assert the two agree, which is the
/// Phase 0 form of §4.8's "incremental vs. recompute" discipline.
pub fn apply_event_pure(state: &TodoState, env: &Envelope<Event>) -> TodoState {
    let mut next = state.clone();
    apply_event(&mut next, env);
    next
}

/// Fold a whole log. This is `beck replay` (§3.7) in one line.
pub fn fold<'a>(events: impl IntoIterator<Item = &'a Envelope<Event>>) -> TodoState {
    let mut state = TodoState::new();
    for env in events {
        apply_event(&mut state, env);
    }
    state
}

/// `validate : (Session, Command) -> list[Event]` (§3.7).
///
/// The general signature is a *list* of events appended atomically at contiguous `seq`s; the
/// sketch's `Option[Event]` is the single-event special case, which is all the todo domain needs.
/// The two obligations the sketch skips and §3.7/F2 demand are enforced here: client-minted ids are
/// accepted only if fresh, and commands referencing existing entities check ownership.
pub fn validate(
    state: &TodoState,
    session: &Session,
    cmd: &Command,
) -> Result<Vec<Event>, Rejection> {
    match cmd {
        Command::Add { id, text } => {
            if text.trim().is_empty() {
                return Err(Rejection::BlankText);
            }
            if state.todos.contains_key(id) {
                return Err(Rejection::IdTaken);
            }
            Ok(vec![Event::Added {
                id: *id,
                text: text.clone(),
            }])
        }
        Command::Toggle { id } => {
            let todo = state.todos.get(id).ok_or(Rejection::NoSuchTodo)?;
            if todo.owner != session.actor {
                return Err(Rejection::NotOwner);
            }
            Ok(vec![Event::Toggled { id: *id }])
        }
        Command::Delete { id } => {
            let todo = state.todos.get(id).ok_or(Rejection::NoSuchTodo)?;
            if todo.owner != session.actor {
                return Err(Rejection::NotOwner);
            }
            Ok(vec![Event::Deleted { id: *id }])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Instant;

    fn env(seq: u64, actor: &str, body: Event) -> Envelope<Event> {
        Envelope::new(seq, Instant(seq as i64), ActorId::new(actor), body)
    }

    #[test]
    fn fold_matches_the_pure_oracle() {
        let log = vec![
            env(
                1,
                "alice",
                Event::Added {
                    id: Id::from_u128(1),
                    text: "write the fold".into(),
                },
            ),
            env(
                2,
                "bob",
                Event::Added {
                    id: Id::from_u128(2),
                    text: "kill the process".into(),
                },
            ),
            env(
                3,
                "alice",
                Event::Toggled {
                    id: Id::from_u128(1),
                },
            ),
            env(
                4,
                "bob",
                Event::Deleted {
                    id: Id::from_u128(2),
                },
            ),
        ];

        let compiled = fold(&log);
        let oracle = log
            .iter()
            .fold(TodoState::new(), |s, e| apply_event_pure(&s, e));
        assert_eq!(compiled, oracle);
        assert_eq!(compiled.digest(), oracle.digest());
        assert_eq!(compiled.len(), 1);
        assert!(compiled.todos[&Id::from_u128(1)].done);
        assert_eq!(
            compiled.todos[&Id::from_u128(1)].owner,
            ActorId::new("alice")
        );
    }

    #[test]
    fn replaying_the_log_is_bit_identical() {
        let log: Vec<_> = (1..=500)
            .map(|i| {
                env(
                    i,
                    if i % 2 == 0 { "alice" } else { "bob" },
                    Event::Added {
                        id: Id::from_u128(i as u128),
                        text: format!("todo {i}"),
                    },
                )
            })
            .collect();
        assert_eq!(fold(&log).digest(), fold(&log).digest());
    }

    #[test]
    fn validate_rejects_blank_text_stale_ids_and_other_peoples_todos() {
        let alice = Session::new(ActorId::new("alice"));
        let bob = Session::new(ActorId::new("bob"));
        let mut state = TodoState::new();

        let id = Id::from_u128(1);
        assert_eq!(
            validate(
                &state,
                &alice,
                &Command::Add {
                    id,
                    text: "  ".into()
                }
            ),
            Err(Rejection::BlankText)
        );

        let events = validate(
            &state,
            &alice,
            &Command::Add {
                id,
                text: "real".into(),
            },
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        apply_event(&mut state, &env(1, "alice", events[0].clone()));

        // First writer wins: a colliding id is rejected, never an overwrite (F2).
        assert_eq!(
            validate(
                &state,
                &bob,
                &Command::Add {
                    id,
                    text: "hijack".into()
                }
            ),
            Err(Rejection::IdTaken)
        );
        assert_eq!(state.todos[&id].text, "real");

        // Ownership is checked against the actor, not asserted by the client (F2).
        assert_eq!(
            validate(&state, &bob, &Command::Toggle { id }),
            Err(Rejection::NotOwner)
        );
        assert!(validate(&state, &alice, &Command::Toggle { id }).is_ok());
        assert_eq!(
            validate(
                &state,
                &alice,
                &Command::Delete {
                    id: Id::from_u128(99)
                }
            ),
            Err(Rejection::NoSuchTodo)
        );
    }
}
