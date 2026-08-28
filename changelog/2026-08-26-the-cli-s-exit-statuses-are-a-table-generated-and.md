- **2026-08-26 — The CLI's exit statuses are a table, generated and gated.**
  [`docs/35`](../docs/35-standards-landscape.md) §35.2's POSIX row,
  [`docs/08`](../docs/08-roadmap.md) §8.5.4's standards ledger,
  [`docs/reference/cli.md`](../docs/reference/cli.md). Four statuses — `0` did what was asked, `1` ran
  and the answer is no, `2` the invocation was wrong, `101` a panic and therefore a bug in this
  binary — written once as a constant in `docs.rs` and generated into the command reference, where
  a person looking for the contract will find it. Four and not more on purpose: the distinction a
  script makes is "worked", "worked and said no", "called it wrong", and splitting the middle one
  between a file that could not be read and a program that does not compile would be inventing a
  contract nobody asked for. The gate reads the **published** table and drives the binary against
  it in both directions — a status no row names fails, and a row nothing produces fails — so a
  `clap` upgrade that moves a usage error off `2` breaks here rather than in somebody's CI. `101`
  is excused from the second half by name rather than by silence, since a deliberate panic is not
  something a test suite should arrange. The row in §35.2 said the table lived "in the diagnostic
  snapshots" and now names where it is.
