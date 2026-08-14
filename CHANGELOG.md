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

- **Fifteen primitives that are a table, a grammar or somebody else's parser compile**, to both
  code generators, as a **call into a runtime library the program links**
  ([`docs/93`](docs/93-the-native-backends-report.md) §93.12,
  [`adr/0029`](docs/adr/0029-the-runtime-library-is-linked-and-owns-the-arena.md)): `digest`,
  `digest_keyed`, `digest_eq`, the two hex and two base64 primitives, `uuid_parse`, `uuid_version`,
  `str_upper`, `str_lower`, `str_to_int`, `str_replace`, `time_format` and `time_parse`. The
  library is `compiler/crates/beck-prim`, and it is **the same crate the evaluator calls** —
  `beck-core`'s digests and `beck-eval`'s civil calendar moved into it — so "both backends compute
  the same digest" is a property of there being one function rather than a claim a differential
  supports. **905 → 941 definitions compile across the tree and refusals go 173 → 137.**
  - **Linked rather than emitted or asked for**, and the argument is a cost: a linked `digest` is
    274 ns a call where the same call *asked* across the worker's pipe is 5.2 µs, measured in one
    run by `measure_native.rs::what_a_linked_primitive_costs`. Emitting instead would be a second
    BLAKE3 and a second Unicode table beside the ones already linked.
  - **No pointer crosses the ABI.** The library owns the arena — `main` asks `beck_prim_arena` for
    the heap instead of `malloc` — so every call carries **offsets**, and `beck-prim` contains no
    `unsafe` block and no raw-pointer read. That is what keeps
    [`docs/43`](docs/43-threat-model.md) §43.4's structural claim true; the crate joins `beck-wasm`
    and `beck-play` in `mode_b.rs::an_exported_symbol_is_the_only_exception_to_forbid_unsafe`,
    which counts its two export attributes exactly.
  - **A call costs its answer and nothing else**, because the outcome record sits *above* the
    arena's mark: `a_linked_call_costs_its_answer_and_nothing_else` counts bytes rather than
    seconds, in both emitters. The archive is embedded in `beck` (6.1 MiB compressed, from
    21.4 MiB) and written out only for a module that calls one of the fifteen — which takes a
    compiled program from 16 KiB to 4.9 MiB, so "only when it is called" is a gate.
  - **A latent bug in Cranelift's `narrow`**: its `Bool` arm extended an `I8` to an `I8`, which the
    verifier rejects. It had been unreachable since the four host primitives arrived, because none
    of them answers a `Bool`, and `digest_eq` is the first that does.
  - **Corrected in place**: `docs/93`'s not-built list carried seven primitives that compile —
    `str_trim`, `str_repeat` and `sort_by` had been compiling before this change — and its opening
    named `validate` as the definition `examples/todo.beck` could not compile, which was also out
    of date. All nine of that program's definitions compile.

- **`case [first, *rest]` compiles**, to both code generators
  ([`docs/93`](docs/93-the-native-backends-report.md)) — the last pattern form they refused. The
  length is tested before any element is read, so nothing can load past the end of a short list;
  everything inside a pattern is [`docs/90`](docs/90-pattern-matching-report.md)'s recursion,
  unchanged.
  - **The refusal it replaces had been false for three reports.** It read "a collection is not on
    this heap yet", and a collection has been on this heap since
    [`docs/93`](docs/93-the-native-backends-report.md).
    `a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one` exists to catch exactly this
    and could not: it resolves the **type** a reason blames, and this sentence names no type. The
    corpus pass now also holds every refusal against a list of sentences this backend may no longer
    say about itself — checked by putting the old refusal back and watching it name its six
    definitions.
  - **The tail is copied, not borrowed.** A suffix header offset into the element run would have an
    element read as the data block's `used`, which is the word
    [`docs/93`](docs/93-the-native-backends-report.md)'s append writes at. The evaluator copies too
    (`Arc<Vec<_>>` cannot share a suffix), so neither backend is quietly quadratic against the
    other.
  - **889 → 905 definitions compile and refusals go 189 → 173**: six list patterns, and ten
    definitions that were only waiting on them.

### The image build

