# 43 — The threat model

> **The question this answers**: when Beck says a property is secure, secure *against whom*?

[`42`](42-security-assurance.md) §42.10 says a project is "watertight" only if it has three things,
and names the one Beck was missing first: "a **threat model** that says what is in scope and what is
not". This document is that. It is a **charter**, not a survey and not a report: it states the
current position, it is edited when the position changes, and everything in it is meant to be true
of the tree today.

It is deliberately short. A threat model that enumerates attacks is a document that goes stale; one
that names *adversaries* and *what each is assumed to be able to do* stays useful, because a new
attack is then a question with an answer rather than a missing row.

## 43.1 The four adversaries

Beck defends against four, in the order they will actually be met.

| | Adversary | Can | Cannot |
|---|---|---|---|
| **A1** | **An anonymous author of source** — the playground submitter, the author of a package, the contributor of a pull request | Choose the whole input to the compiler: any bytes, any nesting, any macro, any dependency graph | Choose the compiler's flags, reach the filesystem or network through a macro, or observe another submission |
| **A2** | **An anonymous client of a running application** — a browser, a script, anything that opens a socket | Send any bytes on the wire, claim any identity (today — §43.4), open connections, propose any command | Write to the log except through `validate`, read a view they did not subscribe to, or observe a `secret[T]` |
| **A3** | **A hostile dependency** — a crate in the compiler's graph, a tarn in a program's | Run arbitrary code at *its* build time and inside its own functions | Run code at install time (there are no install hooks — [`16`](16-packages-and-ecosystem.md)), or hide an effect from the effect row a caller sees |
| **A4** | **An operator reading a dashboard** — the one adversary who is not hostile, and is here because they are a *target* | — | — (they are attacked *through* content the other three supply) |

Two are live today and two are anticipated. A1 and A2 can be met by anybody, right now, by running
`beck` on a file or pointing a socket at `beck run`. A3 becomes real when the registry exists
(Phase 4); A4 becomes real when anything is deployed.

## 43.2 What is defended, and by what kind of evidence

The three kinds are [`42`](42-security-assurance.md) §42.1's, and the distinction is the point:
**structural** follows from a construction and no reviewer has to remember it, **tested** means a
harness goes red, **absent** means it is not there and §43.4 says so by name.

| Claim | Against | Kind | Where |
|---|---|---|---|
| A `secret[T]` cannot reach a client partition | A2 | structural + tested | The placement solver; `security.rs`, one test per §3.5 row |
| `durable` and `ingress` are never placed client-side | A2 | structural + tested | Same |
| A fold is deterministic, so replay is exact | A2, A4 | structural + tested | §3.7's rule in the checker; the replay-determinism harness |
| Only validated events are logged | A2 | structural | F1/F3's rule: a command envelope is transient |
| No memory-unsafety in first-party code | all | structural | `unsafe_code = "forbid"`, inherited by all nine crates |
| No SQL injection | A2 | structural | Every value is a bind parameter |
| No hash flooding | A2 | structural | `Map`/`Set` are ordered, not hashed (F8) |
| Deeply nested source is refused, not fatal | **A1** | tested | [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md); `front_end_bound.rs` |
| A macro cannot read the disk or the network | A1, A3 | structural | The expander has no I/O to offer |
| Time enters at the merge point and nowhere else | A2, A4 | tested | `beck_core::clock`; `clock.rs`'s one-reader gate |
| Text in a page is escaped, in both text and attribute context | A2, A4 | tested | `beck-core/src/html.rs`; the dashboard's own escaper |

## 43.3 What is explicitly **not** defended

Stated positively, because an unstated exclusion reads as an oversight and a stated one is a
decision.

1. **Side channels.** Timing, cache, speculation. Nothing in Beck's design attempts constant-time
   anything, and `secret[T]` is a *flow* property — where a value may travel — not a claim about
   what its use costs to observe.
2. **A compromised host.** If the process's memory or its filesystem is under someone else's
   control, no property here survives, and none is claimed to.
3. **An authenticated insider.** Until identity lands (§43.4), there is no authentication to be
   inside of; afterwards, an actor acting within its capabilities is the system working.
4. **The dependency graph's own `unsafe`.** `forbid(unsafe)` is first-party only. tokio, hyper,
   redb and tungstenite are outside it, and the answer for them is pinning, an advisory gate with an
   empty ignore list, and upgrade discipline — not an absence of unsafe code.
