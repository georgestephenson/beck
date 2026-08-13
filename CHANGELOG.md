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

- **The last two list primitives, and one of them was refused for a reason that is false.**
  `concat_lists` and `sort_by` compile on both backends, so **every** higher-order list primitive
  except `list_flat_map` now does. **619 → 646 definitions** compile across the tree and refusals go
  771 → 744.
  - **`concat_lists` was filed with `list_append` as "grows a list", and it does not.** Its answer's
    length is a **sum over the outer list's header words** — one pass, and the allocation happens
    after it, which is exactly the argument `str_join` was corrected under earlier in this section.
    It is one `memcpy` per inner list and one function for the whole module, because an element is a
    word whatever it means. **22 refusals blamed it; 9 of those definitions compile and 13 are
    re-refused for a callee that still does not.** This is
    [`docs/106`](docs/106-lists-arrive-read-only-report.md) §106.7's finding a fourth time: a refusal
    is a claim, and this one was inherited from a primitive it only resembles.
  - **`sort_by`** is decorate–sort–undecorate against a **stable** merge sort over two parallel runs
    of words — the keys and the elements — with a scratch pair, generated once per *key* repr because
    what it needs to know is how to compare two key words and `beck.elem.cmp` already is that.
    Recursive rather than bottom-up: `log n` of host stack against three nested loops to be wrong
    about. **All 14 refusals that blamed it are gone**, 13 of them compiling.
    Stability is one `<=` and it is gated: `by_rank` sorts records whose keys are *all the same*, so
    an unstable sort is free to answer anything — checked by making it red, which took one operator.
  - **Two numbers worth reading, both from
    `measure_native.rs::what_a_closure_costs_against_the_tree_walker`.** The sort is **2.1× the
    tree-walker at 2,000 elements and 1.6× at 16,000** — a small margin with a plain reason: the
    evaluator's sort *is* Rust's stable sort and only the key function is interpreted, so this
    compares a merge written here against one written by somebody who tuned it. The shape is the
    claim, and it holds: 0.76× of the ratio kept over eight times the elements, where a quadratic
    merge would have lost about a factor of six. And **`concat_lists` is 0.24–0.33×, slower than the
    tree-walker, asserted to be** — [`docs/105`](docs/105-text-on-the-heap-report.md) §105.7's
    precedent. The work is a `memcpy` and the *call* is 2,000 list objects down a pipe and an answer
    read back out of a reply, so what that row measures is
    [`docs/93`](docs/93-llvm-backend-report.md) §93.1's round trip and not this primitive.
  - Gated by `native.rs::the_two_backends_agree_on_closures` and
    `cranelift.rs::the_three_backends_agree_on_closures` (1,178 calls each now) and
    `a_sort_costs_four_runs_and_a_concatenation_costs_its_answer` — a shape gate with no clock in it,
    because a concatenation that grew would hold every intermediate and a merge sort that allocated
    per level would pass every differential.
  - **Corrects [`docs/108`](docs/108-closures-arrive-report.md)** §108.4 and §108.8, which name both
    of these as refused. §108.4's claim about `sort_by` — "the next one to build, not one that cannot
    be" — held for one commit.

- **A closure arrives, and it does not leave**
  ([`docs/108`](docs/108-closures-arrive-report.md)): a `lambda` compiles to both code generators as
  an object holding the lambda's **rank** and its captures, applying one is a switch on that word
  into a direct call, and `map_list`, `filter_list`, `list_fold`, `list_all` and `list_any` are five
  generated loops that go through it. There is no indirect call and no code address in the arena,
  because [`adr/0026`](docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md) says a value is an
  offset. A closure is refused at every boundary the host would read one across — a parameter, a
  result, a field, an element, a map's key or value — so nothing in the host changed.
  **605 → 619 definitions** compile across the tree; of the 96 refusals that blamed a closure, 11
  compile, 52 are the boundary and **33 were re-refused for a deeper reason that was always true of
  them**. Ranks are ordered the way `Closure`'s `Ord` compares two closures — the parameters, then
  where the body starts — so `==` on two functions is one word comparison that agrees.
  Gated by `native.rs::the_two_backends_agree_on_closures` and
  `cranelift.rs::the_three_backends_agree_on_closures` (1,108 calls each),
  `a_loop_costs_its_answer_and_one_closure`, `a_tail_call_through_a_closure_costs_nothing` (ten
  million applications in tail position, on both backends) and
  `a_closure_does_not_cross_the_boundary`.