- **A reset connection no longer ends an image build.** `the-image-builds` failed with `the TLS
  handshake with packages.wolfi.dev failed: Connection reset by peer` on the eleventh package of a
  dozen, after the first ten had been fetched and cached — the first thing the live fetch found
  since [`docs/92`](docs/92-supply-chain-and-release-report.md) §92.13 named the transport as the step
  nobody had executed. `beck-cli/src/fetch.rs` now attempts a hop up to four times, backing off from 500 ms,
  and **classifies rather than reports**: a transport that went away or a 408/425/429/5xx is
  attempted again; a 404, a certificate that does not verify, a reply over the size cap or a URL
  that is not HTTPS is answered once, because a second attempt fails the same way and only delays
  the message. Attempts are counted per hop, so a redirect does not spend the budget the package
  behind it needs.
  - **A body that stops early said the wrong thing**, and the classification is what surfaced it:
    a truncated reply and one over the 128 MiB cap arrived at the same `map_err` and both read
    "the reply is longer than 128 MiB". That message sends a reader looking for a package that
    grew, and it would have classified a mid-body reset as permanent. The two are now distinguished
    by `LengthLimitError`.
  - **The gates drive the retry loop itself** with the attempt supplied by the test and no network
    — a reset twice then a body, four resets then one error naming the bound, a 404 attempted once,
    a redirect whose second hop is reset. Checked by setting `ATTEMPTS` to 1: three go red.

### Concurrency

- **A `parallel:` child that fails stops its siblings**
  ([`docs/80`](docs/80-structured-concurrency-report.md)) — the signal
  [`docs/80`](docs/80-structured-concurrency-report.md) §80.12 forecast and
  [`docs/80`](docs/80-structured-concurrency-report.md) §80.12 left open.
  - **It stops the children *after* the failure, and that qualifier cost a defect to learn.** The
    obvious signal — any failing child stops every sibling — passed its own new gate and broke
    `a_childs_failure_joins_at_the_scope_and_the_earliest_child_wins`, green since
    [`docs/80`](docs/80-structured-concurrency-report.md): two children that both raise became a
    race over which error the scope reports. **Eight failures in forty runs of the whole suite, and
    none when run alone.** Cancelling only the children an ordered join would never have reached
    makes this a change in when work stops rather than in what a scope answers; the check that the
    race is gone is 0/40 against 8/40.
  - **It rides `Interp::burn`**, the step counter. A checkpoint at call boundaries would miss an
    iterative loop, because a tail call does not nest; "the program is spending steps" is exactly
    when stopping it is possible and worth doing.
  - **A stopped child's error is discarded rather than raced.** The scope answers with the earliest
    child in source order that failed for a reason of its own — otherwise the answer would depend on
    which thread lost. The flag is a chain, because a scope may nest inside a scope.
  - **Cost to a program with no scope in it**, from
    `measure_concurrency.rs::what_the_cancellation_check_costs_a_program_without_a_scope` (release,
    median of nine): **388.3 / 380.1 ns per iteration with the check, 385.0 / 374.7 without** — about
    1%, and §80.8 declines to call it free, since an earlier run of the same build differed by more
    than the two columns do. Flat across a 10× size change, which is what a loop-invariant branch
    predicts.
  - **Gated by a count, not a clock**: `concurrency.rs::a_failing_child_stops_its_siblings` asserts
    the stopped sibling reached its peer *more than zero* times (so it was running) and *far fewer
    than 400* (so it was stopped) — it reaches 4 or 5. Checked by disabling the signal and watching
    it hit 400/400, and the ordering gate beside it was run forty times rather than once.
- **Which wasm can have threads** (§80.9), which corrects a flat "no" this work first wrote down.
  `wasm32-wasip1-threads` is a **stable** target and really spawns — its module imports
  `thread-spawn` where `wasm32-unknown-unknown` compiles the same call to "operation not supported
  on this platform". But that is WASI, and the two places this repo emits wasm are *browser*
  targets: there threads need the `atomics` feature `rustc` reports as unstable on the pinned
  channel, a `std` rebuilt with it (`-Z build-std`, nightly), and cross-origin isolation plus a
  worker per thread in the page. `wasm32` is the only bitness on stable — there is no `wasm64`
  target. Where the threading target *will* matter is the server-side WASM tier
  [`docs/05`](docs/05-tier-lowering.md) §5.4 names and nothing emits yet. The playground's children
  run in order, which is *correct* for the same reason an ordered join always was — it loses the
  overlap, never an answer.

