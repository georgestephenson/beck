//! Phase 0 runtime — "the Roc platform of Beck": an effectful Rust host owning I/O, scheduling and
//! memory, executing the pure program in `beck-p0-core` (§5.2).
//!
//! The shape here is the one the compiler will synthesise in stage 8 (§4.3): one merge point
//! feeding a sequencer, a durable fold downstream of the log, one signal slice per subscription,
//! and a diff operator per Mode-A subscriber.

pub mod app;
pub mod http;
pub mod metrics;
pub mod session;

pub use app::{App, AppConfig};
pub use metrics::Metrics;