- **The gate that asks whether a refusal's reason is *true* fired**
  ([`docs/108`](docs/108-closures-arrive-report.md) §108.7). `a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one`
  went red because giving a closure a shape made "a closure, which has no layout here" false while
  the refusal saying it stayed. It now asserts both halves of what is true instead: the shape exists,
  and `Heap::crossing` is what refuses it. `what_the_heap_does_not_reach_is_refused_by_name` lost a
  row the same way — `map_list(xs, double_it)` compiles, so `mapped` moved to that test's control
  list.
- **`free_vars` is `beck_core::core`'s, once.** `plan.rs` had it privately for deciding what a
  dataflow operator is handed; a closure needs the same answer for deciding what its object carries.
  A second walk that disagreed about one construct would give a compiled closure a field the
  evaluator's environment does not have.

- **The primitives the layouts had already unlocked**, and the refusals that were hiding them.
  `unwrap_or` and `is_some` take an `Option` apart, `str` renders an `Int`, a `Bool` and a `Str`,
  and `str_join` and `str_repeat` build one. **452 → 625 definitions** compile across the tree and
  refusals go 977 → 806 — the largest jump any of these rounds has produced, from the smallest
  amount of code, because `unwrap_or` alone was the leading cause of 71 refusals and the root of
  hundreds of inherited ones. `lib/bignum.beck` goes 7 → 25, `lib/decimal.beck` 13 → 35 and
  `clbg/pidigits.beck` 10 → 30. Gated by `native.rs::the_two_backends_agree_on_text` and
  `cranelift.rs::the_three_backends_agree_on_text`, now 3,382 calls each.

  **Three of those refusals were wrong about their own reason**, which is
  [`docs/106`](docs/106-lists-arrive-read-only-report.md) §106.7's finding a second time and
  corrects [`docs/105`](docs/105-text-on-the-heap-report.md) §105.4:

  - `str` was refused because "the rendering has to be Rust's to the digit". True of a **real**,
    whose shortest round-trip form is an algorithm — and not of an `Int`, whose decimal is a loop.
    An `Int`, a `Bool` and a `Str` compile; a `Float` is still refused, and now says why.
  - `str_join` was refused because it "builds text whose size is a sum over a list, and the arena
    cannot grow an allocation it has already made". A sum over a list is one pass, and the
    allocation happens after it.
  - `str_repeat` was refused because it "builds text whose size is not a function of its arguments'
    sizes". It is `|s| × n` — a function of an argument's *value*, which is available.

  What is left of text is genuinely a table: `str_trim` and `str_upper`/`str_lower` are Unicode
  properties, and `str_to_int` has to agree with Rust's parser about every input that is not a
  number.
- **`str(b)` interned its two literals during emission**, which
  `the_literal_pool_is_a_function_of_the_program` caught immediately: the pool has to be decided by
  the survey, or it is a function of the fixed point rather than of the program. `"true"` and
  `"false"` are interned in the survey now, where the walk sees a `ToStr` over a `Bool`.


- **A map arrives read-only, and a fold compiles**
  ([`docs/107`](docs/107-a-map-arrives-read-only-report.md)): `Map[K, V]` is a count, every key in
  key order, then every value — so `map_get` is a binary search and `map_keys` is one `memcpy`.
  `map_len`, `map_get`, `map_contains`, `map_keys`, `map_values`, the six comparisons and `{}`
  compile; `map_insert`, `map_remove` and `map_merge` are refused. **403 → 452 definitions**, the
  corpus **30 → 53 across 31 of its 35 files**, and **nine corpus programs compile their
  `apply_event`**. Of 1,026 refusals across the tree beforehand, 472 blamed a `Map`; **none now
  blames a collection for having no layout**. Gated by `native.rs::the_two_backends_agree_on_maps`,
  `cranelift.rs::the_three_backends_agree_on_maps` (898 calls each),
  `a_lookup_costs_the_same_whatever_the_map_holds` and `a_corpus_fold_compiles`.
- **`Repr::order` is the only place either backend names a comparison.**
  [`docs/106`](docs/106-lists-arrive-read-only-report.md) §106.4 said that would prevent a fourth
  record-field-compared-by-offset defect; there was a fourth, in the one site not yet converted, and
  all five go through the accessor now. A new reference kind is a compile error where its comparison
  has to be written (§107.5).
- **`Html` is refused by name.** It fell through to "not a type this module declares", which is true
  about the path taken and misleading about the cause.


