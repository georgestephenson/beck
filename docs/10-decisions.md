# 10 — Decision log

George's answers to [`09`](09-risks-and-open-questions.md) §9.5, recorded with the reasoning spelled
out. Decisions marked **DECIDED** are settled and the other documents assume them. One (D3) is
explained here and awaits a one-word confirmation.

---

## D1 — All state is event-sourced, with escape hatches — **DECIDED**

Folds are the native model for all state. First-class escape hatches: `external store` (existing
databases, no fold guarantees, `external.*` effects) and a content-addressed **blob type** (the log
carries hashes; bytes live in object storage). High-churn ephemera (presence, cursors) get
non-durable folds — same semantics, no log persistence.

## D2 — v1 consistency ceiling accepted — **DECIDED**

One totally-ordered log per application. Envelope reserves per-entity ordering keys and logical-
timestamp fields so sharding is a later implementation upgrade, not a semantics break.
Collaborative text is out of core v1 (see D7 for how it comes back).

## D3 — Migration doctrine — **explained below; awaiting confirmation**

### The question, in plain terms

Tier's database is two things: a **ledger** (every event that ever happened, in order) and periodic
**saved games** (snapshots — the folded-up state at some moment). Normally the running state *is*
the latest snapshot plus the events since. The question is what happens to the ledger's old pages
when your data changes shape — say `Todo` gains a `due_date` field. Events recorded last year don't
have that field. Two philosophies exist, and they disagree about which thing is *the real database*:

### Option A — "the saved game is the truth" (Lamdera's doctrine)

When you deploy a shape change, you write one function — `migrate: OldState -> NewState` — that
converts the latest saved game to the new shape, and the system carries on from there. Old events
only need translating (`upcast`) if they're recent enough to still be inside your **retention
window** (say, the last 90 days). Anything older is archived: still stored, but the system no
longer promises to *replay* it.

- **You get**: bounded liability. You maintain translators only for shapes used in the last 90
  days, not for every shape your data has ever had. Deleting/archiving old events is
  straightforward. The mental model is close to a normal database with an audit log attached.
