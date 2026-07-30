# 14 — Final review pass: findings

An adversarial pass over `docs/00`–`13` for security flaws, weaknesses, bugs, and mistakes.
Findings are ranked. **Status** per finding: `FIXED` (doc corrected in this pass), `DESIGNED`
(resolution specified here, to be built as specified), or `DECIDE` (needs George).

## Critical — design-level

### F1 — Crypto-shredding contradicts genesis replay (D3 × D4 collision) — `APPROVED` (structural shredding)

D3 promises *replay from the first event reproduces state bit-for-bit*; D4 promises *deleting a
subject's key makes their events permanently unreadable*. Both cannot hold naively: shred a
subject's `Added` events and a genesis replay computes a different `remaining` than the live state
ever showed. Worse, erasure as currently written doesn't reach **derived data**: read models,
search indexes, snapshots taken pre-shred, patch streams, OTel traces, and backups all still hold
plaintext derived from the shredded events.

**Resolution (approved by George)**: shredding is *structural, not total* — the envelope
skeleton (seq, type tag, entity ids) stays readable forever; only the **payload fields** are
per-subject encrypted and shreddable. Folds then apply a typed *tombstone* deterministically, and
the invariant is restated honestly: *genesis replay reproduces the post-erasure state* — which is
the state the system is legally required to be in. Erasure becomes a first-class operation, not a
key deletion: delete key → re-snapshot affected stores → rebuild affected read models/indexes →
expire pre-shred snapshots per backup retention. Telemetry never contains payloads at all (F10),
which removes the worst channel outright. The genesis-replay CI gate runs against a
tombstone-containing corpus so the semantics stay tested.

### F2 — Client-minted ids allow overwrite and IDOR as the sketch is written — `FIXED` (docs), `DESIGNED` (language)

