# Changelog

What changed, in the order it changed. **This is where most work is recorded** —
[`AGENTS.md`](AGENTS.md) says when something earns a report in [`docs/`](docs/README.md) instead, and
the answer is "a phase or a subsystem", not "a change worth being pleased about".

An entry is a few lines: what changed, what it measured, and what gate holds it. Link the design
document it derives from and the test that would go red. If the entry needs a table, it might be a
report; if it needs a section heading, it is one.

Newest first.

## Unreleased

### The native backends

- **A map arrives read-only, and a fold compiles**
  ([`docs/106`](docs/106-a-map-arrives-read-only-report.md)): `Map[K, V]` is a count, every key in
  key order, then every value — so `map_get` is a binary search and `map_keys` is one `memcpy`.
  `map_len`, `map_get`, `map_contains`, `map_keys`, `map_values`, the six comparisons and `{}`
  compile; `map_insert`, `map_remove` and `map_merge` are refused. **403 → 452 definitions**, the
  corpus **30 → 53 across 31 of its 35 files**, and **nine corpus programs compile their
  `apply_event`**. Of 1,026 refusals across the tree beforehand, 472 blamed a `Map`; **none now
  blames a collection for having no layout**. Gated by `native.rs::the_two_backends_agree_on_maps`,
  `cranelift.rs::the_three_backends_agree_on_maps` (898 calls each),
  `a_lookup_costs_the_same_whatever_the_map_holds` and `a_corpus_fold_compiles`.
- **`Repr::order` is the only place either backend names a comparison.**
  [`docs/105`](docs/105-lists-arrive-read-only-report.md) §105.4 said that would prevent a fourth
  record-field-compared-by-offset defect; there was a fourth, in the one site not yet converted, and
  all five go through the accessor now. A new reference kind is a compile error where its comparison
  has to be written (§106.5).
- **`Html` is refused by name.** It fell through to "not a type this module declares", which is true
  about the path taken and misleading about the cause.


- **A list arrives read-only** ([`docs/105`](docs/105-lists-arrive-read-only-report.md)): a
  `list[T]` is a value both code generators compile — literals, the six comparisons, `list_len`,
  `list_is_empty`, `list_get`, `list_contains`, `list_index_of`, `list_slice`, `list_take`,
  `list_drop` and `list_reverse` — and **`list_append` is refused** by name, because an arena cannot
  prove nobody else holds the accumulator ([`docs/101`](docs/101-the-heap-report.md) §101.5's
  forecast, cashed). **344 → 403 definitions** compile across the tree. Gated by
  `native.rs::the_two_backends_agree_on_lists` and
  `cranelift.rs::the_three_backends_agree_on_lists` (1,425 calls each), and by
  `a_list_slice_costs_its_answer_and_not_the_list_it_came_from`.
- **`str_index_of` compiles, and the reason it did not was false.**
  [`docs/104`](docs/104-text-on-the-heap-report.md) §104.4 blamed the prelude's `Option` for having
  no layout; it has had one since [`docs/101`](docs/101-the-heap-report.md). Every gate stayed green
  because each asserted a refusal *said* something and none asked whether what it said was so.
  `a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one` is the gate that would have
  gone red (§105.7).
- **The reply decoder is iterative**, so `MAX_DEPTH` bounds the *value* rather than the host
  thread's stack. It recursed, and a debug build aborted at about 1,600 against a declared ceiling
  of 2,048 — which made what could be decoded a function of how the compiler was built. Gated by
  `a_value_at_the_declared_ceiling_decodes_rather_than_aborting`, which builds a value exactly that
  deep by hand (§105.6).
- **The LLVM record comparison compared a `list` field by its offset** — the same defect
  [`docs/104`](docs/104-text-on-the-heap-report.md) §104.5 found in Cranelift for a `Str`, third
  occurrence. Caught by the differential's pairs (§105.4).
- **An unreachable refusal reason deleted.** The higher-order collection primitives are refused a
  line earlier by their own argument, so the table's entry for them could never be produced
  ([`docs/89`](docs/89-query-fusion-report.md) §89.5's pattern).
- **`measure_native.rs` asserted a ratio that depends on the build profile** — that text's
  accumulator is slower than the tree-walker, which is true in release and false in debug. The
  assertion is gone and the claim is gated with no clock in it instead:
  `an_accumulator_costs_the_square_of_what_it_builds` reads the arena and finds 15.9× the bytes for
  4× the steps.

- **Text is on the heap** ([`docs/104`](docs/104-text-on-the-heap-report.md)): a `Str` is a value
  both code generators compile — a layout of two counts and the bytes, a literal pool the host
  writes in front of every request, `+`, the six comparisons, `str_len`, `str_is_empty`,
  `str_slice`, `str_contains`, `str_starts_with` and `str_ends_with`. **283 → 344 definitions**
  compile across the tree and the corpus goes 4 → 28. Gated by
  `native.rs::the_two_backends_agree_on_text` and
  `cranelift.rs::the_three_backends_agree_on_text` (2,893 calls each, over an alphabet with an
  embedded NUL, four-byte characters and a prefix pair), by
  `a_slice_costs_its_answer_and_not_the_string_it_came_from` on both backends, and by
  `the_literal_pool_is_a_function_of_the_program`.
- **The Cranelift record comparison compared a `Str` field by its offset**, so two equal strings
  allocated at different places compared unequal. Found by the three-way differential's pairs one
  minute after the second emitter existed (§104.5).

### The evaluator

- **`str_slice` was charged the length the caller wrote rather than the length it takes**, so
  `str_slice(s, 0, 1_000_000)` on a five-character string cost a million steps and "from here to
  the end" could exhaust the fuel budget on a program doing nothing. The arm one above it in
  `work_of` has stated the rule since it was written. Gated by
  `interp::tests::a_slice_is_charged_what_it_takes`, and found by the native differential answering
  while the tree-walker ran out of fuel (§104.8).

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