- **`parallel:` runs its children at the same time**, on a thread each
  ([`docs/80`](docs/80-structured-concurrency-report.md)). This closes
  [`docs/80`](docs/80-structured-concurrency-report.md) §80.12's first sentence and the last named
  remainder of [`docs/08`](docs/08-roadmap.md)'s structured-concurrency bullet.
  - **The soundness is not in this change.** §80.12 put it in the checker: no child may name another
    (`B0398`) and none may perform an effect another could observe (`B0399`), so the scope's answer
    does not depend on the order. This adds no analysis and no scheduler — it starts two threads,
    because the program has already been proved not to care.
  - **One of the three blockers §80.12 named was removed by accident.** The `Host` trait became
    thread-safe in [`docs/93`](docs/93-the-native-backends-report.md), which wanted it so three
    backends could ask one question. Recorded because the lane table predicts file collisions and
    has nothing to say about a branch that removes another's blocker.
  - **Fuel is split, not shared** (§80.6). Sharing it is an atomic read-modify-write on every step
    of every program — the hot path [`docs/70`](docs/70-the-evaluator-gets-fast-report.md) is about
    — and it makes *which* child runs out a race. Each child gets an equal share of what remains and
    the scope charges the parent what they actually spent, so the total matches a serial run. The
    cost is that a child which would have used more than its share now runs out where a serial run
    would have let it continue; `spawn` is discharged by `Tier::Server` alone, so none of this can
    reach a replay.
  - **Nothing is cancelled.** A scope whose first child fails still waits for its siblings. §80.12
    forecast that a backend starting children together would need a cancellation signal; that
    backend exists now and the signal still does not.
  - **What it is worth**, from `measure_concurrency.rs` (release, medians): two children that each
    wait 200 ms take **201.1 ms** against **400.7 ms** in order — **1.99×**, and 2× is the ceiling
    for two children. Children that compute get the same once each is worth a thread, and the
    crossover is measured rather than guessed: **0.34×** at a child of ~170 µs, **1.30×** at
    ~580 µs, **1.85×** at 7.9 ms.
  - **Where the per-child cost goes** is not where the question expects: a bare thread is 94.4 µs on
    this machine and the 256 MiB stack reservation adds 29.3 µs, so the reservation is the *smaller*
    half. Neither is a knob — the reservation is what the depth ceiling needs
    ([`adr/0007`](docs/adr/0007-evaluator-stack-is-declared-not-discovered.md)) and the thread is
    what running two things at once is.
  - **The gate is a deadlock-or-pass, not a clock**: `concurrency.rs::two_children_actually_overlap`
    uses a host that will not answer until every child has arrived, so a serial evaluator cannot
    pass it at any speed. Checked by forcing it back to an ordered join and watching it go red.

### The native backends

- **The four primitives that ask the host compile** — `now()`, `uuid()`, `secret_env` and
  `http_fetch`, to both code generators ([`docs/93`](docs/93-the-native-backends-report.md)).
  The worker's protocol has a **second direction**: a compiled call may stop mid-flight, write a
  question frame and block until the host answers. This empties
  [`docs/08`](docs/08-roadmap.md) §8.5.5's **Lane E**, whose last row said this was the one item in
  the lane that is not a missing emitter.
  - **What a question carries is a shape and a word per argument**, so the host decodes and encodes
    through `beck_llvm::heap` without a second table of what each primitive's types are — the trick
    [`docs/93`](docs/93-the-native-backends-report.md)'s deferred leaves play, one
    subsystem over. The answer is a **tail appended at the arena's high-water mark**, never a whole
    arena, so nothing a live value points at can be rewritten by an answer.
  - **The blocker for two of the four was a type, not the protocol.** `secret[T]` had no machine
    representation, so `secret_env` could not answer with one and `HttpRequest` — whose `secrets`
    field holds them — had no layout at all. It is laid out as the one-field object it already is at
    run time; unwrapping would make a secret and its `Str` indistinguishable in compiled code, which
    is the one thing §3.5 claims they are not.
  - **A failed `http_fetch` needed nothing new.** The host answers with `Trap::Raised` and the same
    two-word pair a compiled `raise` builds
    ([`docs/93`](docs/93-the-native-backends-report.md)), so the program's own `try:` catches it
    without knowing an upcall happened.
  - **The host is one description now**: `beck_core::host::Atoms`, which the evaluator's `Host`
    extends and both compiled backends ask. That is what makes a differential over `now()` a
    comparison of the program rather than of two clocks —
    `native.rs::the_two_backends_agree_on_the_host_effects` and its Cranelift twin drive all three
    backends from one stated host over 16 calls, and assert the outbound count so a silent fallback
    cannot pass.
  - **870 → 889 definitions compile across the tree and refusals go 208 → 189**, over 64 programs
    each compiled alone. None of the four appears in a refusal anywhere, and
    `every_corpus_program_produces_a_module_llvm_accepts` is the gate that says so rather than a
    grep somebody ran once.
  - **Cost**, from `measure_native.rs::what_asking_the_host_costs` (release, median of nine): a
    question that cannot point into the heap is a flat round trip — **24.5 µs** at 16 live elements
    and **29.0 µs** at 4,096 — and one that can carries the live arena, **26.8 µs** and
    **162.7 µs** for 664 and 163,864 bytes. The shape is gated without a clock by
    `native.rs::what_a_question_carries_is_a_decision_and_not_an_accident`, which counts the bytes
    at the same two sizes.
  - **A limit on compiled time is not a limit on a call** (§93.11). The worker's wall-clock bound
    exists because there is no fuel in compiled code; it was killing an `http_fetch` that was
    waiting on a peer. The deadline is now stood down while the host works and re-armed as a fresh
    one, so it still covers every instruction the worker executes and nothing the host does.

