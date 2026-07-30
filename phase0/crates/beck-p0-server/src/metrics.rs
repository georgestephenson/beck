//! Metrics — exported from day one, not retrofitted.
//!
//! §5.3 names the ones that matter and why: "subscription count, shared-prefix hit rate, and
//! per-session memory are metrics the runtime exports from day one, because this is where a naive
//! implementation quietly becomes Meteor-at-scale." Phase 0 has no shared prefixes yet (there is
//! no dataflow engine until Phase 3), so it exports the rest — and the numbers below are what the
//! Phase 0 report is written from.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Metrics {
    pub recovery_millis: AtomicU64,
    pub recovered_to: AtomicU64,

    pub commands_in: AtomicU64,
    pub commands_rejected: AtomicU64,
    pub commands_deduped: AtomicU64,

    pub batches: AtomicU64,
    pub batched_commands: AtomicU64,
    pub events_committed: AtomicU64,
    pub snapshots: AtomicU64,

    pub subscriptions: AtomicU64,
    pub subscriptions_total: AtomicU64,

    pub patches_sent: AtomicU64,
    pub patch_ops: AtomicU64,
    pub patch_bytes: AtomicU64,
    /// Commands that changed nothing this subscriber can see. Worth watching: a high ratio means
    /// clients are being told about work that does not concern them.
    pub up_to_date_notices: AtomicU64,

    pub resumptions_fresh: AtomicU64,
    pub resumptions_resumed: AtomicU64,
    pub resumptions_reset: AtomicU64,
    pub resume_replay_micros: AtomicU64,

    pub ssr_renders: AtomicU64,
    pub ssr_bytes: AtomicU64,
}

impl Metrics {
    pub fn subscription_opened(&self) {
        self.subscriptions.fetch_add(1, Ordering::Relaxed);
        self.subscriptions_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn subscription_closed(&self) {
        self.subscriptions.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn up_to_date_sent(&self) {
        self.up_to_date_notices.fetch_add(1, Ordering::Relaxed);
    }

    pub fn patch_sent(&self, ops: usize, bytes: usize) {
        self.patches_sent.fetch_add(1, Ordering::Relaxed);
        self.patch_ops.fetch_add(ops as u64, Ordering::Relaxed);
        self.patch_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Prometheus text exposition (§7.3).
    pub fn render(&self, store_kind: &str, head: u64) -> String {
        let g = |m: &AtomicU64| m.load(Ordering::Relaxed);
        let mut out = String::with_capacity(2048);
        let mut metric = |name: &str, help: &str, kind: &str, value: u64| {
            out.push_str(&format!(
                "# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}\n"
            ));
        };

        metric("beck_log_head", "highest assigned seq", "gauge", head);
        metric(
            "beck_recovery_millis",
            "time spent folding the log at startup",
            "gauge",
            g(&self.recovery_millis),
        );
        metric(
            "beck_recovered_to_seq",
            "seq the fold recovered to at startup",
            "gauge",
            g(&self.recovered_to),
        );
        metric(
            "beck_commands_total",
            "commands accepted at ingress",
            "counter",
            g(&self.commands_in),
        );
        metric(
            "beck_commands_rejected_total",
            "commands validate refused (never logged)",
            "counter",
            g(&self.commands_rejected),
        );
        metric(
            "beck_commands_deduped_total",
            "commands recognised as retries by envelope identity",
            "counter",
            g(&self.commands_deduped),
        );
        metric(
            "beck_batches_total",
            "group commits",
            "counter",
            g(&self.batches),
        );
        metric(
            "beck_batched_commands_total",
            "commands included in group commits",
            "counter",
            g(&self.batched_commands),
        );
        metric(
            "beck_events_total",
            "events appended to the log",
            "counter",
            g(&self.events_committed),
        );
        metric(
            "beck_snapshots_total",
            "fold snapshots written",
            "counter",
            g(&self.snapshots),
        );
        metric(
            "beck_subscriptions",
            "live subscriptions (the fanout number)",
            "gauge",
            g(&self.subscriptions),
        );
        metric(
            "beck_subscriptions_total",
            "subscriptions established since start",
            "counter",
            g(&self.subscriptions_total),
        );
        metric(
            "beck_patches_total",
            "patch frames sent",
            "counter",
            g(&self.patches_sent),
        );
        metric(
            "beck_patch_ops_total",
            "patch operations sent",
            "counter",
            g(&self.patch_ops),
        );
        metric(
            "beck_patch_bytes_total",
            "patch bytes sent",
            "counter",
            g(&self.patch_bytes),
        );
        metric(
            "beck_up_to_date_notices_total",
            "commands that changed nothing in the sender's own view",
            "counter",
            g(&self.up_to_date_notices),
        );
        metric(
            "beck_resumptions_fresh_total",
            "subscriptions that started from nothing",
            "counter",
            g(&self.resumptions_fresh),
        );
        metric(
            "beck_resumptions_resumed_total",
            "subscriptions that replayed a gap by (subscription, seq)",
            "counter",
            g(&self.resumptions_resumed),
        );
        metric(
            "beck_resumptions_reset_total",
            "subscriptions that could not resume and were reset",
            "counter",
            g(&self.resumptions_reset),
        );
        metric(
            "beck_resume_replay_micros_total",
            "time spent folding the log to reconstruct resuming subscribers' views",
            "counter",
            g(&self.resume_replay_micros),
        );
        metric(
            "beck_ssr_renders_total",
            "server-side renders (first paint)",
            "counter",
            g(&self.ssr_renders),
        );
        metric(
            "beck_ssr_bytes_total",
            "bytes of server-side rendered HTML",
            "counter",
            g(&self.ssr_bytes),
        );

        out.push_str(&format!(
            "# HELP beck_store_info the durable substrate in use\n\
             # TYPE beck_store_info gauge\n\
             beck_store_info{{kind=\"{store_kind}\"}} 1\n"
        ));

        // Resident set size, straight from the kernel. The per-idle-session memory exit criterion
        // is (RSS with N subscribers − RSS with none) / N, and it is the number that decides
        // whether this architecture survives, so the runtime reports it itself rather than leaving
        // it to a benchmark harness's arithmetic.
        if let Some(rss) = resident_bytes() {
            out.push_str(
                "# HELP beck_process_resident_bytes resident set size\n\
                 # TYPE beck_process_resident_bytes gauge\n",
            );
            out.push_str(&format!("beck_process_resident_bytes {rss}\n"));
        }
        out
    }
}

/// RSS in bytes from `/proc/self/statm`, which is Linux-only and precisely where this runs.
pub fn resident_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}
