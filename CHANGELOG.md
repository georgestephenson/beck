# Changelog

What changed, in the order it changed. **This is where most work is recorded** —
[`AGENTS.md`](AGENTS.md) says when something earns a report in [`docs/`](docs/README.md) instead, and
the answer is "a phase or a subsystem", not "a change worth being pleased about".

An entry is a few lines: what changed, what it measured, and what gate holds it. Link the design
document it derives from and the test that would go red. If the entry needs a table, it might be a
report; if it needs a section heading, it is one.

Newest first.

## Unreleased

### The editor

- **`beck lsp` edits: references, document highlight, prepare-rename, rename and inlay hints**
  ([`docs/110`](docs/110-the-editor-edits-report.md)). Every answer is in `beck_core::editor`, so a
  browser tab can ask for them too; the server translates JSON-RPC and nothing else (§4.6). This
  empties [`docs/08`](docs/08-roadmap.md) §8.5.5's **Lane C** and closes the two rows
  [`docs/65`](docs/65-lsp-report.md) §65.5 called its largest gaps.
  - **A rename is the claim that a set of byte ranges is every place a name is written**, and it is
    made only when the file's two accounts of the name agree: the lexical one, which misses nothing
    and knows nothing, and the checked program's, which knows what each reference means and is not
    complete. Where they disagree the rename declines. The edits are the *lexical* ranges, because a
    `Global` node that is called carries the span of the **call** — editing that span would replace
    a call with a name.
  - **Then the edit is made and re-analysed.** A rename whose result does not compile is not
    offered, and neither is one that still compiles as a *library* rather than as the application it
    was. That costs a second compile of one file and is what makes the paragraph above a fact about
    the text rather than an argument about the IR.
  - **316 of the corpus's 325 names rename** — verified compiling *and* publishing the same
    interface with one name substituted. Eight decline because a signal's name is also a model
    field; one declines because the edit was made and the file no longer type-checked, and that one
    is asserted to keep happening, because a corpus that stopped triggering the net would stop
    testing it. Gated by `lsp.rs::renaming_every_name_in_the_corpus_either_works_or_says_why`.
  - **`page` is a keyword** — `expect page contains "…"` reads it as syntax — *and* the name of a
    signal in nearly every corpus program. Counting only identifier tokens meant the most common
    name in the language had no occurrences at all, so the lexical account counts a keyword run too,
    and a test block's clauses are treated as the grammar they are.
  - **Inlay hints are the two inferred halves of a signature and nothing else**: the tier §3.4 solves
    for and the row §3.6 infers, each labelled with the text an author could paste in at that
    offset — asserted by pasting it and re-analysing. `@on(any)` is never hinted, because §3.3's
    unplaced tier is the absence of a placement rather than one.
  - **`Def::tier_is_annotated` does not mean somebody wrote it.** `project::link` sets it on every
    definition it links, so in the linked program an editor holds it is true of everything, and
    every definition was hinted with the annotation it already had. The checker's own answer is kept
    beside it as `tier_is_written`, set once and never overwritten. Placement is unchanged: the
    solver reads the original flag.
  - **`Expectation::Place` kept no span for the name it names**, so `expect place(page) == client`
    could not be edited or pointed at. It has one.
  - **`beck fmt` is still not wired to `textDocument/formatting`**, and now for a reason rather than
    by omission: the lexer skips ordinary comments, so a format-on-save would delete every `#` line
    in the file. The missing piece is comment-preserving printing.
  - **Cost**, from `measure_compile.rs::what_an_inlay_hint_and_a_rename_cost` (release, median of
    five): a 59-line file analyses in 1.02 ms, hints in 0.056 ms and renames in 1.12 ms; the largest
    real file in the tree, 914 lines, in 16.84 ms, 2.10 ms and 19.03 ms. A rename is **one more
    analysis**, which is the verification pass priced. Hinting was `definitions × tokens` when it
    was first written — [`docs/64`](docs/64-compile-speed-report.md) §64.2's defect in a new file —
    and is now gated at ×1.42 per definition across 16× as many, inside
    `compile_speed.rs::the_front_end_cost_per_declaration_does_not_grow_with_a_module`.

