# 42 — Security assurance, August 2026

> **The question**: are Beck's security, thread-safety and memory-safety claims backed by
> evidence today — and what would have to be built before "watertight" is a word this project is
> entitled to use?

This is a survey and an assessment, in the sense [`35`](35-standards-landscape.md) established and
[`38`](38-literature-survey.md) reused: nothing below is adopted by being written here, and the
verdicts use the same four words — **adopt** (take it into a named piece of work), **borrow** (take
the concept, cite the source, build nothing yet), **watch** (a dated pin with a named trigger),
**decline** (with the reason recorded so the question is not reopened by accident).

It differs from those two surveys in one way. They looked outward; this one had to look inward
first, because a security posture cannot be assessed from a design document — only from what the
code does. So §42.1 is a **reading of the tree** at `0853b79` and §42.2 is a **measurement**, with
the commands that reproduce it; everything after them is the outside record checked against those
two rather than against the design documents. The external
statuses were confirmed by web search in August 2026 and are dated claims: a regulation's
commencement date and a specification's version, unlike a design decision, move under us.

One framing, before the detail. "Watertight" is not a state a language reaches; it is three
things a project either has or does not: a **threat model** that says what is in scope and what is
not, **mechanised evidence** for each claim inside that scope, and a **gate that goes red** when a
claim stops being true. Beck has the third instinct better than most projects its age —
`security.rs` tests each §3.5 property by writing the program it forbids — and has neither of the
other two written down. That is the shape of the gap; §42.11 says what each fix has to produce,
and [`08`](08-roadmap.md) §8.5 says in what order.

## 42.1 The posture today, measured

Taken from the tree, not from the design documents. Each row says what kind of claim it is:
**structural** (it follows from a construction, and no reviewer has to remember it), **tested**
(a harness fails when it stops being true), or **absent**.

