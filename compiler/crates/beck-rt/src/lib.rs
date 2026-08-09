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
pub mod http;
pub mod identity;
pub mod log;
pub mod oidc;
pub mod outbound;
pub mod patch;
pub mod pgwire;
pub mod program;
pub mod protocol;
pub mod quota;
pub mod session;
pub mod telemetry;
pub mod testing;

pub use app::{replay_from_genesis, replay_to, App, AppConfig};
pub use beck_core::diff::{self, diff, Op, Path};
pub use dash::{Dashboard, ResourceRow};
pub use log::{
    Durability, Envelope, Instant, LogStore, MemoryLog, PgLog, RedbLog, Seq, Snapshot, SqliteLog,
};
pub use patch::{Codec, PatchFrame};
pub use program::Runtime;
pub use telemetry::{telemetry, timed, Telemetry};
pub use testing::{run as run_tests, Case, Options as TestOptions, Outcome, Report as TestReport};

/// The patch interpreter and the socket, shared by both rendering modes (§5.1).
///
/// "Hand-written JavaScript never appears in the source — it's compiler residue: the patch
/// interpreter plus the compiled view. You stopped writing it the moment the page became a
/// function." These three files are that residue, and they hold no application logic.
pub const PATCH_CLIENT: &str = include_str!("../client/beck-patch.js");

/// Mode A: apply the patches the server sends, post commands back up the socket.
pub const THIN_CLIENT: &str = include_str!("../client/beck-thin.js");

/// Mode B: load the kernel, hold the state, render locally ([`beck_core::render`]).
pub const MODE_B_CLIENT: &str = include_str!("../client/beck-mode-b.js");

/// Mode B's service worker: the shell, cached, so a cold start with no network is a page.
///
/// Served with the program's wire id substituted for `%WIRE%`, which is what keys the cache to the
/// program and what deletes the previous one on a deploy.
pub const SERVICE_WORKER: &str = include_str!("../client/beck-sw.js");