### The release

- **The release attests build provenance, and the installer can check it** —
  [`docs/109`](docs/109-provenance-report.md),
  [`adr/0028`](docs/adr/0028-a-release-carries-provenance-and-still-no-signature.md), which
  supersedes [`0027`](docs/adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md) by taking
  the one route that record named as right and deferred. `actions/attest` over
  `subject-checksums: staging/SHA256SUMS`, so the digests the attestation vouches for are the
  digests `install.sh` verifies against, read from the same bytes; `BECK_VERIFY_PROVENANCE=1` runs
  `gh attestation verify` with `--signer-workflow` pinned, and a missing `gh` is a failed install
  rather than a skipped step. The gates are in `release.rs` and — for the absence that remains, a
  default install that checks a checksum and nothing else — in `pending_security.rs`.
  **Nothing has been attested yet**: no tag has been pushed, so the step is written and not
  executed, which is why it is deliberately unconditional and a `workflow_dispatch` dry run
  exercises it. §109.5 records seven mutations, one gate each, and the one that did not fire until
  it was rewritten.
### The front end

- **Macro expansion is bounded by what it produces** (`B0214`), which is
  [`docs/14`](docs/14-review-findings.md)'s **F17** — open since the review — and a row of
  [`docs/43`](docs/43-threat-model.md) §43.4 that now says *built*. Expansion was bounded in **depth**
  twice over and in **work** not at all, and a macro that doubles its output is shallow: eight
  nestings of a two-line macro is 256 copies of its argument, twenty-four is sixteen million, and
  every one of those programs is six lines long and satisfies every other limit the front end has.
  That is [`docs/85`](docs/85-what-the-generator-found-report.md) §85.7's pattern a fourth time — a
  limit added at the one production somebody thought of, bypassed through a different one — and it
  matters here rather than in the abstract because [`docs/42`](docs/42-security-assurance.md) §42.2's
  playground compiles a stranger's source in their own browser tab.
  - The expander charges what each expansion **produces**, per node, against **100,000 nodes for the
    whole module** — per module because that is what a compile is, and because a per-call budget
    would let a program spend it once per call
    ([`docs/84`](docs/84-a-quota-is-only-as-good-as-its-actor-report.md) §84.4's arithmetic one
    subsystem over).
  - **The number is measured, not declared.** Across the corpus, both benchmark suites, both SICP
    chapters, the examples and the standard library, the largest total expansion is **138 nodes**
    (`sicp/ch3.beck`; `examples/todo.beck`'s page is 94), so the budget is about 725× the biggest
    real one and about seventeen nestings of a doubling macro.
  - The count is **iterative and self-bounding**: it walks with its own stack, because the tree being
    measured is one a macro just built and a recursive count would be a claim about the *host's*
    stack ([`docs/106`](docs/106-lists-arrive-read-only-report.md) §106.6 one subsystem over) — and
    it stops when the budget does, so a macro that would produce a billion nodes is refused after a
    hundred thousand have been counted.
  - Gated by `macro_bomb.rs` in **both** directions and with the control a refusal needs: a doubling
    macro 24 deep is refused *by the budget* and not by either depth counter, the same macro 8 deep
    still compiles, and every program in the tree still expands. `pending_security.rs`'s F17 test is
    deleted, which is what that file's own rule asks for.

### The native backends

- **A map grows, as the tree it always was**
  ([`docs/112`](docs/112-a-map-grows-report.md)): `map_insert`, `map_remove` and `map_merge` compile
  to both code generators, and a fold that keeps a `Map` is `Θ(n log n)` rather than `Θ(n²)`.
  **895 → 1,137 definitions** compile across the tree and refusals go 523 → 281.
  `examples/todo.beck` compiles **eight of its nine definitions**; the one left needs a Unicode
  table.
  - [`docs/111`](docs/111-a-list-grows-report.md) §111.7 forecast that a list's answer would not work
    here, and it was right: a list's refusal was about a *layout* and a map's is about a *structure*.
    An insert lands in the middle of a sorted run and every entry after it shifts, however the header
    is arranged. What removes it is the structure `beck_core::pmap` already uses — a
    **weight-balanced tree**, whose insert rebuilds the path and shares every subtree it did not
    touch. Five words a node (subtree size, key, value, two children), the same `DELTA` and `RATIO`
    the evaluator's module argues for, and an empty map is the offset `0`.
  - **Sound for free**: a node is never written after it is built, so the map an insert was given is
    exactly what it was — [`docs/111`](docs/111-a-list-grows-report.md) §111.2's argument again,
    arriving here as a property of the structure rather than as a design.
  - **Everything that moves nodes is one function for the whole module.** Rebalancing shuffles
    *words* and never asks what a key is, so `size`, `node`, `balance`, `nth` and the in-order walk
    are written once; only `find`, `insert`, `remove`, `merge` and the two-map order are generated
    per repr, because those are the ones that compare.
  - Gated by `a_fold_over_a_map_is_not_quadratic` (**4.9× the arena for 4× the entries**, no clock in
    it) and by the differential's `branched` — two maps grown from one, answering with the original's
    length and both lookups, so a rotation that wrote through a shared node fails on the first case —
    and `descending`, the insertion order a tree that did not rebalance degenerates on and a sorted
    run handled by accident.
  - **The finding is a name collision** (§112.5): `awfy/richards.beck` has a definition called
    `dispatch`, every user definition was mangled to `beck.<name>`, and the module's own dispatcher
    is `beck.dispatch` — so a program that had done nothing wrong got *"invalid redefinition of
    function"*. Latent since [`docs/93`](docs/93-llvm-backend-report.md), and it surfaced here
    because a collision needs both halves to exist: `dispatch` had never compiled before. A user
    definition is `beck.def.<name>` now, in both emitters.

- **A list grows, and the refusal was about a layout**
  ([`docs/111`](docs/111-a-list-grows-report.md)): `list_append` compiles to both code generators and
  the accumulator every loop is written as is **linear**. **711 → 895 definitions** compile across
  the tree and refusals go 707 → 523 — the largest jump any of these rounds has produced, because
  `list_append` appears in 65 definitions and the other 119 had inherited the refusal from a callee.
  - **The reason on record was true and the conclusion did not follow.** It was *ownership*: the
    tree-walker pushes in place when last-use analysis proves the accumulator is nobody else's, and
    an arena cannot prove that. Every sentence of that holds. What forced the copy was the **layout**
    — a count sitting in front of the elements, so an append could copy them or overwrite what other
    holders see. A list is two objects now: an immutable **header** `[count, block]` and a shared
    **data block** `[cap, used, elements…]`.
  - **Sound by the shape of the writes, not by an argument about who holds what.** Every header over
    a block has a count of at most `used`, so the slot at `used` is one no reader can see: an append
    writes it, bumps `used`, and answers a *new* header. A second list grown from the same one finds
    the slot taken and copies. No ownership analysis, no reference count, no last-use flag.
  - **Costs one load, once per operation** — every generated loop takes the data pointer before it
    starts — and 24 bytes per list, which three arena-shape gates now carry as moved constants (16 →
    40 bytes for a one-element slice, 96 → 144 for a row of the todo page). What those gates assert
    is unchanged: the number does not grow with `n`.
  - Gated by `an_appended_accumulator_is_linear` (**4.0× the arena for 4× the elements**, no clock in
    it) and by `forked` in the differential — two lists grown from one, so the soundness argument is
    a program rather than a paragraph. Measured at **11.4× the tree-walker at 2,000 elements and
    7.0× at 8,000**, against a control that holds 80× flat: the arena is linear and what grows is the
    reply, which is [`docs/93`](docs/93-llvm-backend-report.md) §93.1's round trip again.

- **A raise arrives, and a handler catches it**
  ([`docs/110`](docs/110-a-raise-arrives-report.md)): `raise` and `try:` compile to both code
  generators. The mechanism was already there — every compiled function takes an error cell, stores
  into it and returns, and every caller checks it — so this adds a fourteenth trap code, two words of
  arena for the raised value, and a **handler**: a label the checks branch to instead of the
  function's exit. **688 → 711 definitions** compile across the tree, refusals go 730 → 707, and the
  38 refusals that blamed `raise` are **none** (18 compile, 20 inherit a deeper reason). A caught
  raise from 3,000 frames is **17.0×** the tree-walker against **20.0×** for the same recursion that
  does not fail. Gated by `the_two_backends_agree_on_failure` /
  `the_three_backends_agree_on_failure` (84 calls each, including a fault inside a `try:` and a
  different error type inside one — both of which must **not** be caught),
  `an_uncaught_raise_names_the_value_it_carried`, and `unwinding_costs_nothing_per_frame` — the same
  168 bytes of arena whether the raise was 25 frames down or 200.
  - **The finding is about the protocol, not the feature** (§110.8): the handler cleared the trap
    code with `store i32 0`, and the cell's first word is a code *and* a span while the worker's loop
    reads it as one `i64` to decide whether the call answered. So a caught failure came back with a
    stale span in the high half, looking like a trap with an empty arena. Two pieces of one program
    disagreeing about what "cleared" means, which is
    [`docs/107`](docs/107-a-map-arrives-read-only-report.md) §107.5's class of defect one level down.

- **A view arrives, as a recipe** ([`docs/109`](docs/109-a-view-arrives-as-a-recipe-report.md)): a
  definition that returns `Html` compiles to both code generators. What goes in the arena is the
  **call** `html_el(tag, attrs, children)` would have been given rather than the tree, and the host
  bakes it with `beck_core::html::element` — the evaluator's own `html_el`, lifted out and called
  from both. **650 → 688 definitions** compile across the tree, refusals go 768 → 730, and **21 of
  the 32 corpus programs have a `view` that compiles**, `examples/todo.beck`'s among them. Gated by
  `native.rs::the_two_backends_agree_on_views` (253 calls),
  `cranelift.rs::the_three_backends_agree_on_views` (127), the `ui:` block's own pair, and
  `a_page_costs_its_own_nodes_and_nothing_per_page` — 96 bytes a row and 504 a page at 100 rows and
  at 800, a shape gate with no clock in it. **Not faster**: 0.80×–1.33× the tree-walker at two
  sizes, and §109.6 says why that is the design rather than a constant to tune.

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
- **Thirty-one citations named roadmap sections that have never existed.** `§8.6` (Phase 3's bullet
  list), `§8.7` and `§8.19` (Lane E) are cited by nine documents and four Rust doc comments, and
  `git log -S"## 8.6" -- docs/08-roadmap.md` returns nothing: the phase headings have carried names
  rather than numbers since the file's first commit. So this was never a rename — the numbers were
  invented at the citing end, and inconsistently enough that `§8.7` means Phase 4 in
  [`98`](docs/98-playground-report.md) and [`103`](docs/103-playground-phase-3-report.md) and the
  **lane table** in [`93`](docs/93-llvm-backend-report.md). Each is repointed at what it claims:
  Phase 3 and Phase 4 by the names the roadmap gives them, and Lane E to
  [`08`](docs/08-roadmap.md) §8.5.5 — which is what [`105`](docs/105-text-on-the-heap-report.md),
  [`106`](docs/106-lists-arrive-read-only-report.md) and
  [`108`](docs/108-closures-arrive-report.md) already called it. No claim, measurement or refusal
  moved; this edits reports, which are history, and the warrant is that a pointer that never
  resolved is a repair rather than a rewrite — the same operation the consolidation below performed
  when it remapped every `§number` to the section that carries the claim.
- **There is no gate on the citing end, and the reason is the finding.** Of 157 `§N.M` citations
  that resolve to no heading, the 31 above are the only ones this project owns: almost all of the
  rest are **SICP and IEEE 754** section numbers — `§2.5.1`, `§1.1.7`, `§5.11` — which share the
  notation, are somebody else's to number, and are the whole point of
  [`25`](docs/25-benchmarks-and-expressiveness.md) §25.5. A rule that every `§N.M` resolves would
  fire on all of them, so the gate stays on the defining end where it is sound. One internal
  exception is left standing and named rather than fixed:
  [`01`](docs/01-vision-and-premise.md) cites its own §1.4.5, which is the fifth item of §1.4's
  list rather than a heading.

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
