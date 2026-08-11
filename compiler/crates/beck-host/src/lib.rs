//! The platform, minus the host.
//!
//! [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.2 calls the runtime "the
//! 'Roc platform' of Beck — an effectful Rust host owning I/O, scheduling and memory, executing the
//! pure program". This crate is everything in that sentence *except* the I/O and the scheduling:
//! the bridge to a compiled program ([`program::Runtime`]), what a logged occurrence is
//! ([`record`]), what a subscription says to a client ([`protocol`]), and the rules the merge point
//! applies to a batch of proposals ([`mod@sequence`]).
//!
//! # Why it is a crate rather than four modules of `beck-rt`
//!
//! Because a browser tab is a second host. [`docs/17-playground.md`](../../../../docs/17-playground.md)
//! §17.2 says the playground's worker-server is "the rung-0 platform compiled to WASM", and the
//! whole force of that claim is the word *the*: a tab that ran a second implementation of the
//! sequencer would be a demo of something adjacent to Beck. `beck-rt` cannot cross to
//! `wasm32-unknown-unknown` — it holds Postgres, redb, SQLite, TLS and a multi-threaded reactor —
//! and none of that is what a merge point *is*. So the part that is program-shaped rather than
//! machine-shaped lives here, `beck-rt` re-exports it unchanged, and `beck-play` links the same
//! code into a tab.
//!
//! Nothing here reads a clock, a socket or a disk. That is the membership rule: if it needs the
//! machine, it belongs upstairs in `beck-rt`.

pub mod program;
pub mod protocol;
pub mod record;
pub mod sequence;

pub use program::{describe, At, Runtime, Viewer};
pub use protocol::{ClientMsg, Resumption, ServerMsg};
pub use record::{Envelope, Instant, Pending, Seq, Snapshot};
pub use sequence::{sequence, Committed, Decision};
