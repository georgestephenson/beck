- **2026-08-16 · #67 — A `parallel:` child blocked in an outbound call is stopped inside the
  call.** Cancellation rode the evaluator's step counter, and a child blocked in a socket takes no
  steps — so a scope whose first child failed waited out a sibling's ten-second timeout.
  `beck_core::net::Stop` is the deadline [`docs/80`](../docs/80-structured-concurrency-report.md)
  §80.12 said belonged on the seam: the same question `burn` asks, as a predicate
  `Outbound::fetch` takes as a parameter (not a defaulted second method — an implementation that
  ignored it would be a gate that cannot fail) and the real client polls every 5 ms while an
  exchange is watched. `Stop::never()` keeps the unwatched path, which is every call outside a
  scope. Gated by a counter rather than a clock — the host says whether the scope reached it or it
  hit its own backstop (`concurrency.rs`) — and over a real socket that accepts and says nothing
  (`outbound.rs`). §80.14 is the section; the compiled half is still open, because a worker holds
  its pipe for a whole call ([`docs/93`](../docs/93-the-native-backends-report.md) §93.15).
