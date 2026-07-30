//! Phase 0 core — the pure, unplaced tiers of the todo sketch
//! ([`docs/00-original-idea.md`](../../../../docs/00-original-idea.md)), hand-written in Rust as
//! the output the compiler will eventually generate ([`docs/08-roadmap.md`](../../../../docs/08-roadmap.md)
//! Phase 0).
//!
//! Nothing in this crate performs I/O, reads the clock, or generates randomness. That is not a
//! style preference: it is the replay-purity rule of
//! [`docs/03-type-and-effect-system.md`](../../../../docs/03-type-and-effect-system.md) §3.7,
//! which is what makes "replaying the log reproduces the state, bit for bit" true and testable.
//! The compiler will enforce it with effect rows; Phase 0 enforces it by construction — this crate
//! has no dependency that could break it.

pub mod css;
pub mod diff;
pub mod domain;
pub mod envelope;
pub mod html;
pub mod patch;
pub mod protocol;
pub mod view;

pub use diff::{diff, Op, Path};
pub use domain::{ActorId, Command, Event, Id, Rejection, Session, Todo, TodoState};
pub use envelope::{CommandEnvelope, Envelope, Instant, Seq};
pub use html::Html;
pub use patch::{Codec, PatchFrame};
pub use view::{page, remaining, visible};
