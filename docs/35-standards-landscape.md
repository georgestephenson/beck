# 35 — The standards landscape, mid-2026

> **The question**: beyond what [`12`](12-standards-and-conformance.md) already adopts, are there
> ISO/IEC, IEEE, Ecma, Open Group or consortium standards Beck should use — and should Beck
> publish its own?
>
> **This is a dated survey, and it keeps its date.** A survey's value is the evidence for a verdict
> rather than the verdict, so folding the surviving verdicts into the design documents and deleting
> this would keep the conclusion and throw away the argument — and the next person would re-survey
> from scratch. What is *adopted* lives in [`12`](12-standards-and-conformance.md) and
> [`10`](10-decisions.md); what is *scheduled* lives in [`08`](08-roadmap.md) §8.5. Nothing here is
> either by being written here.


This is a survey, not a charter change. [`12`](12-standards-and-conformance.md) §12.1's rule
governs: a standard enters the project only as an executable conformance artefact wired into CI.
This document decides *whether* a standard deserves that treatment; nothing below is adopted by
being written here. Statuses were checked against the publishers' catalogues in July–August 2026
and are dated claims — a standard's edition, unlike a design decision, changes under us.

The verdicts use four words consistently: **adopt** (build the artefact, then add the row to
[`12`](12-standards-and-conformance.md)), **borrow** (take a concept or a vocabulary, cite the
source, conform to nothing), **watch** (a dated pin, revisited on a named trigger), **decline**
(with the reason stated, so the question is not reopened by accident).

## 35.1 The charter's IEEE 754 row, checked against the numeric tower that now exists

Surveying outward forced a look inward, and the charter's first row is the one that needs it. The
ground moved while the survey was being written: the reals landed in
[`27`](27-the-walls-come-down-report.md), so the row is now *partially* backed, and the
unbacked parts are nameable.

**What the row now has.** SICP's printed doubles are asserted as digit-for-digit equalities
([`27`](27-the-walls-come-down-report.md) §27.2): `sqrt(9.0)` prints
`3.00009155413138` on both sides because both are IEEE 754 binary64 running the same operation
sequence, and a fused multiply-add or a reassociation anywhere in the compiler would change the
last digit and fail the suite. That is precisely the cross-implementation oracle the charter row
asks its differential tests to be — supplied free by a textbook that states its answers, which is
the property §12.10 prizes.

**What it still lacks.** The row claims `f32`/`f64` "across tiers — WASM and native must agree
bit-for-bit". There is one float (binary64) and one tier computing it — the evaluator; no WASM or
native backend exists ([`27`](27-the-walls-come-down-report.md) §27.10). The cross-tier half of the claim is
still design. [`14`](14-review-findings.md) F9's price — a correctly-rounded deterministic libm —
has since been **paid**: `sin` and `cos` are computed in the runtime library rather than asked of
the host ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)),
`sqrt` is IEEE 754's own correctly-rounded answer and stays each target's instruction, and `pow`
does not exist yet and would arrive the same way. Exact rationals and bignums remain refused
(`rational.beck`); the tower has one floor of three,
said plainly in §27.10.