The canonical example's `validate` accepts `Add(id, …)` without checking id freshness and
`Toggle/Delete(id)` without checking ownership. As literal semantics: any client can overwrite any
todo by resubmitting an existing id (the fold's `set` clobbers), and mutate anyone's rows. The
sketch is deliberately auth-free — fine for a shared demo list, wrong as the pattern people copy,
and it undercut our own CWE-639 claim.

**Fix applied** ([`03`](03-type-and-effect-system.md) §3.7): client-minted ids are accepted only if
*fresh* (first-writer-wins insert; colliding `Add` rejected, never applied), and entity references
check ownership against `actor`. **Language design consequence**: `insert`-shaped fold operations
get first-writer semantics as the primitive, and the `requires owns(ref)` capability pattern is the
documented default for mutating commands — so the secure form is the path of least resistance.

### F3 — Events-forever default turns attacker traffic into permanent storage — `FIXED` + quotas `APPROVED` (on by default)

With D3's `retain=forever`, anything that becomes an event is immortal. Two channels: (a) rejected
garbage — closed by the rule, now explicit in [`03`](03-type-and-effect-system.md) §3.7, that **only
validated events are durably logged** (command envelopes are transient, kept briefly for
idempotency only); (b) *validated* spam from a legitimate but abusive session — permanent by
design. Remediation for (b): per-actor rate/volume quotas enforced at `validate` (a stdlib
combinator, on by default with generous limits), and per-actor crypto-shredding as the abuse
cleanup path. **Decided**: quotas are **on by default** with generous limits, overridable per command type.

### F4 — `beck fork --from prod` is a data-exfiltration channel — `DESIGNED`

Fork-from-production and `fork(log=…)` test fixtures hand a developer the entire production event
log — the single most privacy-dense artefact the system produces; test fixtures risk prod logs
committed to repos. **Resolution**: fork is a privileged, audited operation (an RBAC verb in the
operator, logged like any admin action); the default fork is **redacted** — shredded subjects
excluded, `secret[T]` fields never serialized, optional deterministic pseudonymisation transform
for PII fields (typed, so the compiler knows which fields qualify); raw fork requires an explicit
elevated capability. CI fixtures use synthetic or redacted logs only, lint-enforced.

## High

### F5 — Envelope persisted the live `Session` capability — `FIXED`

`actor: Session` in the envelope would have persisted a live capability (and plausibly token
material) into an immutable log. Now `actor: ActorId` — a stable identity, with the rule stated:
capabilities and tokens are never persisted ([`03`](03-type-and-effect-system.md) §3.7).

### F6 — Poison events: a fold that panics is a deterministic crash loop — `APPROVED`

Determinism cuts both ways: if a deployed `apply_event` panics on event N (overflow, missed case
reaching via upcast, stdlib bug), every restart replays into the same crash — availability zero
until code changes. **Proposed semantics**: (a) prevention — the `partial` effect is *banned in
folds* like `time`/`rand` already are: no `unwrap`-shaped stdlib in fold position, checked
arithmetic, exhaustiveness already enforced; (b) containment — a panic that slips through (compiler
bug, FFI) halts *that store only*, wedging its subscribers at last-good `seq` while the rest of the
app serves; (c) recovery — the documented runbook is hotfix-the-fold + redeploy; replay heals state
because determinism means the fix recomputes correctly. Approved: (a)–(c) are the semantics; the constrained fold-position stdlib is accepted.

### F7 — `validate` returned at most one event; real commands need atomic batches — `FIXED`

`Option[Event]` cannot express PlaceOrder → OrderPlaced + StockReserved without two commands and a
consistency hole (and [`02`](02-syntax.md)'s `atomically:` example already assumed batches).
General form is now `list[Event]`, appended atomically — contiguous `seq`s, all-or-nothing, so no
fold observes half a command ([`03`](03-type-and-effect-system.md) §3.7).

### F8 — Determinism × HashDoS: the two requirements fight — `DESIGNED`

Bit-identical replay requires deterministic collection iteration; deterministic hashing is
attacker-exploitable via collision-flooding — and clients *mint the ids* used as keys (F2). Random
hash seeds (the usual HashDoS fix) break replay. **Resolution**: language-level `Map`/`Set` are
**ordered trees (B-tree), not hash maps** — deterministic by construction, no seed to attack,
`O(log n)` worst case an adversary cannot degrade. Hash maps remain an explicit opt-in
(`HashMap`, seeded per-store with a *persisted* random key so replay still holds) for hot paths
that need them.

### F9 — "Bit-for-bit floats across tiers" is undeliverable without owning libm — `DESIGNED`

IEEE 754 arithmetic is portable; **transcendentals are not** — `sin`/`cos`/`pow` differ across
libms, so a fold using them replays differently on WASM vs native, silently breaking the
determinism spine. **Resolution**: Beck ships its own deterministic math library (correctly-rounded
implementations, rlibm-lineage) compiled into every tier; FMA contraction and fast-math are
disabled in fold-reachable code; the cross-backend differential suite gains transcendental
torture vectors. Documented cost: fold-position math trades a little speed for replayability.

### F10 — Auto-instrumentation is a PII/secret exfiltration channel — `DESIGNED`

Spans "browser click → command → event → fold" must not carry payloads by default, or telemetry
becomes the unencrypted shadow of the log (and F1's worst leak). **Resolution**: telemetry
attributes are an allowlist derived from types — ids, seqs, sizes, durations, operation names;
never body fields. The type system enforces the sharp edge: `secret[T]` has no telemetry
serialization *at all*, and PII-marked fields require an explicit, greppable opt-in to appear in
attributes.

## Medium

### F11 — DST cannot be retrofitted — `FIXED` (constraint recorded)

The deterministic-simulation promise ([`13`](13-testing.md) §13.4) silently assumed a runtime that
can be simulated. FoundationDB's lesson: virtualize clock/network/disk **from the first line of
runtime code** (Phase 1), or DST never happens. Now stated as a hard prerequisite in 13.

### F12 — Deploy choreography buffers commands unboundedly — `DESIGNED`

"Commands buffer at the gateway" during quiesce ([`06`](06-kubernetes-and-packaging.md) §6.4) is an
OOM and timeout trap under long migrations. Resolution: bounded buffer with a declared budget, then
reject with `Retry-After`; Mode B clients queue locally (they already can, D7); the TLA+ spec of the
choreography ([`13`](13-testing.md) §13.5) models the bounded buffer, not the fiction.

### F13 — Generated NetworkPolicy example omitted DNS egress — `FIXED`

The classic generated-policy bug: deny-all egress without kube-dns breaks everything. Example
corrected; the platform layer adds infrastructural egress (DNS, telemetry) systematically
([`06`](06-kubernetes-and-packaging.md) §6.5).

### F14 — The flagship demo defaults to a round trip per click — `APPROVED` (demos run Mode B)

Mode A is the right *default*; it is the wrong *first impression* on a 100 ms RTT — reviewers will
click a todo and feel the network. The demo apps should mark their interactions `optimistic`
(Mode B) so the first-contact experience shows the latency-compensation story. One-line decision,
disproportionate marketing consequence.

### F15 — Subscription amplification and connection quotas — `DESIGNED`

A cheap client can open many subscriptions to expensive views. Per-session quotas (max
subscriptions, per-view cost budget from the solver's own estimates) and connection-rate limits at
the gateway, metrics exported — folded into R5's mitigation set.

## Low / notes

- **F16** Presence signals (D6) leak who-is-online; gate behind a capability like any other view.
- **F17** Compile-time macro fuel: typed-literal parsers and macro expansion get bounded
  fuel/timeouts so a malicious dependency cannot DoS the compiler (macro sandbox already blocks
  I/O).
- **F18** NATS/Synadia governance dispute (2025): the "verify licence at adoption" note applies to
  NATS JetStream specifically; it is post-1.0 and optional either way.
- **F19** Keycloak is JVM-heavy for the distroless ethos; fine as `managed()` default (it is its
  own pod), Ory Kratos remains the light alternative — no change, noted.
- Checked and found sound: cross-document references after the Beck rename (common-noun "tier"
  and "tierless" intact; transcript preserved); licence table consistency (no BUSL/SSPL
  dependencies); phase ordering (identity lands before the Phase 3 exit test that needs it);
  decision log D1–D13 against the documents that implement each; wire-id truncation (64-bit,
  collision-safe at realistic scale); `.becki` reviewability workflow; the D3/D9/D10 renames'
  ripple edits.

## What changed in this pass

Direct fixes: [`03`](03-type-and-effect-system.md) (ActorId envelope; validated-events-only
logging; `list[Event]` atomic batches; id-freshness/ownership obligations),
[`06`](06-kubernetes-and-packaging.md) (DNS egress), [`13`](13-testing.md) (DST prerequisite).
Everything `DESIGNED` above is specified here and binds the implementation. All four open
items (F1, F3, F6, F14) were approved by George as proposed — recorded as
[`10`](10-decisions.md) D14; nothing in this review remains unresolved.
