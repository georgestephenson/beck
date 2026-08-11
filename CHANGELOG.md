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
