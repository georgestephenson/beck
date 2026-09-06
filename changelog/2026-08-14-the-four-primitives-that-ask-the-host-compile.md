- **2026-08-14 · #55 — The four primitives that ask the host compile** — `now()`, `uuid()`,
  `secret_env`, `http_fetch` — via a second direction in the worker's protocol: a compiled call
  writes a question frame and blocks for the answer
  ([`docs/93`](../docs/93-the-native-backends-report.md)). The host is one description,
  `beck_core::host::Atoms`, asked by all three backends. 870 → 889; refusals 208 → 189. Gated by
  `native.rs::the_two_backends_agree_on_the_host_effects` and its Cranelift twin.
