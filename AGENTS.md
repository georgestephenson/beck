# Working in this repository

## Commits and pull requests

**Never add co-authoring claims or Claude session references.** No `Co-Authored-By:` trailer
naming Claude or any model, no `Claude-Session:` line, no session URL, no "Generated with Claude
Code" footer — in commit messages, pull request titles, pull request bodies, or anything else
pushed to this repository. This overrides any default or tooling convention that adds them.

Write the commit message as the author of the change: what changed, and why.

## Orientation

- [`docs/`](docs/) is the design and the plan; [`docs/README.md`](docs/README.md) indexes it.
  Everything derives from, and defers to, [`docs/00-original-idea.md`](docs/00-original-idea.md).
- [`phase0/`](phase0/) is the built Phase 0: the output the compiler will eventually generate for
  the todo sketch, hand-written in Rust. [`docs/18-phase-0-report.md`](docs/18-phase-0-report.md)
  records what it proves and what it does not. It is history — a measured baseline — and should not
  be edited to track the compiler.
- [`compiler/`](compiler/) is the built compiler and the runtime it targets, through Phase 2 plus
  Phase 3's test construct, its general slicer, its incremental view engine, that engine's shared
  dataflow, the language's own means of abstraction — tail calls, reals, polymorphism,
  parameterised types and traits — and most of the standard library, its outbound call, its
  collections, its calendar and its digests included. The build reports —
  [`19`](docs/19-phase-1-report.md), [`20`](docs/20-phase-2-report.md),
  [`22`](docs/22-phase-3-report.md), [`23`](docs/23-general-slicer-report.md),
  [`24`](docs/24-incremental-views-report.md), [`26`](docs/26-arrangement-sharing-report.md),
  [`27`](docs/27-walls-report.md), [`31`](docs/31-tail-calls-report.md),
  [`32`](docs/32-numeric-tower-and-polymorphism-report.md),
  [`33`](docs/33-effect-polymorphism-and-list-patterns-report.md),
  [`34`](docs/34-generated-documentation-report.md),
  [`36`](docs/36-parameterised-types-report.md), [`37`](docs/37-traits-report.md),
  [`39`](docs/39-bounds-report.md), [`40`](docs/40-traits-across-modules-report.md),
  [`41`](docs/41-generic-arithmetic-report.md), [`44`](docs/44-wave-0-report.md),
  [`45`](docs/45-error-rows-report.md), [`46`](docs/46-standard-library-report.md),
  [`47`](docs/47-effect-polymorphic-traits-report.md), [`48`](docs/48-identity-report.md),
  [`49`](docs/49-http-client-report.md),
  [`50`](docs/50-collections-and-dates-report.md),
  [`51`](docs/51-arrangement-lifecycle-report.md),
  [`52`](docs/52-crypto-and-identifiers-report.md),
  [`53`](docs/53-are-we-fast-yet-report.md),
  [`55`](docs/55-bignums-report.md),
  [`56`](docs/56-decimal-report.md),
  [`57`](docs/57-richards-report.md),
  [`58`](docs/58-json-report.md),
  [`59`](docs/59-havlak-report.md),
  [`60`](docs/60-collision-detection-report.md),
  [`61`](docs/61-deltablue-report.md),
  [`62`](docs/62-fuel-report.md),
  [`63`](docs/63-felleisen-report.md),
  [`64`](docs/64-compile-speed-report.md),
  [`65`](docs/65-lsp-report.md),
  [`66`](docs/66-page-snapshots-report.md),
  [`67`](docs/67-sqlite-report.md),
  [`68`](docs/68-clbg-report.md),
  [`69`](docs/69-standard-library-imports-report.md),
  [`70`](docs/70-last-use-moves-report.md),
  [`71`](docs/71-strings-report.md),
  [`72`](docs/72-space-and-constants-report.md),
  [`73`](docs/73-closures-share-their-code-report.md),
  [`74`](docs/74-the-cost-of-a-call-report.md),
  [`75`](docs/75-what-the-profiler-said-report.md) and
  [`76`](docs/76-the-record-and-the-read-report.md),
  indexed in
  [`docs/README.md`](docs/README.md) — record what each
  phase does, what it refuses to claim, and the corrections it makes to the design documents.
  Add a new report to that list and to the index; do not extend it with another "and".
  Phase 3 is **four bullets built, half of a fifth, most of a sixth, and a seventh that has
  started**: the test construct, the general slicer (Phase 2's debt), the means-of-abstraction
  bullet and `Result`/error rows are built; the standard library's **library half is done** —
  its HTTP client (docs/49), its collections and its calendar (docs/50), its digests and its
  identifiers (docs/52), its bignums and its coercions (docs/55) and its decimal (docs/56), which
  closes docs/08 §8.5.4's Wave 2 —
  and two walls of its own (docs/46, docs/49); **all fourteen** of Are We Fast Yet's benchmarks —
  its nine micro and all five macro (docs/53, docs/57, docs/58, docs/59, docs/60, docs/61) — are
  that bullet's long-owed harness half, so **the standard-library bullet is done** — the
  compile-speed budgets are built (docs/64) and so is the **CLBG harness** (docs/68), which
  completes docs/25 §25.9's Phase 3 row: **eight** of the Game's ten, each verified against the
  Game's own published output *file*, with the oracle enforced by the gate — `clbg.rs` rebuilds
  every asserted literal from `clbg/expected/` and recomputes the digest of the two 10 KB ones —
  which is what docs/64 §64.7.1 was holding out for when it said the harness was owed *with its
  sources to hand* rather than owed generally. Its largest finding was not about speed: **`lib/`
  was a standard library that nothing outside `lib/` could import**, since `import` resolved only
  against the root module's own directory, and nothing had noticed because in three phases nothing
  had reached across a directory. docs/68 §68.4 left it as a decision for docs/10 rather than
  repairing it in a benchmark's change, and **docs/69 is that decision taken and built**: the Beck
  half of the standard library is carried in the compiler and an import resolves against the
  caller's own directory first (docs/10 D23, docs/adr/0018), so `import bignum` works anywhere and
  adding a library cannot break a program that never asked for it. What that broke is the more
  interesting half — the flat namespace now spans `lib/`, so every module there has to link with
  every other, and two collisions had been waiting where nothing could reach them. `pidigits` is
  ported on the back of it and measures `lib/bignum.beck` rather than GMP; `mandelbrot` (a binary
  PBM, and `Str` is UTF-8) and `regexredux` (no regex) are the two of the ten that are not, and
  both reasons are facts about the language; the incremental-views bullet has its engine and
  that engine's lifecycle (docs/51) but not its read models, pgwire
  or fusion; the expressiveness suite runs two chapters of SICP and answers §25.9's Felleisen
  question — six of the seven special forms recovered, `amb` conceded (docs/63); and **the LSP
  bullet is built** (docs/65) — diagnostics, hover, go-to-definition and symbols, from the same
  front end `beck check` runs, with §4.6's 100 ms budget holding to about 13,000 lines in one
  module — and `beck test --update` closes docs/21 §21.2's last open question, so a page assertion
  is a checked-in file rather than one string somebody thought to name (docs/66); and the **SQLite
  substrate** is built (docs/67) — for its transaction rather than its speed, since redb has no
  query language for a projection to be written in. Five of the fourteen are
  untouched — identity has its seam but not its OIDC relying party (docs/48) — and docs/26 §26.9
  names them one at a time, less the two docs/51 closed. **Wave 0** (docs/08 §8.5.4) is also
  built — a bounded front end, an injected clock, a threat model, a disclosure policy and an
  identifier profile — and is debt rather than a phase bullet, so it is in docs/44 and not in that
  list. All six of
  [`docs/25`](docs/25-benchmarks-and-expressiveness.md)'s walls are down, and so are the three that
  removing them wrote (docs/33, docs/36, docs/41); docs/41 §41.7 names what stands in their place —
  and docs/46 §46.5 added one more, found by writing a library rather than a compiler — a trait's
  declared row was a ceiling, so a fallible operation could not be a trait method — which docs/47
  removed the next day. docs/49 §49.4 found the next one the same way: a `secret[Str]` cannot be
  read, so a credential could not reach a header, and the fix moved *when* the secret is unwrapped
  rather than weakening §3.5. docs/50 §50.5 has two more findings of that kind and **neither is a
  wall**: the diagnostic for a missing type argument named a syntax the language does not have, and
  a record orders by field name rather than as declared — the second pinned as a test rather than
  changed, for the reason that section gives. docs/52 §52.5 has two more of the same kind: a `test`
  block cannot exercise a capability, so the layer of a library holding a key is the layer Beck
  cannot test, and nine match arms in the evaluator cost a thousand levels of recursion. It also
  took the first decision to let a `secret[Str]` become a `Str` — one function, behind `cap.sign`,
  with a test that keeps the count at one (docs/adr/0014). docs/53 §53.5 has three more, found by
  porting somebody else's benchmark suite rather than by writing anything of ours — and the first of
  them was a **defect**: `and` and `or` did not short-circuit, so a guard written as a conjunction
  did not guard. It is fixed, in the checker rather than the evaluator, because short-circuiting is
  a property of the language and not of one backend; `beck explain incremental` over all 31 corpus
  and example programs is byte-for-byte identical either side of it. The other two stand: Beck has
  no bitwise operators, and a nested `if` is not a statement. docs/56 §56.5 has three more, and all
  three are one shape: a module that imports another is a *tool* gap rather than a compiler one —
  `beck check`, `beck test` and `beck iface` were right about it and `beck doc` was wrong three
  ways, which the first `lib/` file to import a sibling found in an afternoon. docs/59 §59.5 has one
  more, and it is a class of bug specific to benchmarks: a benchmark whose workload is *discarded*
  has no oracle for its workload, so a loop that ran once instead of fifty times passed every
  verification the suite publishes. docs/60 §60.2 is the third time the suite found something the
  *language* was missing, after docs/53's short-circuiting and its bitwise operators: `sin`, `cos`
  and a real-to-integer truncation did not exist, and now do. Three of the fourteen could not run at
  the size the suite measures at because the evaluator's 50,000,000-step fuel budget refused them
  (docs/59 §59.3, docs/61 §61.3) — three incidents that were one missing feature, which docs/62
  built: `beck test --fuel`, because a backstop nothing can raise is a ceiling. docs/63 §63.3 found
  the next of the same kind by asking a different question: `() -> T` parsed and would not check,
  because nothing in the prelude, the corpus, either SICP chapter, the standard library or fourteen
  benchmarks had ever written a function type taking **no arguments** — a macro expanding into a
  thunk was the first thing to need one. Its §63.4 has one more that is **not** fixed and is a
  defect: `b.make()` on a function-valued field resolves to the field and drops the application
  without saying so. docs/64 §64.2 is the largest of this kind so far and was found by a *budget*
  rather than by a program: **a phase of the compiler was quadratic and nothing knew**, because
  placement re-summed the whole program three times per definition to compute explanations nobody
  had asked for. Every program in the tree is small enough that a quadratic and a linear cost the
  same, which is why three phases of work never saw it. Its §64.3 and §64.4 record two more that
  are **not** fixed: a residual `n^1.35` in `check` along a long call chain, and that
  `MAX_NESTING` does not count a *flat* block — sequential local bindings abort the process at
  12,000 in a debug build and 100,000 in a release one, so which programs compile depends on how
  the compiler was built, which is docs/42 §42.2's defect on an axis its ceiling does not measure.
  Reports are history: a later phase's correction to an earlier one goes in the later report, not
  into the earlier text.
- [`docs/reference/`](docs/reference/README.md) is **generated** by `beck doc reference` from the
  compiler's own tables and checked in. Never edit it by hand: change the compiler, then run
  `beck doc reference --out ../docs/reference` from `compiler/` and commit the result in the same
  change. A new diagnostic code needs an entry in `beck-diag/src/index.rs` or `cargo test` fails.
  [`docs/34-generated-documentation-report.md`](docs/34-generated-documentation-report.md) records
  what is generated, what is written, and what it does not do.
- [`compiler/lib/`](compiler/lib/README.md) is the standard library's **Beck half**: a host's table
  or grammar is a primitive in `prelude.rs`, and composition is a file there. Each carries its own
  `test` blocks, and `beck-cli/tests/stdlib.rs` gates the directory rather than a list — a file
  added there is gated by being there. Every file is **compiled into the binary**
  (`beck_core::stdlib`) and an import resolves against the caller's own directory first and that
  table second, so `import bignum` works anywhere and a local module of the same name wins
  (docs/10 D23, docs/adr/0018, docs/69). Two consequences: a file added there needs a line in
  `MODULES`, which `stdlib.rs` checks by importing every file from outside `lib/`; and the flat
  namespace spans the directory, so a name defined in two library files cannot be imported by one
  program — `the_whole_library_links_into_one_program` is that gate.
- [`compiler/corpus/`](compiler/corpus/) is 30 programs — 29 single files and one three-module
  project — carrying **no placement annotations**, and the measurement behind Phase 2's exit
  criterion. A program added there has to place itself.
- [`compiler/sicp/`](compiler/sicp/) is the expressiveness benchmark
  ([`docs/25`](docs/25-benchmarks-and-expressiveness.md) §25.5): chapters of SICP in Beck, with the
  book's own stated answers as the oracle, and one file per remaining wall in `sicp/refusals/` whose
  test asserts the wall is still there. A wall coming down is a test that starts failing. All six
  §25.6 measured are down, and so are the three the removals wrote, so `sicp/refusals/` is
  **empty** — which claims that every wall this project has found has been removed, and not that
  Beck expresses SICP. `sicp/refusals/README.md` holds that distinction and says what puts a file
  back. `sicp/felleisen.beck` is the suite's *formal* half rather than its running half — §25.9's
  table, one section per special form SICP introduces, each carrying the code that recovers it or
  the reorganisation that concedes it (docs/63).
- [`compiler/awfy/`](compiler/awfy/README.md) is the performance benchmark
  ([`docs/25`](docs/25-benchmarks-and-expressiveness.md) §25.2): **all fourteen** of Are We Fast
  Yet's benchmarks — its nine micro and all five macro — ported, each verified against the constant
  the original's own `verifyResult` checks, except DeltaBlue, which publishes none and whose oracle
  is the six assertions inside its own planner.
  Those constants are **somebody else's** and were read out of the MIT-licensed source, so a number
  invented here would defeat the whole point; `beck-cli/tests/awfy.rs` gates the directory, the nine
  names and the attribution, and `measure_awfy.rs` prints wall-clock and gates on nothing —
  §25.9 holds every comparative claim until there is a second backend.
- [`compiler/clbg/`](compiler/clbg/README.md) is the other performance benchmark
  ([`docs/25`](docs/25-benchmarks-and-expressiveness.md) §25.2): **eight** of the Computer Language
  Benchmarks Game's ten, ported, each verified against the Game's own published **output file** —
  checked in verbatim under `clbg/expected/` under its BSD 3-clause notice. The oracle is
  *enforced* rather than transcribed, which is the difference from `awfy/` and the reason docs/64
  §64.7.1 held the harness back until the files were reachable: `beck-cli/tests/clbg.rs` rebuilds
  every asserted literal from `expected/` and recomputes the digest of the two 10 KB ones, so a
  wrong constant fails the Beck test and a wrong constant with a matching wrong expectation fails
  the Rust one. It also gates the eight names, the two absent ones and the attribution;
  `measure_clbg.rs` prints wall-clock and gates on nothing. `mandelbrot` and
  `regexredux` are the two not ported and docs/68 §68.6 is why each; `pidigits` was the third until
  docs/69 made `lib/bignum.beck` importable. Every file there runs under the **default** fuel
  budget, `pidigits` included, and that is a fact about the library rather than about the gate:
  it needed 100,000,000 steps until docs/69 §69.6 bracketed long division's trial-digit search, and
  needs under 16,000,000 now. A budget table in `clbg.rs` would have hidden that. **docs/69 §69.7 is
  the larger thing the same question found**: `list_append` copied the whole list, so the
  tail-recursive accumulator every loop in the language is written as was O(n²) in time — docs/19
  §19.4's fold defect in a second place. **docs/70 fixes it** with last-use moves: `beck_core::liveness`
  marks the read of a local nothing reads again, `Env::read` hands the value over when
  `Arc::get_mut` proves the frame is unshared, and `list_append` pushes in place. Neutral on today's
  programs and 25× at 8,000 elements; `scaling.rs` gates the shape, because `--fuel` cannot see a
  copy — a primitive copying ten thousand values is one step. docs/70 §70.6 is the audit beside it,
  and **docs/71 is its two string findings fixed**: a string carries its character count and an
  ASCII flag, so `str_len` is O(1) and a slice is a byte range, and it holds a `String` so `+`
  pushes into it under the same ownership test. Three quadratics found and removed in one branch —
  division's trial digit (docs/69 §69.6), the list accumulator (docs/70) and both halves of text —
  each by asking what an operation should cost and measuring at two sizes. **docs/72 is the same
  question asked of space and of the constants**: a `Value` was 48 bytes because of the record
  variant most values are not, and is 16 now — a third off the memory of everything the language
  holds, −23% peak on `havlak`. It also makes `--fuel` charge for the work a primitive does over a
  length the caller chose, so the budget bounds work rather than nodes and a scaling gate can be
  deterministic; the default is unchanged, because the charge is the size of the work rather than a
  tax on it. **docs/73 is the largest single number any of it produced and is one line**: a closure
  held its body by value, so evaluating a `lam` deep-copied the whole function body once per call of
  a named function — every benchmark in the tree is **56–72% faster** now that it shares an `Arc`.
  It survived four reports about performance because it is a constant rather than a shape, and
  because `--fuel` cannot see a copy that happens between nodes rather than inside a primitive.
  **docs/74 is the rest of that question**: what a *call* costs, worked down from 287 ns to 174 ns —
  a definition's closure built once instead of per call, a frame that is one allocation instead of
  three, an environment taken by refcount, and the two smallest of all, a cheaper hash for a
  definition's name and a field test in place of the virtual call that asked whether a stub was
  installed. Every benchmark is a further 17–27% faster. Its §74.6 is the one that measurement
  *refused*: an argument stack removes the last avoidable allocation and made a call 20% slower,
  and the version that would pay needs `unsafe`, which this workspace forbids — so two allocations
  a call is the floor rather than an oversight. **docs/75 is the first time this project profiled
  itself**, and it corrects docs/74's own list of what was left: `callgrind` put **35% of every
  instruction inside glibc's `malloc` and `free`**, so mimalloc is the binary's global allocator
  (docs/adr/0019) and a record's fields are a small sorted array rather than a `BTreeMap` — 4.9–17%
  faster together, and 26–33% faster than docs/73. Three of docs/74 §74.8's four were **measured and
  rejected**: a small-vector for arguments and a smaller `Core` are both slower, and a globals index
  and a cheaper `let` are third-order. Reasoning about what an operation should cost found five real
  defects and is still the first move; it examines the operations somebody names, and the largest
  cost here was the sum of every small one. Its §75.6 is the first update to docs/25 §25.3's
  standing measurement: the same `fib(30)` is **0.797 s** rather than 4.120 s, so the tree-walker is
  **7.3× CPython** on calls, 5.1× on allocation and 2.7× on arithmetic where §25.3 measured 33×.
  A measurement rather than a claim — §25.9 still holds those until there is a second backend.
  **docs/76 is the next three off that profile**: a record literal sorts once instead of
  binary-searching per field, `with` scans by equality because `==` can stop at the length,
  `Env::read` no longer proves the scope chain unshared on a read that is not a last use — two
  atomic loads per scope *level*, paid by every read in the language — and a field name is decided
  on its first byte. 4–8% on record-heavy programs and nothing on the one that builds no records.
  Its §76.4 is the correction to docs/75's method: callgrind counts **instructions**, and the
  6.21% of them that were `memcmp` were worth about 2% of the clock — an instruction profile ranks
  candidates, and the wall clock decides between them.
- Security posture is [`docs/43-threat-model.md`](docs/43-threat-model.md) (who is defended
  against, and who is not) and `SECURITY.md` (how a report reaches us). What is *absent* is asserted
  as absent in `beck-cli/tests/pending_security.rs`: building one of those controls turns a test
  red, and correcting docs/43 §43.4 in the same change is what the red test is for.
- Design decisions are numbered in [`docs/10-decisions.md`](docs/10-decisions.md). If a change
  contradicts one, say so rather than quietly diverging. Engineering decisions — a dependency
  taken or refused, a gate's shape, an upgrade path — are recorded in [`docs/adr/`](docs/adr/).

## Standards for changes

- **Know the complexity of what you write, and measure it at two sizes.** Beck's premise is a
  language that is *fast* — [`01`](docs/01-vision-and-premise.md) — so a cost is part of a change's
  correctness rather than a follow-up to it. Two rules, both learned by breaking them:
  - **State the order of growth** of anything that loops, allocates or copies, where it is not
    obvious from three lines of code, and **measure it at two sizes rather than one**. One
    measurement cannot tell linear from quadratic; two can, and the second costs a minute. A gate
    on a *shape* — cost per unit must not grow with the number of units — is
    [`docs/64`](docs/64-compile-speed-report.md)'s pattern and does not flake the way a rate does.
  - **A bad number is a design question, not a fact to write down.** If something is slower than it
    has any business being, the first hypothesis is that the approach is wrong — not that the
    machine is slow, not that the interpreter is a placeholder, and never that it is a cost to be
    "paid knowingly". Every one of this project's performance findings was sitting behind a number
    somebody had already measured and accepted: a fold that copied its accumulator
    ([`19`](docs/19-phase-1-report.md) §19.4), a placement pass that re-summed the program
    ([`64`](docs/64-compile-speed-report.md) §64.2), a division that searched where it could
    estimate and a list that copies where it could move
    ([`69`](docs/69-standard-library-imports-report.md) §69.6–§69.7). Ask what the operation *should*
    cost before asking how to make this one faster; the answer is often a different design rather
    than a faster version of the same one.
  - A performance defect in the semantics — a copy the language forces, an accumulator that cannot
    be reused — **survives into every backend**, so it is not the tree-walker's problem to grow out
    of. `beck-cli/tests/scaling.rs` is the gate that says so, and it is where a new shape gate goes.
- Claims in docs are stated from evidence. If you write a number, it must be reproducible —
  `phase0/tests/measure.sh` is where the Phase 0 numbers come from; the Phase 1 numbers come from
  `cargo test` and the commands quoted in [`docs/19-phase-1-report.md`](docs/19-phase-1-report.md);
  the Phase 2 numbers come from `cargo test --release --test measure_phase2 -- --nocapture` and the
  commands quoted in [`docs/20-phase-2-report.md`](docs/20-phase-2-report.md); the Phase 3 numbers
  come from `cargo test --workspace`, from `cargo test --release --test measure_incremental --
  --nocapture`, from `cargo test --release --test shared_arrangements -- --nocapture`, from
  `cargo test -p beck-eval -- --nocapture` (the evaluator's stack-per-level figures), and from the
  commands quoted in [`docs/22-phase-3-report.md`](docs/22-phase-3-report.md),
  [`docs/23-general-slicer-report.md`](docs/23-general-slicer-report.md),
  [`docs/24-incremental-views-report.md`](docs/24-incremental-views-report.md),
  [`docs/26-arrangement-sharing-report.md`](docs/26-arrangement-sharing-report.md) and
  [`docs/51-arrangement-lifecycle-report.md`](docs/51-arrangement-lifecycle-report.md); the SICP numbers
  come from `cargo test --release --test sicp` and from `beck test sicp/ch1.beck`,
  `beck test sicp/ch2.beck` and `beck test sicp/felleisen.beck`, quoted in
  [`docs/27-walls-report.md`](docs/27-walls-report.md) §27.5,
  [`docs/31-tail-calls-report.md`](docs/31-tail-calls-report.md) §31.5–§31.6 and
  [`docs/32-numeric-tower-and-polymorphism-report.md`](docs/32-numeric-tower-and-polymorphism-report.md)
  §32.5 and
  [`docs/33-effect-polymorphism-and-list-patterns-report.md`](docs/33-effect-polymorphism-and-list-patterns-report.md)
  §33.6 and
  [`docs/36-parameterised-types-report.md`](docs/36-parameterised-types-report.md) §36.6 and
  [`docs/41-generic-arithmetic-report.md`](docs/41-generic-arithmetic-report.md) §41.4 and
  [`docs/63-felleisen-report.md`](docs/63-felleisen-report.md) §63.2; and the
  standard library's come from `cargo test -p beck-cli --test stdlib` and from `beck test` on each
  file in `compiler/lib/`, quoted in
  [`docs/46-standard-library-report.md`](docs/46-standard-library-report.md),
  [`docs/50-collections-and-dates-report.md`](docs/50-collections-and-dates-report.md) and
  [`docs/52-crypto-and-identifiers-report.md`](docs/52-crypto-and-identifiers-report.md); and the
  Are We Fast Yet numbers come from `cargo test --release --test measure_awfy -- --nocapture` and
  from `beck test` on each file in `compiler/awfy/`, quoted in
  [`docs/53-are-we-fast-yet-report.md`](docs/53-are-we-fast-yet-report.md) — where each benchmark's
  *verification* constant is the original suite's own, read from its source rather than chosen here;
  and the Benchmarks Game numbers come from `cargo test --release --test measure_clbg --
  --nocapture` and from `beck test` on each file in `compiler/clbg/`, quoted in
  [`docs/68-clbg-report.md`](docs/68-clbg-report.md) and
  [`docs/69-standard-library-imports-report.md`](docs/69-standard-library-imports-report.md) §69.5 —
  where every expected output is the Game's own file under `clbg/expected/` and `clbg.rs` is what
  holds a port's assertion to it; and the cost of importing a standard-library module comes from
  `beck check` on the probes docs/69 §69.4 lists;
  and the compile-speed numbers come from `cargo test --release --test measure_compile --
  --nocapture` and `cargo test --release --test compile_speed -- --nocapture`, quoted in
  [`docs/64-compile-speed-report.md`](docs/64-compile-speed-report.md) — where the gate asserts a
  *shape* rather than a rate, because docs/13 §13.7's "a gate that flakes gets deleted" rules out a
  wall-clock threshold on a shared runner.
- The harnesses are the project's conscience (§4.8, §8.3): `compiler/crates/beck-cli/tests/` holds
  the differential, replay-determinism, backend-seam, scaling, security, corpus, placement-property,
  general-slicer, incremental-analysis, incremental-engine, shared-arrangement, subscription,
  view-metrics, SICP, Are We Fast Yet, Benchmarks Game, tests-in-Beck, UI, workflow-cross-check,
  documentation, outbound, compile-speed and diagnostic-snapshot suites, plus the five release-only
  measurement suites (`measure_phase2`, `measure_incremental`, `measure_awfy`, `measure_compile`,
  `measure_clbg`). Keep them green.
- The CI workflow is an artefact too, and Phase 2 found that it had never run
  ([`docs/20-phase-2-report.md`](docs/20-phase-2-report.md) §20.4 item 8). If you change
  `.github/workflows/`, run the steps you changed by hand before trusting them.
- `beck-rt` must not depend on any backend crate. Execution goes through
  `beck_core::backend::Backend`, and `tests/backend_seam.rs` drives the runtime with an
  implementation that is not the evaluator so the seam stays load-bearing (docs/19 §19.9). Anything
  the runtime needs to *know* about a backend goes on that trait — `Backend::stack_bytes` is how the
  runtime sizes a thread for the tree-walker without naming it (docs/31 §31.3).
- A program's own behaviour is asserted in Beck, not only in Rust. `beck test` runs `test` and
  `property` blocks ([`docs/21-tests-in-beck-and-proof.md`](docs/21-tests-in-beck-and-proof.md)
  §21.2–§21.3); a change to what a program *means* should move a test in the program, and
  `compiler/crates/beck-cli/tests/tests_in_beck.rs` is where the construct itself is held to account.
- Say plainly when something is written but unproven. "Built" and "runs" and "measured" are three
  different claims.
- A code comment states the point — a constraint, an invariant, a non-obvious why — and the
  context a reader needs, nothing else. Never narrate history ("this was broken, now it works"),
  a review, or a conversation ("we decided…"); that belongs in the commit message, an ADR
  ([`docs/adr/`](docs/adr/)), or a report. Docs and comments are the current state of things.

## Working in an isolated or cloud environment

- **The first `cargo` command downloads the pinned toolchain** (`rust-toolchain.toml`, 1.94.1,
  ~2 minutes). Do not run a second `cargo` or `rustup` process until it finishes: concurrent
  first-runs race inside rustup and corrupt the install. Repair:
  `rustup toolchain uninstall 1.94.1 && rustup toolchain install 1.94.1`.
- **Verification, cheapest first** (from `compiler/`): `cargo test -p <crate>`, then
  `cargo test --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
  `cargo fmt --all --check` and `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps`
  before pushing. CI denies warnings, and the last one is a separate gate rather than a
  formality: `.github/workflows/docs.yml` runs it, and a broken intra-doc link fails there and
  nowhere else.
- **Environment-dependent suites degrade by skipping, and a skip prints itself** — read the
  output for it:
  - Kubernetes conformance (`beck-infra/tests/conformance.rs`): skips without a cluster;
    `BECK_REQUIRE_CLUSTER=1` forbids the skip. Do not claim this rung ran without one.
  - Postgres log contract (`beck-rt/src/log.rs`): runs only with `BECK_PG=<url>`
    (`BECK_REQUIRE_PG=1` forbids the skip). A local server works: `initdb`/`pg_ctl` as the
    `postgres` user, in a directory that user can traverse (`/tmp`, not a root-owned dir).
  - Compose parity needs Docker; the thin-client budget needs `brotli` (apt-installable).
- **The measurement suites are release-only by convention**: the reproducible form is
  `cargo test --release --test <suite> -- --nocapture`. They also run in debug under the full
  suite with their tables swallowed; that is expected.
- **The network is proxied and partial.** crates.io and the toolchain host work; docs hosts may
  not. Read a dependency's API from its vendored source under `~/.cargo/registry/src/`.
- **Run CI steps you change by hand** (the §20.4 rule above): the deterministic gates need only
  `python3` (with PyYAML) and git, so they run anywhere.
