//! A `tracing` layer that copies log records into the runtime's telemetry ring.
//!
//! Every diagnostic the runtime writes already goes through `tracing`, so this captures them all
//! without a second logging call at each site — the alternative, a `telemetry().log(...)` beside
//! every `tracing::warn!`, is the kind of duplication that goes stale one site at a time.
//!
//! It lives in the CLI rather than in `beck-rt` so the runtime crate does not depend on subscriber
//! machinery: the runtime *emits*, and whoever assembles a process decides where records go.
//!
//! The one field it looks for is `seq`. [`beck_rt::telemetry`] explains why: a sequence number is
//! not a correlation id, it is a reproducible state, so a record that has one is worth more than a
//! record that does not.

use beck_rt::telemetry::telemetry;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

pub struct Capture;

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut visitor = Collect::default();
        event.record(&mut visitor);
        let level = match *event.metadata().level() {
            Level::TRACE => "TRACE",
            Level::DEBUG => "DEBUG",
            Level::INFO => "INFO",
            Level::WARN => "WARN",
            Level::ERROR => "ERROR",
        };
        telemetry().log(
            level,
            event.metadata().target(),
            visitor.render(),
            visitor.seq,
        );
    }
}

/// Collects a record's message and its `seq`, and appends any other fields to the message.
#[derive(Default)]
struct Collect {
    message: String,
    fields: Vec<String>,
    seq: Option<u64>,
}

impl Collect {
    fn render(&self) -> String {
        if self.fields.is_empty() {
            self.message.clone()
        } else {
            format!("{} ({})", self.message, self.fields.join(", "))
        }
    }
}

impl Visit for Collect {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "seq" {
            self.seq = Some(value);
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "seq" && value >= 0 {
            self.seq = Some(value as u64);
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
            // `tracing`'s message field arrives as a `Debug` of a formatted string, which renders
            // with quotes it did not have when it was written.
            self.message = self
                .message
                .trim_start_matches('"')
                .trim_end_matches('"')
                .to_string();
        } else {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }
}
