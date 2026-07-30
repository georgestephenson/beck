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

## 16.2 What Rails got right (and Beck's version)

| Rails lesson | Beck's version |
|---|---|
| **Convention over configuration** — the omakase stack | Beck *is* omakase by construction: the compiler derives what Rails conventions imply. Zero config isn't a convention here; it's semantics |
| **Generators** (`rails g scaffold`) — the first five minutes | `beck new` (app templates) and `beck g` (model+commands+events+fold+view scaffolds, upcaster stubs on schema change). Generators emit ordinary source the user owns — no framework magic left behind |
| **Gems extend the framework, not just the library path** (railties/engines) | **The macro system is our railtie**: a package can contribute derive traits, typed literals (`graphql"…"`), `validate` combinators, `ui:` components, `Surface` renderers, `Platform` implementations, lint rules — all as ordinary, capability-restricted macros ([`02`](02-syntax.md) §2.4), no plugin API to design or version separately |
| **Engines** — a mountable sub-application | **Feature packages** (§16.4) — Beck's sharpest ecosystem idea |
| **The doctrine documents** — Rails sells a worldview | `docs/00`–`01` already are this; the book (Phase 5) carries it |

## 16.3 What npm/React got right (and Beck's version)

| npm/React lesson | Beck's version |
|---|---|
| **Components as packages** — the ecosystem is mostly UI | `component` values are ordinary exports; a design system is a package. WCAG compile-time checks ([`12`](12-standards-and-conformance.md) §12.4) apply to third-party components too — quality floor built in |
| **Instant add** — friction kills contribution | `beck add auth` — resolves, locks, fetches, and **shows the effect diff** (§16.5) in one command |
| **Registry search + docs** — discoverability | A central **index + docs site** (docs.rs model: documentation generated from types and doc-comments for every published version, automatically). Hosting stays decentralised on OCI registries; the index is thin: names, versions, digests, docs |
| **SemVer ranges + lockfile** | SemVer with teeth: `beck check --api` computes the *actual* compatibility of a release (types + effects + wire), so a package literally cannot publish a breaking change labelled minor. `beck.lock` pins digests |
| **package.json scripts / postinstall** | **Deliberately absent.** npm's install-time code execution is its worst security legacy; Beck packages contain no install hooks, and macros run capability-restricted at compile time. The npm supply-chain disaster class is unrepresentable |
| **Go's proxy + sumdb** (the quieter lesson) | Decentralised hosting + a **transparency log** for the index (Sigstore Rekor, already in the stack) — every publish is publicly auditable, no single registry to trust or to fail |

## 16.4 Feature packages: the thing only Beck can do

Rails engines and npm packages extend *one tier*. A Beck package can ship a **vertical slice of
application** — because commands, events, folds, views, and infra requirements are all one
language:

```console
$ beck add payments-stripe
  payments-stripe 2.1.0 (digest sha256:…, signed: stripe-community, audits: 3)

  This package contributes:
    commands   ChargeCard, RefundCharge
    events     ChargeSucceeded, ChargeDeclined, RefundIssued
    fold       payments : durable                      (its own store, partition_by=customer)
    process    reconcile_payouts                       (saga, timeout 24h)
    ingress    stripe_webhooks (CloudEvents)           ← new merge point
    ui         CardForm, PaymentHistory                (WCAG-checked)
    effects    net.out(api.stripe.com:443), durable, ingress
  Accept? [y/N]
```

An `auth` package, a `payments` package, a `search` package, a `comments` package — each a working
feature across all five tiers, typed end-to-end, its migrations and upcasters included, its infra
needs declared. This is "Rails engines, but the engine spans the browser, the database and the
NetworkPolicy" — and it is the ecosystem flywheel bet: the day someone assembles a SaaS from
`beck add auth payments billing admin` is the day the ecosystem starts compounding.

## 16.5 Effect-transparent dependencies: trust as a type

The differentiator that falls straight out of [`03`](03-type-and-effect-system.md) §3.6 — a
package's **published signature includes its effects**, so:

- `beck add` shows exactly what a dependency is *allowed to do* (which hosts, which stores, which
  ingress) — and the compiler *enforces* it: a "leftpad" that starts phoning home fails to build,
  because effect widening is a breaking API change caught by `--wire-compat`/`--api` gates.
- `beck why net.out` answers "which of my 40 dependencies talks to the network, and to whom" — a
  question npm fundamentally cannot answer.
- Reviews scale: auditing a dependency starts from its `.becki` — a page of types and effects —
  not from its source tree.

Combined with no install hooks, capability-restricted macros, signed content-addressed artefacts
and the transparency log, the pitch to a security team is: **the dependency supply chain is typed.**

## 16.6 Namespacing, publishing, governance

- **Namespaced names from day one**: `@stripe/payments`, `@beck/std` — npm's unscoped-name
  squatting and typo-squatting wars are avoidable by never having a flat namespace.
- Publishing: `beck publish` = build reproducibly, sign (keyless Sigstore), push to any OCI
  registry, record in the transparency log, index picks it up. Yanking marks-but-never-deletes
  (digests are immutable); the index surfaces advisories (RUSTSEC model).
- The standard library is small and boring (collections, time, money, crypto-primitives-by-
  delegation); the *blessed* layer above it (`@beck/ui`, `@beck/auth`) is versioned separately so
  the language core isn't hostage to library churn — the Rails/Ruby split that worked.
- Private registries = any private OCI registry + optional private index — enterprises get this
  for free from the architecture.

## 16.7 What the foreign-ecosystem bridges are (and are not)

The Python sidecar and typed JS interop remain **runtime bridges**: ways for a Beck app to *use*
NumPy or a JS chart widget, wrapped in typed boundaries with declared effects. They are not the
package system — a bridge dependency appears in `beck.toml` like any other package, but its
contents run outside Beck's guarantees and its effect signature says so (`external.*`, `net.*`),
visibly. The honest sentence for the docs: *Beck packages extend the language; bridges rent from
neighbours.* Both matter; only one compounds into an ecosystem of our own.

## 16.8 Roadmap placement

Phase 3–4 ([`08`](08-roadmap.md)): `beck add`/`publish`/`why`, lockfile resolution, the index +
generated docs site, effect-diff UX, namespaces, transparency log. Phase 5: feature-package
conventions hardened by building `@beck/auth` and `@beck/payments-stripe` ourselves (dogfooding
rule §8.3), generators, and the blessed-layer split. Per [`10`](10-decisions.md) **D15, the
registry is the flagship dogfood application**: its domain is event-sourced by nature (immutable
versions, transparency log, yank-as-event, saga-shaped publish pipeline, counter-heavy read
models), so shipping it in production *is* the proof of the backend and data tier — and the
bootstrap fixed point (the registry serves the packages that build the registry) is the
credibility demo. Hosting stays decentralised OCI, so a static mirror fallback keeps the ecosystem
independent of the dogfood's schedule.
