# Changelog

What changed, in the order it changed. **This is where most work is recorded** —
[`AGENTS.md`](AGENTS.md) says when something earns a report in [`docs/`](docs/README.md) instead, and
the answer is "a phase or a subsystem", not "a change worth being pleased about".

**One bullet per change, newest first, prefixed with the date it merged and the pull request that
carried it.** An entry is a few lines: what changed, what it measured, and what gate holds it, with
a link to the design document it derives from. The story behind a finding — the defect narrative,
the alternatives declined, the argument — belongs in the report, the ADR, or the commit message,
not here; an entry that wants sub-bullets has outgrown this file.

There are no topic headings, on purpose: every branch prepends its bullet at the top of one flat
list, and [`.gitattributes`](.gitattributes) union-merges this file so parallel branches do not
conflict here. After a merge two entries may sit out of date order; the date and PR on each entry
carry the order, so leave them where they land.

## Unreleased

- **2026-08-16 · #63 — The page's flaky timing gate is replaced by one with no clock in it.**
  `measure_native.rs::what_a_page_costs_against_the_tree_walker` asserted a ratio of ratios over
  four wall-clock medians and went red 2 runs in 20 on an unchanged binary under load — a page sits
  near 0.8×, where the number is mostly the runner, not the backend. The claim is now
  `native.rs::a_page_of_keys_and_handlers_costs_equal_bytes_for_equal_rows`: equal steps must cost
  equal bytes of arena at 200, 400 and 600 rows of
  [`viewfix::PAGE`](compiler/crates/beck-cli/tests/support/viewfix.rs), checked against a known
  quadratic before being trusted. 0 of 20 red under the load that reddened the old one.

- **2026-08-16 · #64 — The log's lifecycle gets a position in the order.** Segment archival,
  retention and the analytical substrate — Parquet on object storage, DataFusion over the archive —
  are scheduled in [`docs/08`](docs/08-roadmap.md) Phase 4 and §8.5.4 (class G); five documents had
  committed to them and none gave them a position. Nothing is built, and the corrections ride
  along: ClickBench waits on the archive rather than the incremental engine, `docs/03` §3.7's
  present-tense `durable(retain=…, snapshot=…)` does not parse and now says so,
  [`docs/09`](docs/09-risks-and-open-questions.md) R6 catches up with D26, and a visualization
  vocabulary is recorded as an open question rather than a plan (`docs/09` §9.6).

- **2026-08-16 · #62 — The public surface is designed.** The boundary between a Beck backend and a
  non-Beck consumer is an opt-in `@public` family — `rest`, `mcp`, `grpc`, `events`, `sql` — each a
  rendering of the internal contract, gated by a foreign reader; GraphQL declined with the reason
  recorded ([`docs/101`](docs/101-the-public-surface.md), D28). Design only — no annotation exists
  in the compiler, and §101.11 says so. `beck-rt/src/telemetry.rs`'s module doc corrected in place:
  OTLP export is pull-only.

- **2026-08-16 · #61 — The standards charter states its evidence.**
  [`docs/12`](docs/12-standards-and-conformance.md) audited against the tree and corrected in
  place: every row now carries **verified** (gate named), **partial** (unbacked half named) or
  **chartered** (blocker named). The macro interpreter goes first in the plan
  ([`docs/08`](docs/08-roadmap.md) §8.5.4); D27 records real identity — one NaN, no `-0.0`, a
  canonicalised total order — as chosen ([`docs/10`](docs/10-decisions.md)).

- **2026-08-14 · #59 — The deployment plan, fleshed out.** The managed-cloud path is a landing
  order rather than a paragraph ([`docs/08`](docs/08-roadmap.md) Phase 4,
  [`docs/28`](docs/28-releases-and-deployment.md) §28.3). `kubeconform`, kube-score, Polaris and
  Checkov become a CI gate over `beck build`'s manifests, each suppression named as a refusal or a
  debt ([`docs/21`](docs/21-tests-in-beck-and-proof.md) §21.4 rung 6). The ≥1% rule added
  ([`docs/08`](docs/08-roadmap.md) §8.6); two stale hardening claims corrected in place.

- **2026-08-14 · #58 — Fifteen table-and-grammar primitives compile, as calls into a linked
  runtime library.** `beck-prim` is the same crate the evaluator calls, so backend agreement on a
  digest is one function rather than a differential's claim
  ([`docs/93`](docs/93-the-native-backends-report.md) §93.12,
  [`adr/0029`](docs/adr/0029-the-runtime-library-is-linked-and-owns-the-arena.md)). A linked
  `digest` is 274 ns against 5.2 µs asked across the worker's pipe
  (`measure_native.rs::what_a_linked_primitive_costs`); no pointer crosses the ABI, and the crate
  has no `unsafe`. 905 → 941 definitions compile; refusals 173 → 137.