- **You give up**: the ability to rebuild today's state *from the very beginning*, and the ability
  to compute brand-new views over all of history ("re-score every order we've ever taken with this
  metric we invented yesterday") — for history older than the window, you only have snapshots.

### Option B — "the ledger is the truth" (event-sourcing orthodoxy)

The rule: **replaying from the first event must always reproduce everything.** Snapshots are merely
a speed-up and can be thrown away. Consequences: events are never deleted, and for every event
shape you have *ever* shipped, you keep a translator, forever.

- **You get**: a perfect, permanent audit trail; time-travel to any moment since launch; the power
  to invent a new view years later and compute it over all history. For domains where history *is*
  the product (ledgers, medical records, trading), this is the point.
- **You give up**: every old event shape becomes a permanent maintenance obligation (ten years in,
  you may carry dozens of `upcast` functions and can never delete one); storage grows monotonically
  (cheap, not free); and "delete my data" requests are structurally awkward — mitigated by D4's
  crypto-shredding, but the tension is real.

### The recommendation on the table

**Per-store choice, Option A as the default, Option B as an explicit opt-in**:

```python
todos  = durable(fold(...))                     # default: retain=90.days, snapshots authoritative
ledger = durable(fold(...), retain=forever)     # opt-in: replay-from-genesis is the invariant
```

The default gives every app bounded liability; the stores where history is the product declare it
and pay for it knowingly. The type system enforces the corresponding obligations either way (a
missing `migrate`/`upcast` is unshippable, [`03`](03-type-and-effect-system.md) §3.9).

**→ Please confirm the default, or pick B-everywhere.** (A-default is my recommendation; B-default
is defensible if you expect Tier's flagship domains to be audit-heavy.)

## D4 — Erasure by crypto-shredding — **DECIDED**

Per-subject envelope encryption; deleting the subject's key erases them across log, snapshots and
backups at once. Accepted (the Kleppmann-endorsed approach). Worked design lands in Phase 4
([`08`](08-roadmap.md)).

## D5 — Rendering modes: the detailed explanation — **DECIDED** (thin default, per-component upgrade)

Every component's screen is `view(state)` — a pure function. The design question is only *where
that function runs*, and because it is pure, the same source compiles either way; the choice is
per-component and reversible.

### Mode A — the server mails you diffs

The server runs `view`, compares the new page to the previous one, and sends the browser a small
list of change instructions — "replace the text in node 14", "insert a row after node 7". The
browser runs a tiny fixed program (~10 KB, generated once, same for every Tier app) that applies
patches and reports clicks/keystrokes back as commands.

- **Feels like**: a normal website that updates live. First paint is instant (it's server-rendered
  HTML by construction). SEO, accessibility, `noscript` fallback all come free.
- **Strengths**: near-zero download; no client-side state to corrupt; the security surface is
  minimal (no user code executes in the browser at all — CSP can be draconian).
- **Weaknesses**: every interaction crosses the network (~50–150 ms round trip — fine for clicking
  "complete todo", sluggish for drag-and-drop); the server holds per-connected-user view state
  (memory per session — measured in Phase 0, risk R5); nothing works offline.
- **Right for**: dashboards, admin panels, forms, feeds, content — most of most apps.

### Mode B — the server ships you the app (for that component)

The component's `view` *and* its fold are compiled to WebAssembly and sent to the browser. The
server streams *data* changes instead of DOM changes; the browser renders locally.

- **Feels like**: a native app. Interactions are instant because the browser applies the expected
  event to its local copy *speculatively* — legitimate because it runs the *same pure fold* the
  server runs; when the server's authoritative answer arrives (tagged with its position `seq` in
  the log), the guess is confirmed or corrected. This is why clients mint ids: "browsers here are
  replicas, not terminals."
- **Strengths**: zero-latency interaction; offline capability (D7); less server work per user.
- **Weaknesses**: a real download (budgeted < 150 KB per component bundle); client-side complexity
  exists again (though generated, not hand-written).
- **Right for**: typeahead, drag-and-drop, editors, anything marked `optimistic` or `offline`.

### How the choice is made

Mode A is the default. A component is promoted to Mode B when it declares `optimistic`, `offline`,
or a latency budget the round trip can't meet — or when the placement solver's cost model says the
crossing is cheaper as data than as patches ([`03`](03-type-and-effect-system.md) §3.4). One page
mixes modes freely; `tier explain render <component>` prints which mode and why. The original
sketch contains both readings of where `view` runs — this design makes that ambiguity a *feature*:
rendering location is just another placement decision on pure code.

## D6 — Identity: buy, not build — **DECIDED**

Tier never stores passwords and never invents an auth protocol. The language surface is one block
with two implementations:

```python
identity = managed()                                  # Tier provisions a bundled OSS IdP
identity = external(issuer="https://login.acme.com")  # Tier is a relying party to yours
```

- **`managed()`**: the InfraGraph provisions **Keycloak** (Apache-2.0, CNCF) — or the lighter
  **Ory Kratos** (Apache-2.0) as a configurable alternative — wired via OIDC automatically.
  Passkeys, MFA, social login are the IdP's features, inherited, not ours.
- **`external(...)`**: standard OIDC relying party against Okta/Entra/Auth0/Google/anything.
- Either way, Tier's runtime does exactly the part that must be language-integrated: the OIDC code
  flow (the audited `openidconnect` Rust crate), session-token verification at the websocket
  ingress, and the typed mapping **claims → `Session` capabilities** — so `requires auth(c)` and
  per-session signals ([`03`](03-type-and-effect-system.md) §3.8) hang off verified claims.
- Rung 0 (`tier run`) uses a dev-mode identity: auto-login as declared test users, zero setup.
- **Presence** (who is connected now) ships v1 as a first-class non-durable `Signal` — it is both
  the natural demo of per-session fanout and its permanent stress test.

## D7 — Offline and local-first: the explanation — **DECIDED** (offline-tolerant v1; CRDT-valued types v1.x; peer-to-peer out of scope)

Three rungs of "works without the network", in plain terms:

1. **Online-only**: no connection, no app. (Where LiveView-style systems stop. Mode A alone is
   this.)
2. **Offline-tolerant** — *what Tier v1 ships*: a Mode B component holds a local copy of its state
   and the pure fold. Offline, you can read everything you had and keep acting; your commands queue.
   On reconnect they flow through the server's `validate` in order, and your optimistic local state
   is reconciled against the authoritative answers. **The server is still the single referee** —
   this rung is cheap for us precisely because both sides already run the same pure functions; the
   offline queue is just a longer version of the optimism we already do. Its honest limit: two
   people editing *the same value* while offline resolves by referee order — one of them gets
   corrected. Fine for todos and orders; wrong for a shared essay.
3. **Local-first** (the Figma/Linear ideal): every device is a full peer; concurrent edits *merge*
   instead of conflicting (CRDTs); the server becomes optional. This is not a feature but a
   different philosophy: there is no single referee moment, so `validate`-style invariants
   ("balance never negative", "one booking per slot") become **unenforceable at merge time** — no
   type system absolves you, as the original conversation put it. Adopting it wholesale would
   dissolve Tier's central construct, the single merge point.

**The plan**: rung 2 in v1 (falls out of Mode B + determinism). Then **CRDT-valued types** in v1.x:
a field declared `notes: Text` (automerge/loro-backed, MIT) merges concurrent edits *within the
value*, while the log still totally orders the update events around it — collaborative text without
giving up the referee for everything else. Full peer-to-peer local-first stays a non-goal; if it
ever matters, it is a different product built on the same pure core.

## D8 — Effort posture — **DECIDED**

Verbatim directive: *"Do not worry about team size or dev effort. Go for max completeness, clean
architecture, optimal performance, full closure and pure functions."*

Consequences applied across the plan:

- No descoped "framework-only" variant; the full five-tier language is the target
  ([`08`](08-roadmap.md) drops the trim-scope contingency; durations remain as sequencing
  calibration, not commitments).
- Both rendering modes, the incremental-view engine, the operator, replay/fork tooling, identity,
  and CRDT-valued types are all in-plan.
- "Optimal performance" is enforced the only way that works: budgets as CI gates (interaction p99,
  events/s, per-session memory, payload, image size, build latency) from Phase 0 onward.
- "Full closure and pure functions" is read as: never compromise the functional core for
  expedience — purity violations (impure folds, hidden time, unplaced effects) stay *compile
  errors*, not warnings, even where a shortcut would ship faster.

## Still open (minor)

- **D3 confirmation** (above) — the only blocking item.
- Naming/searchability of `tier` (09 §9.5 Q10) — unanswered, non-blocking.
- The tracked technical opens in [`09`](09-risks-and-open-questions.md) §9.6.
