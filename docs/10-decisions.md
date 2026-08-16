# 10 — Decision log

**A D-number is a rule a Beck program lives under.** It began as George's answers to
[`09`](09-risks-and-open-questions.md) §9.5 and has kept growing as the design settled questions
§9.5 did not ask; what every entry has in common is not who wrote it but what it binds — a program.
The test against the other register is one question: **could a user observe this without reading the
compiler's source?** If no, it is a choice only the compiler lives under and it belongs in
[`adr/`](adr/README.md), whose README argues the split and names the records that predate the rule.
One decision can need both, and D23 is the worked example: the rule is here, the implementation and
its refused alternatives are [`adr/0018`](adr/0018-the-standard-library-is-carried-in-the-compiler.md),
and neither restates the other.

Decisions marked **DECIDED** are settled and the other documents assume them. All decisions D1–D29
are settled. A decision that revises an earlier one says so in both directions rather than quietly
diverging (D20 revises D2).

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

*Revised by D20*: the unit is now the **context** — one totally-ordered log per context, an
application being one or more contexts, with a context-free program having exactly one. Everything
else here stands, per context.

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

**Built** ([`94`](94-the-client-report.md)), with the promotion narrower than this paragraph and a
condition it did not foresee: a component whose view reads the session is **refused** Mode B,
because Mode B sends the browser the state and a page that filters by identity is a page whose
state is not that browser's to hold. Optimism turns out to be the same condition rather than a
separate feature — a client can only guess the next state if it holds the state the fold is of.

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
  the natural demo of per-session fanout and its permanent stress test. **Built**
  ([`48`](48-identity-report.md)): `presence() : Signal[Map[Str, Int]] ! {cap.presence}`, refused to
  the chokepoint (`B0515`) and to a Mode B page (`B0516`), and per subscriber rather than shared
  because the shared dataflow is versioned by the log and this is the one input that is not.

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

**Built** for rung 2 ([`94`](94-the-client-report.md) §94.10), and "falls out of Mode B + determinism"
held with one correction: the server had de-duplicated a retried command since Phase 0 but *replied
that it was rejected*, so replaying a queue took the work back off the page. An idempotent operation
has to be idempotent in its answer. What is still missing is a service worker — the document comes
from the server, so a cold start with no network gets a browser error page rather than the local
copy.

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

## D19 — DDD alignment: claim the equivalence, refuse the jargon; BDD renders prose, never parses it — **DECIDED**

Prompted by the question of whether Beck should be designed for high compatibility with
domain-driven design, with native BDD/Gherkin support in `test` blocks, perhaps as a .NET-style
layer of packages. [`29`](29-domain-driven-design.md) is the analysis; the decision is in three
parts:

**Tactical DDD needs claiming, not building.** The mapping (§29.1) shows the tactical patterns are
either native with stronger guarantees than the pattern book asks for (aggregate = checked fold,
domain event = the substrate, CQRS read model = compiler-maintained view) or *dissolved* —
repository, factory, outbox exist to compensate for problems Beck's semantics do not admit. The
dissolved column is the claim. No DDD vocabulary enters the language surface: by D9's own test, a
convention must not rename a refusable construct after itself. A DDD-dialect tarn is legitimate
ecosystem material and nothing for the core to ship; the core owes the translation — a
practitioner-facing mapping page — not the jargon.

**BDD: the test is the spec; prose is derived, not parsed.** `given`/`when`/`expect` is
Given/When/Then load-bearing, bound by type instead of by regex. Gherkin's cost is its
step-definition glue layer, and parsing `.feature` files would reintroduce it — so that is
**refused**, and §29.3 is the recorded reason. Accepted instead: `beck test --explain` rendering
test blocks as stakeholder-facing prose (the `beck explain` instrument family), and a
`feature`/`scenario` grouping sugar with no new semantics. Migration from Cucumber is by
transcription, exercised once in public via the benchmark below.

