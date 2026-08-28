- **2026-08-21 — Two gates that were failing on the machine rather than on the program.**
  Both were red on this branch, neither on anything the branch changed, and CI's own record is what
  says so: on one commit the `measurements` job passed `measure_native` under `--release` while the
  `check` job failed it under `cargo test`, **same commit, same runner class**.
  **A measurement suite is a release suite, and `--all-targets` had been defeating that.**
  [`AGENTS.md`](../AGENTS.md) says so and `.github/workflows/compiler.yml` says it again in a comment —
  "it runs here rather than under `cargo test` because it is a release measurement" — but
  `cargo test --workspace --all-targets` builds and runs the suite too, in debug, where the harness
  around each native call (marshalling an 8,000-element list, cloning it per run) costs several
  times the generated code it is timing. Measured four times at each profile, `doubled`'s per-element
  ratio across the two sizes ran **0.85–2.41 in debug against 1.13–1.58 in release**, and CI read
  3.24 — past a bound of 3.0 chosen to sit between a flat append (1×) and a copying one (4×). All
  ten of that file's wall-clock thresholds now go through `shape!`, which asserts where the clock is
  evidence and prints one skip line where it is not; `BECK_GATE_DEBUG_TIMING=1` asserts anyway.
  Release still runs every one of them — tightening the bound to 1.0 turns it red, so the gate still
  bites where it means something. `measure_native` was the only suite carrying that class of
  threshold; `measure_concurrency`'s one bound is 200 ms against 300 ms testing parallel-against-
  serial, a gap a profile does not close.
  **And the browser suite's launch deadline was a gate on the runner.** Every test launches its own
  chromium and the harness runs them in parallel, so on a loaded runner the first launch waits
  behind the rest; at 30 seconds that lost a run whose other twenty tests all passed. It is 120
  seconds now — the bound exists so a dead browser fails instead of hanging, so being generous
  costs nothing — and the message says which case it was, because "chromium never opened a port"
  could not tell a crash from a slow start. It now reports the child's exit status, or that it was
  still running.
  **Left undone, deliberately**: the debug sweep still *runs* those suites (~3.5 minutes, of which
  `measure_awfy` is 134 s) and prints ratios that are not evidence. Suppressing that means changing
  which targets the workflow's test step builds, which is wider than this fix.
