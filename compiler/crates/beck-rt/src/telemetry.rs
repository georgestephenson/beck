//! Telemetry — and the question of what it is *for*, in an architecture that already has a log.
//!
//! # Why this is not the usual answer
//!
//! Distributed tracing exists because in a system of services nobody knows what happened. You
//! reconstruct causality after the fact from correlated, sampled spans, and you accept that the
//! reconstruction is partial.
//!
//! Beck already has something strictly stronger for the part tracing usually covers: a durable
//! total order of every state transition. `state = fold(f, init, log[..seq])` is not a sample and
//! not a reconstruction — it is the actual history, and [`crate::replay_to`] will rebuild any state
//! the system was ever in. Tracing the fold's internal call tree as spans would re-record, lossily
//! and at cost, what the log records exactly and for free.
//!
//! So the division of labour is specific:
//!
//! | question | answered by |
//! |---|---|
//! | what happened, in what order, and what state did it produce | the log |
//! | what state was the system in at 14:02 | the log, by replay |
//! | why did this command produce that event | the log, by replay |
//! | how long did the fold take | **here** |
//! | how long did the append wait on Postgres | **here** |
//! | what was rejected, and never became an event | **here** |
//! | how many sessions are connected | **here** |
//! | what the maintained views cost, shared and per session | **here** |
//! | did the pod get killed mid-batch | **here** |
//!
//! Everything in the right column is either wall-clock, resource use, or a *non-event*: something
//! the log deliberately does not record, because §4.8 requires the fold to be replay-pure and a
//! fold that recorded its own duration would not replay identically. Telemetry is not a weaker
//! substitute for the log here; it is the complement of it, and the boundary between them is exactly
//! the boundary of determinism.
//!
//! # Correlation is `seq`, not a trace id
//!
//! A random trace id identifies a request. `seq` identifies a *state*: given one, `beck replay
//! --to <seq>` reproduces the system exactly as it was. So every record that has a sequence number
//! carries `beck.seq`, and a span in any OTel backend is one command away from a reproducible
//! debugging session. That is a property this architecture has and a microservice fleet does not.
//!
//! # Is OpenTelemetry valid here?
//!
//! Yes, for the right column, and this module speaks it: [`Telemetry::otlp_metrics`] and
//! [`Telemetry::otlp_logs`] produce OTLP/HTTP JSON, which is a first-class encoding in the OTLP
//! specification — same field names and semantics as the protobuf form, with no `tonic`, no
//! `prost`, and no code generation. `BECK_OTLP_ENDPOINT` turns on export; without it the same data
//! is served to the dashboard from memory.
//!
//! What Beck should *not* do is adopt OTel's model as its own. Spans belong at the boundaries —
//! ingress, validate, append, fold, view, patch — and not inside the fold, where the log is the
//! better instrument.
//!
//! # Cost
//!
//! Counters and histogram buckets are `AtomicU64`: recording is one relaxed fetch-add and no
//! allocation, so instrumenting the fold does not perturb what it measures. Histograms are fixed
//! power-of-two buckets, so the bucket index is a `leading_zeros`. The log ring is bounded and
//! overwrites oldest-first, so a process that runs for a month does not accumulate.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as J};

/// Buckets covering 1 µs to ~1 s in powers of two, plus an overflow bucket.
const BUCKETS: usize = 21;

/// A histogram of durations in microseconds.
///
/// Power-of-two buckets so that recording is `leading_zeros` and a fetch-add: no locks, no
/// allocation, and no comparison chain. The bound this trades away is resolution — a value is
/// placed within a factor of two — which is the right trade for "is the fold suddenly slow", and
/// the wrong one for a billing meter. Nothing here is a billing meter.
#[derive(Debug)]
pub struct Histogram {
    buckets: [AtomicU64; BUCKETS],
    count: AtomicU64,
    sum_us: AtomicU64,
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            count: AtomicU64::new(0),
            sum_us: AtomicU64::new(0),
        }
    }
}

impl Histogram {
    pub fn record_us(&self, us: u64) {
        let i = if us == 0 {
            0
        } else {
            (64 - us.leading_zeros() as usize).min(BUCKETS - 1)
        };
        self.buckets[i].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
    }