5. **A program's semantics.** Beck contains flows; it does not know that a program's rules are the
   rules its author meant. "Only the owner may toggle their todo" is a program's claim, and Beck's
   contribution is that the claim is *checkable* and enforced identically on every tier — not that
   it is right.
6. **Availability of a service somebody runs.** Quotas, connection limits and admission control are
   §43.4's absent list. An operator who deploys Beck today and is knocked over by traffic has met a
   documented gap, not a vulnerability.
7. **The compiler's own correctness as a security property.** Beck is not a verified compiler and
   never claims a CompCert-style theorem. Miscompilation is a bug; it is not in this model.

## 43.4 What is absent, and how you can tell

The controls a reader would reasonably assume exist, that do not:

- **Identity.** The actor arrives in the client's own `hello` frame and is believed. Every
  ownership check in every corpus program is therefore enforced against a value the caller chose.
  This is Phase 3's identity bullet and is correctly sequenced, but it is *absent*, and the
  distinction between it and §3.5's proven properties is the most likely misquotation of this
  project's security story ([`42`](42-security-assurance.md) §42.5).
- **Per-actor quotas** (F3, `APPROVED` and unbuilt), **subscription and connection quotas** (F15),
  and **the deploy choreography's bounded buffer** (F12).
- **Message size limits and origin checks** on the websocket: the limits are the library's
  defaults, and the upgrade never inspects `Origin`.
- **Macro fuel** (F17). Expansion is bounded in *depth* — twice over, since
  [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md) separated the two counters that
  had been one — and not in *work*.

None of these is a secret, and none of them is prose only:
`compiler/crates/beck-cli/tests/pending_security.rs` asserts each one **as an absence**, in the
pattern `sicp/refusals/` used for expressiveness. The day one is built, its test goes red, and
whoever built it has to correct this section in the same change. That is the mechanism this
document depends on; without it, §43.4 would be exactly the kind of list that is accurate on the
day it is written and quietly wrong six months later.

## 43.5 The trust boundaries, drawn once

```text
   A1 source ─────────▶│ beck check / build │─────▶ artefacts ─────▶│ registry │──▶ A3
                       │  ← bounded (0012)  │      (signing, SBOM: unbuilt — 28)
                       └────────────────────┘
   A2 client ──socket──▶│ ingress │──▶ validate ──▶ log ──▶ fold ──▶ view ──▶ patch ──▶ A2
                        └─ actor is self-asserted (§43.4)
                                   └─ the only writer; the only place time enters (§3.7)
                                                              └─ secret[T] cannot cross (§3.5)
   A4 operator ◀── dashboard ◀── telemetry ◀── (content originating with A2)
```

Four crossings, and what each one is:

1. **Source into the compiler** — the boundary A1 attacks. Bounded in nesting; not bounded in
   macro work.
2. **The wire into the sequencer** — the boundary A2 attacks. Typed and decoded before anything
   else happens; unauthenticated, unquota'd.
3. **The fold into a view, and a view into a patch** — where §3.5's placement properties do their
   work, and the crossing this project has the most evidence about.
4. **Content into an operator's screen** — the one A4 is attacked through, and the reason the
   dashboard escapes in attribute context as well as text.

## 43.6 What would change this document

A threat model that never changes is not being read. These are the events that require an edit,
named so the edit is not left to somebody noticing:

| Event | What has to change here |
|---|---|
| Identity lands | §43.4's first bullet moves to §43.2; A2 splits into authenticated and anonymous |
| The playground ships | A1 stops being hypothetical; the isolation story (§17.3) enters §43.2 |
| The registry ships | A3 stops being anticipated; tarn signing and effect diffs enter §43.2 |
| Any quota is built | §43.4 loses a bullet, `pending_security.rs` loses a test, both in one change |
| A second writer exists | The single-writer row in §43.2 stops being structural, and everything resting on it needs re-argued |

## 43.7 What this document is not

It is not an assessment: [`42`](42-security-assurance.md) is, it is dated August 2026, and it
carries the measurements. It is not a conformance claim against a framework — §12.1's rule applies,
and a claim enters the project as an executable artefact rather than as a paragraph. It contains no
penetration test, no cryptographic review and no assessment of the dependency graph's own `unsafe`.
And it says nothing about *residual risk* in the quantitative sense, because a project that has not
deployed anything has no incident history to compute one from.