- **2026-08-14 · #57 — Fourteen native-backend reports consolidated into one chapter**,
  [`docs/93`](docs/93-the-native-backends-report.md) — the same operation as the earlier
  27-into-3, and the precedent [`AGENTS.md`](AGENTS.md) cites for preferring a changelog entry to
  a report.

- **2026-08-14 · #56 — A reset connection no longer ends an image build.** `beck-cli/src/fetch.rs`
  attempts a hop up to four times and classifies rather than reports: transient failures are
  retried, permanent ones answered once, and a truncated reply is distinguished from an oversize
  one ([`docs/92`](docs/92-supply-chain-and-release-report.md) §92.13). The gates drive the retry
  loop itself, with no network.

- **2026-08-14 · #55 — `case [first, *rest]` compiles**, on both code generators — the last
  pattern form they refused, with the length tested before any element is read and the tail copied
  as the evaluator copies ([`docs/93`](docs/93-the-native-backends-report.md)). Its old refusal had
  been false for three reports, and the corpus pass now holds every refusal against a list of
  sentences the backend may no longer say about itself. 889 → 905; refusals 189 → 173.

- **2026-08-14 · #55 — A `parallel:` child that fails stops its siblings** — the ones an ordered
  join would never have reached, so the scope's answer cannot race
  ([`docs/80`](docs/80-structured-concurrency-report.md) §80.12). Costs about 1% on a program with
  no scope, flat across 10×. Gated by a count, not a clock
  (`concurrency.rs::a_failing_child_stops_its_siblings`); §80.9 records which wasm can have
  threads.

- **2026-08-14 · #55 — `parallel:` runs its children at the same time**, on a thread each, with
  fuel split rather than shared ([`docs/80`](docs/80-structured-concurrency-report.md)). Two
  200 ms children take 201.1 ms against 400.7 ms in order; the compute crossover is measured at
  ~580 µs per child (`measure_concurrency.rs`). Gated by
  `concurrency.rs::two_children_actually_overlap`, a deadlock-or-pass no serial evaluator can pass
  at any speed.

- **2026-08-14 · #55 — The four primitives that ask the host compile** — `now()`, `uuid()`,
  `secret_env`, `http_fetch` — via a second direction in the worker's protocol: a compiled call
  writes a question frame and blocks for the answer
  ([`docs/93`](docs/93-the-native-backends-report.md)). The host is one description,
  `beck_core::host::Atoms`, asked by all three backends. 870 → 889; refusals 208 → 189. Gated by
  `native.rs::the_two_backends_agree_on_the_host_effects` and its Cranelift twin.

- **2026-08-14 · #54 — Macro expansion is bounded by what it produces** (`B0214`), closing
  [`docs/14`](docs/14-review-findings.md)'s F17: 100,000 nodes per module, against a measured
  largest real expansion of 138. Gated in both directions by `macro_bomb.rs`, and the
  `pending_security.rs` F17 test is deleted, which is what that file's rule asks for.

- **2026-08-14 · #54 — A generic definition compiles, once per type it is used at** —
  monomorphisation as a shared backend pass, keyed on the whole type, with polymorphic recursion
  and undecided types refused by name ([`docs/93`](docs/93-the-native-backends-report.md),
  [`docs/38`](docs/38-literature-survey.md) §38.1). 850 → 870; refusals 223 → 208. Gated by
  `the_two_backends_agree_on_generics` and its Cranelift twin, with instantiations asserted by
  name.

- **2026-08-14 · #54 — `str_trim`, `str_split` and `str_chars` compile**, and both old refusals
  were wrong about their own reason — `White_Space` is 25 code points, not case mapping's table,
  and "two loops" is what makes a split cheap
  ([`docs/93`](docs/93-the-native-backends-report.md)). `examples/todo.beck` is the first program
  in the tree to compile whole. 812 → 850 across the two rounds; the text differentials reach
  4,872 calls, all three backends agreeing.

- **2026-08-14 · #54 — A map grows**: `map_insert`, `map_remove` and `map_merge` compile as the
  weight-balanced tree `beck_core::pmap` already is, so a fold that keeps a map is Θ(n log n)
  ([`docs/93`](docs/93-the-native-backends-report.md)). 895 → 1,137; refusals 523 → 281. Gated by
  `a_fold_over_a_map_is_not_quadratic` — 4.9× the arena for 4× the entries, no clock in it.

- **2026-08-14 · #54 — A list grows**: `list_append` compiles via an immutable header over a
  shared data block, sound by the shape of the writes rather than by ownership analysis
  ([`docs/93`](docs/93-the-native-backends-report.md)). 711 → 895 — the largest jump of these
  rounds — and refusals 707 → 523. Gated by `an_appended_accumulator_is_linear` and the
  differential's `forked` case.