    pub fn record(&self, d: std::time::Duration) {
        self.record_us(d.as_micros() as u64);
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn sum_us(&self) -> u64 {
        self.sum_us.load(Ordering::Relaxed)
    }

    pub fn mean_us(&self) -> f64 {
        let n = self.count();
        if n == 0 {
            0.0
        } else {
            self.sum_us() as f64 / n as f64
        }
    }

    /// The upper bound of each bucket, in microseconds — OTLP's `explicitBounds`.
    pub fn bounds() -> Vec<f64> {
        (0..BUCKETS - 1).map(|i| (1u64 << i) as f64).collect()
    }

    pub fn counts(&self) -> Vec<u64> {
        self.buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect()
    }

    /// The bucket at or below which `q` of the observations fall, as an upper bound in µs.
    ///
    /// A bucketed estimate, not an exact quantile: with power-of-two buckets the true value is
    /// within a factor of two of what this returns, and saying "p99 is at most 4 ms" is the honest
    /// form of the claim.
    pub fn quantile_us(&self, q: f64) -> u64 {
        let total = self.count();
        if total == 0 {
            return 0;
        }
        let target = (total as f64 * q).ceil() as u64;
        let mut seen = 0;
        for (i, b) in self.buckets.iter().enumerate() {
            seen += b.load(Ordering::Relaxed);
            if seen >= target {
                return 1u64 << i;
            }
        }
        1u64 << (BUCKETS - 1)
    }
}

#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    pub fn incr(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A gauge — a value that goes up and down, like the number of connected sessions.
#[derive(Debug, Default)]
pub struct Gauge(AtomicU64);

impl Gauge {
    pub fn incr(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    pub fn decr(&self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn set(&self, n: u64) {
        self.0.store(n, Ordering::Relaxed);
    }
    /// Replace one contributor's share: subtract what it held, add what it holds now.
    ///
    /// A gauge that aggregates over many things — the entries every connected subscription is
    /// arranging — cannot be `set` by any one of them, and re-summing them all on every render is
    /// the scan the number exists to avoid. Saturating, because a contributor that reports its
    /// departure twice must not wrap the gauge to `u64::MAX`.
    pub fn adjust(&self, was: u64, now: u64) {
        if now >= was {
            self.0.fetch_add(now - was, Ordering::Relaxed);
        } else {
            let d = was - now;
            let _ = self
                .0
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                    Some(v.saturating_sub(d))
                });
        }
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// One log record, kept in memory for the dashboard and exported as an OTLP log record.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Record {
    pub at_unix_nanos: u64,
    pub level: &'static str,
    pub target: String,
    pub message: String,
    /// The sequence number this record is about, when there is one. This is the correlation key:
    /// with it, any record points at a state `beck replay` can reproduce exactly.
    pub seq: Option<u64>,
}

/// The instruments.
///
/// One value, reached through [`telemetry`], because a metric registry that has to be threaded
/// through every call site gets threaded through some of them.
#[derive(Debug, Default)]
pub struct Telemetry {
    // --- what the log cannot tell you, because it is time ---
    /// How long one fold step took.
    pub fold: Histogram,
    /// How long rendering a view took. The dominant cost until Phase 3 (docs/19 §19.4 item 3).
    pub view: Histogram,
    /// How long the diff between two views took.
    pub diff: Histogram,
    /// How long an append to the log store took — the substrate's latency, not the program's.
    pub append: Histogram,
    /// How long a snapshot took.
    pub snapshot: Histogram,
    /// How long a cold replay took, and over how many events.
    pub replay: Histogram,

    // --- what the log cannot tell you, because it never happened ---
    /// Proposals rejected by `validate`. Rejections never become events, so nothing in the log
    /// records that anyone tried.
    pub rejected: Counter,
    /// Proposals dropped as duplicates by the idempotency key.
    pub deduplicated: Counter,
    /// Appends that failed. If this is non-zero, the log is missing something the program believed.
    pub append_failures: Counter,
    /// Snapshots that failed. Recoverable — the log is still the truth — but slower to recover.
    pub snapshot_failures: Counter,
    /// Client messages that did not parse.
    pub bad_messages: Counter,

    // --- what the log does record, counted here so a rate is available without folding ---
    pub events_appended: Counter,
    pub patch_frames: Counter,
    pub patch_bytes: Counter,

    // --- what is true right now ---
    pub sessions: Gauge,

    /// Recent log records, oldest evicted first.
    ring: Mutex<VecDeque<Record>>,
    /// The `seq` the process has folded to. A gauge in spirit; stored so the dashboard has it
    /// without touching the app.
    pub head: Gauge,

