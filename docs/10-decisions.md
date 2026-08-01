# 10 — Decision log

George's answers to [`09`](09-risks-and-open-questions.md) §9.5, recorded with the reasoning spelled
out. Decisions marked **DECIDED** are settled and the other documents assume them. All decisions
D1–D18 are settled.

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

## D3 — Migration doctrine — **DECIDED: Option B by default, Option A as per-store opt-in**

George's call, reversing the draft recommendation: **the ledger is the truth**. By default every
`durable` store carries `retain=forever` — *replaying from the first event must always reproduce
everything* — and a store may opt **down** to bounded retention where limiting liability matters:

```python
ledger = durable(fold(...))                     # DEFAULT: events forever, genesis replay invariant
todos  = durable(fold(...), retain=90.days)     # opt-in: snapshots authoritative beyond the window
```

Obligations the default creates, now first-class in the plan:

- **Upcasters are permanent** for `forever` stores: every event shape ever shipped keeps its
  translator. The compiler scaffolds them, and exhaustiveness on `union Event` keeps them honest.
- **Genesis replay is a CI gate**: for `forever` stores, CI replays an archived corpus through the
  full upcast chain and asserts state equality ([`04`](04-compiler-architecture.md) §4.8) — an
  invariant you claim is only real if a machine checks it.
- **Storage is tiered, not truncated**: old log segments archive to Parquet on object storage
  (pennies, and they double as the analytical corpus for DataFusion,
  [`05`](05-tier-lowering.md) §5.3). Snapshots remain pure optimisation for `forever` stores.
- Erasure stays compatible via D4's crypto-shredding — the erased subject's events remain in the
  log but are permanently unreadable, which is precisely why that mechanism was chosen.

The original fork, kept for the record:

### The question, in plain terms

Beck's database is two things: a **ledger** (every event that ever happened, in order) and periodic
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

The type system enforces the corresponding obligations either way — a missing `migrate`/`upcast`
is unshippable ([`03`](03-type-and-effect-system.md) §3.9).

## D4 — Erasure by crypto-shredding — **DECIDED**

Per-subject envelope encryption; deleting the subject's key erases them across log, snapshots and
backups at once. Accepted (the Kleppmann-endorsed approach). Worked design lands in Phase 4
([`08`](08-roadmap.md)).

## D5 — Rendering modes — **DECIDED** (thin default, per-component upgrade, free mixing confirmed)

Every component's screen is `view(state)` — a pure function. The design question is only *where
that function runs*, and because it is pure, the same source compiles either way; the choice is
per-component and reversible.

### Mode A — the server mails you diffs

The server runs `view`, compares the new page to the previous one, and sends the browser a small
list of change instructions — "replace the text in node 14", "insert a row after node 7". The
browser runs a tiny fixed program (~10 KB, generated once, same for every Beck app) that applies
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
mixes modes freely; `beck explain render <component>` prints which mode and why. The original
sketch contains both readings of where `view` runs — this design makes that ambiguity a *feature*:
rendering location is just another placement decision on pure code.

## D6 — Identity: buy, not build — **DECIDED**

Beck never stores passwords and never invents an auth protocol. The language surface is one block
with two implementations:

```python
identity = managed()                                  # Beck provisions a bundled OSS IdP
identity = external(issuer="https://login.acme.com")  # Beck is a relying party to yours
```

- **`managed()`**: the InfraGraph provisions **Keycloak** (Apache-2.0, CNCF) — or the lighter
  **Ory Kratos** (Apache-2.0) as a configurable alternative — wired via OIDC automatically.
  Passkeys, MFA, social login are the IdP's features, inherited, not ours.
- **`external(...)`**: standard OIDC relying party against Okta/Entra/Auth0/Google/anything.
- Either way, Beck's runtime does exactly the part that must be language-integrated: the OIDC code
  flow (the audited `openidconnect` Rust crate), session-token verification at the websocket
  ingress, and the typed mapping **claims → `Session` capabilities** — so `requires auth(c)` and
  per-session signals ([`03`](03-type-and-effect-system.md) §3.8) hang off verified claims.
