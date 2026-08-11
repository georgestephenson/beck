# Changelog

What changed, in the order it changed. **This is where most work is recorded** —
[`AGENTS.md`](AGENTS.md) says when something earns a report in [`docs/`](docs/README.md) instead, and
the answer is "a phase or a subsystem", not "a change worth being pleased about".

An entry is a few lines: what changed, what it measured, and what gate holds it. Link the design
document it derives from and the test that would go red. If the entry needs a table, it might be a
report; if it needs a section heading, it is one.

Newest first.

## Unreleased

### Docs

- **Consolidated 27 reports into three chapters, and changed the rule that produced them.**
  Reports 70–79 (the evaluator's quadratics and constants) became
  [`docs/70`](docs/70-the-evaluator-gets-fast-report.md); reports 53 and 57–61 (the Are We Fast Yet
  ports) became [`docs/53`](docs/53-are-we-fast-yet-report.md); reports 27, 31–33, 36–41, 45 and 47
  (the type and effect system's features, in the order SICP's walls forced) became
  [`docs/27`](docs/27-the-walls-come-down-report.md). Every measurement, gate, finding and refusal
  is preserved; what is not is each report's opening paragraph quoting the previous one's "what is
  still not", and nine rounds of superseded wall-clock figures.
  **199,566 words to 149,794** across 24 fewer files, 27 index rows to 3, and the index itself
  129 KB to 101 KB. `docs.rs` is the gate: every relative link in every tracked markdown file, and
  every one out of a rustdoc page, has to land on a file that exists — and every `§number` in a
  Rust doc comment was remapped to the section that now carries the claim, rather than to the
  chapter's front door.

### Releases

- **The release pipeline and the installer**
  ([`docs/104`](docs/104-the-release-and-the-installer-report.md)) — the two items on
  [`docs/08`](docs/08-roadmap.md) §8.5.4's apology list that were on nobody's bullet.
  `.github/workflows/release.yml` turns a tag into four native builds, one `SHA256SUMS` and a
  GitHub Release; it *calls* `compiler.yml` rather than restating a gate, so §28.2's "a release is
  a tag on a commit that passed the whole matrix" is a `needs:` edge. `install.sh` verifies what it
  downloaded and refuses to install on a mismatch. Everything executable is outside the YAML —
  a tag-triggered workflow is the one artefact that cannot be run before it is used — and
  §104.7 splits what was executed from what was only written: no tag has been pushed.
  Gated by `release.rs` (nine tests; the one that matters corrupts an archive and asserts the
  installer exits non-zero *and* installs nothing, checked by breaking the comparison).
- **The version means something.** `0.1.0` on fourteen unpublished crates became **0.3.0**, read
  from one place by `release/version.sh`; a tag that disagrees fails the build, and
  `beck --version` carries the commit and the target triple, because four tarballs share a release.
- **A release publishes a checksum and no signature**
  ([`adr/0027`](docs/adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md)): `beck sign`'s
  subject is an OCI manifest digest and a compiler release is a tarball.
  `pending_security.rs::a_release_artefact_carries_a_checksum_and_no_signature` asserts the absence
  from both ends, and [`docs/43`](docs/43-threat-model.md) §43.4 names it.
- **Documentation brought back in line with the code** on the way through: `docs/13`'s "Cranelift is
  not built", `docs/06`'s "the `Platform` trait does not exist yet", and "all ten crates inherit
  `forbid(unsafe)`" in `SECURITY.md` and `docs/42` — which is twelve of fourteen, with `beck-wasm`
  and `beck-play` at `deny` plus an export-only exception each test asserts the extent of.

### The playground

- **Four refusals closed** ([`docs/103`](docs/103-playground-phase-3-report.md)): the editor's
  answers moved to `beck_core::editor` so the page and `beck lsp` share them, the tab's log survives
  a reload in IndexedDB, a share link carries the program and names its digest, and a
  `@render(client)` program runs in the client iframe. Gated by
  `playground.rs::the_playground_and_the_language_server_answer_the_same_questions` and four browser
  tests.
- **The page's store is serialised, and *forget* means it.** Three defects, all found by
  `browser.rs::the_playground_keeps_its_log_across_a_reload` failing one run in three under parallel
  load — and none of them visible when the test ran alone. (1) A `hello` and a command both say
  "moved", so two stores could interleave and save the same events twice; the stores are one chain
  now, and the position asked for is the length of what is held rather than a counter beside it.
  (2) A store still in flight when *forget* was pressed finished afterwards and put the log back,
  leaving a page that said it had forgotten a log it had just rewritten. Forgetting now stops the
  session keeping anything more — which it has to, because a store that resumed at the next command
  would write a log starting at seq 3, and a restore of one is refused: dense from 1 is the contract
  every fold depends on. (3) The test itself read `dataset.ready` from the *pre-navigation* document
  and then clicked a button in a document being torn down; it now proves the context switched before
  it does anything else.