    // --- what a fanout costs, which §5.3 names as a metric and nothing exported until now ---
    /// Arrangement entries held by the **one** shared dataflow: the operators that do not read the
    /// session, maintained once however many subscribers there are (docs/26).
    ///
    /// Entries rather than bytes. Bytes would need `Engine::footprint`, which walks the accumulator
    /// to charge shared structure to the fold — right for a report, far too expensive to sample on
    /// a live process. Entries are `O(operators)` to read and they are the number that scales.
    pub shared_arranged: Gauge,
    /// Arrangement entries held by connected subscriptions, between them.
    ///
    /// This is the one that multiplies by the fanout, and putting the two side by side is the whole
    /// operational question: `shared_arranged` is paid once, `session_arranged` is paid per
    /// connection. A program whose second number dwarfs the first has its cut in the wrong place.
    pub session_arranged: Gauge,
}

/// How many log records to keep. Bounded so a long-running process does not accumulate; the log
/// store is the durable record, and this is a window onto the present.
const RING_CAPACITY: usize = 2_000;

impl Telemetry {
    pub fn log(&self, level: &'static str, target: &str, message: String, seq: Option<u64>) {
        let record = Record {
            at_unix_nanos: now_unix_nanos(),
            level,
            target: target.to_string(),
            message,
            seq,
        };
        let mut ring = self.ring.lock().expect("telemetry ring poisoned");
        if ring.len() == RING_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(record);
    }

    /// The most recent records, newest first.
    pub fn records(&self, limit: usize) -> Vec<Record> {
        let ring = self.ring.lock().expect("telemetry ring poisoned");
        ring.iter().rev().take(limit).cloned().collect()
    }

    /// Everything, as the dashboard's JSON. Not OTLP: this is shaped for a table on a screen.
    pub fn snapshot(&self) -> J {
        let hist = |name: &str, h: &Histogram| {
            json!({
                "name": name,
                "count": h.count(),
                "mean_us": h.mean_us(),
                "p50_us": h.quantile_us(0.50),
                "p99_us": h.quantile_us(0.99),
            })
        };
        json!({
            "counters": {
                "events_appended": self.events_appended.get(),
                "rejected": self.rejected.get(),
                "deduplicated": self.deduplicated.get(),
                "append_failures": self.append_failures.get(),
                "snapshot_failures": self.snapshot_failures.get(),
                "bad_messages": self.bad_messages.get(),
                "patch_frames": self.patch_frames.get(),
                "patch_bytes": self.patch_bytes.get(),
            },
            "gauges": {
                "sessions": self.sessions.get(),
                "head": self.head.get(),
                "shared_arranged": self.shared_arranged.get(),
                "session_arranged": self.session_arranged.get(),
            },
            "histograms": [
                hist("fold", &self.fold),
                hist("view", &self.view),
                hist("diff", &self.diff),
                hist("append", &self.append),
                hist("snapshot", &self.snapshot),
                hist("replay", &self.replay),
            ],
        })
    }

    /// OTLP/HTTP JSON for metrics — the body of a POST to `/v1/metrics`.
    ///
    /// Field names and the numeric enums (`aggregationTemporality: 2` is CUMULATIVE) are the
    /// specification's, so an ordinary collector accepts this without a Beck-specific receiver.
    pub fn otlp_metrics(&self, service: &str) -> J {
        // Start first: it is lazily initialised, so reading `now` first would make the very first
        // export report a start *after* the observation it bounds. A collector is entitled to drop
        // a cumulative point whose window runs backwards, and it would drop it silently.
        let start = start_unix_nanos().to_string();
        let now = now_unix_nanos().to_string();

        let sum = |name: &str, v: u64, monotonic: bool| {
            json!({
                "name": name,
                "unit": "1",
                "sum": {
                    "dataPoints": [{
                        "asInt": v.to_string(),
                        "startTimeUnixNano": start,
                        "timeUnixNano": now,
                    }],
                    "aggregationTemporality": 2,
                    "isMonotonic": monotonic,
                }
            })
        };
        let gauge = |name: &str, v: u64| {
            json!({
                "name": name,
                "unit": "1",
                "gauge": { "dataPoints": [{ "asInt": v.to_string(), "timeUnixNano": now }] }
            })
        };
        let histogram = |name: &str, h: &Histogram| {
            json!({
                "name": name,
                "unit": "us",
                "histogram": {
                    "dataPoints": [{
                        "count": h.count().to_string(),
                        "sum": h.sum_us() as f64,
                        "bucketCounts": h.counts().iter().map(u64::to_string).collect::<Vec<_>>(),
                        "explicitBounds": Histogram::bounds(),
                        "startTimeUnixNano": start,
                        "timeUnixNano": now,
                    }],
                    "aggregationTemporality": 2,
                }
            })
        };

        json!({
            "resourceMetrics": [{
                "resource": { "attributes": resource_attributes(service) },
                "scopeMetrics": [{
                    "scope": { "name": "beck" },
                    "metrics": [
                        sum("beck.events.appended", self.events_appended.get(), true),
                        sum("beck.proposals.rejected", self.rejected.get(), true),
                        sum("beck.proposals.deduplicated", self.deduplicated.get(), true),
                        sum("beck.log.append.failures", self.append_failures.get(), true),
                        sum("beck.snapshot.failures", self.snapshot_failures.get(), true),
                        sum("beck.messages.malformed", self.bad_messages.get(), true),
                        sum("beck.patch.frames", self.patch_frames.get(), true),
                        sum("beck.patch.bytes", self.patch_bytes.get(), true),
                        gauge("beck.sessions.active", self.sessions.get()),
                        gauge("beck.log.head", self.head.get()),
                        gauge("beck.views.shared_arranged", self.shared_arranged.get()),
                        gauge("beck.views.session_arranged", self.session_arranged.get()),
                        histogram("beck.fold.duration", &self.fold),
                        histogram("beck.view.duration", &self.view),
                        histogram("beck.diff.duration", &self.diff),
                        histogram("beck.log.append.duration", &self.append),
                        histogram("beck.snapshot.duration", &self.snapshot),
                        histogram("beck.replay.duration", &self.replay),
                    ]
                }]
            }]
        })
    }