- Rung 0 (`beck run`) uses a dev-mode identity: auto-login as declared test users, zero setup.
- **Presence** (who is connected now) ships v1 as a first-class non-durable `Signal` — it is both
  the natural demo of per-session fanout and its permanent stress test.

## D7 — Offline and local-first: the explanation — **DECIDED** (offline-tolerant v1; CRDT-valued types v1.x; peer-to-peer out of scope)

Three rungs of "works without the network", in plain terms:

1. **Online-only**: no connection, no app. (Where LiveView-style systems stop. Mode A alone is
   this.)
2. **Offline-tolerant** — *what Beck v1 ships*: a Mode B component holds a local copy of its state
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
   dissolve Beck's central construct, the single merge point.

**The plan**: rung 2 in v1 (falls out of Mode B + determinism). Then **CRDT-valued types** in v1.x:
a field declared `notes: Text` (automerge/loro-backed, MIT) merges concurrent edits *within the
value*, while the log still totally orders the update events around it — collaborative text without
giving up the referee for everything else. Full peer-to-peer local-first stays a non-goal; if it
ever matters, it is a different product built on the same pure core.

### Addendum: "isn't this just Git? GitHub has rules about what it accepts"

George's follow-up question, answered here because the analogy is *exactly* right and resolves the
apparent contradiction.

**Yes — Git is the canonical local-first system**: every user has a full clone, works offline, and
syncs later. But look at what Git does when two clones edit the same line: it does **not** decide.
It stops, marks a conflict, and hands the problem to a human. That is a fine answer for source code
reviewed by professionals and an impossible one for a live application — you cannot pop a
merge-conflict editor on a user who tapped "buy" on the train. CRDTs are the "never stop, always
merge automatically" alternative, and their mathematical guarantee is narrower than it sounds: all
replicas **converge to the same result** — not that the result satisfies your business rules.

**And the GitHub observation cuts to the heart of it.** Branch-protection rules, required CI,
review approvals — those work because GitHub is a **central chokepoint that can say no** before a
change reaches `main`. GitHub's acceptance rules *are a referee at a merge point*. The moment teams
adopted distributed Git at scale, they voluntarily re-centralised integration through a hub —
because enforcement requires a place where rejection is possible. Pure peer-to-peer Git (patches
emailed between laptops, no shared upstream) is where "rules about what we accept" stop existing,
and almost nobody runs Git that way for exactly that reason.

"Unenforceable" was shorthand for: **without a chokepoint, there is no moment at which a rule can
reject anything.** Concretely — invariant: *seat 14A is sold at most once*. Two offline devices
each sell 14A; each device's local check passed honestly (locally, the seat *was* free). By the
time the replicas meet, both users have already been told they succeeded. A CRDT merge will
faithfully converge — to a state with two sales. No merge function can pick the rightful buyer,
because "who was first?" refers to an order that never existed. The only exits are (a) a referee
that *creates* the order before confirming — which is precisely Beck's merge point — or (b) accept
both and **compensate** (overbook and apologise — airlines do this deliberately; it is a business
policy, expressible in Beck as a fold that detects oversell and emits a compensation workflow, but
it must be *chosen*, never defaulted).

**So the resolution: Beck's architecture already is the GitHub-shaped version of Git.**

| Git/GitHub | Beck |
|---|---|
| `main`'s commit history | the event log |
| Protected branch + required checks | the merge point: `validate` |
| Your local clone with unpushed commits | a Mode B client with optimistic state |
| `git rebase` onto upstream | reconciliation by `seq` |
| Push rejected by CI | command rejected by `validate` (UI un-does the optimistic guess) |
| Patches emailed peer-to-peer, no hub | full local-first — the rung we deliberately don't ship |

What v1 ships (rung 2) is Git-with-GitHub: full local working copies, offline work, ordered
integration through a referee that enforces the rules. What we decline (rung 3) is Git-without-
GitHub. And CRDT-valued types (v1.x) are the analogue of a file format that merges itself cleanly —
usable for fields where order genuinely doesn't matter (text, sets, counters, sketches), inside a
system that still protects `main` for everything that does.

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