**The deviation from IEEE comparison is now stated — but in a report, and reports are history.**
The first draft of this survey observed that float equality deviated from IEEE 754 §5.11 with no
decision recording it. §27.8 then found the stored order was worse than undocumented — raw
`f64::to_bits` answered `-1.0 < 1.0 → false` — fixed it with the monotone transform (the shape of
IEEE 754 §5.10's `totalOrder`), and canonicalises `-0.0` to `0.0` and every NaN to one NaN on the
way into a `Value`. So Beck's `==` on reals is structural and `NaN == NaN`, stated in §27.8 as a
deviation a porter must know about. What survives of the survey's recommendation: a report is
history by charter, so when the Phase 5 spec exists the comparison semantics belong in it as
current state — arithmetic per IEEE 754-2019 clause 5, `Value` identity and ordering per the
canonicalised total order — and the deviation earns a D-number in [`10`](10-decisions.md) so it
reads as chosen rather than archaeological. One more dated fact, requiring nothing: the next
IEEE 754 revision is projected for [~2029](https://standards.ieee.org/ieee/754/6210/), and
ISO/IEC 60559:2020 is the same text with an ISO cover — cite both when the spec cites either.

The Unicode row has the same dated-pin problem in miniature: it says "Unicode 15+", and
[Unicode 17.0](https://www.unicode.org/versions/) (2025) is current, with
[UAX #31](https://www.unicode.org/reports/tr31/) reclassifying scripts under it (Bopomofo moved
out of Recommended, newer scripts Excluded). "15+" is the wrong shape for a conformance claim —
identifier rules that float with the Unicode version are a compatibility hazard. The policy should
be: the Unicode version is pinned per Beck release, bumped deliberately, and the identifier
profile adds [UTS #39](https://www.unicode.org/reports/tr39/)'s General Security Profile
(confusable and mixed-script detection) — a language with hygienic macros over a homoiconic AST
should not be spoofable at the identifier layer.

## 35.2 The starting points, assessed

Seven families were proposed as starting points. Each gets a verdict.

### ISO/IEC 24772 — avoiding vulnerabilities in programming languages: **adopt, as a mapping**

The series (JTC 1/SC 22/WG 23) matured in 2024: the language-independent catalogue was promoted
from Technical Report to International Standard as
[ISO/IEC 24772-1:2024](https://www.iso.org/standard/83629.html), with language-specific parts for
[Ada (TR 24772-2:2020)](https://www.iso.org/standard/71092.html) and C (TR 24772-3:2020)
published and further parts (Python among them) in development.

This is the ASVS move from [`12`](12-standards-and-conformance.md) §12.7 played at the language
layer instead of the application layer, and Beck's entire premise — bug classes made
unrepresentable — is *stated in this catalogue's vocabulary*. The artefact: a maintained
entry-by-entry matrix over 24772-1's catalogue, each entry marked **unrepresentable by
construction** (with the negative test proving it — the §12.7 discipline), **possible**
(with the guidance 24772 asks a language to give), or **not applicable** (with the reason). Type
confusion, unchecked array indexing, dead stores across tiers, injection — a reviewer from any
safety-critical shop can then read Beck's claims in the only cross-language vulnerability
vocabulary that exists.

**The matrix exists and this half of it does not** ([`43`](43-threat-model.md) §43.8): the seven CWE
rows are written and gated, and the 24772-1 mapping is blocked on the standard's text, which is
paywalled and not in this tree. That is a real blocker rather than a scheduling one — it needs
somebody with the document open — and it is recorded in the matrix itself so a reader does not
mistake the gap for an oversight. Long term, a language-specific "Beck part" written in the series' own
format is a document worth publishing ourselves (§35.4); an actual ISO part needs a WG 23 work
item and is not worth the committee overhead before 1.0.

### ISO/IEC TR 10182 — guidelines for language bindings: **borrow, at FFI design time**

[TR 10182:2016](https://www.iso.org/standard/67465.html) (with its companion TR 14369:2018 on
language-independent service specifications) classifies binding methods and gives guidelines for
future bindings. It is guidance, not a conformance target — there is nothing executable to
conform to. Its moment is Phase 3–4's C ABI FFI ([`09`](09-risks-and-open-questions.md) §9.4
"table stakes"): read it when designing `extern def`, alongside the fact that "the C ABI" is
itself not one standard but ISO/IEC 9899:2024 (C23, published October 2024) plus per-platform
psABI documents. The FFI's conformance artefact will be differential tests against real C
libraries, not a claim against a TR.

### RM-ODP — ISO/IEC 10746 / ITU-T X.901–904: **borrow the vocabulary, decline conformance**

The Reference Model of Open Distributed Processing (parts published 1998–2010, stable since) is
the most conceptually on-target standard in this survey and the least adoptable. Its five
viewpoints and, above all, its catalogue of **distribution transparencies** — access, location,
replication, migration, failure transparency — are a 1990s ISO committee describing what
placement-as-effect, the cost model, and deploys-riding-the-stream actually deliver. Beck is, in
RM-ODP's terms, a language where the transparencies are compiler obligations instead of middleware
promises.

But there is no RM-ODP test suite, no reference implementation, no conformance artefact anywhere
in the world — by §12.1's rule it cannot enter the charter. The right use is two sentences of
prior art in [`01`](01-vision-and-premise.md)'s lineage table and the borrowed word
*transparency* where [`15`](15-scale-and-distribution.md) needs it, with the citation. That is
not nothing: reviewers from the distributed-systems tradition will recognise the frame, and the
frame is exact.

### ISO/IEC 15408 / 18045 — Common Criteria: **decline for the project, note for the ecosystem**

[CC:2022](https://www.iso.org/standard/72891.html) (15408 parts 1–5 plus the 18045 evaluation
methodology, 2022 editions; part 5 currently under revision) certifies *products* through
*licensed laboratories* under national schemes. A language is not a Target of Evaluation, and a
research compiler buying an EAL evaluation would be theatre. Declined. The honest one-sentence
note: a Beck-built product entering evaluation would find that `beck build` already emits much of
what a lab asks for — reproducible builds, SBOMs, provenance, an effect-row statement of what the
program can reach — and making that mapping explicit is ecosystem enablement for later, not a
project commitment now.

### ISO/IEC 17000 series — conformity assessment: **decline now, revisit only for a mark**

The 17000 series ([17000:2020](https://www.iso.org/standard/73029.html) vocabulary, 17025 for
laboratories, 17065 for product certification bodies, 17067 for scheme design) governs bodies
that certify, not projects that are certified. It becomes relevant on exactly one trigger: if
`beck-conformance` ever grows a trademark-gated program — "Certified Beck", the
[CNCF Certified Kubernetes](https://www.cncf.io/training/certification/software-conformance/)
model of self-run suite plus trademark licence, which Beck already meets from the consumer side
in [`beck-infra`'s conformance tests](../compiler/crates/beck-infra/tests/conformance.rs). If
that day comes, design the scheme with 17067 open on the desk. Until then, nothing.

### ISO/IEC 9646 — conformance testing methodology: **borrow its one great idea**

The OSI conformance-testing framework (1991–1998, confirmed and frozen since) is a museum piece
as a standard, and one exhibit is worth taking home: the **Implementation Conformance Statement**
(ICS/PICS) — a machine-readable declaration, per implementation, of which capabilities it
supports, against which test selection is then made. Beck already has a proto-ICS and doesn't
call it one: `Platform::unsupported` in
[`compose.rs`](../compiler/crates/beck-infra/src/compose.rs) *declares* that Docker Compose has
no network policy rather than silently skipping it. The borrow: formalise that — every backend
and platform ships a conformance statement listing supported capabilities, the conformance suite
selects against it, and an undeclared gap is a failure rather than a skip. That is also the
mechanism a future second implementation would use to claim partial conformance honestly
(§35.4).

### ISO/IEC/IEEE 29119 — software testing: **decline**

The series ([29119-1:2022](https://www.iso.org/standard/81291.html) concepts, -2 processes and
-3 documentation (2021), -4 techniques, -5 keyword-driven (2024 edition)) standardises test
*process and documentation*. Beck's testing position ([`13`](13-testing.md)) is the opposite
bet: the free oracles — determinism, differentials, replay, TLA+ — make the tests themselves the
documentation, and [`29`](29-domain-driven-design.md) D19 already refused prose-as-spec once
("the test is the spec; prose is derived, not parsed"). Adopting a documentation standard for
tests would reverse that decision without saying so. Declined with one salvage: part 4's
technique catalogue (boundary-value, equivalence-class, combinatorial) is a decent checklist to
hold against the type-directed generator in
[`gen.rs`](../compiler/crates/beck-core/src/gen.rs) as it grows.

## 35.3 What the survey adds that the charter lacks

Standards not on the proposal list, found by walking the same layers
[`12`](12-standards-and-conformance.md) walks. Each is a candidate: it enters the charter when
its artefact exists, not before.

| Standard | Status, mid-2026 | Why Beck | Proposed artefact | Verdict |
|---|---|---|---|---|
| **POSIX.1-2024** ([IEEE 1003.1-2024](https://standards.ieee.org/ieee/1003.1/7700/), SUS Issue 8) | Published June 2024 — first revision since 2017 | Beck's containers must die well: Kubernetes termination is SIGTERM → grace → SIGKILL, and drain/resume ([`06`](06-kubernetes-and-packaging.md)) rides it; CLI exit statuses and utility argument syntax are POSIX conventions | Signal-handling contract test in `beck-rt` (SIGTERM begins drain) — still owed, and blocked on the choreography defining what drain is. **The CLI exit-status table is built**: `docs/reference/cli.md` carries it, generated from the compiler's own constant, and `docs.rs::the_exit_status_table_is_what_the_binary_does` drives the binary against the *published* table in both directions — a status no row names fails, and a row nothing produces fails | **adopt the clauses we touch** — the runtime and CLI surface, not the 4,000 pages |
| **RFC 9457** (Problem Details for HTTP APIs) | Current (obsoleted RFC 7807, 2023) | Every `@public(rest)` surface needs an error body; the schema-derived OpenAPI 3.1 story is incomplete without a standard error shape | Generated REST errors emit `application/problem+json`; round-trip tests beside the RFC 8259 ones | **adopt** with the REST emitter |
| **RFC 9557** (IXDTF — RFC 3339 with time zone/calendar suffix) | Published 2024 | The envelope `at` stays RFC 3339 UTC — replay wants an instant, not a zone — but UI-edge formatting will meet zoned timestamps | None for the wire format (deliberately); adopt only if a zoned type ever enters the standard library | **watch** |
| **WebAssembly 3.0** ([completed September 2025](https://webassembly.org/news/2025-09-17-wasm-3.0/)) | The live W3C standard; GC, 64-bit memories, exception handling, and **guaranteed tail calls** | The charter's "W3C WebAssembly core" now has a version worth naming. Proper tail calls became a language guarantee in [`27`](27-the-walls-come-down-report.md) and are already load-bearing (`fixed_point` iterates to convergence, [`27`](27-the-walls-come-down-report.md) §27.2) — so every future backend inherits the obligation, and 3.0's guaranteed `return_call` is what lets a WASM tier honour it without a trampoline. A compiling backend also recovers the 13% the evaluator paid for it. WASI: pin the 0.2.x line; 0.3's native async is in progress, not shipped | Pin the charter row to 3.0; when the WASM backend exists, its tail-call lowering tests cite the 3.0 spec suite | **adopt the pin** |
| **Unicode 17.0 + UTS #39** | 17.0 current (2025); UAX #31 script classifications moved under it | §35.1 — identifier security for a macro language | Pinned Unicode version per release; confusables/mixed-script vectors in the conformance suite | **adopt** with the identifier rules |
| **SPDX 3.0** (SPDX 2.2.1 is [ISO/IEC 5962:2021](https://www.iso.org/standard/81870.html)) | 3.0 published 2024; 2.x remains the ISO-anchored, tooling-dominant line | This survey's draft believed the charter pinned SPDX 2.3; the audit found `beck sbom` emits **CycloneDX 1.6** and no SPDX at all, which [`12`](12-standards-and-conformance.md) §12.6 now records | Keep what is emitted honest in the charter; the CycloneDX 1.7 / SPDX 3.0 switch is an ADR when the toolchain moves | **watch** |
| **SLSA v1.1** | Clarifying revision of v1.0 (2025) | The charter targets "v1.0 Build L3"; same levels, tightened wording | Update the reference when the supply-chain bullets are built — **they are, since this survey was written** ([`92`](92-supply-chain-and-release-report.md)), and the reference now reads v1.2 ([`12`](12-standards-and-conformance.md) §12.6, corrected at audit; no level is claimed there either way) | **watch** |
| **EU Cyber Resilience Act** (Regulation 2024/2847) | In force; vulnerability-reporting obligations began phasing in from September 2026, full obligations December 2027; CEN/CENELEC harmonised standards in drafting | Not a standard — the *reason* standards will be demanded of anything Beck-built sold in the EU. Beck's chartered posture (SBOM, signing, reproducibility, coordinated disclosure) is the evidence shape the CRA asks manufacturers for | None yet; revisit when harmonised standards publish | **watch** |
| **ISO/IEC 10967** (Language-Independent Arithmetic, parts 1–3) | Dormant but not withdrawn | The only ISO vocabulary for specifying how a numeric tower's integers, floats and conversions relate. The reals floor is built ([`27`](27-the-walls-come-down-report.md)); read this once when the rationals/bignums floors are attempted (`rational.beck` is that wall), then set down | None — read at spec-writing time | **borrow** |

Considered and declined, one line each, so the survey is falsifiably complete rather than
selectively flattering: **ISO/IEC/IEEE 12207 / 15288** (lifecycle process standards — Beck's
process artefacts are its harnesses and this repo); **ISO/IEC 25010:2023** with 25002/25019 (the
SQuaRE quality model — vocabulary for brochures, no artefact); **ISO/IEC 5055** (CISQ code
quality — measures C/Java-family weaknesses, not applicable); **ISO/IEC 27001** (certifies
organisations, not languages); **ISO 8601-1/-2:2019** (already present via the RFC 3339 row,
which is the profile that matters on a wire); **ECMA-404** (RFC 8259 already carries the JSON
row); **Linux Standard Base / ISO/IEC 23360** (dormant upstream; OCI containers made it moot —
Beck's images are distroless by design); **ISO/IEC 9075 SQL:2023** (the charter already declines
it by name for Postgres-as-spec, §12.5, and that honesty holds).

## 35.4 Should Beck publish its own standard?

Yes — and the charter already chose the right form, so the real content of this answer is what
the 2026 landscape says about *route* and *timing*.

What §12.1 already commits to is precisely a self-published standard: a specification whose every
normative paragraph carries an ID, every ID referenced by a test in a published, versioned
conformance suite (`beck-conformance`, the test262 model), an RFC process with editions, stable
diagnostic codes, and — [`08`](08-roadmap.md) Phase 5 — the spec written against the
S-expression core with a stability and deprecation policy. [`28`](28-releases-and-deployment.md)
§28.2 is equally clear that *no stability promise exists until then*. Nothing in this survey
amends that; the survey's contribution is evidence from four routes that it is correct:

- **The ISO route** works where the committee *is* the implementers and the cadence is meant to
  be slow — C, C++, Ada, Fortran. Its cautionary tale is
  [ISO/IEC 30170:2012](https://www.iso.org/standard/59579.html), the Ruby standard: it describes
  a circa-1.8/1.9 language, was reviewed and confirmed unchanged in 2021, omits core library
  Ruby programs need, and no working Rubyist has opened it. A standard without a maintained
  conformance suite drifts into fiction while remaining officially "current" — the exact failure
  §12.1 is built to prevent.
- **The Ecma route** works where the spec is the coordination artefact between multiple
  competing implementations: [ECMAScript 2026, the 17th edition, was approved June
  2026](https://ecma-international.org/publications-and-standards/standards/ecma-262/) on an
  annual cadence, and it works *because of test262*, not because of the Ecma cover page.
- **The community route** is slow even with brilliant people:
  [R7RS-large](https://r7rs.org/) — the standard for the language SICP is written in — remains
  unfinished with a target of 2028. Design-by-committee ahead of implementations does not
  converge. (Beck's SICP oracle is unaffected: the book states its own answers.)
- **The self-published route** is where the momentum is: the Rust project
  [adopted the Ferrocene Language Specification in 2025](https://blog.rust-lang.org/2025/03/26/adopting-the-fls/)
  — a spec written *after* and *from* the implementation, with normative-paragraph IDs, because
  safety-critical consumers needed a document to qualify against. Java's JLS + TCK is the
  same architecture a generation earlier: spec, test kit, and a trademark gate on conformance
  claims. This is `beck-conformance`'s model, independently validated by the ecosystems that had
  to solve it under regulatory pressure. It is also no longer hypothetical for Beck:
  [`34`](34-generated-documentation-report.md) built the first of it — a language reference
  *generated from the compiler's own tables* and gated against drift, with the
  [error index](reference/errors.md) complete in both directions and the
  [effects page](reference/effects.md) evaluated from the same predicate the placement solver
  runs. A reference is documentation rather than specification, but the discipline — derived,
  gated, never a second account that can go stale — is the one the Phase 5 spec extends.

So: **publish our own, under the project, in Phase 5 as chartered.** No standards body, no
committee, no external submission. External standardisation has exactly two triggers worth
naming now, so the question is not reopened by vibes: a **second independent implementation**
(the spec's declared purpose — "the spec's teeth outlive us", §12.9), or a **regulated-industry
consumer** who needs a qualifiable document (the FLS trigger). If either fires, the Ecma
fast-track is the fit for an annually-editioned, test-anchored spec; the ISO route is not, on
the Ruby evidence.

The survey also adds three documents to what "Beck's own standards" should mean, all publishable
without a body: the **24772-1 vulnerability mapping** (§35.2 — Beck's premise in the industry's
catalogue), the **per-platform implementation conformance statements** (§35.2, borrowed from
9646 — already half-built as `Platform::unsupported`), and the **security conformance matrix**
already chartered in §12.7. A fourth moved from "should" to "begun" while this survey was being
written: §12.1's third instrument, stable diagnostic codes, now has its index —
[`34`](34-generated-documentation-report.md)'s 95 codes, gated complete in both directions, with
`beck explain error` in the `rustc --explain` shape — and what remains before it is the chartered
"documented, versioned contract" is versioning and a gate on the prose, which
[`34`](34-generated-documentation-report.md) §34.7 names itself. Together with the spec and suite
they are the full answer a standards-literate reviewer expects from a language that intends to be
taken seriously.

## 35.5 What this survey recommends, in one place

Each item enters [`12`](12-standards-and-conformance.md) only with its artefact, per §12.1.

1. **Numeric follow-ons, now that the reals are built** ([`27`](27-the-walls-come-down-report.md)):
   the `NaN == NaN` / `-0.0` canonicalisation deviation moves from §27.8 — a report, which is
   history — into the Phase 5 spec as current state, with a D-number; ISO/IEC 10967 read once
   when the rationals/bignums floors are attempted (§35.1, §35.3). F9's deterministic libm was the
   third item here and is **done**
   ([`adr/0031`](adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md)).
2. ~~**Identifier rules**: pin the Unicode version per release; add UTS #39's security profile
   with conformance vectors (§35.1).~~ **Done** — `beck_syntax::security` pins the version and
   states the profile Beck is at (**ASCII-Only**, UTS #39's strictest restriction level, satisfied
   by construction rather than by filtering), and `beck-cli/tests/identifiers.rs` is the vector
   suite, grouped by the attack each vector defeats. It also closed the half an identifier
   restriction does not reach: bidirectional confusion — Trojan Source, CVE-2021-42574 — is refused
   in both surfaces, with `\u{...}` as the escape a program uses when it wants one of those
   characters as a value.
3. **New charter candidates with small artefacts**: RFC 9457 problem details for `@public(rest)`
   errors; POSIX.1-2024 signal/exit contract for `beck-rt` and the CLI; the WebAssembly row
   pinned to 3.0, whose guaranteed `return_call` is how a future WASM tier honours the
   proper-tail-call guarantee [`27`](27-the-walls-come-down-report.md) made language-level (§35.3).
4. **The 24772-1 mapping** as a maintained matrix with negative tests, in the §12.7 style
   (§35.2).
5. **Implementation conformance statements** formalised from `Platform::unsupported` — declared
   gaps, never silent skips, suite selection against the declaration (§35.2).
6. **RM-ODP cited as prior art** for the transparencies vocabulary; no conformance claim
   (§35.2).
7. **Declined, with reasons recorded above so they stay declined**: Common Criteria, the 17000
   series (absent a certification mark), 29119, SQuaRE, lifecycle standards, LSB (§35.2,
   §35.3).
8. **Self-standardisation**: Phase 5's self-published, test-linked spec stands, and
   [`34`](34-generated-documentation-report.md)'s generated reference is its first built
   discipline — derived from the compiler, gated against drift, with the diagnostic-code index
   §12.1 charters; versioning and a prose gate are what remain of that instrument. External
   standardisation only on a second implementation or a qualification-driven consumer, and via
   Ecma, not ISO, if ever (§35.4).
