- **2026-08-16 — The host half of the native protocol becomes one definition, and three sweeps of
  dead code and stale links.** A codebase audit against the standards
  [`AGENTS.md`](../AGENTS.md) already sets. `beck-clif` and `beck-llvm` each carried their own
  `Artifact::exchange` — 65 lines, byte-identical but for the comments — encoding arguments,
  decoding a trap, decoding a raise payload and reading the arena back. That is host code, so the
  argument that keeps the two *emitters* apart (a shared selection would make `cranelift.rs`'s
  agreement gate true by construction and therefore worth nothing) never reached it; `beck-clif`'s
  own manifest says the worker protocol is "one definition, not two", and `service.rs`'s header
  already claimed both backends call it for the host. Now they do:
  `beck_llvm::service::exchange` is the other direction of the module that holds
  `service::answer`, and a new trap code has one place to be forgotten rather than two. The same
  shape held for the two WebAssembly modules, whose buffer table and length-prefixed frame — a
  contract with two *pages*, `playground.js` and `beck-mode-b.js` — was written twice; it is now
  `beck-frame`, with the exports left in the modules that answer them so `playground.rs` and
  `mode_b.rs` keep counting each crate's `forbid(unsafe_code)` exception locally.
  `docs.rs::a_relative_link_out_of_a_rustdoc_page_lands_on_the_file_it_names` was found to skip
  exactly the files that had broken links: it filters to `src/` and to targets containing `docs/`,
  and under that scope all 304 links resolve, while the 150 in the harnesses were checked by
  nothing and **eleven named a file that does not exist**. That is
  [`82`](../docs/82-the-edge-report.md) §82.10's pattern again — a gate written to the shape of the
  fix, its scope frozen where the fix was. `a_relative_link_in_a_harness_lands_on_the_file_it_names`
  is the second rule, counted from the file rather than from a rendered page because nothing
  renders a harness; it was confirmed red on all eleven before they were corrected. Seventeen
  `pub fn`s that nothing referenced are gone, which is dead code rather than API because every
  crate is `publish = false` — two of them documented callers that do not exist
  (`parse_expr_str` "used by `beck ast` and by tests", `Types::rows_equal` "used … `.becki`
  agreement and `--wire-compat`") and one, `Artifact::codegen_time`, was exported so §7.3's
  compile-time claim could be checked by something, and was checked by nothing.
  `cargo clippy --workspace --all-targets` and `cargo fmt --all --check` were clean before this
  change and are clean after it; `cargo test --workspace` is 1345 tests over 102 suites.