## D9 — A language, not a framework on Python or Clojure — **DECIDED**

Prompted by the fair challenge: the tour's code fences are tagged `python`/`clojure` (a
syntax-highlighting hack, [`11`](11-language-tour.md)) — so is this even a separate language? Could
the goals be met by reusing Python or Clojure?

**The test that decides it: a framework can suggest; only a compiler can refuse.** Walk the
project's non-negotiable guarantees and ask what each requires:

| Guarantee | Needs | On Python | On Clojure |
|---|---|---|---|
| Secrets provably never reach the browser ([`03`](03-type-and-effect-system.md) §3.5) | sound static types + effect rows | mypy is optional and unsound; monkey-patching defeats any flow analysis | dynamically typed — flow proofs unavailable |
| Folds are replay-pure (the determinism rule behind replay, fork, optimism, DST) | compile-time purity checking | unenforceable — any callee may hide `time.time()` | unenforceable — convention only (Electric's honest position) |
| Same fold, both tiers, identical results | one code generator, identical numeric semantics | browser Python = Pyodide, ~6–10 MB payload; semantics drift | JVM vs ClojureScript: two runtimes, floats/ints/laziness diverge at the edges |
| ~10 KB thin client; aggressive per-tier DCE | whole-program compilation we control | no | JVM/CLJS artefacts are not in this class |
| "Optimal performance" (D8) on the service tier | native codegen (our LLVM path) | 10–100× interpreter penalty on hot paths; GIL | good JIT throughput, but 100–300 MB per service and JVM cold starts vs our ~10–20 MB static binaries — against the distroless/scale-to-zero goals |
| Views incrementalized by the compiler | analyzable IR of user logic | bytecode analysis of a dynamic language is a tar pit | macros could capture *some* — but untyped plans forfeit the checked-columns/row-size cost model |
| Migration refusal, exhaustive event matching, WCAG-at-compile-time, effect-widening = breaking API change | a type checker in the build path | no | no |

Clojure deserves the respectful version of the answer: it is the *closest existing world* —
homoiconicity, macros, Electric's tier-splitting all live there, and Electric Clojure is standing
proof our semantics are implementable. It is also proof of the ceiling: every guarantee above rests
on programmer discipline there, and Meteor already taught us what convention-without-proof becomes
at scale ([`01`](01-vision-and-premise.md) §1.6). The Python version of the answer is shorter: a
"Beck SDK" for Python would be Meteor-in-Python — the magic without the proofs, plus a 50×
performance ceiling.

**What we do reuse — aggressively — is everything below the language**: Tokio, DataFusion,
differential dataflow, Wasmtime, LLVM/Cranelift, Postgres, Kubernetes, Keycloak
([`07`](07-dependencies.md)). The project is not "forming our own world"; it is building **our own
front door onto the best engines in the world** — the Materialize shape the original conversation
identified ("the engine in Rust, the language as its configuration"). The language layer is
precisely the part that cannot be borrowed, because the guarantees *are* the language.

A prototype-as-Clojure-framework stepping stone was considered and rejected: it would validate the
parts Electric has already validated and none of the moat (proofs, determinism, codegen), while
Phase 0 already validates the runtime claims in Rust directly ([`08`](08-roadmap.md)).

## D10 — Name: **Beck** — **DECIDED** (pending routine go-public checks)

George's pick, being Cumbrian. It is on inspection close to a perfect fit:

- A **beck** is a small, fast upland stream — the project's central metaphor, in the dialect of the
  fells: becks *merge* into rivers (the merge point), and they run.
- English keeps a second meaning: a **beck** is a summoning gesture — "at your beck and call" —
  which is literally what a `Command` is. A language of streams and commands, named by a word that
  means both.
- Short, typable, and the CLI reads as a sentence: `beck run`, `beck up`, `beck deploy`,
  `beck replay`.
- Searchable handle: **becklang** (golang precedent). Known namesakes (the musician, Kent Beck) are
  far from language-tooling search space; trademark/domain checks happen at go-public, not before.

Adoption: **the rename is executed** — CLI (`beck run`), `beck.toml`, `.beck`/`.becki` files, and
all documents now say Beck; George renames the repository. The seed transcript
([`00`](00-original-idea.md)) keeps the historical name verbatim, with a note. "tier" survives only
as the common noun it always was (execution tiers, the data tier) and in the academic term
*tierless*.

## D11 — The OS is substrate — **CONFIRMED (already in the design)**

George's instinct, checked against the plan: yes, the design already treats the operating system as
compiler output at the container level. A Beck image is a statically linked binary in a distroless
base — no shell, no package manager, no distro to patch; the kernel belongs to the platform
([`06`](06-kubernetes-and-packaging.md) §6.2). The deeper rungs — a microVM/unikernel `Platform`
(Firecracker-class) and the zero-OS WASI target — are post-1.0 options the current artefact shape
already permits without change.

## D12 — Mobile as a future surface — **CONFIRMED extensible; explicitly not v1**

The question was extensibility, not scope. Answer: yes — the typed semantic UI tree (not HTML), the
LLVM-compiled pure core, and Mode B's offline/optimism model mean native Android/iOS are "another
renderer behind the `Surface` trait", mapping the same `view` onto Jetpack Compose and SwiftUI
([`05`](05-tier-lowering.md) §5.5). The one genuinely new problem is app-store deploys versus
"deploys ride the stream" (old clients live for months → wire-compat and upcasters become
critical). v1 pays only two disciplines to keep this cheap: no HTML-isms in the `ui:` core
vocabulary, and the renderer behind a trait.

## D13 — Marketing headline — **DECIDED**

Productivity leads: *"one file is a whole running system — no routes, no DTOs, no SQL, no
Dockerfile, no YAML."* Security/least-privilege is the second slide and the enterprise/platform
pitch: *"the compiler proves the API key can't reach the browser; infra policy is derived from the
code's effects."* Not in tension — an ordering. The playground demo serves both in 60 seconds.

## D14 — Review-pass resolutions — **DECIDED**

The four open findings of [`14`](14-review-findings.md), approved as proposed: **F1** structural
shredding — envelope skeletons stay readable, payloads are shreddable, folds apply typed tombstones,
and the D3 invariant is restated as *genesis replay reproduces the post-erasure state*, with erasure
a first-class cascading operation (read models, indexes, snapshots, backups). **F3** abuse quotas at
`validate` are on by default with generous limits. **F6** fold totality: the `partial` effect is
banned in folds (checked arithmetic, no unwrap-shaped stdlib in fold position); an escaped panic
halts only its store; recovery is hotfix-and-replay. **F14** flagship demos mark interactions
`optimistic` (Mode B) so first contact shows latency compensation, not round trips.

## D15 — Flagship dogfood: beck.dev and the package registry, built in Beck — **DECIDED**

George's framing, adopted: *the best demo is Beck's own website; the real test is the package
registry built into it, which exercises the backend and data tier.*

**Why the registry is the perfect stress test — the domain is a homomorphism of the semantics.**
A package registry's requirements *are* Beck's constructs, one for one:

| Registry requirement | Beck construct it exercises |
|---|---|
| Published versions are immutable, forever | `retain=forever` (D3) is a *product requirement* here, not a policy choice |
| Transparency log — every publish auditable | The event log **is** the transparency log; no second system |
| Yank marks, never deletes | An event, not a deletion — the semantics' native idiom |
| Publish pipeline: verify signature → scan → index → generate docs, with failure cleanup | A `process` (saga) with explicit compensations ([`15`](15-scale-and-distribution.md) §15.4) |
| Publisher auth, keyless signing | Identity subsystem (D6) + Sigstore at a CloudEvents ingress |
| Spam/typosquat abuse | Quotas on by default (D14/F3), namespaces, first-writer ids (F2) — every review-pass defence, live |
| Package artefacts | The content-addressed blob type (D1) at real scale |
| Search, download counts, advisories | Read models: full-text index, **high-volume counter folds** (the write-heavy case the todo app never tests), windowed aggregation |
| Private packages | Per-session filtered signals — the fanout stress (R5) with real authorization |
| Rebuild the index from history | **Genesis replay as an operational tool**, not just a CI gate |
| Ecosystem analytics for maintainers | pgwire read models queried by outsiders |

**The website** (beck.dev: docs, playground, registry front-end) exercises the other half: Mode A
content rendering with SEO/CWV budgets as *public* receipts, WCAG compile-time gates on real pages,
the playground as a Mode B component wrapping the WASM-compiled compiler — and "view source" links
to the site's own Beck source, so the site is its own demo.

**The bootstrap fixed point**: the registry serves the packages that build the registry — the
crates.io/cargo circularity, which reads as credibility. One honest guard: package *hosting* is
decentralised OCI by design ([`16`](16-packages-and-ecosystem.md) §16.4), so the index is
availability-critical but not correctness-critical — a static mirror fallback means the ecosystem
is never hostage to the dogfood app's schedule.

**Sequencing** ([`08`](08-roadmap.md)): playground in Phase 3 (its full design — including the
whole-stack-in-a-tab rung and the cloud rung — is [`17-playground.md`](17-playground.md)); beck.dev
in early Phase 4; the registry through Phases 4–5, and **"the registry runs in production on Beck,
serving real packages" becomes a Phase 5 exit criterion** — the production application Phase 4
demanded, chosen so that shipping the proof and shipping the ecosystem are the same act. The triad:
**playground proves the language, the site proves the web tier, the registry proves the backend
and data tier.**

## D16 — Package-system vocabulary: **tarns**, **forces**, and **the Mere** — **DECIDED**

Cargo has crates; Bundler has gems. Beck's package system ([`16`](16-packages-and-ecosystem.md))
takes its nouns from the same landscape the language's name came from (D10) — Cumbrian
hydrology — so that every term teaches the architecture it names:

- **A package is a `tarn`.** A tarn is a small, high mountain pool — still, clear,
  self-contained — and it is where becks rise. The metaphor is exact for what a Beck package
  actually is: immutable, content-addressed, sealed behind a published signature of types and
  effects ([`16`](16-packages-and-ecosystem.md) §16.6), sitting upstream and feeding the flow.
  Practically it checks every box an artefact name needs: one syllable, unambiguous spelling and
  pronunciation, a clean plural, no tech-namespace collision, and "beck tarn" is trivially
  searchable. `beck add payments-stripe` fetches the `payments-stripe` tarn.
- **A vertical-slice feature package is a `force`.** In Cumbria a force is a waterfall (Aira
  Force, High Force) — water dropping *vertically*. A force is a tarn that drops through all five
  tiers: commands, events, folds, views, infra ([`16`](16-packages-and-ecosystem.md) §16.5). The
  sharpest ecosystem idea gets its own word instead of the generic "feature package".
- **The index is `the Mere`.** A mere is the lake the becks gather into (Windermere,
  Buttermere) — the natural name for the thin central index + docs site where everything published
  collects ([`16`](16-packages-and-ecosystem.md) §16.4). "Publish to the Mere." Hosting stays
  decentralised on OCI registries ([`06`](06-kubernetes-and-packaging.md) §6.7); naming the
  *index* rather than "a registry" states that architecture honestly. The Mere is the flagship
  dogfood application of D15, running on Beck at beck.dev.
- **`beck.lock` keeps its plain name.** `cairn` — the stone marker recording the proven path
  across the fell — is semantically exact for a lockfile, but `.lock` is instantly legible to
  every newcomer; `cairn` is held in reserve for a future vendored/offline snapshot artefact.

For the record, the runner-up: **`gill`** (a ravine stream feeding a beck) is arguably the better
metaphor for "packages merge into your app", but loses on practicalities — two competing spellings
(gill/ghyll), a pronunciation trap (hard *g*, which outsiders will miss), and collisions (fish
gills, the liquid measure). Artefact names are said out loud constantly; *tarn* has none of these
problems. Ruled out: *syke* and *dub* (too obscure even by these standards), *spring*/*source*/
*stream* (generic, unsearchable), and anything "confluence"-shaped (occupied mindshare).

The full sentence the vocabulary makes: **you write Beck; you package tarns; a tarn spanning all
tiers is a force; everything published gathers in the Mere; `beck.lock` pins the path.** One
landscape, one metaphor, every term load-bearing.

## D17 — Observability: the log is the trace; telemetry carries what cannot replay — **DECIDED** (Phase 1, measured)

The question was whether to adopt OpenTelemetry, prompted by wanting an Aspire-style dashboard.
Answer: **yes for one specific half, and the halves are separated by determinism, not by taste.**

Distributed tracing exists because in a fleet of services nobody knows what happened, so causality
is reconstructed from sampled, correlated spans. Beck already has something strictly stronger:
`state = fold(f, init, log[..seq])` is the actual history, durable and total, and `beck replay --to`
rebuilds any state the system was ever in. Emitting the fold's call tree as spans would re-record —
lossily, and at a cost the fold pays — what the log records exactly.

So the log answers *what happened, in what order, and what state it produced*. Telemetry answers
what the log **must not** record, because [`04`](04-compiler-architecture.md) §4.8 requires the fold
to be replay-pure: wall-clock durations, resource consumption, and **non-events** — a rejected
proposal, a dropped connection, a failed append. A fold that recorded its own duration would not
replay identically. The boundary is not a convention; it follows from the replay requirement.

Two consequences:

- **Correlation is `seq`, not a trace id.** A trace id names a request; `seq` names a *state*, and
  the state is reproducible from it. Every telemetry record about a position carries `beck.seq`, so
  a line in any backend is one command away from a reproducible debugging session. No service fleet
  can offer this, and it should be said out loud in the positioning ([`13`](13-testing.md), D13).
- **Spans stop at the boundaries.** Ingress, validate, append, fold, view, patch — never inside the
  fold.

Wire format: OTLP over HTTP with **JSON** encoding, which the OTLP specification makes first-class.
Same field names as protobuf, accepted by ordinary collectors, and no `tonic`, `prost` or code
generation in the runtime's dependency tree. Built and measured in
[`19`](19-phase-1-report.md) §19.8.

The corollary for the dashboard: because the program *is* its own AppHost — placement, the splitter
and the effect-derived object graph already are the topology — the resource list and dependency
graph need no second declaration and cannot drift from what is deployed. Aspire needs an AppHost
project; Beck must never grow one.

## D18 — Benchmarks are third-party, and the premise is falsifiable — **DECIDED**

The two questions were: are there standard tests to measure Beck's performance against
alternatives, and what if the SICP exercises were written in Beck as an *expressiveness* test — on
the claim that Beck should have Scheme's full means of combination and abstraction while being no
more verbose. Both are adopted. [`25`](25-benchmarks-and-expressiveness.md) is the analysis,
[`08`](08-roadmap.md) §8.4 is the schedule, [`12`](12-standards-and-conformance.md) §12.9–§12.10
folds both into the conformance discipline.

**Performance: somebody else's rules.** §12.9 already committed to a public benchmark methodology
"versus a named baseline stack"; the decision here is that the yardsticks are **third-party suites
that alternatives already have numbers on** — TechEmpower and js-framework-benchmark for the
shipped system, Are We Fast Yet and CLBG for the core, YCSB, TPC-H/ClickBench, Sightglass,
Lighthouse. A suite we design and win proves nothing. Three riders, each written down because each
is a place a number could be published dishonestly: **TPC-C is excluded** (it assumes
update-in-place OLTP, which is not our data model, and entering it would be a claim we do not
make); js-framework-benchmark is published as three columns and never averaged, because Mode A puts
a network in a loop every other entrant runs in the tab; and **incremental view maintenance has no
standard suite**, which §25.2 records as a gap we would be defining rather than borrowing.

**And the numbers arrive before they are good.** §8.4's sequencing rule — stand every harness up a
phase before its number is publishable — means the first published figures will be bad, because
§25.3 measures the tree-walking evaluator at roughly 33× CPython on `fib(30)` and native codegen is
unbuilt. That is the cost of a trend line that predates the backend, and it is accepted: a
benchmark adopted at 1.0 to support a launch claim has no regression-detecting power, which is the
only thing a benchmark is actually for.

**Expressiveness: the premise had no test.** [`01`](01-vision-and-premise.md) §1.1 says Beck is
SICP's three moves made into a language and D9 says the Python surface loses none of Lisp's power.
Nothing in four phases could have falsified either — the corpus measures placement, the
differential measures splitting, and every program in both is shaped like the todo sketch. SICP is
the right instrument for three reasons and not for sentiment: it is the project's own origin
([`00`](00-original-idea.md)'s first line), it supplies an **oracle** because the book states its
answers, and the verbosity claim has a rigorous form in **Felleisen's criterion** (1991) — which is
checkable rather than rhetorical here, because Beck's hygienic macros make "recovered as a local
rewrite" a thing a test can assert.

Three constraints on how it is run, without which it would be self-congratulation:

- **The pass rate is not the metric.** Every exercise lands in one of three registers —
  *translated*, *re-expressed*, *refused* — and the counts are the result. Chapters 3.1–3.4 and 5
  are expected to be mostly the latter two, because they are about mutable state and machine models
  Beck refuses on purpose; transliterating them would measure how well Beck imitates a design it
  rejects. A silent omission is a bug in the suite.
- **The losses are forecast in advance and published.** §25.5 already records where Beck should
  lose: §2.4–2.5's generic operations (a three-line dispatch table against a trait with impls) and
  chapter 4's evaluator. If the eventual report does not concede those rows it was not run
  honestly.
- **The refusals are asserted, not just described.** [`compiler/sicp/refusals/`](../compiler/sicp/)
  holds one program per wall and the harness asserts each wall still stands, so progress is a
  failing test rather than a fact somebody notices.

**Three further candidates, assessed in §25.8 and decided here.** **Nand2Tetris** is *declined as a
performance test* — a gate-level simulator sits inside the scope §1.5 concedes, so a number there
is evidence about a claim we do not make, and Are We Fast Yet already supplies a fair branch- and
array-heavy workload. Its projects 6–11 are recorded as a **stage-5 expressiveness option
conditional on SICP chapter 4**, since they need what chapter 4 needs and would largely re-test it.
**LeetCode** is *declined as a benchmark* on methodology — no published harness, no fixed workload,
no controlled hardware, and results that move when strangers submit — but **adopted as an
ergonomics smoke test of 30–50 problems**, because it targets the half of D9 that SICP structurally
cannot: not Lisp's power but Python's mass appeal. §25.8 measures that "Two Sum" cannot currently be
written at all, in four diagnostics. **DDIA** is *not a benchmark* and is adopted as a **conformance
matrix** ([`15`](15-scale-and-distribution.md) §15.6) on §12.7's ASVS pattern — with the explicit
finding that "solutions to all the problems" is not achievable and should not be claimed, since the
book raises impossible ones, ones Beck declines, and ones that are business trade-offs. Its
*Conceded* rows are the valuable ones.

The decision has already paid for itself, twice. Ninety minutes of SICP chapter 1 surfaced a
row-unification defect in the checker (§25.6 item 6); one LeetCode problem surfaced that the
imperative idiom is missing as a category from a language whose surface is advertised as Python's.
26 corpus programs and four harnesses had found neither, because every one of them is an
event-sourced application shaped like the todo sketch.

## Still open (minor, non-blocking)

- Security-headline vs productivity-headline positioning ([`09`](09-risks-and-open-questions.md)
  §9.5).
- Routine Beck trademark/domain checks at go-public (rename itself is done).
- The tracked technical opens in [`09`](09-risks-and-open-questions.md) §9.6.
