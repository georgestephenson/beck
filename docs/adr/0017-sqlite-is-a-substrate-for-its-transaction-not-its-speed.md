# ADR 0017 — SQLite is a substrate for its transaction, not its speed

**Status:** accepted
**Date:** 2026-08-04
**Context:** [`67`](../67-sqlite-report.md), [`07`](../07-dependencies.md) §7.8.1,
[`08`](../08-roadmap.md)

## The decision

`rusqlite`, with the **`bundled`** feature, is added to `beck-rt` as a third durable `LogStore`.

`Durability` is a **public enum on the store**, not a pragma inside it, and its default is
`Fsync` — the same promise redb and Postgres make.

## Why the dependency is taken

[`07`](../07-dependencies.md) §7.8.1 already argued it and the measurement did not change the
argument: **SQLite is also a read-model engine.** "Append and project in one transaction" is what
Postgres gives production, and redb cannot offer it at any speed because it has no query language
for a projection to be written in. That makes rungs 0–2 the same *shape* as production rather than
merely similar, which is the sentence §7.8.1 ends on.

Licences are inside [`0004`](0004-full-cargo-deny-gate.md)'s allowlist: `rusqlite` MIT,
`libsqlite3-sys` MIT, and SQLite itself public domain.

## Why `bundled`

SQLite is compiled from the vendored amalgamation rather than linked against whatever the host has.
A log written on a laptop and a log written in CI are then written by the same engine, at the same
version — which for an append-only history that [`03`](../03-type-and-effect-system.md) §3.7 makes
the only description of a program is worth a C build step.

The cost is real and is named: a C compiler becomes a build requirement for `beck-rt`, and this is
the first one. `libsqlite3-sys` **0.38 does not build** on the pinned toolchain (it uses
`cfg_select!`, still unstable), so the version is pinned at `rusqlite 0.37` — a constraint to
remember at the next toolchain bump rather than a defect.

## Why `Durability` is a type and not a pragma

This is the part the measurement forced, and it is the whole reason this record exists.

The first version set `synchronous = NORMAL` — WAL's commonly recommended pairing — and `beck bench
log` reported SQLite at **26× redb**. That number is real and it is not about SQLite: at `NORMAL`,
a commit can be lost in a power loss, so the comparison was measuring a weaker promise rather than
a faster engine. At `FULL`, the two are within noise of each other on a shared runner
([`67`](../67-sqlite-report.md) §67.3).

§3.7 makes the log the only description of a program's history. "An acknowledged event may vanish"
is therefore a change to what the system *means*, not a tuning knob — so it is:

- **named** — `Durability::Fsync` / `Durability::Relaxed`, in the public API;
- **defaulted to the strong option**, because a weaker promise arrived at by not reading a manual
  is not a decision anybody made;
- **visible in `kind()`** — the relaxed store reports `sqlite-relaxed`, so no measurement can label
  the two the same;
- **printed both ways** by `beck bench log`, so the flattering number is never the only one on
  screen.

## What would reverse it

- **A read model actually being built on it.** Nothing in Beck projects into SQL yet
  ([`26`](../26-arrangement-sharing-report.md) §26.9), so the property this dependency was taken for
  is *available* and unused. If the read-model work lands on something else, this becomes a third
  substrate justified only by taste.
- **`libsqlite3-sys` requiring a toolchain we do not pin.** It already does at 0.38.
- **A system-SQLite requirement.** If `bundled` ever conflicts with a packaging constraint, the
  choice between "the same engine everywhere" and "no C build step" gets made again.
