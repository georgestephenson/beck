- **2026-08-14 · #56 — A reset connection no longer ends an image build.** `beck-cli/src/fetch.rs`
  attempts a hop up to four times and classifies rather than reports: transient failures are
  retried, permanent ones answered once, and a truncated reply is distinguished from an oversize
  one ([`docs/92`](../docs/92-supply-chain-and-release-report.md) §92.13). The gates drive the retry
  loop itself, with no network.