**The benchmark is Evans's own.** Per D18's pattern — somebody else's workload, their stated
answers as the oracle, refusals checked in — the cargo-shipping system (the DDD book's running
example, with the DDDSample reference implementation as the published oracle) is adopted as the
DDD expressiveness test (§29.4). Its single-context subset belongs in the corpus now; its full
form needs two contexts and an external routing system, which makes it the forcing function and
acceptance test for D20. The number that travels is the dissolved column made quantitative: how
much of the reference implementation is plumbing the Beck version does not contain.

## D20 — Bounded contexts: one log per context, contexts as deployables, the outside world on a ladder — **DECIDED** (design settled; build staged per [`30`](30-bounded-contexts-and-microservices.md) §30.9)

The strategic-DDD gap in §29.1, plus three requirements stated with it: microservices supported
natively; external microservices and systems easy to connect, because "your entire architecture as
one Beck project" is the ideal and the real world makes it imperfect; heterogeneous hosting for
contexts within one project — ideally one Kubernetes cluster — without surrendering the
supersedes-IaC claim. [`30`](30-bounded-contexts-and-microservices.md) is the design.

**This revises D2, explicitly**: "one totally-ordered log per application" becomes **one
totally-ordered log per context; an application is one or more contexts**. A program that declares
no context has exactly one, so nothing built changes meaning, and D2's envelope reservations and
rung-2 partitioning compose per context. Contexts divide the project into *models* (partitioning
divides a model's keyspace); cross-context state reads are a compile error; the only cross-context
write path is published events and sagas, which promotes [`15`](15-scale-and-distribution.md)
§15.4's `process` to load-bearing boundary semantics.

The load-bearing choices, each argued in [`30`](30-bounded-contexts-and-microservices.md):
contexts are the unit of deployment, with the context map lowered to derived least-privilege
NetworkPolicy and per-context deploy cadence gated by two-sided wire-compat (§30.3); the outside
world is a three-rung ladder — context in-project, another Beck project via exchanged `.becki`,
foreign system via declared protocol with a compiler-demanded translation layer (the
anti-corruption layer as typed code) — with the guarantees kept and forfeited stated per rung and
`beck explain` obliged to say which boundaries are proved and which are rented (§30.4, §30.6);
hosting choices are **constraints on derivation, never patches on output** — per-context node,
architecture and region constraints live in the deploy target and feed the solver, because the
moment a team hand-edits generated manifests the IaC-supersession claim dies (§30.5). One cluster
is the recommended and tested shape; multi-cluster arrives with rung 3's geo-homes, not before.

The property that pays for the construct: a full microservices architecture — several models,
several logs, eventual consistency between them — develops and tests as one process with
deterministic cross-context tests and no network, then deploys per-context *without changing the
program*. The microservices tax (integration environments, Pact-style contract infrastructure,
mesh configuration) is either derived or dissolved.

## D21 — Effects are signature clauses; placement is a decorator — **DECIDED**

[`02`](02-syntax.md) §2.9 held this open with a recommendation, and
[`09`](09-risks-and-open-questions.md) §9.6 item 5 attached a deadline to it: "cheap now, expensive
after Phase 3". [`08`](08-roadmap.md) §8.5.2 classifies it **R** — a retrofit whose cost rises with
delay — and Wave 0 is where it comes due. It is taken now, and taken *as a split* rather than as
one answer to two questions, because four phases of implementation have already demonstrated that
the two annotations are not the same kind of thing.

**An effect or a capability is a clause in the signature.** `def f(x: T) -> U uses durable(orders)`,
never `@uses(...)`. Three reasons, in the order they carry weight:

1. **It is part of the type.** An effect row unifies, it is inferred, it is generalised over
   ([`27`](27-the-walls-come-down-report.md)'s effect polymorphism), and it is a
   bound an impl is held to ([`27`](27-the-walls-come-down-report.md)). A decorator is an AST transform (§2.3)
   that runs before the checker — a transform cannot be the notation for something the checker
   *solves*.
2. **It is published.** §3.6 requires the module interface to carry it; `.becki` does, and
   `--wire-compat` classifies a change to it. An annotation that appears in the published contract
   belongs in the signature the contract is a rendering of.
3. **It reads as part of the sentence.** `-> Todos uses durable(todos)` is one line of English; a
   decorator stack above the definition is a second place to look for the first thing a reader of a
   signature wants to know.

**A placement is a decorator.** `@on(server)`, never a `runs on server` clause. The reason is what
Phase 2 changed underneath this question: **placement is inferred**, and `@on(...)` is an
*override* — a constraint handed to the solver, not a fact about the definition. The measurement
settles it. Of the 28 single-file corpus programs, **one** carries `@on(...)`, and that one exists
to test that pinning still works ([`20`](20-phase-2-report.md)); the other 27 place themselves. An
annotation almost nobody writes should not occupy space in the signature everybody reads. Ten of
the 28 carry a `uses` clause, which is the same measurement pointing the other way.

The consequence for `.becki` is already built and is the thing to keep true: a published interface
renders the placement as `@on(tier)` above the item and the effects inside its signature, because
by then the tier is a decided fact about a compiled module rather than a request. The notation is
the same in both directions, and the two annotations keep their two shapes.

**What would reopen this.** A placement that becomes part of a *published contract a caller must
satisfy* rather than a fact about the callee — that is what [`30`](30-bounded-contexts-and-microservices.md)'s
cross-context deployment could turn it into, and if it does, placement moves into the signature and
this decision is superseded rather than amended.

## D22 — `ui:` is a block macro, not a JSX-like literal — **DECIDED**

The other half of [`02`](02-syntax.md) §2.9 and of §9.6 item 5, taken on the same deadline. **A page
is written as a `ui:` block, which is an ordinary call carrying a quoted block under §2.3's block
rule, expanded by a macro into a typed `Node` tree.** There is no literal element syntax, no
angle brackets, and no second grammar.

The argument that decided it is not aesthetics but *what else the decision costs*:

- **It is not a language feature at all**, which is the whole point. `ui:` is a call with a block,
  so it needs nothing the language does not already have for `test:`, `atomically:` or
  `retry(times=3):`. A JSX-like literal is a second surface grammar, a second thing the printer
  must round-trip, a second thing macros must be able to produce, and a second thing every future
  target has to be taught.
- **A user can write the next one.** A terminal UI, a native tree, an email renderer — each is a
  block macro somebody writes, with no change to the parser. That is [`01`](01-vision-and-premise.md)
  §1.1's claim about Lisp's means of abstraction cashed on the surface Beck actually presents.
- **The compiler reads its structure.** The incremental engine re-renders one row of a `for` inside
  a `ui:` block rather than the page ([`23`](23-incremental-views-report.md)) because the block is a
  tree of `Node`s the analysis walks. A literal syntax could have been given the same treatment;
  the point is that this one needed no special case to receive it.
- **It costs nothing when unused.** Two chapters of SICP run as libraries with no `ui:` anywhere
  ([`27`](27-the-walls-come-down-report.md), [`27`](27-the-walls-come-down-report.md)) — a surface
  feature would have been in the grammar whether or not a program had a page.

Its lineage is stated rather than hidden: the output is Hiccup's, and
`[:main [:h1 "todos"] ...]` maps one-to-one onto the block's `Node` tree, so the original sketch's
pages *are* these pages.

**What this forfeits, said plainly.** Editor tooling for a bespoke literal syntax is a thing other
ecosystems have and Beck will not: no element autocompletion from a schema without the LSP knowing
about `ui:` specifically, and a mistyped tag is a macro-expansion error rather than a parse error.
That is a real cost and it is accepted; §2.7's list of honest losses is where this belongs, and the
mitigation is the LSP's, not the grammar's.

## D23 — The standard library is on an implicit path, and the caller's directory wins — **DECIDED**

[`46`](46-standard-library-report.md) §46.12 found that `import` resolved against the root module's own directory
and against nothing else, and left the fix here rather than taking it in a benchmark's change:
making `import bignum` work from anywhere "is deciding that `lib/` is on an implicit search path …
but it changes name resolution for every program in the language". That is the decision, and it is
taken in two halves.

**The standard library needs no declaration.** A program does not add a dependency, name a path or
carry a manifest entry to write `import bignum`; the library is part of the language the way the
prelude's primitives are, and [`16`](16-packages-and-ecosystem.md) §16.7's "small and boring" is a
statement about its *contents* rather than about its reachability. The alternative — an explicit
dependency on the language's own library — is a tax on every program to express a choice no program
has.

**An import resolves against the caller's directory first and the library second.** This half is the
one that could have gone the other way, and it is forward compatibility that settles it: with the
directory first, a program that already has a `text.beck` keeps working the day the standard library
grows a `text`, so **adding to the library can never break a program that never asked for it**. With
the library first, every name in it would be reserved for all time, and each addition would be a
breaking change for somebody. The cost is that a local module silently shadows a library one, which
is the same cost Python pays and is visible in the one place it matters: a diagnostic about a module
says where it looked.

**What this is not.** It is not a package system: there is no third-party name, no version, no lock
entry, and nothing here decides how `@beck/std` will be spelled when
[`16`](16-packages-and-ecosystem.md) §16.7's namespaces arrive. It is not a *search path* either —
the library is carried inside the compiler rather than found on disk, so it cannot be missing or
stale, and [`adr/0018`](adr/0018-the-standard-library-is-carried-in-the-compiler.md) is the
engineering record of that and of the alternatives.

**What it makes true that was not.** Every module in `lib/` now has to link with every other, because
a program can import two — Beck's namespace is flat and has no qualified reference (`B0601`). Two
collisions were waiting on the day this was taken, and the gate that finds them is
`stdlib.rs::the_whole_library_links_into_one_program`.
[`46`](46-standard-library-report.md) is the build report.

**What would reopen this.** The package system. A namespaced import (`@beck/std/bignum`) changes the
notation and could change the precedence; that is a decision for
[`16`](16-packages-and-ecosystem.md)'s wave, and it supersedes this record rather than amending it.

## D24 — Concurrency is a scope, and its children may not observe each other — **DECIDED**

[`38`](38-literature-survey.md) §38.4 said what shape to adopt — "a scope owns its children, and
errors and cancellation join at the scope" — and left the rule that makes it *mean* something in
Beck open. This is that rule.

**A `parallel:` scope claims that its answer does not depend on which child ran first**, and the
claim is enforced rather than documented. Two conditions, both compile errors: no child may name
another (`B0398`), and no child may perform an effect another child could observe (`B0399`) — the
log, the document, the merge point, a file, an external store. A backend may therefore run the
children in any order or all at once, and running them in the order they are written is a correct
implementation.

**The alternative was a scheduler**, and it is the one this rejects. Letting a child read a sibling
and ordering the children by their dependencies would accept more programs, and it would make the
shape of a program's concurrency something a reader has to derive rather than see. The refusal is
the feature: a child that has to run second is a next line, and writing it as one costs a keyword.

**`net.out(host)` is deliberately not on the refused list.** Two outbound calls are the case the
form exists for, and Beck has never claimed to order a remote host's state — §3.2 treats
`net.out` as an effect on the outside world, and this decision declines to start ordering it. The
consequence is honest and worth stating: two children calling the *same* host may interfere, and
Beck will not say so. It cannot, and a rule that pretended otherwise would refuse every useful
scope.

**What this makes true that was not.** `spawn` — an atom §3.2 has listed since Phase 2 with nothing
able to perform one — now decides a placement for real: a scope lands on the server off §3.3's
table, `client` refuses it (`B0401`), and a function that starts spawning is a breaking change in
the sentence §4.3 wrote for `net.out`.
[`80`](80-structured-concurrency-report.md) is the build report.

**What this reopened, and D25 closes.** `fs(path)` was one atom for a read and a write, so refusing
concurrent writes meant refusing the pair, and two children reading two files was a thing this form
should allow and could not (§80.2). That is now D25.

**What would reopen the rest.** A scope over a *collection* rather than over written-out children.
"No child names another" is a scope check when the children are `let`s and a property of one lambda
when they are elements; that is a different rule for a different form, and this record does not
decide it.

## D25 — `fs` is two atoms, `fs.read(path)` and `fs.write(path)` — **DECIDED**

D24 left this open, and [`80`](80-structured-concurrency-report.md) is it taken and built.

**An effect atom that names a resource has to say what is being done to it.** `fs(path)` was the
only atom in §3.2's list that did not. It was not wrong for three phases because nothing had asked
it a question it could not answer; D24's rule asked one — could a second child of a `parallel:`
scope tell this one had run? — and the honest answer for a read and for a write are different.

**The precedent is in the same list.** §3.8's escape hatches have always been `external.read(store)`
and `external.write(store)`, and `net.out(host)`/`net.in` have always been two. This is that split,
for the same reason, applied to the one atom that had not had it.

**What it changes.** Two children of a scope may read files; a child may not write one. Nothing
else moved: `Tier::discharges` never looked at the operation, `breaks_replay` is true of both
(a file can change between replays, which is a fact about reading), and both are stubbable, because
§21.3's "genuinely external" is about the boundary rather than the direction.

**What it costs.** The spelling `fs(path)` no longer parses, and gets a diagnostic naming both
replacements rather than the generic "neither an effect nor a row". No program in this repository
wrote it, so nothing broke — which is itself worth noting, because the atom has been in the
vocabulary since Phase 2 with no primitive able to perform one. This makes the vocabulary correct;
it does not make filesystem access available.

**What it enables and does not do.** [`06`](06-kubernetes-and-packaging.md) §6.5's derived
least-privilege manifests can now distinguish a `readOnly: true` mount from a writable one. They do
not yet: `beck-infra` derives from `ingress`, `durable` and `net.out` and has never read this atom.
[`80`](80-structured-concurrency-report.md) §80.13 is the correction to
[`80`](80-structured-concurrency-report.md) §80.12, which said otherwise.

**What would reopen this.** A file operation whose interference is finer than the path — a
lock, an append, an atomic rename. The scope rule refuses two writers to *any* paths rather than
comparing them (§80.13), and a real filesystem library is where that becomes worth revisiting.

## D26 — A read model is the arrangement, not a second copy of it — **DECIDED**

[`05`](05-tier-lowering.md) §5.3 has said since the design was written that a read model is
"generated tables in the same Postgres", queried over pgwire by whoever wants them. Building it
found that the first half of that sentence and the second half are separable, and that only the
second half is what the row is *for*.

**A read model is the collection the fold already holds and the arrangement the view engine already
maintains, projected as relations.** Nothing is written on the append path, no table is created, and
there is no projection to lag behind. What an outside tool connects to is a SQL surface over the
same values the page is rendered from.

**Why not the durable projection §5.3 describes.** Three reasons, in the order they bite.

1. **It puts view maintenance on the write path**, which is exactly the choice
   [`23`](23-incremental-views-report.md) §23.9 argued *out* of the design for subscribers: the
   sequencer would pay, per event, for a projection nobody may read. The read model's own case is
   weaker still than the page's, because a BI tool connects twice a day.
2. **It is a second code path over the same events**, and a second code path can drift. The
   recompute oracle covers the arrangement; it would not cover a projection written beside it.
3. **It doubles the storage** of every maintained collection, to hold what is already in memory.

**What this forfeits, said plainly.** The "append and project in one transaction" property
[`07`](07-dependencies.md) §7.8.1 gives as the reason for the SQLite substrate, and which
[`67`](67-sqlite-report.md) §67.1 was loud about being *available and unused*. It is still unused.
A query is as fresh as the log — it advances the dataflow itself and reads under the accumulator's
lock — so the property buys atomicity between a durable log and a durable projection that does not
exist. The day a read model has to survive the process, or be reachable by a tool that cannot reach
this port, that transaction is what it is built on, and this record is what it supersedes.

**What decides which signals are tables.** The same cut §5.3 draws for arrangement sharing: a table
is a view that does not depend on *who is asking*. A `per_session` signal is not a table, because a
SQL client has no session and inventing one would answer a question nobody asked.
[`23`](23-incremental-views-report.md) is the build report and
[`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md) the engineering record of the wire.

## D27 — Real identity is canonical: one NaN, no `-0.0`, a total order — **DECIDED** (built; recorded from [`27`](27-the-walls-come-down-report.md) §27.8 per [`35`](35-standards-landscape.md) §35.5)

Arithmetic on reals is IEEE 754-2019 binary64, clause 5 — what the hardware does, held
digit-for-digit against SICP's printed answers. **Identity and ordering deviate from §5.11,
deliberately**: on the way into a `Value`, `-0.0` is canonicalised to `0.0` and every NaN to one
NaN, and comparison uses the monotone transform in the shape of §5.10's `totalOrder`. So Beck's
`==` on reals is structural — `NaN == NaN` is true — and every real is ordered.

Why deviate: a `Value` is a `Map` key, an arrangement's collation, a state digest and a patch
stream a replay must reproduce bit-for-bit. An element unequal to itself, or two zeros that compare
equal but hash apart, breaks each of those in turn. The deviation is the price of determinism, and
this record is what makes it read as chosen rather than archaeological — [`35`](35-standards-landscape.md)
§35.1 found it stated only in a report, and reports are history. A porter must know it; the Phase 5
spec states it as current state (arithmetic per clause 5, identity and order per the canonicalised
total order), citing ISO/IEC 60559:2020 alongside IEEE 754-2019 when it cites either.

Where it is held: canonicalisation at `Value` construction in `beck-core`, the printed-digit
equalities in `beck-cli/tests/sicp.rs`, and the backends' agreement in `beck-cli/tests/native.rs`.

## D28 — The public surface is an opt-in family of derived contracts, and Beck never transports over one — **DECIDED** (design settled; build staged per [`101`](101-the-public-surface.md) §101.10)

The question was how a third-party system consumes a Beck backend, and whether Beck should meet
the industry's consensus artefacts — OpenAPI, gRPC, CloudEvents, MCP — or ask consumers to learn
its own. Answer: **meet the consensus, by rendering, opt in.** `@public(<form>)` is a family, one
member per consumer kind ([`101`](101-the-public-surface.md) §101.2), each member a *rendering of the
internal contract* in somebody else's standard, gated by a reader that is not the writer
([`92`](92-supply-chain-and-release-report.md) §92.2's pattern, extended to the API). A program
without the annotation has no public surface at all.

Three edges of the decision carry the weight:

- **The consumer chooses the shape; the compiler keeps the properties** (§101.3). Naming,
  versioning, auth scheme and error vocabulary are configurable, because a public surface serves
  its consumers' needs rather than Beck's opinions. Schema-from-types, the `secret[.]` discipline,
  shared `validate`, and the derived ingress are not configurable, because they are the point.
- **Beck's own seam stays internal** (§101.4). The public contract is never the internal transport —
  the internal seam carries placement, effect rows and `seq`-tagged patch streams that a public
  contract cannot, and coupling internal evolution to an external deprecation clock would invert
  the reason both derive from one set of types. Honesty between the two is shared derivation plus
  a drift gate, not shared transport.
- **GraphQL is declined, with the reason recorded** (§101.2): a query surface whose worst case is
  the consumer's to choose is a cost the premise cannot carry until it has a cost model.
- **The edge absorbs what the fold must never see** (§101.7). Auth, rate limits, idempotency,
  response vocabulary, hostile-input bounds: every obligation a public surface imports lives at
  the runtime edge, per surface, and a rejected request is a non-event in D17's sense — telemetry,
  never the log. Two obligations are genuine prerequisites rather than derivations — F15's quotas
  and the inbound TLS posture — and they land before the first form ships.

`@public(sql)` — the pgwire read models, D26 — is named into the family retroactively: it was the
first member built, and its gate (`tokio-postgres`, a foreign reader) is the shape every other
member's gate copies. `@public(events)` is where the family meets enterprise event-driven
architecture, and §101.6 says which of EDA's standing problems dissolve on Beck's semantics (the
outbox pattern has nothing to patch; the dedupe key and resume cursor are given by `(context,
seq)`) and which are imported. The trust corollary — maximalist telemetry derived from the log by
replay rather than paid for in the serving path — is §101.8, and it is a consequence of D17 rather
than a revision of it.

## D29 — Beck absorbs Tailwind's design system and not its delivery, on by default with a switch — **DECIDED** (design settled; build staged per [`102`](102-styling-and-the-component-library.md) §102.11)

The question was whether CSS is really absorbed, given that the stylesheet a running application
serves is eight rules hard-coded in Rust and `css:` has never had a parser. Answer: **take the
design system, refuse the delivery mechanism, and default it on.**

- **The vocabulary is Tailwind's.** The spacing scale, the colour ramps, the type scale, the variant
  grammar and above all the *names*. They are MIT-licensed, they are a decade of taste, and every
  web developer — and every model a developer asks for help — already knows them. A Beck-invented
  vocabulary would be strictly worse at the one thing a vocabulary does, so nothing is renamed for
  the pleasure of owning it.
- **The extraction is the compiler's.** No scanner, no safelist, no `@source`, and no `npm install`
  on the path to a styled page. Tailwind finds class names by regular expression over source text
  because it cannot resolve an import; Beck can, so `beck build` collects the class strings that
  reach a `class=` from the typed tree and emits exactly those.
  [`102`](102-styling-and-the-component-library.md) §102.3 is the measurement that decides this and
  not the aesthetics of it: 71 Beck files that style nothing emit 15 rules extracted from English
  prose in comments, a misspelled utility and a computed class name both vanish at exit 0, and an
  application whose components are an imported module yields 1 utility of 12 — which is fatal
  rather than untidy, because a Beck package is a content-addressed OCI artefact with no source
  tree for a scanner to read.
- **A name Beck does not know is a diagnostic**, with a suggestion. That is the whole difference
  between using a tool and absorbing one, and it is why the accepted table is gated against
  Tailwind's own compiler rather than typed in: a candidate Tailwind emits a rule for is one Beck
  must accept, and one it emits nothing for is one Beck must refuse
  ([`92`](92-supply-chain-and-release-report.md) §92.2's rule that a gate reads a rendering, and
  [`clbg/`](../compiler/clbg/README.md)'s that the constants come from somebody else's artefact).
- **On by default, and switchable** — §8.3's standard applied to a choice the system makes for you.
  A new program is styled, `beck new` produces something that looks deliberate, and the sheet is
  emitted with no configuration. **`styles = none` turns the whole of it off**: no sheet emitted, no
  class checking, `class=` an unexamined `Str` again, and a program free to link its own stylesheet
  or ship a foreign design system. The switch exists because a default nobody can leave is not a
  default but a requirement, and because a team arriving with an existing design system must not
  have to argue with the compiler about it. The switched-off path belongs in a gate beside the
  switched-on one, per §8.3 — a default nobody has run is a claim.
- **The npm package stays available and stays optional.** For the full utility surface, plugins, or
  an existing Tailwind configuration, the same exact extraction feeds the real thing: `beck build`
  emits the class list as an artefact the scanner reads instead of guessing at source, which is
  strictly better input than it gets from any other language. What is refused is *depending* on it —
  24 packages, 20 MB and three prebuilt native binaries at install time, against a project whose
  absence checklist names "no Dockerfile" and whose package system exists partly because npm's
  install-time code execution is ([`16`](16-packages-and-ecosystem.md) §16.4) "its worst security
  legacy".

The corollary for components is the same shape and is not a separate decision: a Beck component
library is **markup and utilities**, not a port of a JavaScript kit, because porting one would port
its workarounds for state it could not place and positioning the platform could not do. What is
worth taking from that ecosystem is its specification work — the WAI-ARIA Authoring Practices
keyboard tables as each component's oracle ([`102`](102-styling-and-the-component-library.md)
§102.10).

## Still open (minor, non-blocking)

- Security-headline vs productivity-headline positioning ([`09`](09-risks-and-open-questions.md)
  §9.5).
- Routine Beck trademark/domain checks at go-public (rename itself is done).
- The tracked technical opens in [`09`](09-risks-and-open-questions.md) §9.6.
