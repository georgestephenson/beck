# 12 — Standards and conformance

> **The question**: are there rulesets or standards Beck can conform to, so there is rigour that we
> have done things correctly — not just a scratchpad of ideas?

Yes, at every layer — and the discipline that makes conformance *real* rather than aspirational is
stated first, because for a long time this document did not follow it. Early drafts stated targets
in the present tense — a TLA+ model checked in CI, an OpenAPI generator, a WCAG compile-time
checker — none of which existed, which is precisely the failure §12.1 names. The document was
audited against the tree and corrected in place. Every claim below now carries one of three words:

- **Verified** — the artefact exists, runs under `cargo test --workspace` or a named CI job, and
  the row names it.
- **Partial** — part of the claim has its artefact; the unbacked part is named in the row rather
  than implied away.
- **Chartered** — adopted as a target with nothing built. A chartered row is only honest if it has
  a *position*: either a place in [`08`](08-roadmap.md) §8.5's order or a named predecessor that
  blocks it. "A list of things to do eventually is not a plan" (§8.5), and that goes for standards
  twice over.

## 12.1 The governing rule: a claim is a test or it is marketing

Every standard adopted below enters the project in one form only: **an executable conformance
artefact wired into CI.** A sentence in documentation ("Beck speaks OIDC") is worth nothing; a test
suite that fails the build when we drift is the actual standard. Three project-level instruments
implement this:

1. **The language specification is test-linked** — **Chartered (Phase 5)**. The design: every
   normative paragraph of the Beck spec carries an ID; every ID is referenced by at least one test
   in the public conformance suite (`beck-conformance`, the
   [test262](https://github.com/tc39/test262)/WASM-spec-suite model); a spec change without a test
   change fails CI. **No spec and no suite exist today.** The nearest built discipline is the
   generated reference ([`34`](34-generated-documentation-report.md)) — derived from the compiler's
   own tables and gated against drift by `beck-cli/tests/docs.rs` and
   `.github/workflows/docs.yml` — and the Phase 5 spec extends exactly that shape: derived or
   test-linked, never a second account that can go stale.
2. **An RFC process for language evolution** with editions — **Chartered (Phase 5, beside the
   stability policy)**. What exists today and does the near-term job: ADRs
   ([`adr/`](adr/README.md)) for engineering decisions, D-numbers ([`10`](10-decisions.md)) for
   design decisions, and the correct-in-place rule for documents. There are no RFC templates, no
   tracking issues and no editions machinery; the process earns its overhead when there is a second
   implementation or an outside contributor base, and pretending to run it before then would be
   ceremony.
3. **Stable diagnostic codes** — **Verified, with a named remainder.** The codes are `B0xxx` (the
   model is rustc's `E0xxx`; the prefix is ours): 137 codes in `beck-diag/src/index.rs`, an index
   gated complete in both directions against every literal the compiler can emit
   (`beck-cli/tests/docs.rs`), `beck explain error <CODE>` answering for every one, and
   error-message snapshots in `beck-cli/tests/ui.rs`. What remains of the chartered "documented,
   versioned contract" is versioning and a gate on the prose, which
   [`34`](34-generated-documentation-report.md) §34.7 names itself.

One more instrument was built without ever being cited here, and it is the one that makes the other
three checkable: **the workflow cross-check** (`beck-cli/tests/workflows.rs`, CI's
`the-workflows-are-yaml` job, [`adr/0005`](adr/0005-workflows-cross-check.md)) parses every
workflow and asserts the jobs this document leans on actually exist and run what they say. Phase 2
found the Phase 1 CI workflow had been invalid YAML for an entire phase
([`20`](20-phase-2-report.md) §20.4 item 8); "wired into CI" is itself a claim, and this is its
test.

## 12.2 Language and data semantics

| Standard | Status | What exists | What is missing, and where it is scheduled |
|---|---|---|---|
| **IEEE 754-2019** | **Partial** | Arithmetic is binary64. SICP's printed doubles are asserted digit-for-digit (`beck-cli/tests/sicp.rs`) — a reassociation or FMA anywhere would fail the suite. The differential runs corpus programs on the evaluator and both native backends ([`93`](93-the-native-backends-report.md)). `Value` identity canonicalises `-0.0` and NaN — a chosen deviation from §5.11, recorded as [`10`](10-decisions.md) D27 | No WASM tier yet, so "across tiers" is a two-backend claim. F9's deterministic libm is **done**: `sin` and `cos` are computed in the runtime library and correctly rounded, so they are a fact about the mathematics rather than about the host ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)) |
| **Unicode, pinned per release** — currently **17.0**, identifiers **UTS #39 ASCII-Only** | **Partial** | The pin is `beck_syntax::security::UNICODE`; the identifier profile is UTS #39's strictest restriction level, satisfied by construction; bidirectional and zero-width controls are refused in both surfaces (`B0102`, the Trojan Source class, CVE-2021-42574). Vectors: `beck-cli/tests/identifiers.rs`, grouped by the attack each defeats. Source files are UTF-8 | This row previously claimed "Unicode 15+", NFC normalisation and UAX #29/#31 — none of which was the artefact built. NFC and segmentation have no implementation and no caller today; they come due when the string library grows operations that need them ([`46`](46-standard-library-report.md) §46.16), and enter this table with vectors when they do |
| **RFC 3339** | **Verified, narrow** | `time_format`/`time_parse` in the prelude, UTC-only by choice — replay wants an instant, not a zone, so an offset is rejected (`beck-cli/tests/stdlib.rs`) | RFC 9557 zoned suffixes: watch, per [`35`](35-standards-landscape.md) §35.3 — adopted only if a zoned type ever enters the library |
| **RFC 8259 (JSON)** | **Partial** | A JSON reader/writer in the standard library ([`46`](46-standard-library-report.md)), `serde_json` host-side | No conformance vectors run. A JSONTestSuite-class vector run is a small artefact — chartered, [`08`](08-roadmap.md) §8.5.4's standards ledger |
| **JSON Schema 2020-12** | **Chartered** | Nothing | Blocked on the `@public(rest)` emitter (Phase 4); lands with it, beside OpenAPI 3.1 and RFC 9457 (§12.3) |
| **SemVer 2.0.0** | **Partial** | The wire/log compatibility rules layered on it: `beck check --wire-compat` classifies interface diffs (`beck-core/src/compat.rs`, with the command/event asymmetry worked out), run in CI including the `--breaking` acceptance path; the release/tag/`--version` agreement is gated (`beck-cli/tests/release.rs`, `release/version.sh`) | This row previously named `beck check --api`, a flag that does not exist. Package-API semver arrives with the package system (Phase 4, [`16`](16-packages-and-ecosystem.md)) |
| **TOML 1.0** | **Chartered** | Nothing — there is no `beck.toml` | The file arrives with the package directory model (Phase 4); reference parser tests enter with it |

## 12.3 Protocols and interop

| Standard | Status | Where it stands |
|---|---|---|
| **HTTP semantics: RFC 9110/9112, HTTP/2 (9113)** | **Partial** | Hyper (`http1` + `http2`) serves every generated endpoint, exercised across the harnesses. No conformance suite runs; an h2spec-class run is chartered in the standards ledger. **HTTP/3 (9114): no implementation and no QUIC dependency** — the earlier claim of quinn was fiction; watch until a workload demands it |
| **RFC 6455 (WebSocket)** | **Partial** | The patch/command channel, functionally exercised in five suites (`browser`, `mode_b`, `runtime_edge`, `playground`, `oidc`). An Autobahn-class vector run is chartered in the standards ledger |
| **TLS 1.3 (RFC 8446), no legacy downgrade** | **Partial** | rustls over aws-lc-rs ([`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md)), real handshakes in tests, certificate verification refusals gated in `beck-cli/src/fetch.rs`'s tests. The "1.3 only" half has no gate — a test that a 1.2-only peer is refused is chartered in the standards ledger |
| **OpenAPI 3.1 + RFC 9457 problem details** | **Chartered** | Blocked on the `@public(rest)` emitter (Phase 4). The three land as one artefact — the generated surface, its schema derived from types, and `application/problem+json` as its error shape ([`35`](35-standards-landscape.md) §35.5 item 3) — with round-trip tests beside them. The family design, what a consumer configures, and the foreign-reader gate are [`101`](101-the-public-surface.md) |
| **gRPC / Protobuf** | **Chartered** | With `@public(grpc)` — [`101`](101-the-public-surface.md) §101.10 stages it in the second wave (Phase 5), after `rest`. Nothing today; `beck-rt/src/telemetry.rs` deliberately avoids tonic/prost, and the gate's foreign client stays a dev-dependency for the same reason |
| **Model Context Protocol** | **Chartered** | With `@public(mcp)` (Phase 4, [`101`](101-the-public-surface.md) §101.5): the command union rendered as tools, views as resources, effect rows populating the tool annotations. The gate is the official MCP SDK driving the emitted server |
| **AsyncAPI 3** | **Chartered** | With `@public(events)` (Phase 5, [`101`](101-the-public-surface.md) §101.6) — the channel description whose envelope half is the CloudEvents row below |
| **OAuth 2.1 / OpenID Connect Core + Discovery** | **Partial** | The relying party is built ([`48`](48-identity-report.md)) with its own adversarial vectors — `beck-cli/tests/oidc.rs`: discovery, JWKS, algorithm confusion, `aud`/`iss`/`exp`, key rotation, the PKCE code flow, a plaintext issuer refused. **The OpenID Foundation conformance suite has not been run**; chartered with a pre-1.0 trigger |
| **WebAuthn** | **Corrected — no claim today** | The earlier row claimed inheritance "via the bundled IdP (Keycloak)". No IdP is provisioned today: [`48`](48-identity-report.md) built a dev provider, a symmetric provider and a relying party, and `identity = managed()` is a declaration whose Keycloak provisioning ([`10`](10-decisions.md) D6) is still InfraGraph design. Until that lands, authenticator requirements belong to whatever issuer a deployment names, and Beck's obligation ends at validating what that issuer signs |
| **CloudEvents 1.0** | **Chartered** | Both halves now have a position: inbound with `ingest(source)` ([`30`](30-bounded-contexts-and-microservices.md) §30.4), outbound with `@public(events)` ([`101`](101-the-public-surface.md) §101.6, Phase 5) — events cross the boundary as CloudEvents in both directions, identity derived from `(context, seq)` |
| **W3C WebAssembly — pinned to core 3.0** | **Partial** | The pin adopts [`35`](35-standards-landscape.md) §35.3: 3.0's guaranteed `return_call` is what lets a WASM tier honour the proper-tail-call guarantee [`27`](27-the-walls-come-down-report.md) made language-level. What exists is Mode B's kernel ([`94`](94-the-client-report.md)) — **core WebAssembly only**, a `wasm32-unknown-unknown` module with four `i32` exports, no WASI, no component model — and, since [`103`](103-the-wasm-emitter-report.md), an emitter that **writes** core WebAssembly and uses 3.0's `return_call` for exactly the reason this row pins the version. Nothing here runs the spec suites: what `wasm_backend.rs` asserts is agreement with the *language's* semantics, and a real engine validating the emitted module on every run is the cheap half of conformance rather than conformance. The spec-suite obligation stays chartered against the emitter's heap half ([`08`](08-roadmap.md) §8.5.4); WASI pins to the 0.2.x line when a WASI target exists |

## 12.4 Web output: accessibility as a compile-time property

**Chartered, the whole section — nothing is built.** This section is kept because the design claim
is real and rare: the `ui:` macro emits a typed tree, so **WCAG 2.2 AA and ARIA checks are
*checkable* at compile time** in a way no template language can match. But checkable is not
checked: there is no accessibility diagnostic among the 137 codes, no contrast check over any style
value, and no axe-core anywhere in the tree. Until an artefact exists this is a design advantage,
not a conformance claim. The prerequisite is now met: `ui:` **has** a vocabulary
([`104`](104-styling-and-the-component-library.md) §104.8), so the tree these checks would run over
is one whose element and attribute names mean something — `beck_macro::vocabulary::ELEMENTS` is
what tells an `img` from a `button`, and it is a table these checks read rather than one they
bring.

The positions ([`08`](08-roadmap.md) §8.5.4's standards ledger, then Phase 4):

- **The first three checks** — an `img` without alt text, a button without an accessible name, a
  form input without a label — are a small artefact over the existing `ui:` tree, each a compile
  error with the escape hatch `@a11y(exempt, reason=...)`, lintable and auditable. These need
  nothing that is not built, and since the vocabulary landed they need no table of their own
  either: `B0217`/`B0218` are the shape each of them takes.
- Colour-contrast over statically-known style values, and runtime conformance (focus preservation,
  live-region announcements) tested with axe-core in the e2e suite: **Phase 4**, with the client
  polish it audits.
- **Core Web Vitals**: what is gated today is *bytes*, not vitals — the CI `budgets` job holds the
  thin client under 10 KB and Mode B under 150 KB, brotli-compressed. Lighthouse/LCP/INP/CLS gates
  on the example apps are Phase 4, already scheduled in §8.4.

## 12.5 Data tier

| Standard | Status | Where it stands |
|---|---|---|
| **PostgreSQL wire protocol (pgwire)** | **Verified, partly conformed to — and the scope is stated** ([`23`](23-incremental-views-report.md)) | The startup exchange, the simple query and the parameterless extended query in both text and binary format, verified in CI against `tokio-postgres` (`beck-cli/tests/read_models.rs`) — a real driver, not a client written beside the server ([`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md)). `psql`'s backslash commands are **not** supported, because they query `pg_catalog` and this SQL has no joins; JDBC and BI drivers are untried. §23.19 is the row-by-row list |
| **Apache Arrow + Parquet** | **Chartered** | No dependency and no artefact. Conditioned on the data tier's means of combination ([`99`](99-the-data-tier-means-of-combination.md)) and the analytics substrate — the second of which now has a position, [`08`](08-roadmap.md) §8.5.4's G item, rather than the standing condition this row used to name; official interop test files enter with the first reader or writer |
| **SQL** (PostgreSQL dialect, documented subset) | **Partial** | We conform to Postgres-as-spec rather than abstract SQL:2023, and say so honestly. Generated SQL runs in CI against **one pinned major** (`postgres:17`), not the "pinned majors" previously claimed; a supported-majors matrix is chartered with the operator's version policy (Phase 4) |

## 12.6 Containers, supply chain, and the "prove the build" story

This is where Beck's reproducibility premise meets external, auditable yardsticks — and where the
strongest artefacts were built without rows, while some rows had no artefacts. Both directions are
corrected below.

| Standard / framework | Status | Where it stands |
|---|---|---|
| **OCI image / artifact specs** | **Partial verified** | `beck image` writes the layout, index and blobs in one process ([`92`](92-supply-chain-and-release-report.md)); the layer is read back by the system `tar` and the signature by `openssl`, because a writer checked by its own reader agrees with itself (`beck-cli/tests/image.rs`); CI's `the-image-builds` job asserts digest determinism. The **distribution spec** waits on a registry push existing at all — chartered with Phase 4's managed-cloud path, whose item 2 is that push |
| **Reproducible Builds** | **Partial — the sentence had outrun the artefact** | What runs: a rebuild-and-diff of the image digest, same runner, in CI. The previously-claimed two-independent-runner double build of every release is chartered on the release workflow — which **has never run; no tag has been pushed** (§92.13). The residual gaps stay named rather than papered over: `build.rs`, proc macros, the `cc` crate. Diverse double-compiling belongs to the self-hosting milestone ([`42`](42-security-assurance.md) §42.7) |
| **SLSA v1.2** | **Partial** | Provenance is **emitted**: in-toto attestation over the release listing via the release workflow (`actions/attest` over `SHA256SUMS`), a Sigstore certificate whose identity is the workflow, the public transparency log ([`92`](92-supply-chain-and-release-report.md), [`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md)) — for a release nobody has yet tagged. The **level is not claimed**: the builder's isolation is GitHub's and unaudited, and a user's `beck build` attests nothing. The **Source track** is chartered — mostly a repository-settings exercise, worth claiming because it is nearly free |
| **SBOM** | **Partial — the row was wrong** | `beck sbom` emits **CycloneDX 1.6**, gated by `beck-cli/tests/supply_chain.rs`. Not SPDX, not 1.7, not signed, and CISA's 2026 Minimum Elements are not asserted. The signature lands on the release pipeline ([`28`](28-releases-and-deployment.md)); the CycloneDX 1.7 / SPDX 3.0 move is an ADR when the toolchain moves ([`35`](35-standards-landscape.md) §35.3) |
| **Licence and advisory policy** | **Verified** | `compiler/deny.toml` — a licence allow-list, `yanked`/unknown-registry/unknown-git denied, wildcard versions denied, an empty ignore list — run by CI's `licences` job ([`adr/0004`](adr/0004-full-cargo-deny-gate.md)). This is [`07`](07-dependencies.md)'s open-source constraint machine-enforced, and the one supply-chain standard that has gated every commit since it landed. It had no row here until the audit |
| **Toolchain and dependency pinning** | **Verified** | `rust-toolchain.toml` (1.94.1) in both workspaces, `Cargo.lock` committed, `--locked` builds in the workflows. Also previously uncited |
| **Trusted publishing on crates.io** | **Chartered — an open one-way door** | Short-lived OIDC credentials configured **before** the first `cargo publish`; a token used once is a risk already taken. [`08`](08-roadmap.md) §8.5.2 and §8.5.3 hold it as one of the project's two one-way doors |
| **Sigstore / cosign** | **Partial verified** | Keyed signing for images is built and CI-verified — the `the-image-builds` job verifies the signature **and refuses a different image**. The release listing itself carries **no signature**: `beck-cli/tests/pending_security.rs` asserts that absence so building the control forces correcting the claim ([`adr/0027`](adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md), [`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md)). SBOM and provenance signing land with the release pipeline. **Package signature verification does not exist and is the largest security gap, named as one** ([`08`](08-roadmap.md) Phase 3 exit table) |
| **OpenSSF Best Practices badge + Scorecard** | **Chartered** | Nothing runs. A Scorecard workflow is a small artefact — the standards ledger has it |
| **REUSE 3.0** | **Chartered** | Today: a single MIT `LICENSE` and workspace licence fields. Per-file metadata is a small artefact — the standards ledger has it |

## 12.7 Security: map the guarantees to the industry's vocabulary

Beck's type-level guarantees ([`03`](03-type-and-effect-system.md) §3.5) are stated internally in
our terms; conformance means restating them in the auditor's terms and testing that mapping.

- **The negative tests exist and are the strong half — Verified.** `beck-cli/tests/security.rs`
  holds one refused program per row of §3.5's table (secret flow to the client, XSS-as-text, egress
  against the derived policy, the capability chokepoint, and more); `front_end_bound.rs` is CWE-674
  (uncontrolled recursion) with [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md) as
  its fix; `macro_bomb.rs`, `grammar_fuzz.rs` and `identifiers.rs` cover the front door. And
  `pending_security.rs` is the instrument this document previously failed to cite: **the absences
  of [`43`](43-threat-model.md) §43.4 asserted as absences**, six tests that go red when an unbuilt
  control is built, so closing a gap forces correcting the claim in the same change.
