# 12 — Standards and conformance

> **The question**: are there rulesets or standards Beck can conform to, so there is rigour that we
> have done things correctly — not just a scratchpad of ideas?

Yes, at every layer — and the discipline that makes conformance *real* rather than aspirational is
stated first.

## 12.1 The governing rule: a claim is a test or it is marketing

Every standard adopted below enters the project in one form only: **an executable conformance
artefact wired into CI.** A sentence in documentation ("Beck speaks OIDC") is worth nothing; a test
suite that fails the build when we drift is the actual standard. Three project-level instruments
implement this:

1. **The language specification is test-linked.** Every normative paragraph of the Beck spec
   carries an ID; every ID is referenced by at least one test in the public conformance suite
   (`beck-conformance`, the [test262](https://github.com/tc39/test262)/WASM-spec-suite model). A
   spec change without a test change fails CI. This is the single strongest "not a scratchpad"
   instrument available to a language project.
2. **An RFC process for language evolution** (Rust's model): design changes are written proposals
   with a tracking issue, a stabilisation checklist, and a documented no-breakage policy —
   plus **editions** for the rare opt-in breaking change.
3. **Stable diagnostic codes** (`E0xxx`, rustc's model): every compiler error is a documented,
   versioned contract with its own explainer page and snapshot tests ([`04`](04-compiler-architecture.md) §4.5).

## 12.2 Language and data semantics

| Standard | What conforms | Enforced by |
|---|---|---|
| **IEEE 754-2019** | `f32`/`f64` arithmetic, including across tiers — WASM and native must agree bit-for-bit (NaN canonicalisation specified) | cross-backend differential tests ([`13`](13-testing.md)) |
| **Unicode 15+ / UTF-8 everywhere** — normalisation (NFC), segmentation (UAX #29), identifiers (UAX #31) | `str`, source files, identifier rules | conformance suite vectors |
| **RFC 3339 / ISO 8601** | `Instant`, envelope `at` timestamps, all serialised time | wire golden tests |
| **RFC 8259 (JSON)** + **JSON Schema 2020-12** | generated public JSON APIs; schemas generated from types | round-trip tests against reference validators |
| **SemVer 2.0.0** | Beck packages, the compiler itself, and the *wire/log compatibility* rules layered on it ([`04`](04-compiler-architecture.md) §4.3) | `beck check --api` / `--wire-compat` |
| **TOML 1.0** | `beck.toml` | reference parser tests |

## 12.3 Protocols and interop

| Standard | Where |
|---|---|
| **HTTP semantics: RFC 9110–9114** (incl. HTTP/2, HTTP/3) | all generated endpoints; via Hyper/quinn, verified with h2spec-class suites |
| **RFC 6455 (WebSocket)** | the patch/command channel |
| **TLS 1.3 (RFC 8446)** only; no legacy downgrade | rustls configuration, pinned in generated infra |
| **OpenAPI 3.1** | generated for every `@public(rest)` surface — the schema *is* derived from types, so it cannot drift |
| **gRPC / Protobuf** | generated for `@public(grpc)` |
| **OAuth 2.1 / OpenID Connect Core + Discovery** | the identity subsystem ([`10`](10-decisions.md) D6); validated against the OpenID Foundation conformance suite |
| **WebAuthn L3** | inherited via the bundled IdP (Keycloak) |
| **CloudEvents 1.0** | `ingest(source)` — external webhooks/feeds enter the merge point as CloudEvents, so every event bus on earth can feed a Beck app |
| **W3C WebAssembly core + WASI Preview 2 + Component Model** | Mode B client artefacts and the WASM server target; validated with the official spec test suites |

## 12.4 Web output: accessibility as a compile-time property

The `ui:` macro emits a typed tree, which means **WCAG 2.2 AA and ARIA Authoring Practices become
checkable at compile time** — a differentiator no template language can match:

- an `img` without alt text, a button without an accessible name, a form input without a label, or
  an interactive element unreachable by keyboard is a **compile error** (with the usual explicit
  escape hatch, `@a11y(exempt, reason=...)`, which is lintable and auditable);
- colour-contrast checking runs against the typed `css:` values where statically known;
- generated DOM patches preserve focus and announce live-region updates per ARIA — a *runtime*
  conformance matter, tested with axe-core in the e2e suite ([`13`](13-testing.md) §13.6).

Also: **Core Web Vitals** budgets (LCP/INP/CLS) as CI gates on the example apps — Mode A's
server-rendered-first architecture should make these trivially green, and the gate proves it stays
so.

## 12.5 Data tier

| Standard | Where |
|---|---|
| **PostgreSQL wire protocol (pgwire)** | Beck read models are browsable by any Postgres client; verified against `psql`, JDBC and BI drivers in CI |
| **Apache Arrow + Parquet** | columnar interchange and log archives; verified with official interop test files |
| **SQL** (PostgreSQL dialect, documented subset) | all generated SQL runs against pinned Postgres majors in CI — we conform to Postgres-as-spec rather than abstract SQL:2023, and say so honestly |

## 12.6 Containers, supply chain, and the "prove the build" story

This is where Beck's reproducibility premise meets external, auditable yardsticks:

| Standard / framework | Target |
|---|---|
| **OCI image / distribution / artifact specs** | all images and packages ([`06`](06-kubernetes-and-packaging.md) §6.2, §6.7) |
| **Reproducible Builds** (reproducible-builds.org definition) | bit-for-bit: CI builds every release twice on independent runners and diffs digests — the definition made executable |
| **SLSA v1.0 — target Build L3** | provenance attestations for compiler releases and for every user `beck build`; hardened, isolated builders |
| **SPDX 2.3 SBOMs** (CycloneDX emitted on request) | every image and package, generated by apko + `cargo-about` |
| **Sigstore / cosign + in-toto attestations** | signing for images, packages, SBOMs, provenance |
| **OpenSSF Best Practices badge + Scorecard** | the project repo itself — branch protection, review, fuzzing, SAST all scored externally |
| **REUSE 3.0** | licensing metadata for every file in the repo |

## 12.7 Security: map the guarantees to the industry's vocabulary

Beck's type-level guarantees ([`03`](03-type-and-effect-system.md) §3.5) are stated internally in
our terms; conformance means restating them in the auditor's terms and testing that mapping:

- **OWASP ASVS 4.x**: a maintained control-by-control matrix — each control marked *unrepresentable
  by construction* (with the test proving it), *generated* (e.g. session management via the IdP),
  or *user's responsibility* (documented). This document is what a security team actually asks for.
- **CWE coverage claims, each backed by a negative test**: CWE-79 (XSS), CWE-89 (SQLi), CWE-915
  (mass assignment), CWE-639 (IDOR — via capability-typed ids), CWE-200 (secret exposure —
  `secret[T]` flow tests), CWE-352 (CSRF — command channel design). The claim "this bug class
  cannot be written in Beck" appears only where a CI test generates adversarial programs and
  asserts they fail to compile.
- **NIST SP 800-63B** via the bundled IdP for authenticator requirements; **OWASP ASVS V2/V3**
  delegated likewise.
- The macro sandbox and dependency policy ([`07`](07-dependencies.md) §7.10) address the
  supply-chain rows of **SLSA** and **OpenSSF** above.

## 12.8 Observability and operations

| Standard | Where |
|---|---|
| **OpenTelemetry semantic conventions** | every auto-generated span/metric/log attribute uses the registry names — dashboards and vendors work out of the box |
| **OpenMetrics / Prometheus exposition** | runtime and operator metrics |
| **Kubernetes API conventions + Gateway API spec** | generated objects pass `kubeconform`-class schema validation for every supported k8s minor (N-2 policy) in CI; the operator follows controller conventions (status conditions, server-side apply field ownership) |
| **12-factor principles** (as a checklist, not dogma) | generated services: config from env, stateless-except-declared-`durable`, disposability via drain |

## 12.9 Process standards we hold ourselves to

- **Formal spec for the semantic core**: the `Stream`/`Signal`/`fold`/placement calculus gets a
  small-step semantics in the spec appendix; the deploy choreography and subscription-resume
  protocols get **TLA+ specifications model-checked in CI** ([`13`](13-testing.md) §13.7) — the
  strongest available answer to "prove the drain/resume dance is correct".
- **Conformance suite as a product**: published, versioned, runnable against any future alternative
  implementation — the spec's teeth outlive us.
- **Public benchmark methodology**: versus a named baseline stack, reproducible from a repo, with
  the harness published — performance claims follow the same "test or marketing" rule as
  everything else. **The suites are named and third-party**
  ([`24`](24-benchmarks-and-expressiveness.md) §24.2, adopted as D18): TechEmpower and
  js-framework-benchmark for the shipped system, Are We Fast Yet and the Computer Language
  Benchmarks Game for the core, YCSB for the log, TPC-H/ClickBench for read models, Sightglass for
  WASM, Lighthouse for the page. A suite we designed ourselves and won is worth nothing; the whole
  value of a standard one is that somebody else chose the workload and the alternatives already
  have numbers on it. §24.2 also records where the honest answer is that **no standard exists** —
  incremental view maintenance — and §24.2's three methodology notes are part of the commitment,
  not commentary on it. [`08`](08-roadmap.md) §8.4 schedules them.

## 12.10 Expressiveness: the premise, made falsifiable

Every standard above measures a shipped artefact. None of them measures what
[`01`](01-vision-and-premise.md) §1.1 and [`10`](10-decisions.md) D9 actually claim — that Beck is
SICP's three moves made into a language, and that a Python-shaped surface carries Lisp's power
without losing any of it. Until [`24`](24-benchmarks-and-expressiveness.md) there was **no artefact
that could falsify either**, which by §12.1's own rule makes them marketing.

Two instruments, adopted as D18:

- **Felleisen's criterion** (*On the Expressive Power of Programming Languages*, 1991) is the
  formal half, and it is checkable rather than rhetorical because Beck's macros are hygienic and
  operate on the same `Node` AST as everything else: every special form SICP introduces is either
  recovered as a Beck macro — a *local* rewrite — or recorded as requiring a global reorganisation,
  which is exactly the 1991 definition of being less expressive. Passing it makes the line count a
  question about ergonomics rather than about power, which is the separation that stops the
  exercise collapsing into an argument about syntax.
- **The SICP suite** is the empirical half, and it has an oracle: the book states its answers, so
  §13.1's "the hardest problem in testing is knowing the right answer" does not apply. It enters
  CI the same way every other standard here does — as an executable artefact
  ([`compiler/sicp/`](../compiler/sicp/), gated by
  [`beck-cli/tests/sicp.rs`](../compiler/crates/beck-cli/tests/sicp.rs)) — and its **refusals are
  asserted as well as its passes**, so a wall coming down is a failing test rather than something
  somebody notices.

The counting protocol (§24.5) is part of the standard, because lines of code is a real metric and
an easy lie: a third-party Scheme baseline pinned by commit, the same algorithm on both sides or
the exercise does not count, and three counts published together — lines, tokens, and lines
excluding type signatures — since Beck is typed and Scheme is not. A single headline number is the
tell that somebody chose.
