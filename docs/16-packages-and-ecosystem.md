# 16 — Packages and the ecosystem

> **The questions**: what can we learn from Rails-with-gems and React-with-npm — systems that are
> intuitive for web developers *and* endlessly extensible? And given the Python-ecosystem bridge,
> do we need our own package system to be serious?

## 16.1 Yes, Beck needs its own package system — and half of it is already designed

To be serious, unambiguously yes. The Python sidecar and JS interop
([`09`](09-risks-and-open-questions.md) §9.2) are *runtime bridges* to foreign ecosystems; they
cannot carry Beck-native code — macros, typed components, folds, effects — which is the thing an
ecosystem is made of. No successful language has outsourced its own package story.

What the plan already has is the **plumbing**: packages as OCI artefacts via ORAS
([`06`](06-kubernetes-and-packaging.md) §6.7) — content-addressed, signed (Sigstore), SBOM'd,
air-gap-friendly, hosted on registries teams already run. What Rails and npm teach is that plumbing
is the *least* important part. The ecosystem-winning parts are developer experience and extension
points, specified below.

## 16.2 The vocabulary: tarns, forces, and the Mere

Cargo has crates; Bundler has gems. Beck's nouns come from the same Cumbrian landscape as the name
itself ([`10`](10-decisions.md) D10, D16), chosen so that each term teaches the architecture it
names:

| Term | Names | Why it is exact |
|---|---|---|
| **tarn** | a package | A tarn is a small, high mountain pool — still, clear, self-contained — and it is where becks rise. A Beck package is the same thing: immutable, content-addressed, sealed behind a published signature of types and effects (§16.6), sitting upstream and feeding the flow. `beck add auth` fetches the `auth` tarn |
| **force** | a vertical-slice feature package (§16.5) | A force is a Cumbrian waterfall (Aira Force, High Force) — water dropping *vertically*. A force is a tarn that drops through all five tiers |
| **the Mere** | the central index + docs site (§16.4, D15) | A mere is the lake the becks gather into (Windermere, Buttermere). Hosting stays decentralised on OCI registries; the Mere is the thin place where everything published collects. "Publish to the Mere" |

`beck.lock` keeps its plain, instantly-legible name (D16 records *cairn* — the stone marker of the
proven path — as considered and held in reserve). The sentence the vocabulary makes: **you write
Beck; you package tarns; a tarn spanning all tiers is a force; everything published gathers in the
Mere; `beck.lock` pins the path.**

## 16.3 What Rails got right (and Beck's version)

| Rails lesson | Beck's version |
|---|---|
| **Convention over configuration** — the omakase stack | Beck *is* omakase by construction: the compiler derives what Rails conventions imply. Zero config isn't a convention here; it's semantics |
| **Generators** (`rails g scaffold`) — the first five minutes | `beck new` (app templates) and `beck g` (model+commands+events+fold+view scaffolds, upcaster stubs on schema change). Generators emit ordinary source the user owns — no framework magic left behind |
| **Gems extend the framework, not just the library path** (railties/engines) | **The macro system is our railtie**: a package can contribute derive traits, typed literals (`graphql"…"`), `validate` combinators, `ui:` components, `Surface` renderers, `Platform` implementations, lint rules — all as ordinary, capability-restricted macros ([`02`](02-syntax.md) §2.4), no plugin API to design or version separately |
| **Engines** — a mountable sub-application | **Forces** (§16.5) — Beck's sharpest ecosystem idea |
| **The doctrine documents** — Rails sells a worldview | `docs/00`–`01` already are this; the book (Phase 5) carries it |

## 16.4 What npm/React got right (and Beck's version)

| npm/React lesson | Beck's version |
|---|---|
| **Components as packages** — the ecosystem is mostly UI | `component` values are ordinary exports; a design system is a package. WCAG compile-time checks ([`12`](12-standards-and-conformance.md) §12.4) apply to third-party components too — quality floor built in |
| **Instant add** — friction kills contribution | `beck add auth` — resolves, locks, fetches, and **shows the effect diff** (§16.6) in one command |
| **Registry search + docs** — discoverability | **The Mere** (§16.2) — a central index + docs site (docs.rs model: documentation generated from types and doc-comments for every published version, automatically). Hosting stays decentralised on OCI registries; the Mere is thin: names, versions, digests, docs |
| **SemVer ranges + lockfile** | SemVer with teeth: `beck check --api` computes the *actual* compatibility of a release (types + effects + wire), so a package literally cannot publish a breaking change labelled minor. `beck.lock` pins digests |
| **package.json scripts / postinstall** | **Deliberately absent.** npm's install-time code execution is its worst security legacy; Beck packages contain no install hooks, and macros run capability-restricted at compile time. The npm supply-chain disaster class is unrepresentable |
| **Go's proxy + sumdb** (the quieter lesson) | Decentralised hosting + a **transparency log** for the Mere (Sigstore Rekor, already in the stack) — every publish is publicly auditable, no single registry to trust or to fail |

## 16.5 Forces: the thing only Beck can do

Rails engines and npm packages extend *one tier*. A **force** — named for the Cumbrian waterfall,
because it is a vertical drop through the whole stack — is a tarn that ships a **vertical slice of
application**, possible only because commands, events, folds, views, and infra requirements are all
one language:

```console
$ beck add payments-stripe
  payments-stripe 2.1.0 (digest sha256:…, signed: stripe-community, audits: 3)

  This is a force — it contributes:
    commands   ChargeCard, RefundCharge
    events     ChargeSucceeded, ChargeDeclined, RefundIssued
    fold       payments : durable                      (its own store, partition_by=customer)
    process    reconcile_payouts                       (saga, timeout 24h)
    ingress    stripe_webhooks (CloudEvents)           ← new merge point
    ui         CardForm, PaymentHistory                (WCAG-checked)
    effects    net.out(api.stripe.com:443), durable, ingress
  Accept? [y/N]
```

An `auth` force, a `payments` force, a `search` force, a `comments` force — each a working
feature across all five tiers, typed end-to-end, its migrations and upcasters included, its infra
needs declared. This is "Rails engines, but the engine spans the browser, the database and the
NetworkPolicy" — and it is the ecosystem flywheel bet: the day someone assembles a SaaS from
`beck add auth payments billing admin` is the day the ecosystem starts compounding.

## 16.6 Effect-transparent dependencies: trust as a type

The differentiator that falls straight out of [`03`](03-type-and-effect-system.md) §3.6 — a
tarn's **published signature includes its effects**, so:

- `beck add` shows exactly what a dependency is *allowed to do* (which hosts, which stores, which
  ingress) — and the compiler *enforces* it: a "leftpad" that starts phoning home fails to build,
  because effect widening is a breaking API change caught by `--wire-compat`/`--api` gates.
- `beck why net.out` answers "which of my 40 dependencies talks to the network, and to whom" — a
  question npm fundamentally cannot answer.
- Reviews scale: auditing a dependency starts from its `.becki` — a page of types and effects —
  not from its source tree.

Combined with no install hooks, capability-restricted macros, signed content-addressed artefacts
and the transparency log, the pitch to a security team is: **the dependency supply chain is typed.**

## 16.7 Namespacing, publishing, governance

- **Namespaced names from day one**: `@stripe/payments`, `@beck/std` — npm's unscoped-name
  squatting and typo-squatting wars are avoidable by never having a flat namespace.
- Publishing: `beck publish` = build reproducibly, sign (keyless Sigstore), push to any OCI
  registry, record in the transparency log, the Mere picks it up. Yanking marks-but-never-deletes
  (digests are immutable); the Mere surfaces advisories (RUSTSEC model).
- The standard library is small and boring (collections, time, money, crypto-primitives-by-
  delegation); the *blessed* layer above it (`@beck/ui`, `@beck/auth`) is versioned separately so
  the language core isn't hostage to library churn — the Rails/Ruby split that worked.
- Private registries = any private OCI registry + an optional private Mere — enterprises get this
  for free from the architecture.

## 16.8 What the foreign-ecosystem bridges are (and are not)

The Python sidecar and typed JS interop remain **runtime bridges**: ways for a Beck app to *use*
NumPy or a JS chart widget, wrapped in typed boundaries with declared effects. They are not the
package system — a bridge dependency appears in `beck.toml` like any other package, but its
contents run outside Beck's guarantees and its effect signature says so (`external.*`, `net.*`),
visibly. *Tarns extend the language; bridges rent from neighbours.*

**That sentence is right and it is two categories short**, which mattered because the missing two
are where most of the answer lives ([`102`](102-the-ecosystem-answer.md)). Measured against the
most-downloaded packages of four ecosystems, the largest categories are neither extended nor rented:

| | |
|---|---|
| **Dissolved** | The library patches a problem Beck's semantics do not have — DI containers, ORMs and migration tools, cache libraries, runtime schema validators, logging frameworks, build plumbing. [`29`](29-domain-driven-design.md) §29.1 does exactly this to four DDD patterns |
| **Absorbed into the language** | HTTP, JSON, dates, digests, decimals — [`46`](46-standard-library-report.md) — and the two that are missing rather than optional: the view algebra's join and aggregates ([`99`](99-the-data-tier-means-of-combination.md)), which is what pandas actually is, and an `svg:` vocabulary, which is what charting actually is |
| **Rented (bridged)** | ML inference, scientific computing, PDF — at **merge points only**, per §9.2 |
| **Linked** | BLAS, compression, crypto primitives — somebody's kernel behind a primitive, legal in a fold because the function is pure |

Only the second compounds into an ecosystem of our own, and the first is the one a new language is
never given credit for.

## 16.9 Roadmap placement

Phase 3–4 ([`08`](08-roadmap.md)): `beck add`/`publish`/`why`, lockfile resolution, the Mere
(index + generated docs site), effect-diff UX, namespaces, transparency log. Phase 5: force
conventions hardened by building `@beck/auth` and `@beck/payments-stripe` ourselves (dogfooding
rule §8.3), generators, and the blessed-layer split. Per [`10`](10-decisions.md) **D15, the
registry — the Mere — is the flagship dogfood application**: its domain is event-sourced by nature
(immutable versions, transparency log, yank-as-event, saga-shaped publish pipeline, counter-heavy
read models), so shipping it in production *is* the proof of the backend and data tier — and the
bootstrap fixed point (the Mere serves the tarns that build the Mere) is the credibility demo.
Hosting stays decentralised OCI, so a static mirror fallback keeps the ecosystem independent of the
dogfood's schedule.