- **The mapping is the missing half — Chartered, as one artefact.** No CWE number appears anywhere
  in the tree, and no ASVS matrix exists. Rather than two documents, the charter adopts
  [`35`](35-standards-landscape.md) §35.2's shape once: **a single vulnerability matrix** over
  ISO/IEC 24772-1's catalogue and the CWE claims (CWE-79, -89, -915, -639, -200, -352, -674), each
  entry marked *unrepresentable by construction* — naming the negative test that proves it —
  *possible* (with the guidance 24772 asks a language to give), or *not applicable* (with the
  reason), and a gate asserting every named test exists. OWASP Top 10:2025's A03 (supply chain —
  §12.6) and A10 (exceptional conditions — CWE-674's home) fold into the same matrix. Position:
  the standards ledger, feeding Phase 4's external security review. The full **ASVS 5.0** matrix
  (~350 requirements) is Phase 4's, written with that review, never against 4.x
  ([`42`](42-security-assurance.md) §42.8).
- **The threat model is a precondition for every row here — Verified.** A control matrix answers
  "against whom" or it answers nothing: [`43`](43-threat-model.md) is that document, and
  `SECURITY.md` states the ISO/IEC 29147 and 30111 disclosure policy that receives what the matrix
  misses.
- **NIST SP 800-63B — corrected, no claim today.** The earlier row delegated it to a "bundled
  IdP" that is not provisioned: `identity = managed()` is a declaration, and D6's Keycloak
  provisioning is still InfraGraph design. Until it lands, authenticator requirements belong to
  the deployment's issuer.
- **The macro sandbox — Verified, and no longer vacuously.** It was: expansion was substitution
  over a template, so there was no name a macro body could use and nothing to check. Macro bodies
  now run Beck at compile time ([`102`](102-the-macro-interpreter-report.md)), so
  [`02`](02-syntax.md) §2.4's capability-restricted environment is a claim rather than a shape, and
  `beck-cli/tests/macro_sandbox.rs` is the gate that landed **with** the interpreter rather than
  after it: the compile-time environment is a whitelist, the prelude's effectful primitives are
  refused by name, and an enumeration over the prelude fails when a primitive that performs an atom
  is added without the interpreter learning about it.

## 12.8 Observability and operations

| Standard | Status | Where it stands |
|---|---|---|
| **OpenTelemetry** | **Partial — restated to what D17 actually decided** | Telemetry is OTLP/HTTP JSON — metrics and logs, `BECK_OTLP_ENDPOINT` — which is the vendor-neutral wire the ecosystem ingests ([`08`](08-roadmap.md) §8.6). The module **deliberately does not adopt OTel's span model**: the log is the trace ([`10`](10-decisions.md) D17), and the correlation key is `beck.seq`. The earlier "every attribute uses the registry names" claim shrinks to the attributes actually emitted; a semantic-conventions check over those is a small chartered artefact in the standards ledger |
| **OpenMetrics / Prometheus exposition** | **Chartered** | Metrics are served today as JSON to the in-repo dashboard. A Prometheus exposition endpoint beside it is a small artefact — the standards ledger has it |
| **Kubernetes API conventions + Gateway API** | **Partial verified — stronger than claimed in one way, weaker in another** | Stronger: generated objects pass **real API-server admission** (`kubectl --dry-run=server`) against a k3d cluster with Gateway API CRDs in CI, and the skip is forbidden (`BECK_REQUIRE_CLUSTER=1`) — more than the kubeconform-class schema validation previously claimed. Weaker: **one cluster minor**, not the claimed N-2 matrix, which is chartered with the operator's version policy (Phase 4) |
| **12-factor principles** | **Reclassified: borrow, not conformance** | A checklist is not an executable artefact, so by §12.1's own rule this was never a conformance row. The properties that matter are held individually where they are real — config through `AppConfig`, statelessness-except-declared-`durable` by the semantics, disposability via drain when the operator exists |

## 12.9 Process standards we hold ourselves to

- **Formal spec for the semantic core, and TLA+ for the protocols** — **Chartered, and previously
  stated as present**: no `.tla` file exists and nothing model-checks in CI. The claim is
  repositioned as a **G-class gate**: the TLA+ specifications of the deploy choreography and
  subscription-resume protocols are written and model-checked **immediately before the operator is
  built** (Phase 4), never after — a protocol modelled after it ships is archaeology. The
  small-step semantics of the `Stream`/`Signal`/`fold`/placement calculus belongs to the Phase 5
  spec appendix. [`08`](08-roadmap.md) §8.5.4 records both.
- **Conformance suite as a product** — **Chartered (Phase 5, with the spec)**: published,
  versioned, runnable against any future alternative implementation — the spec's teeth outlive us.
  Its companion, adopted from [`35`](35-standards-landscape.md) §35.2: **implementation
  conformance statements** formalised from `Platform::unsupported` — every backend and platform
  declares its capabilities, the suite selects against the declaration, and an undeclared gap is a
  failure rather than a skip.
- **Public benchmark methodology** — **Partial, on schedule.** The suites are named and
  third-party ([`25`](25-benchmarks-and-expressiveness.md) §25.2, adopted as D18), and four are
  built and CI-run with skips forbidden where they matter: Are We Fast Yet and the Benchmarks Game
  verified against the original suites' own constants, the compile-speed budgets, and
  `compiler/xlang/` — the one place a Beck number sits beside another language's
  ([`adr/0006`](adr/0006-ci-measurements-lane.md)). TechEmpower, js-framework-benchmark, YCSB and
  Lighthouse are Phase 4; TPC-H/ClickBench Phase 5, conditioned on
  [`99`](99-the-data-tier-means-of-combination.md) — §8.4 is the schedule, and its rule stands:
  stand every harness up one phase before its number is publishable. §25.2 also records where the
  honest answer is that **no standard exists** — incremental view maintenance
- **Determinism is an instrument here too — Verified**: `beck-cli/tests/clock.rs` holds the
  runtime to one `SystemTime::now()` behind the injected clock, and the replay-determinism harness
  is the mechanised statement of the central promise. Neither had a row; both are load-bearing.

## 12.10 Expressiveness: the premise, made falsifiable

Every standard above measures a shipped artefact. None of them measures what
[`01`](01-vision-and-premise.md) §1.1 and [`10`](10-decisions.md) D9 actually claim — that Beck is
SICP's three moves made into a language, and that a Python-shaped surface carries Lisp's power
without losing any of it. Until [`25`](25-benchmarks-and-expressiveness.md) there was **no artefact
that could falsify either**, which by §12.1's own rule made them marketing.

Two instruments, adopted as D18 — both **Verified as artefacts**:

- **Felleisen's criterion** (*On the Expressive Power of Programming Languages*, 1991) is the
  formal half: every special form SICP introduces is either recovered as a Beck macro — a *local*
  rewrite — or recorded as requiring a global reorganisation, which is exactly the 1991 definition
  of being less expressive. [`63`](63-expressiveness-report.md) is the result — six of seven forms
  recovered, `amb` conceded — and `beck-cli/tests/sicp.rs` holds the table with the code behind
  each verdict.
- **The SICP suite** is the empirical half, and it has an oracle: the book states its answers. It
  is in CI as an executable artefact ([`compiler/sicp/`](../compiler/sicp/), gated by
  [`beck-cli/tests/sicp.rs`](../compiler/crates/beck-cli/tests/sicp.rs)) — and its **refusals are
  asserted as well as its passes**, so a wall coming down is a failing test rather than something
  somebody notices.

**And one honesty the audit added, because this section is where it belongs — now half
discharged.** The macro system behind both instruments was **template macros only**: hygienic,
real, and enough for six of Felleisen's seven forms, but with no compile-time computation at all.
A macro body now runs Beck ([`102`](102-the-macro-interpreter-report.md)) and can construct,
inspect and return a `Node`, so *programs that compute programs* is backed rather than gestured
at. What is still not backed is the **run-time** half of code-as-data: a `quote` that survives
expansion is error `B0332`, so a `Node` is a compile-time value and not yet a value a running
program holds, and `derive`'s `.as_model()` and typed macros want the checker's answers, which
this interpreter runs before. So "a Python-shaped surface carries Lisp's power" is backed for the
notation, the special forms and compile-time metaprogramming, and **not for run-time reflection**.
[`08`](08-roadmap.md) §8.5.4 carries what is left.

The counting protocol (§25.5) is part of the standard, because lines of code is a real metric and
an easy lie: a third-party Scheme baseline pinned by commit, the same algorithm on both sides or
the exercise does not count, and three counts published together — lines, tokens, and lines
excluding type signatures — since Beck is typed and Scheme is not. A single headline number is the
tell that somebody chose.