- **2026-08-14 · #54 — `raise` and `try:` compile**, as a fourteenth trap code and a handler
  label; unwinding costs nothing per frame, and a caught raise from 3,000 frames is 17.0× the
  tree-walker ([`docs/93`](docs/93-the-native-backends-report.md)). 688 → 711. Gated by the
  failure differentials (84 calls each) and `unwinding_costs_nothing_per_frame`.

- **2026-08-14 · #54 — A view compiles, as the call that builds it**, baked by the evaluator's own
  `beck_core::html::element` ([`docs/93`](docs/93-the-native-backends-report.md)). 650 → 688, and
  21 of the 32 corpus programs compile their `view`. Not faster than the tree-walker
  (0.80×–1.33×), and §93.5 says why that is the design.

- **2026-08-14 · #53 — `beck lsp` edits**: references, document highlight, prepare-rename, rename
  and inlay hints, every answer in `beck_core::editor` so a browser tab can ask too
  ([`docs/65`](docs/65-the-editor-report.md)). A rename is verified by making the edit and
  re-analysing; 316 of the corpus's 325 names rename and every decliner is asserted. The largest
  real file (914 lines) analyses in 16.84 ms and renames in 19.03 ms (`measure_compile.rs`).

- **2026-08-13 · #52 — The release attests build provenance, and the installer can check it**
  ([`adr/0028`](docs/adr/0028-a-release-carries-provenance-and-still-no-signature.md), superseding
  [`0027`](docs/adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md)): `actions/attest`
  over the same `SHA256SUMS` that `install.sh` verifies, and `BECK_VERIFY_PROVENANCE=1` runs
  `gh attestation verify`. Written and not executed — no tag has been pushed. Gates in
  `release.rs` and `pending_security.rs`.

- **2026-08-13 · #51 — A report was carrying another report's number**, renumbered on merge with
  its headings left behind; fixed, and gated by
  `docs.rs::a_documents_sections_are_numbered_for_the_document_they_are_in` over all 86 documents.
  Thirty-one citations to roadmap sections that never existed are repointed; the citing end stays
  ungated because SICP's and IEEE 754's section numbers share the notation
  ([`docs/25`](docs/25-benchmarks-and-expressiveness.md) §25.5).

- **2026-08-13 · #50 — A closure compiles, and it does not leave**: a rank and its captures,
  applied by a switch into a direct call, refused by name at every boundary the host would read
  one across ([`docs/93`](docs/93-the-native-backends-report.md)). `concat_lists` and `sort_by`
  follow — one refused for a reason that was false — and the gate that asks whether a refusal's
  reason is *true* fired for the first time (§93.14). 605 → 646 across the two rounds. Gated by
  the closure differentials (1,178 calls each) and shape gates with no clock in them.

- **2026-08-12 · #49 — Text is on the heap, and the read-only collections follow.** A `Str`
  compiles — layout, literal pool, comparisons, ten primitives — then read-only lists and maps,
  then the primitives those layouts had unlocked (`unwrap_or`, `is_some`, `str`, `str_join`,
  `str_repeat`), three of which were refused for reasons that were false
  ([`docs/93`](docs/93-the-native-backends-report.md) §93.9). 283 → 625 across the rounds;
  differentials reach 3,382 text calls on all three backends. Record fields compared by offset
  found in both emitters — `Repr::order` is now the only place a comparison is named — and the
  evaluator's `str_slice` was charged the length the caller wrote rather than what it takes, found
  by the differential and gated in `interp`.

- **2026-08-11 · #46 — The release pipeline and the installer**
  ([`docs/92`](docs/92-supply-chain-and-release-report.md)): `release.yml` turns a tag into four
  native builds, one `SHA256SUMS` and a GitHub Release; `install.sh` refuses to install on a
  mismatch; the version is 0.3.0, read from one place. A release publishes a checksum and no
  signature ([`adr/0027`](docs/adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md)),
  asserted from both ends in `pending_security.rs`. Gated by `release.rs`, including the test that
  corrupts an archive and asserts nothing installs.

- **2026-08-11 · #45 — The playground's four refusals closed**
  ([`docs/98`](docs/98-playground-report.md)): shared editor answers, a log that survives reload,
  a share link that names its digest, `@render(client)` in the client iframe — plus three
  store-serialisation defects found by the browser gate failing one run in three under parallel
  load. Also: 27 reports consolidated into three chapters
  ([`docs/70`](docs/70-the-evaluator-gets-fast-report.md),
  [`docs/53`](docs/53-are-we-fast-yet-report.md),
  [`docs/27`](docs/27-the-walls-come-down-report.md)) — 199,566 words to 149,794 — and the rule
  that produced them changed.