    /// OTLP/HTTP JSON for logs — the body of a POST to `/v1/logs`.
    pub fn otlp_logs(&self, service: &str, limit: usize) -> J {
        let records: Vec<J> = self
            .records(limit)
            .into_iter()
            .map(|r| {
                let mut attributes = vec![json!({
                    "key": "code.namespace",
                    "value": { "stringValue": r.target }
                })];
                // The correlation key. Not a trace id: with this, the exact state is reproducible.
                if let Some(seq) = r.seq {
                    attributes.push(json!({
                        "key": "beck.seq",
                        "value": { "intValue": seq.to_string() }
                    }));
                }
                json!({
                    "timeUnixNano": r.at_unix_nanos.to_string(),
                    "severityNumber": severity_number(r.level),
                    "severityText": r.level,
                    "body": { "stringValue": r.message },
                    "attributes": attributes,
                })
            })
            .collect();

        json!({
            "resourceLogs": [{
                "resource": { "attributes": resource_attributes(service) },
                "scopeLogs": [{ "scope": { "name": "beck" }, "logRecords": records }]
            }]
        })
    }
}

/// OTel's severity numbers: DEBUG 5, INFO 9, WARN 13, ERROR 17.
fn severity_number(level: &str) -> u8 {
    match level {
        "TRACE" => 1,
        "DEBUG" => 5,
        "WARN" => 13,
        "ERROR" => 17,
        _ => 9,
    }
}

fn resource_attributes(service: &str) -> J {
    json!([
        { "key": "service.name", "value": { "stringValue": service } },
        { "key": "telemetry.sdk.name", "value": { "stringValue": "beck" } },
        { "key": "telemetry.sdk.language", "value": { "stringValue": "rust" } },
    ])
}

pub fn now_unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn start_unix_nanos() -> u64 {
    static START: OnceLock<u64> = OnceLock::new();
    *START.get_or_init(now_unix_nanos)
}

/// The process-wide instruments.
pub fn telemetry() -> &'static Telemetry {
    static T: OnceLock<Telemetry> = OnceLock::new();
    T.get_or_init(|| {
        // Stamp the start when the instruments come into existence, so a cumulative metric's
        // window begins when the process began rather than when someone first asked for it.
        start_unix_nanos();
        Telemetry::default()
    })
}

/// Time a block and record it, returning what the block returned.
pub fn timed<T>(h: &Histogram, f: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let out = f();
    h.record(started.elapsed());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_are_powers_of_two_and_bound_the_value() {
        let h = Histogram::default();
        for us in [0u64, 1, 3, 100, 5_000, 900_000, 90_000_000] {
            h.record_us(us);
        }
        assert_eq!(h.count(), 7);
        assert_eq!(h.sum_us(), 1 + 3 + 100 + 5_000 + 900_000 + 90_000_000);
        // Every observation lands in a bucket, including the one that overflows the range.
        assert_eq!(h.counts().iter().sum::<u64>(), 7);
        // A bucketed quantile is an upper bound, and must not understate.
        let h2 = Histogram::default();
        for _ in 0..99 {
            h2.record_us(10);
        }
        h2.record_us(100_000);
        assert!(h2.quantile_us(0.50) >= 10, "p50 understated");
        assert!(h2.quantile_us(0.99) >= 10);
        assert!(
            h2.quantile_us(1.0) >= 100_000,
            "the max must not be understated: {}",
            h2.quantile_us(1.0)
        );
    }

    #[test]
    fn the_ring_is_bounded_and_keeps_the_newest() {
        let t = Telemetry::default();
        for i in 0..RING_CAPACITY + 500 {
            t.log("INFO", "test", format!("message {i}"), Some(i as u64));
        }
        let records = t.records(RING_CAPACITY * 2);
        assert_eq!(records.len(), RING_CAPACITY, "the ring is not bounded");
        assert_eq!(
            records[0].message,
            format!("message {}", RING_CAPACITY + 499),
            "records() must be newest first"
        );
        assert_eq!(records[0].seq, Some((RING_CAPACITY + 499) as u64));
    }

    #[test]
    fn the_metrics_body_is_shaped_like_otlp() {
        // Not a schema validation — a guard that the field names stay the ones a collector reads.
        // Getting `aggregationTemporality` or `asInt`-as-a-string wrong produces a body that is
        // accepted and silently dropped, which is the failure mode worth a test.
        let t = Telemetry::default();
        t.events_appended.add(7);
        t.fold.record_us(1_500);
        let body = t.otlp_metrics("todo");

        let metrics = &body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
        let by_name = |n: &str| {
            metrics
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["name"] == n)
                .unwrap_or_else(|| panic!("no metric {n}"))
                .clone()
        };

        let appended = by_name("beck.events.appended");
        assert_eq!(
            appended["sum"]["dataPoints"][0]["asInt"], "7",
            "int64 fields are JSON strings"
        );
        assert_eq!(appended["sum"]["aggregationTemporality"], 2, "CUMULATIVE");
        assert_eq!(appended["sum"]["isMonotonic"], true);

        let fold = by_name("beck.fold.duration");
        let point = &fold["histogram"]["dataPoints"][0];
        assert_eq!(point["count"], "1");
        assert_eq!(point["sum"], 1500.0);
        assert_eq!(
            point["bucketCounts"].as_array().unwrap().len(),
            point["explicitBounds"].as_array().unwrap().len() + 1,
            "OTLP requires exactly one more bucket count than bound"
        );

        assert_eq!(
            body["resourceMetrics"][0]["resource"]["attributes"][0]["value"]["stringValue"],
            "todo"
        );

        // A cumulative point's window must not run backwards. It did, on the first export, because
        // the start was initialised lazily *after* `now` was read — and the failure mode is a
        // collector silently dropping the point.
        for m in metrics.as_array().unwrap() {
            let point = m
                .get("sum")
                .or_else(|| m.get("histogram"))
                .map(|k| &k["dataPoints"][0]);
            if let Some(p) = point {
                let start: u64 = p["startTimeUnixNano"].as_str().unwrap().parse().unwrap();
                let now: u64 = p["timeUnixNano"].as_str().unwrap().parse().unwrap();
                assert!(
                    start <= now,
                    "{} reports a window ending before it began",
                    m["name"]
                );
            }
        }
    }

    #[test]
    fn a_log_record_carries_the_sequence_number_it_is_about() {
        // The claim this module rests on: correlation is `seq`, so a record in any backend points
        // at a state `beck replay --to` can reproduce.
        let t = Telemetry::default();
        t.log("ERROR", "beck_rt::app", "append failed".into(), Some(41));
        t.log("INFO", "beck_rt::http", "listening".into(), None);

        let body = t.otlp_logs("todo", 10);
        let records = body["resourceLogs"][0]["scopeLogs"][0]["logRecords"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(records.len(), 2);

        let failure = records
            .iter()
            .find(|r| r["severityText"] == "ERROR")
            .unwrap();
        assert_eq!(failure["severityNumber"], 17);
        assert_eq!(failure["body"]["stringValue"], "append failed");
        let seq = failure["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["key"] == "beck.seq")
            .expect("an error about an append must say which one");
        assert_eq!(seq["value"]["intValue"], "41");

        // …and a record with no sequence number does not invent one.
        let listening = records
            .iter()
            .find(|r| r["severityText"] == "INFO")
            .unwrap();
        assert!(
            !listening["attributes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["key"] == "beck.seq"),
            "a record about no particular state must not claim one"
        );
    }
}
