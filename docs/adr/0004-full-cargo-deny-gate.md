# 0004 — The full cargo-deny check gates the compiler workspace

**Context.** `deny.toml` carries licence *and* advisory policy, but the compiler workflow ran
`check licenses bans sources` from its first commit — the advisory half was written and never
executed. phase0's workflow ran the full check all along.

**Decision.** `command: check` — advisories included. The advisory database is external and
mutable, so a new advisory can red-build an unchanged PR; that is accepted, and the answer is
the root fix `deny.toml` already mandates (first instance: ADR-0002).

**Consequences.** Both workspaces hold the same supply-chain bar. cargo-deny jobs install no
Rust toolchain — the action ships its own binary.
