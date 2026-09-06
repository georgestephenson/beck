- **2026-08-19 — `h2` taken to 0.4.16, which is what RUSTSEC-2026-0258 asks for.**
  `cargo deny`'s advisories check went red on `h2 0.4.15` — unbounded empty DATA frames, reachable
  through `hyper` — in the `licences` job, with `bans`, `licenses` and `sources` all still green. The
  advisory is new rather than newly noticed: nothing in this repository moved, and the same lockfile
  fails on any branch that has it. The fix is the one the advisory names, `cargo update -p h2`, and
  it is taken rather than muted: [`deny.toml`](../compiler/deny.toml)'s `ignore` list is empty on
  purpose and says why — an advisory is fixed at the root, not silenced beside it.