| Property | Kind | Evidence |
|---|---|---|
| No `unsafe` in first-party code | structural | `unsafe_code = "forbid"` in the workspace manifest, and **twelve of the fourteen** crates carry `[lints] workspace = true` — inheritance is what makes a workspace lint real. ~~"all ten crates"~~, twice overtaken: [`97`](97-cranelift-report.md) §97.8 corrected it to twelve, and the count is now fourteen with two of them held to `deny` instead. Those two are `beck-wasm` and `beck-play`, and the reason is the same in both: rustc classifies `#[no_mangle]` as unsafe code, and a WebAssembly module must export something. Each carries one `#[allow]` per export attribute and **a test** that no other allow site, `unsafe` block or `unsafe fn` exists in it (`mode_b.rs`, `playground.rs`) — an exception whose extent is asserted rather than described. The lint is also why `beck-llvm` exists in the shape it does: a native backend that bound LLVM or ran compiled code in process would have needed `unsafe`, so it writes textual IR and spawns a process instead ([`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)), where `beck-clif`'s safe API cost nothing ([`adr/0024`](adr/0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md)) |
| No hand-written `Send`/`Sync` | structural | no `unsafe impl` anywhere; every cross-thread type is auto-derived, so the compiler is the reviewer |
| Single-writer state | structural | writes funnel through one sequencer task (`beck-rt/src/app.rs`); the state behind it is a `tokio::sync::RwLock`. Concurrency bugs are excluded by there being one writer, not by lock discipline |
| No lock held across `.await` | structural (by inspection) | the `std::sync::Mutex` uses in `log.rs` and `telemetry.rs` are each scoped inside a single expression with no `.await` between acquire and drop |
| No SQL injection | structural | every value is a bind parameter; the one `format!` in `log.rs` builds `($1,$2,$3)` placeholder *positions* and never a value |
| Deterministic collections, no hash flooding | structural | `Map`/`Set` are `BTreeMap`/`BTreeSet` — [`14`](14-review-findings.md) F8's resolution, and one of the few F-numbers that is built rather than designed |
| The §3.5 security properties | tested | `beck-cli/tests/security.rs`: one test per row of §3.5's table, each writing the program a mistaken or malicious author would write and asserting refusal **by diagnostic code**. This is the right pattern and the project should be proud of it |
| Licence and advisory policy | tested | `deny.toml` with an **empty** `advisories.ignore` (advisories are fixed at the root, and the comment says so), `yanked = "deny"`, `wildcards = "deny"`, `allow-git = []`; gated in CI ([`adr/0004`](adr/0004-full-cargo-deny-gate.md)) |
| Warnings are errors | tested | `RUSTFLAGS: -D warnings` with `cargo clippy --workspace --all-targets --all-features` |
| Escaping, text vs attribute | tested | `beck-core/src/html.rs` separates the two contexts; `a_todo_whose_text_is_a_script_tag_renders_as_text` is the negative test |
| Front-end recursion bound | **absent** | §42.2 |
| Authentication | **absent** | §42.6 |
| Request/connection limits | **tested** | [`83`](83-the-runtime-edge-report.md): the socket's limits are numbers this project argues for, and `Origin` is checked; `runtime_edge.rs` |
| Per-actor write quotas | **tested** | [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md): 600 events a minute, on by default; `runtime_edge.rs` asserts on the log's head. **Subscription quotas (F15) are still absent** |
| Macro expansion fuel | **absent** | F17, unbuilt: nothing in `beck-macro` bounds expansion |
| A written threat model | **absent** | §42.8 |
| A disclosure policy | **absent** | no `SECURITY.md`, no contact, no policy |

Two things this table is careful about. Everything marked *structural* is worth more than
everything marked *tested*, and the project has more structural rows than most language
implementations do at this stage — `forbid(unsafe)` plus a single-writer runtime plus ordered
collections removes whole categories rather than testing for them. And nothing here is graded on
design intent: [`14`](14-review-findings.md)'s `DESIGNED` statuses were read as what they say —
designed — and appear in this table as **absent** where the code is silent.

**A fuzz probe, labelled as what it is.** 600 iterations of byte-level mutation over
`compiler/corpus/*.beck` — insert, delete, duplicate and swap, from an alphabet including NUL,
U+202E and astral-plane characters — each result fed to `beck check`, produced **zero** crashes or
hangs: every input was either accepted or refused with diagnostics. This was a throwaway script,
not a harness; it is repeatable in method but not checked in, and the number should be read as a
smoke test rather than as one of the project's measurements. It is worth recording for two
reasons. It says something real about the lexer and the resolver, which is where a hand-written
layout algorithm would be expected to break first. And it is the reason §42.2 exists: random
mutation cannot *generate structure*, so the one crash class the front end actually has is
precisely the one this method is blind to. §42.11's grammar-aware fuzzing row is the version of
this that counts — and [`85`](85-what-the-generator-found-report.md) is it, built: a structure-aware
generator found **three** productions the recursion ceiling did not cover, plus the flat-block axis
[`64`](64-compile-speed-report.md) §64.4 had recorded and not fixed. This paragraph's caution was
right, and understated: byte mutation was not merely blind to the class, the class was populated.

## 42.2 The front end has no recursion bound, and ADR 0007 already argued why it needs one

`beck check` aborts on deeply nested input. The parser is recursive descent — `expr_bp` →
`primary` → `expr_bp` in `beck-syntax/src/parser.rs` — and nothing counts the depth; the `depth`
locals in that file are bracket- and indent-balance counters serving error recovery and the layout
algorithm, not recursion limits.

```
$ python3 -c "open('deep.beck','w').write('def f() -> Int:\n    return ' + '('*3785 + '1' + ')'*3785 + '\n')"
$ beck check deep.beck
thread 'beck-eval' has overflowed its stack
fatal runtime error: stack overflow, aborting
```

Measured thresholds, bisected on `0853b79`:

| Profile | Deepest accepted | Aborts by | Source size at the threshold |
|---|---|---|---|
| debug | 3,642 | 3,785 | ~7.6 KB |
| release | 50,000 | 55,000 | ~110 KB |

The debug figure is not stable across commits — it was ~3,000 one commit earlier, and moved when
[`27`](27-the-walls-come-down-report.md)'s work changed the checker's frames. That instability is not an aside;
it is the defect, restated as a number.

Three observations, in increasing order of how much they should sting.

**It is the well-known class, with a current CVE stream.** CWE-674, *uncontrolled recursion*: the
2026 record includes Python Protobuf (CVE-2026-0994), the `yaml` package (CVE-2026-33532, node
composition without a depth limit, overflowing at 1,000–5,000 levels of nesting) and Apache bRPC
(CVE-2025-59789). The remedy is equally well known — a counted depth with a diagnostic — and the
Scriban advisory (GHSA-p6q4-fgr8-vx4p) is the cautionary half: a depth limit added at one
production was bypassed through nested array initializers, because the bound belongs at the
recursion site, not at the grammar rule somebody happened to think of first.

**It is in Beck's own threat model already.** [`17`](17-playground.md) §17.3 says "the compiler is
the first sandbox" for playground submissions. A playground is a service that compiles source
chosen by an anonymous stranger; an ~8 KB file that aborts the process is a denial of service on
that service, and no amount of gVisor or Firecracker underneath makes the crash not happen — it
only contains it. In OWASP's 2025 revision this is A10, *Mishandling of Exceptional Conditions*, a
category added because this failure shape is common enough in production to deserve its own row.

**The project has already written the correct argument against it.**
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) chose a fixed count over a
stack-headroom budget for the evaluator, and gave determinism as the deciding reason: a headroom
budget "would let the same program over the same log succeed in a release build and refuse in a
debug one, or on one machine and not another." The front end has exactly the behaviour that ADR
rejected, in its worst form — not "refuse in debug, succeed in release" but *abort* in debug and
succeed in release, the two profiles more than an order of magnitude apart, with no span and
nothing catchable.

Worse, and this is the sharpest way to state it: `beck-eval::STACK_BYTES` is 64 MiB and
`DEFAULT_MAX_DEPTH` is 4,000, and the thread named in the crash above is that same declared
stack. **The 64 MiB sized to hold 4,000 evaluator frames is exhausted by roughly 3,600 parser
frames in a debug build.** The declaration is not merely incomplete; in one profile it is already
false, because only one of the two recursive consumers of that stack counts itself — and the margin
by which it is false moves every time the checker changes.

The bound belongs wherever the front end recurses over user-controlled structure — the parser, the
checker's type walk, and lowering — and it should be a count, for ADR 0007's reason, with the
ceiling and the stack it implies held to each other by a test the way the evaluator's already is.
Verdict: **adopt**, and [`08`](08-roadmap.md) §8.5 puts it first in Wave 0.

## 42.3 Memory safety: the posture is right, and the 2026 record says what it is worth

Two external facts settle how much further to go.

**The regulator's bar is already cleared, and clearing it is one paragraph of writing.** CISA and
the FBI's *Product Security Bad Practices* asks manufacturers of software in memory-unsafe
languages to publish a **memory safety roadmap** by January 2026, showing a prioritised plan for
network-facing and security-sensitive components. Beck's roadmap is a sentence: the compiler, the
runtime and every generated artefact are Rust with `unsafe` forbidden at the workspace root, and
the network-facing tier — the parser, the protocol decoder, the patch encoder — has no unsafe code
to prioritise. The honest version of that sentence names its limit in the same breath: the
dependencies underneath the network edge (hyper, tungstenite, tokio) are not `forbid(unsafe)`, so
the roadmap's answer for them is the ordinary one — pinned versions, an advisory gate with an empty
ignore list, and upgrade discipline — not an absence of unsafe code. What the project lacks is not
the property but the **statement** of it in the form an external reader expects. That is cheap and
worth doing (§42.11).

**A very large campaign says model checking will not find memory-safety bugs here.** The
verify-rust-std effort (AWS and the Rust Foundation, results through March 2026) integrated four
tools — Kani, ESBMC, Flux, VeriFast — into CI over the standard library, produced **16,748**
automatic proof harnesses of which **11,970** verified against Kani's supported classes of
undefined behaviour, and established **989** contract-verified proofs. It found **zero**
previously-unknown memory-safety vulnerabilities. The authors' own reading is the useful part:
the null result speaks to how much existing tests and Miri already catch, and the campaign's value
is *the guarantee*, not the bug-finding.

The conclusion for Beck is not "skip verification" but "aim it somewhere else". In a
`forbid(unsafe)` workspace, undefined behaviour is not the interesting adversary; **the placement
solver's security invariants are** — no `secret[T]` reaches a client partition, `durable` and
`ingress` are never placed client-side. Those are bounded, structural, and the exact claims the
project markets ([`09`](09-risks-and-open-questions.md) §9.6 already names them). Verdict on Kani:
**adopt, for the solver's invariants only**, with the memory-safety framing explicitly declined and
this paragraph as the reason. Verdict on Miri: **decline for now** — its yield against
`forbid(unsafe)` code is close to nil, and saying so with the campaign's numbers behind it is
better than running it out of habit.

## 42.4 Thread safety, and the determinism debt that is accruing now

The concurrency shape is sound (§42.1), and the honest description of *why* is architectural
rather than diligent: one writer, one lock, no locks across suspension points. Three notes.

**Lock poisoning is handled two ways.** `beck-core/src/engine.rs` recovers with
`PoisonError::into_inner`; `beck-rt/src/log.rs` and `telemetry.rs` use `.expect(…)`. A panic in
one thread therefore turns the telemetry ring into a process-wide panic. Small, and worth one
decision rather than two habits.

**`loom` is not needed yet, and should be pinned as a trigger rather than adopted.** Exhaustive
interleaving exploration earns its cost when there is a hand-rolled synchronisation protocol to
explore. There is not one: the sequencer is a channel. Verdict: **watch**, trigger = the first
lock-free structure or the first time two writers exist.

**DST is the item where delay has a price, and the price is being paid now.**
[`13`](13-testing.md) §13.4 calls deterministic simulation the crown jewel and
[`14`](14-review-findings.md) F11 records that it cannot be retrofitted — FoundationDB's lesson,
adopted as a hard constraint. The tree currently calls `SystemTime::now()` directly in
`beck-rt/src/app.rs` and in `beck-eval`. That is the ambient clock F11 warned about, and every
month it stays ambient the retrofit gets larger. The external record has moved decisively in the
same direction since the design documents were written — DST is now ordinary practice at TigerBeetle,
WarpStream, S2 and Antithesis's customers rather than an exotic FoundationDB technique, with mature
Rust substrate (`madsim`, `turmoil`) that did not exist when [`13`](13-testing.md) was drafted.

The cheap move is not to build DST. It is to make the clock **injected** — a source on the seam,
the way `Backend::stack_bytes` put a resource there — so that the retrofit F11 forbids never has to
happen. Verdict: **adopt the injected clock now**; **watch** DST proper, trigger = the second
runtime substrate or the first consistency bug that survives a week.

## 42.5 What §3.5's suite proves, and the one claim it does not

`security.rs` is the best security artefact in the repository, and its two stated rules — test the
attempt, not the API; where the property is structural, assert the absence — are worth keeping as
policy. Two things should be said about it that it does not say about itself.

**The theorem behind it has a name, and stating it would be worth more than another test.** "A
secret cannot reach the browser" is a *non-interference* claim: what a low observer (the client
partition) sees is independent of high inputs (`secret[T]`). That is the oldest well-studied
property in language-based security, and the current literature is unusually close to Beck's
shape — OOPSLA 2025's *Structural Information Flow: A Fresh Look at Types for Non-interference*,
and 2026 work on graded coeffect types for information-flow control carrying an Agda-mechanised
non-interference theorem. Beck's `secret[T]`, its internal facts and its declassifying chokepoint
are a two-point lattice with an explicit declassifier; that is a textbook object, and today it is
defended by twenty-five examples rather than by a statement over the calculus. Verdict:
**borrow now, adopt at Phase 5** — state non-interference over the core calculus in the spec, cite
the lineage, and let §42.11's Kani row discharge the bounded version of it against the solver.

**Placement is not identity, and the suite can be misread as covering both.** Every §3.5 test is
about where code and data may live. None is about *who is asking* — and the runtime has no answer
to that question at all (§42.6). "A capability required outside the chokepoint has no holder" is
true and proven; "only the owner may toggle their todo" is enforced against a self-asserted string.
The two read alike in a slide deck and are entirely different guarantees. The suite should say so
in its own header, which is a two-line change and prevents the most likely misquotation of this
project's security story.

## 42.6 The runtime edge

What an untrusted client can do to a running Beck app today:

- **Claim any identity.** `actor` arrives in the client's own `hello` frame; `protocol.rs` admits
  it in a comment ("Dev-mode identity … D6's OIDC relying party is Phase 3"). Every ownership check
  in every corpus program is therefore enforced against a value the caller chooses. This is honestly
  documented and correctly sequenced — it is Phase 3's work, not a defect — but it is *absent*, and
  §42.1 records it that way.
- ~~**Send a 64 MiB message.**~~ **Closed** ([`83`](83-the-runtime-edge-report.md)). The handshake
  passed `None`, so the limits were tungstenite's — 64 MiB a message, 16 MiB a frame. They are now
  256 KiB a message and a frame, 8 KiB of eagerly-allocated read buffer per connection rather than
  128 KiB, and a bounded write buffer; §83.2 is the argument for each number, and a unit test holds
  the file to them so a drift back to somebody else's defaults is a decision.
- ~~**Open a socket from any origin.**~~ **Closed** ([`83`](83-the-runtime-edge-report.md)). The
  upgrade compares `Origin`'s authority against `Host` and answers `403` when they differ. An absent
  `Origin` is allowed, because the attack needs a browser and every browser sends one; the scheme is
  not compared, because §6.5's gateway terminates TLS in front of a plaintext hop. §83.3 is why each
  went that way.
- ~~**Spend the log.**~~ **Closed for F3** ([`84`](84-a-quota-is-only-as-good-as-its-actor-report.md)):
  600 events a minute per actor, on by default, charged before the ingress queue and counted as
  `throttled` apart from the other two ways a proposal can fail. F15's connection quotas and F12's
  bounded deploy buffer are still unbuilt. §84.4 is what the F3 bound is worth in practice — a
  per-actor limit composes with whichever identity provider is configured, so under `DevIdentity` an
  attacker who rotates names is bounded by the *table* rather than by the limit.

None of this is surprising for a project at Phase 3, and none of it should be fixed by writing code
today. What it should do is stop being invisible: these are four F-numbers whose status in
[`14`](14-review-findings.md) is a word, and whose status in the code is silence. §42.11's
`pending_security` row makes the silence audible.

*Three of the four are closed*, and the mechanism worked for two of them: [`83`](83-the-runtime-edge-report.md)'s
pair were bullets whose gap was a **failing test**, so building them turned that test red and the
person who built them had to come back to this paragraph.

*It did not work for the third.* Both tests guarding F3 stayed **green** through the change that
closed it — one grepped for identifiers the implementation did not happen to use, and the other
proposed 200 events against a limit that turned out to be 600.
[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5 is the post-mortem, and the caveat it
leaves belongs to every grep-shaped test in `pending_security.rs`: a proxy for a control is defeated
by naming, and a behavioural test for an absence cannot be calibrated against a limit that does not
exist yet.

~~One smaller item, recorded so it does not rot: `dash.html`'s `esc` escapes `&<>` only, and the
graph renderer interpolates `class="${n.tier}"` into an attribute without it.~~ **Already fixed when
[`83`](83-the-runtime-edge-report.md) went looking**, and this paragraph is the correction. `esc`
escapes `&<>"'` and says in a comment why quotes are in the set — "half the interpolations below are
into attributes" — and the graph renderer writes `class="${esc(n.tier)}"`. Every other interpolation
in the file is escaped, a number computed by the layout, or a literal chosen by a ternary; §83.5
records the audit. The item rotted in the *other* direction: it was fixed and the record was not, so
this document has been describing a defect that stopped existing. That is the failure mode a
`pending_security` test does not have, and it is the argument for turning a paragraph into one.

## 42.7 Supply chain: where Beck is ahead, and the four rows that have moved

[`adr/0004`](adr/0004-full-cargo-deny-gate.md)'s posture — full `cargo deny check`, empty ignore
list, no git dependencies, wildcards denied — is ahead of most of the ecosystem. A second habit
paid off this year independently of it: CVE-2026-33056 (the `tar` crate, `unpack_in` following
symlinks to chmod arbitrary directories, exploitable by a malicious crate during extraction) is a
flaw in *Cargo's own* extraction path rather than in this workspace's dependency graph, so the
advisory gate would never have seen it — it was fixed in Rust 1.94.1, which is exactly what
`rust-toolchain.toml` pins. Worth stating explicitly, because it marks the boundary of what
`cargo deny` covers: the graph a project declares, not the toolchain that builds it.

Four rows in [`12`](12-standards-and-conformance.md) §12.6 have moved since they were written:

| §12.6 says | August 2026 | What to do |
|---|---|---|
| SLSA v1.0, target Build L3 | **v1.2**, adding a whole **Source track** (L1 version control → L2 tamper-resistant history with verified contributor identity → L3 enforced, documented protections → L4 no unilateral change to protected branches); backwards compatible, additive | Reference updated. The **Build track now has something to point at**: the compiler's release attests in-toto provenance over every artefact it publishes, signed by a Sigstore certificate naming the release workflow and logged publicly ([`109`](109-provenance-report.md), [`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md)) — a level is *not* claimed, because L3's isolated-builder requirement is a statement about GitHub's runners that this project has not audited. Source L3 is still mostly a GitHub settings exercise the project may already satisfy, and is worth claiming because it is nearly free |
| SPDX 2.3 SBOMs | CISA's **2026 Minimum Elements** (29 July 2026, with NSA, FBI and sixteen international agencies) supersede the 2021 NTIA baseline: ten new fields, four revised, plus a **mandatory digital signature**, component hashes, licences, generation tool and context; SWID dropped, **SPDX 3.0 and CycloneDX 1.7** named as the accepted formats | Retarget the row at the 2026 elements. The signature and hash requirements were said to land on [`28`](28-releases-and-deployment.md)'s pipeline rather than on the compiler; **half of that is now wrong** — the compiler signs ([`99`](99-supply-chain-report.md) §99.5), and what it signs is the **image**, not the SBOM. So the mandatory-signature element has machinery and is still unmet, and component hashes remain unmet for the reason [`92`](92-sbom-report.md) §92.5 gives |
| — (absent) | **Trusted publishing** on crates.io: short-lived OIDC credentials replacing long-lived tokens, plus a trusted-publishing-only mode | Add the row. This is the single highest-value account control for anything that will ever publish a crate, and the Shai-Hulud npm worm (September 2025, 500+ packages, self-replicating through stolen maintainer credentials, with stolen tokens outliving the cleanup) is the argument |
| Reproducible Builds, build twice and diff | Still right, and better tooled: `cargo-repro` for crate-level byte comparison, and rustc's own `repro-check` building stage-2 twice and comparing sysroots by SHA-256. Known residual gaps are `build.rs`, proc macros and the `cc` crate | Keep; name the known gaps rather than claiming bit-for-bit unconditionally |

One row is missing entirely and belongs here rather than in §12.6, because it is about the compiler
rather than about images: **bootstrappability**. A self-hosting Beck compiler is a Phase 5 premise,
and Thompson's trusting-trust attack becomes reachable the moment `beck` is built by `beck`. The
countermeasure is known, cheap to design in and expensive to retrofit: diverse double-compiling
(Wheeler), which reproducible builds make practical — build the compiler with an independent
toolchain, rebuild with the result, compare bit for bit. Verdict: **borrow now** (record the
constraint against the self-hosting milestone), **adopt when the compiler first compiles itself**.

## 42.8 Process: the two artefacts that do not exist, and the deadline that is six weeks away

**Coordinated vulnerability disclosure.** There is no `SECURITY.md`, no security contact and no
policy. For a pre-1.0 research compiler that is defensible; it stops being defensible the first
time an outsider runs `beck` on input they did not write, which the playground makes a product
feature. The conventional shape is settled: a policy aligned to **ISO/IEC 29147** (disclosure) and
**ISO/IEC 30111** (handling), reachable through `SECURITY.md` and, once anything is hosted,
`/.well-known/security.txt` per **RFC 9116**. Note for [`35`](35-standards-landscape.md): its ISO
survey covered 24772, TR 10182, RM-ODP, 15408/18045, the 17000 series, 9646 and 29119, and did not
reach 29147/30111 — which are the two ISO standards in that family Beck is most likely to actually
adopt, because they cost a page each. Verdict: **adopt 29147/30111 as a page-shaped policy**.

**The EU Cyber Resilience Act, dated.** [`35`](35-standards-landscape.md) pinned this as **watch**
and the dates have now arrived: conformity-assessment-body rules applied 11 June 2026, **reporting
obligations for actively exploited vulnerabilities and severe incidents apply from 11 September
2026** (24-hour early warning, 72-hour notification, 14-day final report), and the remaining
obligations on 11 December 2027. The verdict does not change and should be restated so it is not
mistaken for complacency: Beck is not a product placed on the EU market, and non-commercial open
source is outside the regulation's scope; **open-source stewards** carry lighter duties and no
fines. The trigger is unchanged — first commercial distribution — but the *evidence shape* the CRA
asks for (SBOM, signing, reproducibility, a disclosure process) is the same set §42.7 and this
section already recommend for their own reasons, which is the argument for doing them early rather
than under a deadline.

**Two frameworks worth adopting as checklists rather than as claims.** The OpenSSF **OSPS
Baseline** (current version 2026-02-19; 41 requirements across three maturity levels, grouped by
category — access control, build and release, documentation, governance, legal, quality, security
assessment, vulnerability management) is the closest thing to a ready-made scorecard for a
project of Beck's size, and OpenSSF Scorecard's 2026 roadmap makes Baseline conformance its first
evaluation use case. NIST **SSDF** (SP 800-218) is the practice vocabulary US procurement and the
CRA's harmonised standards both map onto; **v1.2 is in draft** (initial public draft December 2025,
comments closed January 2026, not final as of August 2026), so the right move is to map against
v1.1 and pin v1.2 as **watch**. Verdict on OSPS Baseline: **adopt as a self-assessment**, Level 1
now and Level 2 as the gap list — with the explicit note that a Baseline level is a claim about
*process*, and §12.1's rule still governs claims about the *language*.

**A correction to [`12`](12-standards-and-conformance.md) §12.7 while we are here.** It targets
OWASP **ASVS 4.x**; **ASVS 5.0** shipped 30 May 2025 (~350 requirements, 17 chapters, renumbered
IDs, mobile and IoT split out), so the control-by-control matrix §12.7 charters should be written
against 5.0 and never against 4.x. §12.7's CWE list should likewise gain the two categories the
**OWASP Top 10:2025** added (final release January 2026): **A03 Software Supply Chain Failures**,
which §42.7 is already about, and **A10 Mishandling of Exceptional Conditions**, which is where
§42.2's crash lives.

## 42.9 The verdicts, in one place

| Area | Adopt | Borrow | Watch (trigger) | Decline |
|---|---|---|---|---|
| Front end (§42.2) | Counted recursion bound in parser, checker and lowering, with a diagnostic and a stack-fits-ceiling test, per ADR 0007's argument | Scriban's incomplete-fix lesson: bound the recursion site, not one grammar rule — **and the tree contained three violations of it**, found by the row to the right ([`85`](85-what-the-generator-found-report.md)) | ~~Grammar-aware fuzzing (trigger: the bound lands)~~ — **built** ([`85`](85-what-the-generator-found-report.md)); what is still watched is *coverage-guided* fuzzing, trigger: a nightly toolchain is taken for another reason | — |
| Memory safety (§42.3) | A written memory-safety roadmap paragraph, in CISA's terms | verify-rust-std's null result as the stated reason for aiming verification elsewhere | — | Miri as routine CI (no yield against `forbid(unsafe)`); Kani *for memory safety* |
| Verification (§42.3, §42.5) | Kani on the solver's security invariants — no `secret[T]` into a client partition, `durable`/`ingress` never client-side | Non-interference as the name of the §3.5 claim; the OOPSLA 2025 and graded-coeffect lineage | Mechanised non-interference over the core calculus (trigger: Phase 5's spec) | — |
| Concurrency (§42.4) | Injected clock on the seam, now, so DST is never a retrofit | TigerBeetle/WarpStream/S2 as evidence DST is ordinary practice | `loom` (trigger: first hand-rolled synchronisation); DST proper (trigger: second runtime substrate) | — |
| Runtime edge (§42.6) | Make the four unbuilt F-numbers visible as failing tests rather than as words | OWASP Top 10:2025 A10 as the category name for §42.2 | Origin checks, message limits, quotas (trigger: first deployment outside a laptop) | — |
| Supply chain (§42.7) | Trusted publishing before the first crate is published; SLSA **v1.2** and CISA's **2026** SBOM elements as the charter's targets. **Signing is built** ([`99`](99-supply-chain-report.md)), over the image and not the SBOM, and **build provenance is built** for the compiler's own release ([`109`](109-provenance-report.md)), which is what supplies the builder identity and the transparency log. **Package signatures are not verified** on the way in — §99.7 names that as the gap, and so is a signature over the release listing | Diverse double-compiling, recorded against self-hosting | SPDX 3.0 / CycloneDX 1.7 tooling maturity (trigger: first signed release) | — |
| Process (§42.8) | ISO/IEC 29147 + 30111 disclosure policy; OSPS Baseline as a self-assessment; ASVS **5.0** in §12.7 | SSDF v1.1 as the practice vocabulary | SSDF v1.2 (trigger: final publication); EU CRA (trigger: first commercial distribution) | CRA conformity work now — out of scope, and saying so is the point |

## 42.10 What "watertight" would actually require

Stated once, plainly, because the word invites overclaiming. Beck can credibly reach:

1. **A stated threat model** — who the adversary is (an anonymous author of source; an anonymous
   client of a running app; a hostile dependency; an operator reading a dashboard), and what is
   explicitly *not* defended (side channels, a compromised host, an authenticated insider before
   Phase 3's identity lands).
2. **Structural exclusion where the language allows it** — the §3.5 properties, which are already
   the strongest part of the story.
3. **Bounded proof for the solver's invariants** — Kani, over a bounded model, for the two or three
   claims the project markets.
4. **Adversarial evidence for everything else** — grammar-aware fuzzing for the front end, DST for
   the runtime, both continuous.
5. **A gate per claim** — the discipline the repository already applies to walls and to the
   generated reference: when a claim stops being true, something goes red.

It cannot reach, and should never claim: a verified compiler in the CompCert sense; freedom from
side channels; safety of arbitrary FFI; or any statement about a program's *semantics* being secure
rather than its *flows* being contained. Writing that sentence down is part of being watertight,
not an admission against it.

## 42.11 What each verdict needs before it counts

§12.1's rule applies to this document as much as to any other: a verdict enters the project as an
executable artefact, not as a paragraph. So each **adopt** above is written out here as the artefact
it requires and the gate that keeps it true. **The order they should be built in is not here** — it
is [`08`](08-roadmap.md) §8.5, with the rest of the project's outstanding work, because a survey
that carries its own schedule competes with the roadmap instead of feeding it.

| Verdict | Artefact | Gate |
|---|---|---|
| Bound the front end's recursion (§42.2) | A counted depth in the parser, the checker's type walk and lowering, with a diagnostic code and a span — plus an ADR making [`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)'s argument for the front end | A nesting one past the ceiling is a *diagnostic*; and the declared stack holds the ceiling. The pair `beck-eval` already has |
| Threat model and disclosure policy (§42.8, §42.10) | A charter section — adversaries, in scope, explicitly out of scope — plus `SECURITY.md` aligned to ISO/IEC 29147 and 30111 | None needed: this is prose whose absence is the defect |
| Make the absent controls fail loudly (§42.6) | A `pending_security` suite asserting that the actor is self-asserted, that no message limit is configured, that no quota exists — `sicp/refusals/`'s pattern applied to security debt | The day somebody builds one of them its test goes red, forcing the doc update |
| Inject the clock (§42.4) | A time source on the seam; no simulator yet | A test that `SystemTime::now()` appears in exactly one place |
| The memory-safety roadmap (§42.3) | One paragraph in CISA's terms, stating what the workspace already guarantees and where the dependency graph is the answer instead | None; its absence is the defect |
| Grammar-aware fuzzing (§42.2) | `cargo-fuzz` targets for the parser and the macro expander, with a structure-aware generator over the corpus | A bounded budget per pull request; found inputs checked in |
| Kani on the solver's invariants (§42.3) | Two or three bounded proofs of the claims §3.5 markets | CI, on the solver's crate only |
| Supply-chain rows (§42.7) | §12.6 retargeted at SLSA v1.2 and the 2026 SBOM elements; trusted publishing configured before the first publish. The signing half is built ([`99`](99-supply-chain-report.md)), and ~~the transparency log~~ and ~~the provenance statement~~ arrived with it ([`109`](109-provenance-report.md)) for the compiler's release; what is left here is **trusted publishing**, a signature over the *SBOM*, and a signature over the release listing | [`28`](28-releases-and-deployment.md)'s pipeline, which now exists |

The first four are days of work between them and would move more of §42.1's table from *absent* to
*structural* than the last four combined. The last four are where the word "watertight" starts to be
earned, and none is worth starting before the threat model says what is being made watertight
against.

## 42.12 What this document is not

It adopts nothing by existing: every **adopt** above is a verdict awaiting its named piece of work,
and the ones that touch design — the injected clock, the disclosure policy as a charter row, the
non-interference statement — need a D-number or an ADR before they are real. Its measurements are
narrow: §42.1's table is a reading of the tree plus one 600-iteration mutation run, not an audit;
§42.2's thresholds are one machine, one commit, two profiles, and the numbers will move. It
contains no penetration testing, no cryptographic review, and no assessment of the dependency
graph's own `unsafe` — `forbid(unsafe)` covers first-party code only, and tokio, hyper, redb and
tungstenite are all outside it, which is a gap this document names and does not measure. Its
external citations are **search-level confirmations**, in [`38`](38-literature-survey.md) §38.9's
sense: version numbers, commencement dates, CVE identifiers and the verify-rust-std figures were
checked against secondary reporting and publisher summaries, and the primary texts of the SLSA
v1.2 specification, the OSPS Baseline and the two information-flow papers were **not** read —
several of those hosts were unreachable from the environment this was written in. Nothing here
rests on a number that only one source carried, but a reader who is about to act on a row should
open the primary document first. And the
external statuses have a shelf life: SSDF 1.2 will finalise, SLSA will move again, and the CRA's
harmonised standards are still in drafting. Everything here is dated August 2026 and should be
re-read, not trusted, when any of those change.
