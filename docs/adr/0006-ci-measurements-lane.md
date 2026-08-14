# 0006 — A release-profile measurements lane in CI

**Context.** The numbers in docs/20, docs/23 and docs/23 come from release-profile suites
(`measure_phase2`, `measure_incremental`, `shared_arrangements` with `--nocapture`). CI ran them
only in debug inside the full suite, tables swallowed — the published numbers were reproducible
only on the machine that wrote them.

**Decision.** A `measurements` job runs the three quoted commands as CI's reported lane —
printed, never thresholded (§13.7). The debug runs stay in the gate as correctness tests.

**Consequences.** Every run republishes the tables. No timing gate exists or is implied.
