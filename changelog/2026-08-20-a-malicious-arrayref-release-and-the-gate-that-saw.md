- **2026-08-20 — A malicious `arrayref` release, and the gate that saw the pin for it expire.**
  The `licences` job went red on a yank rather than an advisory: crates.io withdrew `arrayref 0.3.9`,
  which reaches this tree through `blake3`. **The fix `cargo-deny` suggests was the attack.**
  `cargo update -p arrayref` resolved to `0.3.10`, which added a normal dependency on
  **`proc-macro1`** — one character from `proc-macro2`, with exactly two published versions (1.0.106
  and 1.0.107, `proc-macro2`'s own latest two), copying its feature set and its single normal
  dependency, and declaring `base64`, `rustls` and **`ureq` as build dependencies**: an HTTP client
  and a TLS stack running inside a build script at compile time. `arrayref` is two hundred lines of
  macros for taking a reference to a sub-array; 0.3.9 has no runtime dependencies at all.
  What made it visible was the disproportion — that one-crate update produced a lock file **283 lines
  larger**, pulling in `ureq`, `url`, `webpki-roots` and the whole ICU stack. Nothing was built with
  it and `arrayref-0.3.10.crate` was never downloaded; the version facts were read from the registry
  index.
  So `arrayref@0.3.9` was held in `deny.toml`'s `advisories.ignore` — the yanked version being the
  safe one and the current one the attack. **crates.io removed 0.3.10 and un-yanked 0.3.9 hours
  later**, so the list is empty again and the lock file never moved: it holds the same `0.3.9` it
  held before any of this.
  **What is left is the gate.** `unused-ignored-advisory = "deny"` was added alongside the pin
  because `cargo-deny` reports `yanked-not-detected` only as a warning, which no CI job reads. It
  failed the build the moment the pin stopped being needed — which is how the entry came out the same
  day rather than sitting there until the next yank of the same crate was waved through by a
  permission granted for something else. A gate whose first firing is a true positive, on its first
  day, is the argument for writing it at the same time as the thing it guards.