- **A list arrives read-only** ([`docs/106`](docs/106-lists-arrive-read-only-report.md)): a
  `list[T]` is a value both code generators compile — literals, the six comparisons, `list_len`,
  `list_is_empty`, `list_get`, `list_contains`, `list_index_of`, `list_slice`, `list_take`,
  `list_drop` and `list_reverse` — and **`list_append` is refused** by name, because an arena cannot
  prove nobody else holds the accumulator ([`docs/101`](docs/101-the-heap-report.md) §101.5's
  forecast, cashed). **344 → 403 definitions** compile across the tree. Gated by
  `native.rs::the_two_backends_agree_on_lists` and
  `cranelift.rs::the_three_backends_agree_on_lists` (1,425 calls each), and by
  `a_list_slice_costs_its_answer_and_not_the_list_it_came_from`.
- **`str_index_of` compiles, and the reason it did not was false.**
  [`docs/105`](docs/105-text-on-the-heap-report.md) §105.4 blamed the prelude's `Option` for having
  no layout; it has had one since [`docs/101`](docs/101-the-heap-report.md). Every gate stayed green
  because each asserted a refusal *said* something and none asked whether what it said was so.
  `a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one` is the gate that would have
  gone red (§106.7).
- **The reply decoder is iterative**, so `MAX_DEPTH` bounds the *value* rather than the host
  thread's stack. It recursed, and a debug build aborted at about 1,600 against a declared ceiling
  of 2,048 — which made what could be decoded a function of how the compiler was built. Gated by
  `a_value_at_the_declared_ceiling_decodes_rather_than_aborting`, which builds a value exactly that
  deep by hand (§106.6).
- **The LLVM record comparison compared a `list` field by its offset** — the same defect
  [`docs/105`](docs/105-text-on-the-heap-report.md) §105.5 found in Cranelift for a `Str`, third
  occurrence. Caught by the differential's pairs (§106.4).
- **An unreachable refusal reason deleted.** The higher-order collection primitives are refused a
  line earlier by their own argument, so the table's entry for them could never be produced
  ([`docs/89`](docs/89-query-fusion-report.md) §89.5's pattern).
- **`measure_native.rs` asserted a ratio that depends on the build profile** — that text's
  accumulator is slower than the tree-walker, which is true in release and false in debug. The
  assertion is gone and the claim is gated with no clock in it instead:
  `an_accumulator_costs_the_square_of_what_it_builds` reads the arena and finds 15.9× the bytes for
  4× the steps.

- **Text is on the heap** ([`docs/105`](docs/105-text-on-the-heap-report.md)): a `Str` is a value
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
  minute after the second emitter existed (§105.5).

### The evaluator

- **`str_slice` was charged the length the caller wrote rather than the length it takes**, so
  `str_slice(s, 0, 1_000_000)` on a five-character string cost a million steps and "from here to
  the end" could exhaust the fuel budget on a program doing nothing. The arm one above it in
  `work_of` has stated the rule since it was written. Gated by
  `interp::tests::a_slice_is_charged_what_it_takes`, and found by the native differential answering
  while the tree-walker ran out of fuel (§105.8).

### Docs

- **A report was carrying another report's number, and every reference to it named nothing.**
  [`docs/104`](docs/104-the-release-and-the-installer-report.md) was renumbered on merge — the
  filename and the prose inside it moved to `104`, the title and eleven headings stayed at `101`,
  which [`docs/101`](docs/101-the-heap-report.md) already had. So `§101.5` named a section in each
  of two documents, and the 29 references to `§104.x` from `README.md`, `AGENTS.md`, this file,
  [`release/README.md`](release/README.md), [`08`](docs/08-roadmap.md),
  [`28`](docs/28-releases-and-deployment.md), [`43`](docs/43-threat-model.md),
  [`86`](docs/86-getting-started.md), [`adr/0027`](docs/adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md)
  and the index named a heading that did not exist. Only the headings, the title and the index's
  three self-references moved: the report's own prose had been corrected already, which is why
  reading it did not show the fault — the half a reader sees was right.
- **The gate is the shape of that gap rather than of the fix**: a document's numbered headings and
  its numbered title carry its own filename's number. That is also what makes a section number
  unique across `docs/` without anything checking for a collision, and it is enforced against the
  document that *owns* the number — the file a rename is free to fix — because reports are history
  and a rule that every `§N.M` citation resolves would be enforced against files nobody may edit.
  `docs.rs::a_documents_sections_are_numbered_for_the_document_they_are_in` over all 86 documents,
  with the `links` job in [`docs.yml`](.github/workflows/docs.yml) keeping its own copy the way it
  already does for the link rule. Checked by putting the defect back: both go red and name all
  twelve lines. A heading opening with `§` cites a section rather than declaring one and is skipped
  — [`26`](docs/26-arrangement-sharing-report.md) has the only one.

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
