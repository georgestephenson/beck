//! The Beck runtime — the effectful host a compiled program runs on.
//!
//! [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.2 calls this "the 'Roc
//! platform' of Beck — an effectful Rust host owning I/O, scheduling and memory, executing the
//! pure program". Concretely: the log engine, the sequencer, the structural differ, the patch
//! protocol, the websocket, and the SSR path.
//!
//! Phase 0 built all of this against one hand-written application. The interesting property of
//! Phase 1 is what is *missing* here: no `Todo`, no `Command`, no view. Those arrive as compiled
//! `Core` through [`program::Runtime`].

pub mod app;
pub mod css;
pub mod dash;
pub mod diff;
pub mod http;
pub mod identity;
pub mod log;
pub mod outbound;
pub mod patch;
pub mod program;
pub mod protocol;
pub mod session;
pub mod telemetry;
pub mod testing;

pub use app::{replay_from_genesis, replay_to, App, AppConfig};
pub use dash::{Dashboard, ResourceRow};
pub use diff::{diff, Op, Path};
pub use log::{Envelope, Instant, LogStore, MemoryLog, PgLog, RedbLog, Seq, Snapshot};
pub use patch::{Codec, PatchFrame};
pub use program::Runtime;
pub use telemetry::{telemetry, timed, Telemetry};
pub use testing::{run as run_tests, Case, Options as TestOptions, Outcome, Report as TestReport};

/// The thin client: compiler residue, and the only JavaScript in the system (§5.1).
///
/// "Hand-written JavaScript never appears in the source — it's compiler residue: the patch
/// interpreter plus the compiled view. You stopped writing it the moment the page became a
/// function." This file is that residue, and it holds no application logic: it applies patches,
/// captures declared events, and posts commands back up the socket.
pub const THIN_CLIENT: &str = include_str!("../client/beck-thin.js");
