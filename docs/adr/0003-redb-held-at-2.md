# 0003 — redb held at major version 2

**Context.** redb 4 is current; the workspace pins 2. A redb file is a durable log substrate
(`beck-rt/src/log.rs`), so a major bump is a question about existing logs, not a version edit.
The format stamp guards the *content* encoding, not the container format.

**Decision.** Stay on 2 until the upgrade comes with an answer for on-disk compatibility —
whether newer redb opens v2 files, and if not, a migration through replay.

**Consequences.** A known-stale dependency, deliberately. Supersede this record when the
migration story exists.