### The editor

- **`beck lsp` edits: references, document highlight, prepare-rename, rename and inlay hints**
  ([`docs/65`](docs/65-the-editor-report.md)). Every answer is in `beck_core::editor`, so a
  browser tab can ask for them too; the server translates JSON-RPC and nothing else (§4.6). This
  empties [`docs/08`](docs/08-roadmap.md) §8.5.5's **Lane C** and closes the two rows
  [`docs/65`](docs/65-the-editor-report.md) §65.8 called its largest gaps.
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
  [`docs/92`](docs/92-supply-chain-and-release-report.md),
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
  exercises it. §92.14 records seven mutations, one gate each, and the one that did not fire until
  it was rewritten.

### The front end

- **Macro expansion is bounded by what it produces** (`B0214`), which is
  [`docs/14`](docs/14-review-findings.md)'s **F17** — open since the review — and a row of
  [`docs/43`](docs/43-threat-model.md) §43.4 that now says *built*. Expansion was bounded in **depth**
  twice over and in **work** not at all, and a macro that doubles its output is shallow: eight
  nestings of a two-line macro is 256 copies of its argument, twenty-four is sixteen million, and
  every one of those programs is six lines long and satisfies every other limit the front end has.
  That is [`docs/82`](docs/82-the-edge-report.md) §82.10's pattern a fourth time — a
  limit added at the one production somebody thought of, bypassed through a different one — and it
  matters here rather than in the abstract because [`docs/42`](docs/42-security-assurance.md) §42.2's
  playground compiles a stranger's source in their own browser tab.
  - The expander charges what each expansion **produces**, per node, against **100,000 nodes for the
    whole module** — per module because that is what a compile is, and because a per-call budget
    would let a program spend it once per call
    ([`docs/82`](docs/82-the-edge-report.md) §82.5's arithmetic one
    subsystem over).
  - **The number is measured, not declared.** Across the corpus, both benchmark suites, both SICP
    chapters, the examples and the standard library, the largest total expansion is **138 nodes**
    (`sicp/ch3.beck`; `examples/todo.beck`'s page is 94), so the budget is about 725× the biggest
    real one and about seventeen nestings of a doubling macro.
  - The count is **iterative and self-bounding**: it walks with its own stack, because the tree being
    measured is one a macro just built and a recursive count would be a claim about the *host's*
    stack ([`docs/93`](docs/93-the-native-backends-report.md) §93.9 one subsystem over) — and
    it stops when the budget does, so a macro that would produce a billion nodes is refused after a
    hundred thousand have been counted.
  - Gated by `macro_bomb.rs` in **both** directions and with the control a refusal needs: a doubling
    macro 24 deep is refused *by the budget* and not by either depth counter, the same macro 8 deep
    still compiles, and every program in the tree still expands. `pending_security.rs`'s F17 test is
    deleted, which is what that file's own rule asks for.

### The native backends

- **A generic definition compiles, once per type it is used at**
  ([`docs/93`](docs/93-the-native-backends-report.md)): monomorphisation, on both code generators.
  It was the largest single refusal class left, and the mechanism is a **pass over the program**
  rather than anything in either emitter. **850 → 870 definitions** compile across the tree,
  refusals go 223 → 208, and the ones blaming a type parameter go **63 → 38** — §93.10 is honest
  about the 38.
  - **The type arguments were already recorded and nobody had read them.** There is no
    type-argument list in `Core` and no instantiation table on `Program`, and neither is needed:
    every `Core` node carries its solved type, so a call's `Global` node holds the *instantiated*
    function type while the definition's own `params` and `ret` still name the rigid `Con("T", [])`.
    Matching the two, positionally, recovers `T := Int` in thirty lines.
  - **A backend pass, which [`docs/38`](docs/38-literature-survey.md) §38.1 had already decided**:
    dictionaries are the semantics and monomorphisation is a backend choice, because whole-program
    specialisation fights incrementality. So `beck-core`, the checker and the evaluator are
    unchanged and cannot tell it happened. Shared between the emitters for the reason the heap is —
    it is the program both are handed, not a code generator.
  - **Keyed on the whole type and not on a head constructor or a representation.** `Int` and `Bool`
    are both one immediate word; merging them would answer `1` where the evaluator answers `true`.
    Two parameters are read off in **use** order, so `swapped[Str, Int]` calls `paired@Int,Str` —
    the same instantiation `int_then_text` asks for, and the differential asserts that sharing.
  - **Refused, with reasons that are each real:** polymorphic recursion (`MAX_INSTANTIATIONS` is
    64 against a measured maximum of **three**, over 65 templates and 28 instantiations); a call
    where nothing decides the type, because minting `anything@?3` would make a symbol a function of
    an inference counter rather than of the program; and a **bounded** definition, which is
    [`docs/93`](docs/93-the-native-backends-report.md)'s closure boundary and not this.
  - **The finding is that a partial answer was worse than none** (§93.10). The first version gave up
    part way through and left sixty-four instantiations behind, each refusing because it called the
    next — sixty-four true refusals that together said nothing, none of them naming a definition the
    reader had written. A round that keeps a template it had been specialising is now thrown away
    and re-run with that template forbidden, leaving one refusal naming one definition.
  - Gated by `the_two_backends_agree_on_generics` / `the_three_backends_agree_on_generics` (103 and
    100 calls over `genfix`, written around the ways of picking the **wrong instantiation**),
    with a control **by name** — fourteen instantiations asserted present, every template asserted
    gone, and `paired` asserted to have one instantiation and not two, because a run that compiled
    `firstly` once and called it three times would answer correctly and be wrong. Plus
    `a_polymorphically_recursive_definition_is_refused_rather_than_compiled_forever` and
    `a_generic_whose_type_nothing_decides_is_refused_rather_than_guessed`, which asserts that no
    symbol is named after an inference variable.
  - **Corrects [`docs/93`](docs/93-the-native-backends-report.md)** §93.15, whose table has generic and
    bounded definitions as one row. They were never one item: a generic definition needs no
    dictionary at all.

- **`str_split` and `str_chars` compile, and "two loops" was never a cost.** On both code
  generators. The reason on record was *"answers with a list whose elements it also allocates,
  which is two loops rather than the one every list this backend builds has"* — a description of
  the **code**, not of what it costs. Two loops is what makes it cheap: the first counts the
  pieces and the second fills them, so the answer is allocated once and never grown.
  **837 → 850 definitions** compile across the tree and refusals go 236 → 223.
  - **One function, because the evaluator makes them one**: `str_split` on an empty separator
    answers characters, so `str_chars(s)` *is* `str_split(s, "")` and the two share a body. The
    empty separator arrives as the offset **`0`** rather than a pooled `""` — `0` is never a live
    object, so `str_chars` costs no literal. That correction came from a gate:
    `the_literal_pool_is_a_function_of_the_program` went red on the version that interned one,
    because emitting had discovered a literal the survey never saw.
  - `beck.str.piece` — the bytes of a `Str` in a byte range, with its character count — is factored
    out of `str_trim`'s tail and shared, so the one place that decides what a substring's header
    says is the one place both primitives use.
  - Gated by `a_split_costs_its_answer_and_nothing_per_call`: **4.0× the arena for 4× the
    separators**, with no clock in it, read off the whole reply arena so a split that had grown its
    answer would be caught by the blocks it abandoned. The differentials run every string against
    eight separators — including the empty one, one that is the whole string, one that **overlaps
    itself** (`"aaa"` on `"aa"` is `["", "a"]`), and one longer than the string — reading the
    length *and* an element at six indices, because counting the pieces correctly and allocating
    them wrongly passes the first test and fails the second. 3,912 → **4,872 text calls** compared,
    all three backends agreeing.

- **`str_trim` compiles, and its refusal was a claim about the wrong set.** On both code
  generators. The reason on record was *"trims Unicode whitespace, which is a table for the same
  reason case mapping is"*, and the two are not the same reason at all: `White_Space` is **25 code
  points**, none of them four bytes long, where case mapping is some fourteen hundred mappings and a
  handful that change a string's length. So a trim is a switch over five lead bytes, and
  `str_upper` stays refused for a reason that is true of it.
  - **`examples/todo.beck` compiles all nine of its definitions** — the first program in this tree
    to compile whole, and the row [`docs/93`](docs/93-the-native-backends-report.md) left at eight.
    Across the corpus, both benchmark suites, both SICP chapters, the examples and the standard
    library, **812 → 837 definitions** compile and refusals go 261 → 236.
  - One pass, and it allocates once: the leading run is skipped whole, then every byte is either the
    start of a whitespace character — skipped, and not recorded — or one byte of something else,
    which moves the end. `beck.str.ws` may be asked at **any** byte of well-formed UTF-8 and never
    answers inside a character, because no continuation byte can be `0xC2`, `0xE1`, `0xE2` or
    `0xE3`, so the scan needs no decoder for what it is skipping over.
  - **Gated in two halves, and neither restates the other.**
    `native.rs::the_whitespace_this_backend_knows_is_every_one_rust_does` walks all of Unicode and
    asserts the three facts the emitters were written from — 25 code points, none four bytes long,
    four non-ASCII lead bytes — so a Rust upgrade that changed the set goes red *there*, at the
    place that names what to edit. The differentials then run every code point
    `char::is_whitespace` answers for, four ways each, **derived from that function rather than
    written out**, plus the four near misses (`U+200B`, `U+180E`, `U+FEFF`, `U+2060`) that look like
    whitespace and are not. 3,564 → **3,912 text calls** compared, all three backends agreeing.
    Checked by making it red: dropping the `0xE3` arm from one emitter fails on `U+3000`.
  - `trims` moved from `what_the_heap_does_not_reach_is_refused_by_name`'s refusal list to its
    control list — the fourth row to cross that line — and `a_corpus_fold_compiles`'s "what is still
    refused" control now names a **type parameter**, which is what is left.

- **A map grows, as the tree it always was**
  ([`docs/93`](docs/93-the-native-backends-report.md)): `map_insert`, `map_remove` and `map_merge` compile
  to both code generators, and a fold that keeps a `Map` is `Θ(n log n)` rather than `Θ(n²)`.
  **895 → 1,137 definitions** compile across the tree and refusals go 523 → 281.
  `examples/todo.beck` compiles **eight of its nine definitions**; the one left needs a Unicode
  table.
  - [`docs/93`](docs/93-the-native-backends-report.md) §93.7 forecast that a list's answer would not work
    here, and it was right: a list's refusal was about a *layout* and a map's is about a *structure*.
    An insert lands in the middle of a sorted run and every entry after it shifts, however the header
    is arranged. What removes it is the structure `beck_core::pmap` already uses — a
    **weight-balanced tree**, whose insert rebuilds the path and shares every subtree it did not
    touch. Five words a node (subtree size, key, value, two children), the same `DELTA` and `RATIO`
    the evaluator's module argues for, and an empty map is the offset `0`.
  - **Sound for free**: a node is never written after it is built, so the map an insert was given is
    exactly what it was — [`docs/93`](docs/93-the-native-backends-report.md) §93.7's argument again,
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
  - **The finding is a name collision** (§93.6): `awfy/richards.beck` has a definition called
    `dispatch`, every user definition was mangled to `beck.<name>`, and the module's own dispatcher
    is `beck.dispatch` — so a program that had done nothing wrong got *"invalid redefinition of
    function"*. Latent since [`docs/93`](docs/93-the-native-backends-report.md), and it surfaced here
    because a collision needs both halves to exist: `dispatch` had never compiled before. A user
    definition is `beck.def.<name>` now, in both emitters.

- **A list grows, and the refusal was about a layout**
  ([`docs/93`](docs/93-the-native-backends-report.md)): `list_append` compiles to both code generators and
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
    reply, which is [`docs/93`](docs/93-the-native-backends-report.md) §93.1's round trip again.

- **A raise arrives, and a handler catches it**
  ([`docs/93`](docs/93-the-native-backends-report.md)): `raise` and `try:` compile to both code
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
  - **The finding is about the protocol, not the feature** (§93.8): the handler cleared the trap
    code with `store i32 0`, and the cell's first word is a code *and* a span while the worker's loop
    reads it as one `i64` to decide whether the call answered. So a caught failure came back with a
    stale span in the high half, looking like a trap with an empty arena. Two pieces of one program
    disagreeing about what "cleared" means, which is
    [`docs/93`](docs/93-the-native-backends-report.md) §93.8's class of defect one level down.

- **A view arrives, as a recipe** ([`docs/93`](docs/93-the-native-backends-report.md)): a
  definition that returns `Html` compiles to both code generators. What goes in the arena is the
  **call** `html_el(tag, attrs, children)` would have been given rather than the tree, and the host
  bakes it with `beck_core::html::element` — the evaluator's own `html_el`, lifted out and called
  from both. **650 → 688 definitions** compile across the tree, refusals go 768 → 730, and **21 of
  the 32 corpus programs have a `view` that compiles**, `examples/todo.beck`'s among them. Gated by
  `native.rs::the_two_backends_agree_on_views` (253 calls),
  `cranelift.rs::the_three_backends_agree_on_views` (127), the `ui:` block's own pair, and
  `a_page_costs_its_own_nodes_and_nothing_per_page` — 96 bytes a row and 504 a page at 100 rows and
  at 800, a shape gate with no clock in it. **Not faster**: 0.80×–1.33× the tree-walker at two
  sizes, and §93.5 says why that is the design rather than a constant to tune.

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
    [`docs/93`](docs/93-the-native-backends-report.md) §93.9's finding a fourth time: a refusal
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
    tree-walker, asserted to be** — [`docs/93`](docs/93-the-native-backends-report.md) §93.5's
    precedent. The work is a `memcpy` and the *call* is 2,000 list objects down a pipe and an answer
    read back out of a reply, so what that row measures is
    [`docs/93`](docs/93-the-native-backends-report.md) §93.1's round trip and not this primitive.
  - Gated by `native.rs::the_two_backends_agree_on_closures` and
    `cranelift.rs::the_three_backends_agree_on_closures` (1,178 calls each now) and
    `a_sort_costs_four_runs_and_a_concatenation_costs_its_answer` — a shape gate with no clock in it,
    because a concatenation that grew would hold every intermediate and a merge sort that allocated
    per level would pass every differential.
  - **Corrects [`docs/93`](docs/93-the-native-backends-report.md)** §93.13 and §93.9, which name both
    of these as refused. §93.13's claim about `sort_by` — "the next one to build, not one that cannot
    be" — held for one commit.

- **A closure arrives, and it does not leave**
  ([`docs/93`](docs/93-the-native-backends-report.md)): a `lambda` compiles to both code generators as
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
  ([`docs/93`](docs/93-the-native-backends-report.md) §93.14). `a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one`
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
  [`docs/93`](docs/93-the-native-backends-report.md) §93.9's finding a second time and
  corrects [`docs/93`](docs/93-the-native-backends-report.md) §93.9:

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
  ([`docs/93`](docs/93-the-native-backends-report.md)): `Map[K, V]` is a count, every key in
  key order, then every value — so `map_get` is a binary search and `map_keys` is one `memcpy`.
  `map_len`, `map_get`, `map_contains`, `map_keys`, `map_values`, the six comparisons and `{}`
  compile; `map_insert`, `map_remove` and `map_merge` are refused. **403 → 452 definitions**, the
  corpus **30 → 53 across 31 of its 35 files**, and **nine corpus programs compile their
  `apply_event`**. Of 1,026 refusals across the tree beforehand, 472 blamed a `Map`; **none now
  blames a collection for having no layout**. Gated by `native.rs::the_two_backends_agree_on_maps`,
  `cranelift.rs::the_three_backends_agree_on_maps` (898 calls each),
  `a_lookup_costs_the_same_whatever_the_map_holds` and `a_corpus_fold_compiles`.
- **`Repr::order` is the only place either backend names a comparison.**
  [`docs/93`](docs/93-the-native-backends-report.md) §93.8 said that would prevent a fourth
  record-field-compared-by-offset defect; there was a fourth, in the one site not yet converted, and
  all five go through the accessor now. A new reference kind is a compile error where its comparison
  has to be written (§93.8).
- **`Html` is refused by name.** It fell through to "not a type this module declares", which is true
  about the path taken and misleading about the cause.


- **A list arrives read-only** ([`docs/93`](docs/93-the-native-backends-report.md)): a
  `list[T]` is a value both code generators compile — literals, the six comparisons, `list_len`,
  `list_is_empty`, `list_get`, `list_contains`, `list_index_of`, `list_slice`, `list_take`,
  `list_drop` and `list_reverse` — and **`list_append` is refused** by name, because an arena cannot
  prove nobody else holds the accumulator ([`docs/93`](docs/93-the-native-backends-report.md) §93.15's
  forecast, cashed). **344 → 403 definitions** compile across the tree. Gated by
  `native.rs::the_two_backends_agree_on_lists` and
  `cranelift.rs::the_three_backends_agree_on_lists` (1,425 calls each), and by
  `a_list_slice_costs_its_answer_and_not_the_list_it_came_from`.
- **`str_index_of` compiles, and the reason it did not was false.**
  [`docs/93`](docs/93-the-native-backends-report.md) §93.9 blamed the prelude's `Option` for having
  no layout; it has had one since [`docs/93`](docs/93-the-native-backends-report.md). Every gate stayed green
  because each asserted a refusal *said* something and none asked whether what it said was so.
  `a_refusal_that_blames_a_type_is_asked_whether_that_type_has_one` is the gate that would have
  gone red (§93.9).
- **The reply decoder is iterative**, so `MAX_DEPTH` bounds the *value* rather than the host
  thread's stack. It recursed, and a debug build aborted at about 1,600 against a declared ceiling
  of 2,048 — which made what could be decoded a function of how the compiler was built. Gated by
  `a_value_at_the_declared_ceiling_decodes_rather_than_aborting`, which builds a value exactly that
  deep by hand (§93.9).
- **The LLVM record comparison compared a `list` field by its offset** — the same defect
  [`docs/93`](docs/93-the-native-backends-report.md) §93.8 found in Cranelift for a `Str`, third
  occurrence. Caught by the differential's pairs (§93.8).
- **An unreachable refusal reason deleted.** The higher-order collection primitives are refused a
  line earlier by their own argument, so the table's entry for them could never be produced
  ([`docs/23`](docs/23-incremental-views-report.md) §23.16's pattern).
- **`measure_native.rs` asserted a ratio that depends on the build profile** — that text's
  accumulator is slower than the tree-walker, which is true in release and false in debug. The
  assertion is gone and the claim is gated with no clock in it instead:
  `an_accumulator_costs_the_square_of_what_it_builds` reads the arena and finds 15.9× the bytes for
  4× the steps.

- **Text is on the heap** ([`docs/93`](docs/93-the-native-backends-report.md)): a `Str` is a value
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
  minute after the second emitter existed (§93.8).

### The evaluator

- **`str_slice` was charged the length the caller wrote rather than the length it takes**, so
  `str_slice(s, 0, 1_000_000)` on a five-character string cost a million steps and "from here to
  the end" could exhaust the fuel budget on a program doing nothing. The arm one above it in
  `work_of` has stated the rule since it was written. Gated by
  `interp::tests::a_slice_is_charged_what_it_takes`, and found by the native differential answering
  while the tree-walker ran out of fuel (§93.6).

### Docs

- **A report was carrying another report's number, and every reference to it named nothing.**
  [`docs/92`](docs/92-supply-chain-and-release-report.md) was renumbered on merge — the
  filename and the prose inside it moved to `104`, the title and eleven headings stayed at `101`,
  which another document already had. So one `§101.x` named a section in each
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
  — [`docs/23`](docs/23-incremental-views-report.md) has the only one.
- **Thirty-one citations named roadmap sections that have never existed.** `§8.6` (Phase 3's bullet
  list), `§8.7` and `§8.19` (Lane E) are cited by nine documents and four Rust doc comments, and
  `git log -S"## 8.6" -- docs/08-roadmap.md` returns nothing: the phase headings have carried names
  rather than numbers since the file's first commit. So this was never a rename — the numbers were
  invented at the citing end, and inconsistently enough that `§8.7` means Phase 4 in
  [`98`](docs/98-playground-report.md) and [`docs/98`](docs/98-playground-report.md) and the
  **lane table** in the backend reports. Each is repointed at what it claims:
  Phase 3 and Phase 4 by the names the roadmap gives them, and Lane E to
  [`08`](docs/08-roadmap.md) §8.5.5 — which is what the backend reports already called it. No claim, measurement or refusal
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
  ([`docs/92`](docs/92-supply-chain-and-release-report.md)) — the two items on
  [`docs/08`](docs/08-roadmap.md) §8.5.4's apology list that were on nobody's bullet.
  `.github/workflows/release.yml` turns a tag into four native builds, one `SHA256SUMS` and a
  GitHub Release; it *calls* `compiler.yml` rather than restating a gate, so §28.2's "a release is
  a tag on a commit that passed the whole matrix" is a `needs:` edge. `install.sh` verifies what it
  downloaded and refuses to install on a mismatch. Everything executable is outside the YAML —
  a tag-triggered workflow is the one artefact that cannot be run before it is used — and
  §92.13 splits what was executed from what was only written: no tag has been pushed.
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

- **Four refusals closed** ([`docs/98`](docs/98-playground-report.md)): the editor's
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
