# 13 — Testing strategy

Expands [`04`](04-compiler-architecture.md) §4.8 into the full plan: every kind of test that applies
here, and *why it applies to this project specifically*. The organising insight first.

## 13.1 The principle: Beck's semantics manufacture test oracles

The hardest problem in testing is knowing the right answer. Beck's design keeps handing us **free
oracles** — pairs of independent paths that must agree — and the strategy is built around them:

| Oracle pair | Must agree on | Tests the premise that… |
|---|---|---|
| single-process run **vs** tier-split run | all observable behaviour | splitting is semantics-preserving (the language's core promise) |
| Cranelift **vs** LLVM **vs** reference interpreter | program results | codegen is correct. *Two of the three exist: `beck-cli/tests/native.rs` compares LLVM against the tree-walker over the scalar subset, on the **value and the failure** — 15,441 calls, [`93`](93-llvm-backend-report.md). Cranelift is not built* |
| native **vs** WASM builds of the same pure code | results, incl. float bits | one semantics across tiers (what makes optimism sound) |
| incremental dataflow plan **vs** full recompute | every view's value at every `seq` | "keeping it incremental is the compiler's job" is *correct*, not just fast |
| replaying a log twice / across versions | bit-identical states and patches | determinism, the foundation of replay/fork/optimism |
| optimised **vs** `-O0` builds | results | optimisations are sound |

Any test that can be phrased as a differential against one of these oracles should be — it needs no
hand-written expected values and therefore scales to generated/fuzzed inputs.

Second principle, inherited from [`12`](12-standards-and-conformance.md): **a claim is a test or it
is marketing** — every spec paragraph, security guarantee, and performance number has a CI artefact.

## 13.2 Compiler front end

| Kind | What & why |
|---|---|
| **Unit tests** | Per pass (lexer, layout, resolver, each typing rule). The baseline. |
| **Golden/snapshot** (`insta`) | Parse trees, macro expansions, diagnostics *renderings*, `.becki` signatures, generated manifests. Why: these are human-facing contracts; regressions must be seen in review, not discovered by users. Every diagnostic code has a committed rendering ([`04`](04-compiler-architecture.md) §4.5). |
| **Round-trip properties** (`proptest`) | `parse(print(ast)) == ast` for both surfaces; Python↔S-expression fidelity. Why: the dual-surface promise ([`02`](02-syntax.md)) is exactly a round-trip law. |
| **Grammar-aware fuzzing** (`cargo-fuzz`, continuous) | Parser, layout algorithm, macro expander, `sql"…"`/`html"…"` literal parsers. Why: these consume adversarial input (any text file); crashes and hangs here are CVE-shaped. |
| **Hygiene suite** | Programs where macro capture *must* fail to compile. Why: hygiene bugs are silent miscompilation of user intent. |
| **Typechecker property tests** | Generate well-typed programs; assert acceptance, principality, and that inferred effect rows are minimal. Generate ill-typed mutations; assert rejection *with the right code*. Why: inference bugs otherwise surface as impossible-to-reproduce user reports. |
| **Metamorphic tests** | α-renaming, dead-code insertion, definition reordering, comment/whitespace changes must not change: types, placement, generated wire ids, or output behaviour. Why: this is how solver *stability* ([`03`](03-type-and-effect-system.md) §3.4) becomes a checked property rather than a hope. |

## 13.3 Placement, splitting, security

| Kind | What & why |
|---|---|
| **Solver invariant properties** | Determinism (same input ⇒ same solution), stability under tolerance, no-valid-program-rejected, all-forbidden-placements-∞. Run over generated programs. |
| **Model checking** (`Kani`) | The solver's security invariants as proofs over bounded models: no `secret[T]` flow reaches a client partition; `ingress`/`durable` never placed client-side. Why: these are the claims we market; bounded proof beats sampled testing. |
| **Adversarial security corpus** | Generated programs that *try* to leak (`secret` through closures, through row projections, through macro expansion, through `Sendable` derivation) — must fail to compile, asserted per CWE claim ([`12`](12-standards-and-conformance.md) §12.7). |
| **The split differential** | The flagship suite (§13.1 row 1): a corpus of programs run whole vs split, driving identical command scripts, asserting identical event logs, states, and rendered pages. Grows with every bug ever found. |
| **Wire-compat matrix** | Every release built against the previous N releases' `.becki` signatures: old client ↔ new server and the reverse, asserting the §4.3 compatibility rules. Why: rolling deploys are the norm, and "the deploy broke every open tab" is the adoption-killing failure. |

## 13.4 The runtime as a distributed system

This is where most frameworks under-test and where Beck's determinism pays out hardest:

- **Deterministic Simulation Testing (DST)** — the FoundationDB discipline, and the crown jewel
  here: because everything downstream of ingress is a deterministic fold, the *entire backend* runs
  inside a simulated scheduler with simulated time, network, and disk. The simulator explores
  interleavings, injects faults (dropped/duplicated/reordered messages, partitions, disk-full,
  process crash at arbitrary points), and — the killer feature — **any failure replays exactly from
  a seed**. Heisenbugs stop existing as a category. Beck's semantics were practically designed for
  DST; not building it would be negligent. **Hard prerequisite** ([`14`](14-review-findings.md)
  F11): the runtime must be written against virtualized clock/network/disk interfaces **from
  Phase 1** — FoundationDB's lesson is that DST cannot be retrofitted onto a runtime that calls
  the real world directly, so this constraint binds the first line of runtime code, not Phase 4.
- **Crash-recovery tests**: `kill -9` mid-fold, mid-snapshot, mid-migration, mid-patch-flush;
  restart; assert state equals the log's truth and no patch was lost or duplicated
  (`(subscription, seq)` resume contract).
- **Jepsen-style consistency testing** against the *real* deployed system (k3d, real Postgres, real
  websockets): concurrent clients, induced partitions and clock skew, then a checker validates the
  history against our stated model (total order per app; per-session read-your-writes;
  optimistic-then-confirmed visibility). Why: DST validates our logic under our own simulator;
  Jepsen validates the claims where the real network and real Postgres live.
- **Chaos in the operator's domain**: kill pods mid-rollout, wound the drain/resume choreography at
  every step boundary, partition the operator from the API server — assert the
  [`06`](06-kubernetes-and-packaging.md) §6.4 state machine converges and never double-owns the
  write path.
- **Soak tests**: 24h+ runs at steady load watching per-session memory, log-tail latency, snapshot
  cadence, FD counts. Why: R5's fanout risk is a slow-leak risk by nature.
- **Backpressure tests**: slow clients, huge views, pathological subscription counts; assert
  drop-to-latest semantics for signals and bounded memory.

## 13.5 The formal layer

- **TLA+ / PlusCal specifications, model-checked in CI** for the three protocols with real
  concurrency: deploy choreography (quiesce→drain→snapshot→migrate→resume; safety: never two
  writers; liveness: always completes or rolls back), subscription resume, and optimism
  reconciliation. Why: these are small, subtle, and catastrophic to get wrong — exactly TLA+'s
  sweet spot, and cheap at this size.
- **Small-step semantics in the spec** for the core calculus, with the reference interpreter
  written *from* it — making the interpreter-vs-backend differential (§13.1) a spec-conformance
  test, the WASM-spec-suite trick.
- **Genesis-replay gate** ([`10`](10-decisions.md) D3): archived corpora replayed through full
  upcast chains, state-equality asserted — the D3 invariant, mechanised.

## 13.6 Product-level

- **End-to-end**: the example apps driven by Playwright (Chromium pre-installed here) through real
  browsers against `beck up` clusters — including reconnect-after-deploy and offline/reconnect
  (Mode B) scripts.
- **Accessibility**: axe-core against every example app page state, *plus* the compile-time WCAG
  checks' own negative test corpus ([`12`](12-standards-and-conformance.md) §12.4).
- **Visual regression**: patch-protocol changes can subtly break rendering; screenshot-diff the
  example apps per release.
- **Upgrade tests**: real `beck deploy` from release N-1 to N with live traffic and open
  websockets, on the k3d matrix (Kubernetes N-2 policy, multiple Postgres majors).
- **Docs-as-tests**: every code block in the book and tutorials is extracted and compiled/run in CI
  (Rust's `doctest` discipline). Why: stale docs are the "week two" killer ([`09`](09-risks-and-open-questions.md) §9.3).

## 13.7 Performance as regression testing

Budgets are CI gates with statistical rigour (`criterion`/`divan`, fixed hardware runners,
variance-aware thresholds — a gate that flakes gets deleted, so measure properly):

interaction p99 (Mode A), events/s through the sequencer, fold replay throughput, per-idle-session
memory at 10k subscribers, shared-prefix hit rate, thin-client payload, Mode B bundle sizes, image
size, cold start, keystroke→diagnostic latency, incremental build time, clean build time.
Phase 0 sets the baselines ([`08`](08-roadmap.md)); every merge answers to them.

*Status ([`64`](64-compile-speed-report.md)). The **compile-speed** budgets are built, and not as
thresholds: this section's own "a gate that flakes gets deleted" rules out a wall-clock gate on a
shared runner, so what is asserted is that **cost per declaration does not grow with the number of
declarations** along three axes. Keystroke→diagnostic latency has a number for the first time —
4.7 ms worst and 0.75 ms median over every Beck program in the tree — and the gate found placement
computing explanations in `O(n × (n + e))` within an hour of existing. Incremental and clean build
time are `cargo` numbers about the compiler's own build and are still unmeasured; every other budget
in the list above waits on the phase that produces the thing it measures.*

## 13.8 Meta-testing: testing the tests

- **Mutation testing** (`cargo-mutants`) on the compiler's core crates: if a seeded bug survives
  the suite, the suite — not the code — failed review. Target: >85% mutation score on typechecker,
  solver, splitter; tracked, not aspirational.
- **Coverage** as a *floor alarm* (not a target to game): line+branch on compiler crates, corpus
  coverage for the grammar (every production exercised), spec-ID coverage for the conformance
  suite (every normative paragraph tested — [`12`](12-standards-and-conformance.md) §12.1).
- **Flake policy**: a test that flakes twice is quarantined within 24h and fixed or deleted within
  a week; the quarantine list is public in the repo. Deterministic-by-construction suites (DST,
  differentials) should make flakes rare enough that this policy stays cheap.
- **Bug-to-test rule**: no bug closes without a regression test *in the lowest layer that can
  express it* (prefer a property or differential over an e2e).

## 13.9 What users get (testing as a language feature)

The same machinery, surfaced ([`11`](11-language-tour.md) §11.10): `test`/`property` blocks with
shrinking; `assert place(...)` and secret-flow assertions; **fold law checks** auto-generated for
every `durable` store (replay idempotence, migration round-trips, upcast totality);
`fork(log=...)` fixtures — production incidents as reproducible test cases; and a per-app DST mode
(`beck test --simulate`) that runs the user's own app under fault injection. Nobody else can hand
application developers deterministic-simulation testing of their whole stack as a CLI flag; it
falls out of our semantics.

*Status ([`22`](22-phase-3-report.md)). Built: `test`/`property` blocks with shrinking, `expect
place(…)`, `expect flow(…) reaches nothing on <tier>`, and effect-atom stubs. Not built: fold law
checks, `fork(log=…)` fixtures — which need `beck fork`, Phase 4 — and `--simulate`. The
`Interceptor` seam `beck test` installs stubs through is the shape fault injection needs, so the
last of those is closer than it was ([`22`](22-phase-3-report.md) §22.7 item 4).*
