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

- **2026-08-21 — The page's children were copied, not shared, and "n handles" was never true.**
  [`docs/23`](docs/23-incremental-views-report.md) §23.8 has always named the one cost a maintained
  view does not remove: `html_el` is pointwise, so one event reassembles every element from the page
  down to the list. It described that as "`n` handles are copied". It was not handles.
  `Html::Element` held `children: Vec<Html>` — **owned subtrees** — so `child` deep-copied each child
  it was given, and because every enclosing element re-copied what its own children had just copied,
  one event rebuilt *every node of the page* and the cost compounded with nesting depth. A column
  counting entries cannot see that, which is how it sat behind an accepted number.
  Children are `Vec<Arc<Html>>` now, and an untouched subtree costs a refcount. Same harness, same
  program: **one event on a 5,000-row page goes 14,827 µs → 697 µs**, and the maintained-to-recomputed
  ratio goes 3.4× → 35.0×. The cold render halves too (50,845 µs → 24,370 µs), because
  `beck_core::html::element` is the single function the evaluator *and* both native backends assemble
  a page with — the reason that function was written once.
  **The gate counts allocations, not microseconds**, because §13.7 forbids a shared runner a timing
  threshold and identity is the fact the cost follows from:
  `incremental_engine.rs::one_event_allocates_a_handful_of_html_nodes_whatever_the_page_holds` reports
  **9 new nodes on a 200-row page and 9 on a 1,600-row page** — measured at two sizes, since one
  measurement cannot tell a constant from a linear one. Putting the copy back reports **211 and
  1,611**, which is the shape of the gap it exists to catch.
  This does **not** make a maintained view `O(δ)` end to end, and [`docs/08`](docs/08-roadmap.md)
  §8.5.4's item stays open: the assembly is `n` refcounts instead of `n` subtree copies, which is a
  smaller `n` and still an `n`. What closes it is the engine emitting patches from its own output
  changes, and this was the representation change that had to come first.

- **2026-08-20 — The sweep that found the nested-loop join is a gate, and the cost it leaves is
  the last one.**
  [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.3 found the defect the view algebra
  was missing an operator for by **sweeping the tree by hand**: a per-element function that reads the
  accumulator is a different function after every event, so its whole collection is reapplied. That
  sweep was read three times and went stale twice, always in the flattering direction — the third
  reading found a site that had arrived with `awareness(f)` one change after the second and that
  nothing re-ran the sweep to catch.
  It is now `Plan::reapplied_per_event`, and
  `incremental.rs::no_program_in_the_tree_reapplies_a_collection_per_event` is what re-runs it:
  **42 programs across `corpus/` and `examples/` plan, and none of them reapplies a collection per
  event — against 8 sites in 8 programs with the recognition switched off.** The second number is
  what makes the first mean anything, and it is carried by the green run rather than promised by it.
  `beck explain cost` prints its capture lines from the same computation the gate counts, so the
  report and the gate cannot disagree — §99.9 item 2's lesson applied to a second reader instead of
  rediscovered.
  **What the zero exposes is the entry.** Every one of those 42 programs still has an operator
  costing `O(n)` per event and they all have the *same* reason: a recompute needs a `list` and an
  arrangement is a keyed collection. That is now the only per-event linear cost in the tree.
  [`docs/23`](docs/23-incremental-views-report.md) §23.8 measured it when the engine landed, named
  the fix — the delta at the top of the plan **is** the patch set, so an engine emitting patches from its
  own output changes skips the assembly and the diff together — and called it "a known piece of work
  rather than an open question". It had **no position in [`docs/08`](docs/08-roadmap.md) §8.5's
  order**, which is verbatim the failure mode that section opens by describing. It has one now, as an
  **F** item ahead of Mode B's codegen, because §23.8 says it is the same work that kernel needs.

- **2026-08-20 — The corpus-wide counts, re-derived against thirty-seven programs.**
  `corpus/37-ledger.beck` is the 37th, and [`DEFECTS.md`](DEFECTS.md)'s `corpus-wide-counts-drift`
  says what that costs. It had drifted again, and this time in three directions at once: the
  WebAssembly emitter's corpus denominator read **195** in [`docs/08`](docs/08-roadmap.md), **213**
  in [`docs/93`](docs/93-the-native-backends-report.md) and **220** in
  [`docs/103`](docs/103-the-wasm-emitter-report.md) and [`docs/README`](docs/README.md) — three
  numbers for one quantity, in four places.
  Re-derived: the native backends compile **975** definitions against **143** refused, **all
  thirty-seven** corpus programs compile their `apply_event` and **twenty-five of thirty-seven**
  their `view`; the WebAssembly emitter is measured against **225** corpus definitions, of which
  **220** are refused for one shape — a parameter that lives on the heap, where §103.6 said 138 of
  195 were a `Str` parameter; and the corpus places **382** definitions and signals, not 373, with
  the tier table moved with it — `any` 201 (52.6%), `server` 83, `data` 61, `client` 37.
  [`docs/65`](docs/65-the-editor-report.md)'s rename figure was the worst of them, **316 of 325**
  against a true **366 of 376**, and the reason is worth the entry: the test that derives it never
  printed it. The count lived in a *comment* beside an assertion with a floor of 250, so the only
  way to read today's value was to edit the test. It prints it now, as `native.rs` and
  `wasm_backend.rs` print theirs, which is the cheapest half of the gate that entry still owes.

- **2026-08-20 — A group's total, and the decision that a sum is its answer.**
  [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's last aggregate, and the
  only one that owed a **decision** rather than an operator. The two edges it named were real: a
  float total maintained by adding what arrived and subtracting what left is not the number a left
  fold produces, and an integer one passes through intermediate values `checked_add` raises on, so
  the maintained plan and the recompute could disagree about whether the program *failed*. Both are
  edges of the fold rather than of the sum. **A sum is its answer, not the order it was added in**:
  `list_sum` is the exact total and raises only when *that* leaves `Int`, which makes it a function
  of the numbers alone — and a **conservative extension** of `+`, with the same answer wherever the
  fold has one and an answer for `[Int_MAX, Int_MAX, -Int_MAX]`, where the fold has none.
  `interp.rs::a_sum_is_its_answer_and_not_the_order_it_was_added_in` is that sentence at the two
  functions themselves. **`Float` gets no `sum`**, because there the same definition would *disagree*
  with the fold rather than extend it — a different number in the last bits, on ordinary inputs
  ([`docs/46`](docs/46-standard-library-report.md) §46.16).
  `Agg::Sum` therefore keeps a running total and **no multiset**, since a sum does not care which
  distinct values its group holds: `corpus/37-ledger.beck` shows every account's balance in **47
  backend steps at 200 postings and at 1,600, against 2,060 and 16,060** with the operator switched
  off (`scaling.rs::totalling_a_group_does_not_build_it`, which measures both settings). Unlike the
  extremes there is no worst case to choose — every posting moves its account's total, so an ordinary
  event is already the reassembling one.
  Two things it does that the extremes do not. An empty group is `0` rather than `None`, so the join
  above it reads a missing entry as a *value* — `Matching::Total`, gated by
  `incremental.rs::a_total_is_a_group_by_probed_as_a_value_rather_than_an_option`. And a total no
  `Int` holds is **published rather than raised**: the operator maintains every group while the
  recompute only sums the groups the loop reaches, so raising at maintenance time would fail renders
  that never asked. The raise lands at the probe, and
  `incremental_engine.rs::a_total_outside_int_fails_where_it_is_asked_for_and_nowhere_else` holds the
  two plans to the same failure as well as to the same answer.

- **2026-08-20 — A malicious `arrayref` release, and the gate that saw the pin for it expire.**
  The `licences` job went red on a yank rather than an advisory: crates.io withdrew `arrayref 0.3.9`,
  which reaches this tree through `blake3`. **The fix `cargo-deny` suggests was the attack.**
  `cargo update -p arrayref` resolved to `0.3.10`, which added a normal dependency on
  **`proc-macro1`** — one character from `proc-macro2`, with exactly two published versions (1.0.106
  and 1.0.107, `proc-macro2`'s own latest two), copying its feature set and its single normal
  dependency, and declaring `base64`, `rustls` and **`ureq` as build dependencies**: an HTTP client
  and a TLS stack running inside a build script at compile time. `arrayref` is two hundred lines of
  macros for taking a reference to a sub-array; 0.3.9 has no runtime dependencies at all.
  What made it visible was the disproportion — that one-crate update produced a lock file **283 lines
  larger**, pulling in `ureq`, `url`, `webpki-roots` and the whole ICU stack. Nothing was built with
  it and `arrayref-0.3.10.crate` was never downloaded; the version facts were read from the registry
  index.
  So `arrayref@0.3.9` was held in `deny.toml`'s `advisories.ignore` — the yanked version being the
  safe one and the current one the attack. **crates.io removed 0.3.10 and un-yanked 0.3.9 hours
  later**, so the list is empty again and the lock file never moved: it holds the same `0.3.9` it
  held before any of this.
  **What is left is the gate.** `unused-ignored-advisory = "deny"` was added alongside the pin
  because `cargo-deny` reports `yanked-not-detected` only as a warning, which no CI job reads. It
  failed the build the moment the pin stopped being needed — which is how the entry came out the same
  day rather than sitting there until the next yank of the same crate was waved through by a
  permission granted for something else. A gate whose first firing is a true positive, on its first
  day, is the argument for writing it at the same time as the thing it guards.

- **2026-08-20 — Nine CI gates could not fail, and the sketch's restyle is what found them.**
  CI went red on the styling change: the workflow asserted `grep -q '<footer>0 remaining</footer>'`
  and the footer now carries a class. Two lines to fix — matched as `'>0 remaining</footer>'`, since
  the property is that the *text* was server-rendered, not what the tag wears. What the fix turned up
  is the entry.
  **`! cmd` does not fail a step.** `bash -e` "shall not exit" when the command that failed "is part
  of a `!` expression" — POSIX's own words — so a negation followed by any further line is a comment
  with a process behind it. `compiler.yml` had **ten** such assertions and **nine were dead**: that a
  deliberately-false Beck test fails the build, that a `match` covering one list shape of two is
  refused, that a rigid `T` is not silently generalised, that a breaking wire change needs
  `--breaking`, that a Mode A page does not load the Mode B kernel, that the derived grant carries no
  `DELETE`, that stripping `@on` leaves none, that a deep recursion does not overflow the host stack,
  and that the sheet has no rule for a class the page cannot carry. The tenth was live only because
  it was the last line of its step.
  All ten are now `if cmd; then echo 'why'; exit 1; fi`, which aborts wherever it sits and says what
  broke. **All nine were asserting things that are true** — verified one at a time against the
  current tree — so nothing had been hiding behind them.
  **The file already knew.** The deep-recursion step's own comment reads "an exit status of 134 or
  139 … is exactly the thing a `! cmd` gate would have accepted, so the status is checked rather than
  only the failure": somebody hit the trap, understood it exactly, fixed the instance in front of
  them, and left the nine others. That is
  [`docs/82`](docs/82-the-edge-report.md) §82.10's pattern, now recorded there with this as its
  largest instance.
  `workflows.rs::no_workflow_asserts_with_a_negation_that_cannot_fail` forbids a `run:` line
  beginning with `!` in any workflow, with **no exemption for the last-line case** — an exemption
  that depends on position is lost the moment somebody appends a line, which is how nine of these
  happened.
  **And a second thing the restyle broke silently.** The wire-compat step's "a body edit is not a
  wire change" ran `sed 's/"done" if t.done else ""/…/'` — `done_class`'s *old* body. The `sed`
  matched nothing, so both `check`s were about the same file and the claim was vacuous. It now edits
  a quoted text literal (`"todos"` → `"to-dos"`; unquoted ` remaining` also matches
  `def remaining(…)`, and renaming a definition genuinely *is* an interface change) and `diff`s to
  prove the edit landed.
  New in the serving step: the page's stylesheet is fetched over HTTP and checked to carry a rule for
  a class the page serves, the token that rule reads, and nothing for a class it cannot carry —
  `beck build` writing the file was already gated, this process answering for it was not.

- **2026-08-19 — A misspelled utility is a diagnostic, and the editor answers from the same table.**
  [`docs/104`](docs/104-styling-and-the-component-library.md) §104.4b — the second and third of
  §104.4's four consequences, and what closes
  [`docs/08`](docs/08-roadmap.md) §8.5.4's styling item 4. **`B0222`**: a class that is not a
  utility and is within one edit of one — two, from eight characters up — is a warning naming the
  utility it is one slip from. `rounded-ful` → `rounded-full`, `bg-emerald-550` → `bg-emerald-500`,
  `items-centre` → `items-center`. And the **editor**: `class="fl‸"` completes to `flex` from the
  closed table plus the scale Tailwind documents, and hovering `gap-2` prints
  `.gap-2 { gap: calc(var(--spacing) * 2); }` — the rule the sheet will actually carry, because it
  comes from the function that emits it.
  **A warning rather than a refusal, which is the design and not a hedge.** `B0217` and `B0218` are
  errors because their vocabularies are closed; a class vocabulary is open, `done`, `column` and
  `mine` are names this tree's own programs write, and Beck still has no way to write a rule of your
  own — so refusing them would be refusing an escape hatch that does not exist.
  **The gate asserts the margin, not the outcome.** Every class this tree writes is **three or more**
  edits from anything in the table (`card` to `grid`, `here` to `h-px`, `mine` to `inline`), so the
  threshold sits one edit clear of the population it must not touch.
  `style.rs::a_misspelled_utility_is_a_diagnostic` asserts that distance rather than asserting that
  nothing was said — the second passes on any threshold below it, the first goes red when a family
  added to the table lands near a name somebody chose.
  **And the editor's negative gate took two attempts, which is the entry's other half.** A class is
  a token inside a string, so the only thing between four thousand utility names and every
  completion in the file is the test for where the caret is. The first gate put a caret in a string
  reading `"hello"` and asserted nothing came back — true for the wrong reason, since no utility
  begins with `hel`. Pointed at `placeholder="gap in the diary"`, whose prefix matches seventy, it
  went red at once: `class=` was to the left on the same line and the caret was inside *a* string,
  which was all the context test checked. It now requires the text between `class=` and the caret's
  own string to be the value itself, which is what tells `class=["flex", "ga‸"]` from
  `class="flex", placeholder="ga‸"`.
  The `Class` **type** is not built and is no longer owed: what it would have checked is what
  `B0222` checks, and [`docs/104`](docs/104-styling-and-the-component-library.md) §104.12 records it
  as absent rather than as scheduled.

- **2026-08-19 — The stylesheet is emitted from the program, and `css.rs` is deleted.**
  [`docs/104`](docs/104-styling-and-the-component-library.md) §104.4a and
  [`docs/08`](docs/08-roadmap.md) §8.5.4's styling item 4, second half. `beck build` writes
  `styles.css` and a running program serves the same bytes at `/beck.css`, derived at startup from
  the program it is executing: **one rule per class its pages can carry**, the theme tokens those
  rules read and nothing else, in front of a nine-rule preflight that is Beck's own rather than
  Tailwind's. `AppConfig::styles` is the `styles = none` switch and **both settings are run** by a
  gate, per §8.3 item 8. The eight rules hard-coded in `beck-rt/src/css.rs` are gone with the file.
  **A predicate cannot emit a sheet, and that is what the second half changed about the first.**
  `beck_core::style::rule` turns a name into declarations and `is_utility` is now *defined* as
  "there is a rule for it", so a table and an emitter cannot disagree about a page; the oracle
  records what Tailwind **emits** rather than whether it emits; and the gate compares the at-rules,
  the selector and the declarations byte for byte. **3,474 of the 3,625 names Tailwind 4.3.3 emits a
  rule for are rendered identically here**, 35 it refuses are refused, 151 remain as the families
  this table has not taken, and all 293 theme tokens are Tailwind's own values.
  **Asking the bigger question found four details and seventeen defects.** The details are that `1`
  is `var(--spacing)` and not `calc(var(--spacing) * 1)`, `0` is `0px`, `space-x-0` drops the
  reverse-margin `calc`, and `2xl:flex` escapes to `.\32 xl\:flex` — a hex escape terminated by a
  *space* that the previous reader of the oracle stopped at, so every `2xl:` rule had been silently
  missing from its answer. The defects are `size-screen`, `max-w-auto` and fifteen `-auto` paddings:
  names the table accepted, Tailwind emits nothing for, and **`candidates.txt` had never been asked
  about**, so they were in none of the gate's three buckets through every green run
  ([`docs/82`](docs/82-the-edge-report.md) §82.10 exactly). The table enumerates itself now and
  `style.rs::every_name_the_table_accepts_was_asked_about` fails when it accepts a name the oracle
  never saw.
  **The sketch is the proof and its sheet is 2.3 KB.** `examples/todo.beck` is restyled onto
  utilities — seventeen classes, seventeen rules, six of the theme's 293 tokens — and its `done`
  class, the one name in it that was the program's own and the reason `css.rs` existed, is
  `line-through`. Restyling it found a defect worth its own entry in
  [`DEFECTS.md`](DEFECTS.md): `class=["a", b]` with one non-literal element lowers to a `str_join`,
  which has no delta rule, so **the shape §104.4 recommends turns a page into a recompute**. No
  program in this tree had ever written one. An all-literal list is folded at lowering time and
  costs nothing; the mixed list is `class-list-recomputes`, and the sketch writes two whole
  alternatives behind an `if` until it is fixed.

- **2026-08-19 — Both ends of a group, answered without the group.**
  [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's other two aggregates, and
  the last of that item the language has a spelling for. `list_min` and `list_max` over the same
  `filter_list` the recogniser already reads — bare, or over a `map_list` of it — compile to
  `Op::GroupBy`: one entry per group, holding a **multiset** of what its rows projected to, of which
  `min` and `max` are the two ends. It is the first right side in this algebra that is not an index
  over the collection, so the join above it is a plain `Matching::Unique` — `Some` for a group with
  rows and `None` for one without, which is what `list_min` already answered for a list and for an
  empty one. No syntax, and no edit to any program.
  **`max` costs what `min` costs, and §99.9 said it would not.** The asymmetry it forecast is real of
  the design it forecast it for — a range over an index keyed `(group, value)` can be entered from
  its start and not from its end, because there is no successor of an arbitrary `Value` to bound it
  with — and it dissolves under the rule the *count* established: an aggregate is the reading
  operator's and never the index's, and a tree an operator builds itself is bounded at both ends by
  construction. The design was asymmetric; the problem was not. Corrected in place in §99.9 item 6.
  `corpus/36-auction.beck` is the program — the lowest and the highest bid on every lot, with no
  `group by` and no `min by` in the file. `scaling.rs::asking_a_group_for_one_end_does_not_build_it`
  measures a **new low** landing on a pile of 200 bids and of 1,600 — the worst case, because the
  answer moves and the page is reassembled — at **72 backend steps at both sizes against 4,097 and
  32,097** with the operator switched off, and one entry copied out of an arrangement either way;
  on a clock that is **22 µs against 95 µs at 200 bids and 56 µs against 698 µs at 1,600**, 4.3× then
  12.5× (`measure_incremental::what_answering_a_group_from_its_ends_saves`, whose slow side is the
  same three operators written through a `let`, so the two plans differ in the aggregate and nothing
  else).
  Two gates hold what the corpus-wide differential cannot. A bid *between* the standing ends moves
  the group and neither answer, so the operator publishes nothing and the join, the loop and the page
  do not run — `incremental_engine.rs::a_bid_between_the_ends_does_not_re_render_the_page`, which is
  about the operator's output rather than its cost. And the **multiset** rather than a set: two bids
  of the same amount are two bids, so
  `incremental_engine.rs::a_maintained_extreme_per_group_survives_the_events_that_take_it_down`
  withdraws the standing minimum, the standing maximum, half of a tie and the last bid on a lot from
  a written log — deleting the multiplicity leaves the corpus-wide differential **green** and turns
  that one red, which was checked rather than assumed.
  Two things fell out of it. A site nested inside one that **failed** is now tried, where before
  every nested site was skipped: `list_min` over a filter whose projection reads the loop's element
  is not an aggregate anything can maintain, and refusing the whole body would have taken the index
  down with it and left the loop at `O(n)` per event. And two lookups that index one collection by
  the same key now build **one** index rather than two — `Core` numbers variables per definition, so
  `lambda b: b.lot` and `lambda c: c.lot` reached the hash-consing key as different strings, and an
  arrangement is memory per subscriber as well as work per event. §99.5 decision 4 records it and
  `incremental.rs::two_lookups_by_the_same_key_share_one_index` is the gate.

- **2026-08-19 — The corpus-wide counts, re-derived against thirty-six programs.**
  `corpus/36-auction.beck` is the 36th, and [`DEFECTS.md`](DEFECTS.md)'s `corpus-wide-counts-drift`
  says what that costs: every figure derived from the whole corpus moves and nothing gates it. It had
  already drifted again — [`docs/93`](docs/93-the-native-backends-report.md)'s headline read 963 while
  [`docs/08`](docs/08-roadmap.md), [`docs/README`](docs/README.md) and §93.6's own table read 968.
  Re-derived: the native backends compile **972** definitions against **140** refused, **all
  thirty-six** corpus programs compile their `apply_event` and **twenty-five of thirty-six** their
  `view`; the WebAssembly emitter is measured against **220** corpus definitions, not 213; and the
  corpus places **373** definitions and signals, not 362, with the tier table moved with it — `any`
  196 (52.5%), `server` 82, `data` 59, `client` 36. §93.6's Compiled column now says it is the
  reading taken when each row landed rather than today's, which is what made it look like a
  contradiction rather than a history.

- **2026-08-19 — `h2` taken to 0.4.16, which is what RUSTSEC-2026-0258 asks for.**
  `cargo deny`'s advisories check went red on `h2 0.4.15` — unbounded empty DATA frames, reachable
  through `hyper` — in the `licences` job, with `bans`, `licenses` and `sources` all still green. The
  advisory is new rather than newly noticed: nothing in this repository moved, and the same lockfile
  fails on any branch that has it. The fix is the one the advisory names, `cargo update -p h2`, and
  it is taken rather than muted: [`deny.toml`](compiler/deny.toml)'s `ignore` list is empty on
  purpose and says why — an advisory is fixed at the root, not silenced beside it.

- **2026-08-18 — The utility table, with Tailwind's own compiler as its oracle.**
  [`docs/104`](docs/104-styling-and-the-component-library.md) §104.4's half of styling cluster item
  4: take Tailwind's design system, refuse its delivery mechanism, and hold the accepted names
  against Tailwind itself rather than against a table somebody typed in.
  `beck_core::style::is_utility` knows **631 of 782** candidate names — layout, spacing, colour,
  type, borders, flex and grid, with the variants in front of them — with **no unsound acceptance**
  and every name Tailwind refuses refused here. The gate has three buckets and only two are
  failures: a name Beck accepts that Tailwind refuses is a page missing a rule with every gate
  green, a name Tailwind refuses that Beck accepts is the same error read the other way, and a
  *gap* is counted and printed so a documented subset cannot quietly become the claim. The candidate
  list is deliberately wider than the table, because a list written from the table would make the
  row a restatement rather than a measurement.
  **The oracle is committed, not run.** `compiler/style/generate.sh` asks Tailwind 4.3.3 about every
  candidate and `compiler/style/expected/` holds the answer — [`clbg/`](compiler/clbg/README.md)'s
  pattern, and for its reason: a gate that installs from a package registry fails when somebody
  else's server does.
  **Two things asking it caught that writing it down would not.** Tailwind 4's spacing scale is
  multiplicative — `calc(var(--spacing) * n)` — so `p-2.75` and `gap-13.5` are rules and a table of
  steps would have refused both; §104.4 named `p-[13px]` as the arbitrary case and did not say the
  numeric scale is open too. And the **first generator was wrong**: it asked `grep -F ".rounded-ful"`
  of Tailwind's output and got a hit from `.rounded-full`, so the exact misspelling §104.4 chose to
  illustrate the point came back as a utility. It compares selectors as a set now. A generator that
  reads its oracle wrongly is worse than no oracle, because the answer looks authoritative.
  `beck explain style` now says of every class a page carries whether it is a utility or the
  program's own, and a gate holds that this tree's semantic names — `done`, `mine`, `column` — are
  **not** read as utilities, because a program's own names are the program's own. What is still not
  built is the emitter: nothing writes CSS, `styles = none` does not exist, and `beck-rt/src/css.rs`
  still serves its eight hard-coded rules, so the exit-table row this cluster exists to move is
  unmoved.

- **2026-08-18 — `class=` takes a list, and the compiler says which classes a page can carry.**
  [`docs/08`](docs/08-roadmap.md) §8.5.4's styling cluster item 3 — the **F** everything else in that
  half is behind ([`docs/104`](docs/104-styling-and-the-component-library.md) §104.4). A list where
  HTML defines a space-separated value — `class`, `rel`, and the two ARIA id-list relationships — is
  joined in the **`ui:` lowering** rather than at the seam, so what reaches the checker is one
  `str_join` and no emitter had to learn anything; the three backends agree by construction. An
  existing `class="a b"` is untouched and renders the characters it always did.
  The surface is not decoration: `class=["btn", "primary" if hot else "plain"]` is what a program
  writes instead of `"btn " + variant`, and the difference is that a list of alternatives can be
  **enumerated** where a concatenation cannot — by Beck or by Tailwind's own scanner, which §104.3
  measured over this tree reading English prose out of comments and missing a real utility behind a
  module boundary. So `beck_core::style` enumerates every class that can reach a `class=`, following
  a call and taking both arms of an `if`, which is the shape every dynamic class in this tree is
  already written in: `examples/routed.beck` is `{done, here}`, `corpus/02-chat.beck` is
  `{mine, theirs}`, and neither program was edited. `beck explain style` prints that set and, beside
  it, every site where a class is *built* rather than named, with which of three reasons — a
  concatenation, a value, or a shape the analysis does not enter — because a reader does something
  different about each. Nothing is rejected: the report is what makes the set honest, and the escape
  hatch a stylesheet emitter will need is a decision for the item that emits one.
  `style.rs` is the harness, and it holds both directions of the lowering's table (a list in `class`
  joins, a list in `title` does not) and that one computed class does not hide the ten beside it.
  **What item 3 still owes is the `Class` type**, and it moved *behind* item 4 rather than in front:
  a class name has nothing to be checked against until the utility table exists, and a type whose
  checking is empty is a scaffold. §8.5.4 and §104.11 both say so now.

- **2026-08-18 — The language gets a minimum, and the library stops sorting to find one.**
  [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.9 item 6 named the blocker on `min`
  and `max` per group as a **surface** rather than a delta rule: neither had a spelling the view
  engine could recognise, because `lib/collections.beck`'s `min_of` was `list_get(sorted(xs), 0)` — a
  sort and a copy of the whole list to answer a question about one element of it. `list_min` and
  `list_max` are primitives now, one pass and no allocation, and the library's two are one line over
  each. Neither takes a comparator, for `sort_by`'s reason one level down: ordering is the runtime's
  structural one ([`docs/54`](docs/54-ordering.md)), so the smallest of a list needs nothing from the
  caller, and `Option` says an empty list has no answer rather than raising. Over 64,000 elements the
  library's minimum went from **151 ms of work above the baseline to 33 ms**
  (`beck test` over a generated list, at 16,000 and 64,000).
  `stdlib.rs::the_smallest_and_largest_are_the_runtimes_order_and_an_empty_list_has_neither` holds
  the three decisions; `lib/collections.beck`'s own tests are unchanged and still pass, which is what
  makes this a reimplementation rather than a new function.
  **[`compiler/lib/README.md`](compiler/lib/README.md)'s division gains the row that was always
  missing.** It admitted a primitive only for a host's table or grammar, and that never explained
  `sort_by`, `filter_list` or `list_len` — every one of which is expressible in Beck and is a
  primitive anyway. The third row says why: the incremental engine maintains what it can
  **recognise**, and an aggregate written as a fold is one opaque operator recomputed in full.
  §46.16's set-cost row is corrected to the amended rule, and a set operation is still neither.
  §99.9's own design claim is corrected in the same change: the `arrange_by` keyed by `(group, value)`
  makes a group's **minimum** the first entry of its range and `O(log n)`, and does **not** do the
  same for its maximum — a `BTreeMap` prefix range is entered from its start and not its end, there
  is no successor of an arbitrary `Value` to bound with, and Beck has no descending order to key by.
  So `max` per group is a walk of the group or a maintained extreme with an `O(g)` repair, which is
  the decision §99.9 opened by calling a genuine one, now known to bite one of the two rather than
  both. Neither is built.

- **2026-08-18 — The engine's counters can see inside an application, and two gates stopped looking
  away.** [`DEFECTS.md`](DEFECTS.md)'s `work-cannot-see-inside-an-application`, opened earlier today
  and closed here. `Work` counted what the engine did *to* its arrangements and stopped at the
  boundary of a call, so `examples/board.beck` with the join refused reported the identical four
  numbers at 200 cards and at 1,600 while its clock moved tenfold — and both of this session's shape
  gates had to be written against something other than their own off switch, because the off switch
  was invisible. `Backend` now carries a defaulted `steps()` in the shape `intercepting` and
  `stack_bytes` already established: the tree-walker publishes what its calls spent of their own
  evaluation budget, a compiling backend answers `None`, and `Prepared` takes the counter at prepare
  time so `Engine::render` can subtract. `Work::steps` is that difference, deliberately **not** in
  `Work::total` — it is a different unit by three orders of magnitude and would drown every gate that
  reads the total.
  `scaling.rs::the_work_a_render_reports_includes_what_happened_inside_it` asserts the blindness from
  both ends: the four counters identical at either size, `steps` growing with the collection. Both
  operator gates now measure `Relate::Refuse` directly, which is what §8.3 item 8 asks of an off
  switch: **98 steps at 200 cards and at 1,600 against 12,830 and 101,030** for the join's index, and
  **44 against 1,253 and 9,653** for the group's count. The variant-program contrast the aggregate
  gate needed is gone; `measure_incremental` prints `steps` beside the clock, where the two now agree
  about shape.

- **2026-08-18 — A group's size is answered without building the group.**
  [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.9 item 6's first aggregate, and the
  leftover item 3 handed it: `arrange_by` turned the scan into an index probe and then materialised
  the group in order to count it, so an event still cost the size of the pile it landed on. A
  `list_len` over the same `filter_list` is now recognised as an aggregate rather than as a group —
  `Matching::Count` — and the join keeps a tally per key beside its reverse index, moved by ±1 as the
  index moves. `corpus/35-workload.beck` is the program it exists for: every person, and how many
  issues name them, with the set of people coming from the data rather than written out. It copies
  **one** entry out of an arrangement at 200 issues and at 1,600, against **202 and 1,602** for the
  same page whose count is wrapped so the recogniser reads it as a group — same characters rendered,
  one plan that builds the pile to measure it and one that does not; 2.8× then 4.9× on a clock
  (`measure_incremental::what_counting_a_group_saves`).
  `scaling.rs::counting_a_group_does_not_build_it` is the shape gate and carries that contrast in the
  same run. The tally lives on the **join** rather than on the index, which is the finding worth
  keeping: an operator reads its inputs' values and changes and never their private state, because an
  index in the shared dataflow is not the reading engine's cell at all.
  `incremental_engine.rs::a_maintained_count_per_group_survives_the_events_that_take_it_down` folds a
  written log rather than a generated one, because whether a generated `Closed` names an issue that
  exists is the seed's business and not the test's — deleting the decrement turns both red, which was
  checked rather than assumed. §99.9 now also records that `min`/`max` per group are an `arrange_by`
  keyed by `(group, value)` and therefore the **cheap** ones rather than the hard ones it first
  called them, and that `sum` owes a decision per numeric type before an operator: a `Float` sum
  cannot be maintained by subtraction at all, and an `Int` one can be arithmetically but passes
  through different intermediate values from a recompute, which `checked_add` can turn into a
  disagreement about whether the program failed.

- **2026-08-18 — Six documents' corpus-wide counts were two programs out of date.**
  `corpus/35-workload.beck` is the 35th corpus program, and adding one moves every figure derived
  from the whole corpus. Re-deriving them found that three were already wrong *before* it:
  the native backends compile **968** definitions against 137 refused, not 941
  ([`docs/93`](docs/93-the-native-backends-report.md), [`docs/08`](docs/08-roadmap.md),
  [`docs/README`](docs/README.md)); the WebAssembly emitter is measured against **213** corpus
  definitions, not 195 ([`docs/103`](docs/103-the-wasm-emitter-report.md), `docs/README`); and the
  corpus places **362** definitions and signals, not 353, with the tier table moved with it
  ([`docs/20`](docs/20-phase-2-report.md)). All corrected in place, and
  [`DEFECTS.md`](DEFECTS.md)'s `corpus-wide-counts-drift` records why it recurred — every one of
  these numbers is printed by a release-only suite and gated by nobody — with the marker convention a
  fix needs before a test can check prose against a measurement.

- **2026-08-18 — `arrange_by`, and the join a filter already was.**
  [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.9 item 3, and the program it named:
  `examples/board.beck` renders three columns out of one map of cards, and each is
  `filter_list(map_values(b.cards), lambda c: c.column == n)` inside a loop over the columns — a
  many-to-one equi-join over an index nobody built, so the loop's function captured the accumulator
  and every event re-scanned every card once per column. The recogniser now reads a `filter_list`'s
  **predicate** the way it already read a `map_get`: an equality with one side over the filtered
  element and one over the loop's is a key and a probe. `Op::ArrangeBy` builds the index — the same
  arrangement `sort_by` builds, iterated there and probed here — and the join answers with the group,
  which is the rows the predicate would have kept in the order the collection held them, because `==`
  is the `Value` order the arrangement is a `BTreeMap` in. **4.5–4.9× less work per event at 200
  cards and at 1,600** with the cards spread over the columns, **1.1×** with every card in the one
  column the event touches: it removes the scan and leaves the group, which is §99.9 item 6's.
  `scaling.rs::a_group_a_loop_filters_for_costs_the_group_and_not_the_collection` is the shape gate —
  the group's size is paid and the collection's is not, at two sizes, with the growing case beside it
  as the gate's own evidence it can fail — and the board joins `fusion.rs` and
  `incremental_engine.rs`'s differentials, which is where a wrong group would show as a wrong page.
  Rungs 0–1 of §99.8's ladder still did not come due, and §99.8 now says why rather than predicting
  otherwise: a join inferred from a loop has the loop's order to preserve, so which side is the left
  is fixed before any cost is consulted.

- **2026-08-18 — The engine's counters cannot see inside an application, recorded as a defect.**
  Found while measuring the above: with the join refused, `examples/board.beck` reports the *same*
  `Work` — 3 applications, 3 touched, 3 materialised, 3 recomputed — at 200 cards and at 1,600, while
  the clock over the same two renders goes from 2.3 ms to 21 ms, because the whole page is rebuilt
  inside one per-element function and the engine counts one application for it. `beck explain cost`
  is right about that plan and the counter is not, which makes every `scaling.rs` gate over an opaque
  operator blind to exactly what an opaque operator can hide.
  `measure_incremental::what_a_grouped_join_is_worth` prints the counters beside the clock so the two
  disagree in public, and [`DEFECTS.md`](DEFECTS.md)'s `work-cannot-see-inside-an-application` names
  the gate a fix owes — two plans that do the same work must report the same `Work` — and why it is a
  `Backend` change rather than a line in the operator that found it.

- **2026-08-17 — A red CI gate root-caused to its own denominator.**
  `measure_native.rs::what_an_appended_accumulator_costs_against_the_tree_walker` failed on CI for
  three runs and passed everywhere else, reporting "the ratio collapsed, which is what an append
  that copies looks like" about an append nothing had touched. It gated on how the **speedup**
  moved between two sizes — a quotient of two measurements — so it imported the noise of both, and
  the noisy one was the denominator: on a contended runner the evaluator's 2,000-element median came
  out **20.7×** slower than on a developer machine while its 8,000-element median came out only
  **12.0×** slower, because the first thing measured in a process pays for warm-up. The small
  speedup inflated and the ratio-of-ratios fell through the bound. The native column — where the
  property actually lives — said the same thing on both machines throughout: **1.68× per element on
  CI, 1.54× locally**, against the 4× a copy would cost. So the assertion moves to per-element
  compiled cost at two sizes, which is machine-independent and is the instrument the rest of this
  project's shape gates use (`scaling.rs`, 3× bound); the speedups are still printed, because they
  are what this suite is *for*. [`docs/13`](docs/13-testing.md) §13.7 names this trap and
  [`docs/64`](docs/64-compile-speed-report.md) §64.1 names the cure — gate the shape, print the
  rate — and a gate that divides by something it does not need is a third way to get it wrong.

- **2026-08-17 — `select count(*)` stops building the rows it is not going to return.**
  [`docs/23`](docs/23-incremental-views-report.md) §23.19's last open row: "the plan's `list_len` is
  ±1 per delta; the SQL count is over the rows it scanned". So asking `psql` how many todos there
  are cloned every todo out of the collection and built a `Cell` per column of every one, to answer
  with a single integer — while `Op::Count` two layers down had read the size in `O(1)` since the
  engine existed. `read::Rows::count` is the seam, `Reader::len` answers it for a maintained
  arrangement, and a `Map` or a `list` in the accumulator answers it directly. **The default is
  "not without a scan"**, so a reader that does not implement it falls back and is exactly as
  correct and exactly as slow as it was — the seam cannot make a reader wrong, only faster. Gated by
  `read_models.rs::a_bare_count_is_answered_without_building_a_row`, whose instrument is the
  assertion rather than a measurement of one: a reader that knows the size and **refuses to produce
  a row**, so a query that scanned cannot be answered by it at all, with a second case proving the
  refusing reader really refuses. `a_count_that_narrows_anything_still_scans` fixes where the fast
  path stops — a `where`, `order`, `limit` or `offset` each still scan, because each is applied
  before the count collapses. [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.7
  attributed this row to grouping and is corrected: not every aggregate question is a grouping
  question, and the ungrouped one needed none of it.

- **2026-08-17 — The vulnerability matrix, CWE half, with the gate that makes it evidence.**
  [`docs/12`](docs/12-standards-and-conformance.md) §12.7 chartered one artefact restating §3.5's
  guarantees in the auditor's vocabulary; it lives in [`docs/43`](docs/43-threat-model.md) §43.8
  because that is where the threat model already is. All seven CWEs §12.7 names have a row —
  **CWE-639 is the one an auditor should read first**, since ownership rests on an actor the default
  provider does not verify, and `pending_security.rs` asserts that absence. `CWE-89`'s row states
  the reason precisely rather than claiming parameter binding: a Beck program does not *write* SQL,
  and the client protocol has no message that carries a query. The gate is
  `docs.rs::every_test_the_vulnerability_matrix_names_exists`, which checks both halves of each
  `suite.rs::name` citation — and it **earned itself before it was written**: the first draft cited
  `macro_bomb.rs::a_macro_that_expands_forever_is_refused_rather_than_hanging`, a plausible name for
  a test that does not exist. **The ISO/IEC 24772-1 half is not written, and that is a blocker
  rather than a schedule**: the catalogue is a paywalled standard that is not in this tree, and
  clause numbers from memory would be the failure §12.1 exists to prevent. Recorded in the matrix,
  in §12.7, in [`docs/35`](docs/35-standards-landscape.md) §35.2 and in the ledger.

- **2026-08-17 — Asked what the last three changes mean for Beck code that already exists, and
  found four false documents and one missing gate.** All 81 `.beck` files in the tree compile; the
  five that the accessibility checks refused were fixed with them. The exposure is Beck code *in
  documents*, which no gate reads. [`docs/11`](docs/11-language-tour.md) §11.6 said "`ui:` checks
  neither attribute nor event names, so `cls=` compiles and reaches the browser as an attribute
  nothing reads" — false for as long as the vocabulary has existed — and
  [`docs/README.md`](docs/README.md)'s index row said the same in summary; both corrected.
  [`docs/105`](docs/105-the-ecosystem-answer.md) still described `27-review`'s nested-loop join as
  the cost being paid. Three comments in Beck programs forecast work that has since landed
  (`17-derived`, `22-shared`, `examples/todo`) and now state what is true instead of what was
  expected. **[`docs/01`](docs/01-vision-and-premise.md)'s canonical example is deliberately not
  fixed**: it has not compiled since the surface settled, it is a faithful translation of the
  original sketch, and rewriting it would break the claim the section makes — so it says so, and
  points at `examples/todo.beck`, which is the same program in the language and is gated. The gate
  that would have caught the first two is now in `docs.rs`: a document showing a spelling the
  compiler refuses must name the diagnostic that refuses it, with the list read from
  `beck_macro::vocabulary`'s own alias tables rather than copied, so a new alias is covered the day
  it is added. It goes red on both documents as they stood.

- **2026-08-17 — Accessibility becomes a compile error, and the first run refused every example in
  this tree with a text input.** [`docs/12`](docs/12-standards-and-conformance.md) §12.4's first
  three checks, which that section had carried as **chartered** for as long as it existed: `B0219`
  an `img` with no alt text, `B0220` a button with no accessible name, `B0221` a form control with
  no label. The design claim — a typed `ui:` tree makes WCAG checkable at compile time in a way no
  template language can — was true and unexercised; these are three of it. What they found is the
  argument for them: `todo`, `board`, `editor` and `routed` had each labelled their input with a
  **placeholder and nothing else**, which is WCAG 3.3.2's commonest real failure, and all four are
  fixed. Which element needs what is `beck_macro::vocabulary::NAMING`, a table beside `ELEMENTS`
  rather than three tag names in the expander, held there by a gate that goes red on a misspelled
  tag — a check written against `"imag"` would never fire and no suite of correct programs could
  notice ([`docs/82`](docs/82-the-edge-report.md) §82.10). Two limits are stated rather than hidden:
  an `id` is accepted as evidence of a `label(for=…)` in another function, and a user helper sharing
  an element's name is checked as that element, which is `B0218`'s existing limit. The escape hatch
  is `a11y_exempt="reason"`, stripped before the page is emitted; §12.4 asked for
  `@a11y(exempt, reason=…)` and is corrected in place, because an annotation inside a `ui:` block
  would be new parser syntax for one hatch. Gated by three `tests/ui/` snapshots, the acceptance
  half (every way of naming a control, and the exemption) in `beck-macro`, and the two vocabulary
  gates. This closes the ledger item [`docs/08`](docs/08-roadmap.md) §8.5.4 scheduled behind the
  `ui:` vocabulary's **G**, which is that ordering paying for itself. One more stale figure fell out
  of it: §12.3 said **137** diagnostic codes and there are **145**, three of them these — the same
  decay direction as the corpus-wide numbers below, in the document that states the rule about it.

- **2026-08-17 — Several lookups in one loop are several joins, and the refusal that used to
  replace them was silent.** Two follow-ons to the join below, both found by using it.
  `beck explain cost` printed the *cost* of a loop it had not read as a join and not the reason:
  the reason is recorded on the `map_list` the decomposition builds, every `ui:` loop then fuses that
  `map_list` into the `flatten` above it, and the survivor kept its own empty field — so the one
  shape the explanation exists for was the one shape that never printed it. Fusion now carries it,
  gated by `incremental.rs::a_loop_that_is_not_read_as_a_join_says_why_after_fusion`, which asserts
  the reason appears *under the line it explains*. What that then exposed was a restriction with no
  design behind it: a body looking up in two collections was refused, which left the capture in place
  and the whole collection reconsidered per event. Refusing a shape that keeps a program at `O(n)` is
  not the conservative choice. It is now a **chain** of joins, one per lookup, each taking the
  previous one's rows on its left — so `corpus/33-awareness.beck`, which renders a person's
  whereabouts *and* their note, costs its delta. One of its two lookups is against the **awareness
  roster**, which [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.5 decision 3
  expected would have to be refused because a roster moves when `seq` does not; it does not have to
  be, because the second clock is a problem for *sharing* and not for *joining*, and that paragraph
  is corrected in place. §99.3's sweep is re-run: **16 capture sites in 8 programs and one that moves
  per event**, down from 18 in 10 with three. The one left is `examples/board.beck`, which groups
  rather than looks up.

- **2026-08-17 — The view algebra gets its first binary operator: the join a loop already
  contained.** [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.9 items 1, 4 and 5.
  Every operator the engine had took **one** collection, so a program relating two of them left the
  algebra: the loop body read the accumulator, the per-element function captured the state node, and
  a node that moves on every event makes that function a *different* function — so the whole
  collection was reapplied whatever changed. A nested-loop join with no index, per event.
  `Op::Join` is the operator and `beck_core::relate` is the recognition: `for x in xs:` whose body
  asks `map_get(m, k(x))` compiles to a join over an index, with **no edit to any program and no new
  syntax**, and the conditions are stated as what an expression *reads* rather than as a shape, so a
  lookup written three definitions deep behind a `match` is still found. Maintained from both sides
  per §99.5's bilinear rule — a left row that moved is looked up once, a right row that moved reaches
  exactly the left rows waiting on its key through a reverse index — so neither side costs the
  collection. The index needed no new operator: the right side is a `Map` field of the accumulator
  and `map_values`'s arrangement is already keyed by the join key, which is why §99.9's `arrange_by`
  moved *behind* the join rather than in front of it, and `examples/board.beck` is now named as the
  program waiting for it (it groups by column, which is not a lookup). Held by
  `scaling.rs::maintaining_a_view_whose_loop_looks_something_up_costs_the_same_at_any_size`, which
  measures `27-review.beck` at 200 and 1,600 rows **with the operator on and off**: 19 units of
  maintenance either size against **415 and 3,215** refused, so the gate carries its own evidence
  that it can fail ([`docs/82`](docs/82-the-edge-report.md) §82.10) and proves the off switch
  [`docs/08`](docs/08-roadmap.md) §8.3 item 8 requires — `Relate::Refuse`, reachable as
  `beck explain query --no-join` and `beck explain cost --no-join`. Correctness is the differential
  as before: `incremental_engine.rs` folds a generated log one event at a time and compares the
  maintained page with the recomputed one byte for byte. `corpus/34-assignments.beck` is the program
  §99.9 asked for, one that is *about* a relationship — many issues waiting on one person, so a
  rename is one entry moving on the right and several rows on the left, which `27-review`'s unique
  key cannot reach.

- **2026-08-17 — Three corpus-wide figures re-measured, and every one of them had drifted.**
  [`docs/08`](docs/08-roadmap.md) §8.5.6's first decay direction — a document behind the code — swept
  while re-running the measurement suites beside the join above. The placement share was written as
  43% across 176 placements and quoted as 44%; it is **52% across 353**
  (`measure_phase2`). The native backends' figures were 941 definitions compiled, nine corpus folds
  and twenty-one of thirty-two views; they are **963, all thirty-four, and twenty-four of
  thirty-four** (`BECK_REQUIRE_LLVM=1 … --test native`). No solver and no emitter moved: the corpus
  did, and the programs added to it since — recursive types, traits, error rows, structured
  concurrency, identity, presence, and now a relationship — are mostly pure definitions, so the
  numerators grew faster than the totals. Corrected in place in
  [`docs/20`](docs/20-phase-2-report.md), [`docs/93`](docs/93-the-native-backends-report.md),
  [`docs/08`](docs/08-roadmap.md), [`docs/86`](docs/86-getting-started.md) and the index, each with
  the command that reproduces it. **Nothing gates any of them**, which is why all three drifted
  together; the suites print them and assert shapes.

- **2026-08-17 — `main` merged down, and the union driver's reach written down where it is relied
  on.** This branch reported as conflicting on GitHub while `git merge` on a clone resolved it
  silently: the only conflict was [`CHANGELOG.md`](CHANGELOG.md), the file
  [`.gitattributes`](.gitattributes) sets `merge=union` on, and **GitHub reads neither that file nor
  any merge driver** — so the driver is in force exactly where nobody looks. Merging `main` down
  locally applies it and leaves the pull request nothing to merge. `DEFECTS.md::union-merge-is-local-only`
  records the general case, since every branch is required to prepend a bullet here and so every
  branch open across another's merge hits it; its gate is that two branches recording a change merge
  cleanly **with no `.gitattributes` in the tree**, which is GitHub's configuration and is red today.
  The `.gitattributes` comment claimed the conflict was solved and is corrected in place to say
  where. Both halves of the gate were run before being written down — conflict with the file absent,
  clean with it present — and `core.attributesFile` is recorded as the wrong way to model it, because
  it leaves the in-tree file in force and passes for the wrong reason. Nothing else conflicted:
  `CHANGELOG.md` kept every bullet from both sides and `DEFECTS.md` was untouched by `main`, so the
  driver never ran on it. `cargo test --workspace` is green over 102 suites, with
  `cargo clippy --workspace --all-targets`, `cargo fmt --all --check` and
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` clean.

- **2026-08-16 — The client-local fold, scoped: it splits where awareness splits, and the decision
  is one sentence.** [`docs/104`](docs/104-styling-and-the-component-library.md) §104.8. What it
  needs is a **stream**, not an accumulator: the only stream is `merge_clients()` and §3.5 places it
  on the server, so every `on_click` in the language becomes a proposal. A second, client-placed
  source over a second union — a `Ui` where `Command` is the one the chokepoint sees — routes an
  interaction by its *type*, and `merge_clients()` stays the sole chokepoint because a `Ui` value can
  never reach `validate`. In **Mode B** that needs no wire at all and does not touch the digest, so
  `DEFECTS.md::non-durable-fold`'s open question is not even asked; in **Mode A** the page renders
  where the state is, so a browser-held value reaches it only by being sent — and then it is a
  per-connection accumulator the server folds, which is presence's shape rather than the log's. The
  decision, and it wants a D-number: *does a client-local fold exist only where the client renders,
  or does Mode A get a per-connection accumulator so it works there too?* Also corrected in place:
  §104.8's list said "four homes" over a five-item list and used "the fifth home" for the fourth —
  the homes are now counted right and the one that has to be built is **named** rather than
  numbered, since the numbering is what drifted.

- **2026-08-16 — `awareness(f)` is built: the roster with a payload, for the half a session can
  answer.** [`docs/10`](docs/10-decisions.md) D6 gains the construct beside presence:
  `awareness(f) : Signal[Map[Str, T]] ! {cap.presence}`, where `f : Session -> T` produces one
  client's contribution and **the runtime applies it to every connection it holds** — `f` is a
  function rather than a signal because the subscribers are the runtime's fact and not the graph's,
  so a program cannot name another connection's session. It is a fifth view parameter and a plan
  source beside `presence`, a `Roles::awareness` role beside the view, and `beck_rt::awareness`, a
  registry modelled on `beck_rt::presence` with a second bound presence needs no equivalent of: a
  roster of counts costs its capacity, a roster of values costs its capacity times whatever `f`
  returns, so a contribution past `Config::each` is refused and the actor keeps its last one.
  Refused at the chokepoint (`B0520`) and to a Mode B page (`B0521`), for `B0515`'s and `B0516`'s
  reasons with one noun changed. `corpus/33-awareness.beck` is the program;
  `beck-cli/tests/awareness.rs` is the gate, fourteen tests, including the end-to-end one that
  presence could not have: a second client **navigating** — nobody arriving, nobody leaving, nothing
  appended to the log — moves the first client's page. The control gate was rewritten after a
  mutation: asserting "no frame reaches a program that reads no awareness" passes even with the
  wakeup wrongly armed, because such a page renders identically and diffs to nothing, so what it
  asserts now is the **row** — a client of such a program holds none — which an unconditional join
  turns red ([`docs/82`](docs/82-the-edge-report.md) §82.10). Client-local awareness — a cursor —
  is unchanged and still waits on a client-local stream
  ([`docs/104`](docs/104-styling-and-the-component-library.md) §104.8).

- **2026-08-16 — Awareness, scoped against the tree, and the scoping splits it in two.**
  [`docs/104`](docs/104-styling-and-the-component-library.md) §104.8 now specifies the construct:
  `awareness(f) : Signal[Map[ActorId, T]]`, a signal operation rather than a command, inheriting
  `presence()`'s three rules unchanged — non-log input to a view, capacity-bounded for §82.5's
  reason since the key is again a name the client chooses, and forbidden from the chokepoint, which
  is `B0515`'s reasoning with one noun changed. What the scoping found is that **what `f` may read
  splits the feature**. With `f : Session -> T` it is buildable today and needs **no wire change at
  all**, because the server already holds every subscriber's route — it arrives on `hello` and on
  every `Nav` — so *who is looking at what* costs a source, a role and an aggregation. With `f` over
  a client-local value — a cursor, a selection — it is not, and not for a protocol reason: the
  client has nothing to derive one from, since it listens for five events and `mousemove` is not
  among them. So arbitrary awareness has the **same prerequisite as the client-local fold**, and the
  two are one piece of work rather than two independent ones.

- **2026-08-16 — A search for counter-examples finds one, and it is D1's own: awareness.**
  [`docs/104`](docs/104-styling-and-the-component-library.md) §104.8's recommendation said to build
  nothing server-side, on the reasoning that presence, quotas and caches already answer every
  server-side ephemeral need. A search for counter-examples returned one it missed — **awareness**:
  a cursor, a selection, a typing indicator, which a *second person* must see and which therefore no
  client-local anything can hold. [Yjs](https://docs.yjs.dev/getting-started/adding-awareness) keeps
  it in a protocol of its own because it "isn't stored in the Yjs document, as it doesn't need to be
  persisted across sessions", and its shape is a `Map<client, state>` that is broadcast, expires
  after thirty seconds of silence and is deleted on disconnect. Two things follow. **It is not a
  fold** — it is a keyed map of each client's latest value — so it is still true that nothing found
  needs a server-side ephemeral *fold*. And **Beck has nine-tenths of it**: `presence()` is that map
  with no payload, already a non-log input to a view, already capacity-bounded (§82.5), already
  forbidden from the chokepoint (`B0515`). The homes go from four to five, ordered, and the
  correction underneath them is that ephemerality comes from the stream and the audience, never from
  the absence of a `durable` wrapper — which is what D1's sentence gets wrong.

- **2026-08-16 — A non-durable fold says what it is, and the reason it is unbuilt is written down.**
  A program whose only accumulator is a `fold` nobody wrapped in `durable` was reported as *a
  library with no durable state* — which sends its author to add the `durable` they deliberately
  left off. **B0519** names the construct instead ([`docs/10`](docs/10-decisions.md) D1), says it is
  decided rather than built, and says what stands in the way. The construct itself is still unbuilt,
  and the investigation is why: an accumulator outside the log is **not a function of the log**,
  `replay.rs` asserts `digest(replayed) == digest(live)`, and D3 rests on that digest — so the first
  question is what the digest covers, which is a decision and not a branch. The volume half of D1's
  own motivation is untouched by any of it, because [`docs/03`](docs/03-type-and-effect-system.md)
  §3.7 logs **every validated event**: a cursor that moves a hundred times a second writes a hundred
  entries whether or not the accumulator is durable, so an un-journalled accumulator is not an
  un-journalled stream. `DEFECTS.md::non-durable-fold` is rewritten around that finding, and
  [`docs/104`](docs/104-styling-and-the-component-library.md) §104.8's Wall 1 gains a survey of what
  Redux, Remix, SwiftUI, Akka and Phoenix LiveView do — they agree that the lifetime is a
  declaration and that the assignment is by audience — and a recommended order of four homes, marked
  as a recommendation because adopting it wants a D-number.

- **2026-08-16 — Correct what `docs/15` said about non-durable folds, which was wrong twice.**
  Its Redis-replacement ladder said hot ephemeral state is answered by non-durable folds, "already a
  language construct (D1)", and that "quota counters (F3) are exactly this". Neither holds. The
  construct is decided and **unbuilt** — that is `DEFECTS.md::non-durable-fold` — and F3's quota is
  not an instance of it and could not be: it is a **sharded** fixed table precisely because a
  per-actor map is unbounded memory keyed by a name the client chooses, which is the denial of
  service it exists to prevent ([`docs/82`](docs/82-the-edge-report.md) §82.5), and a fold would be
  that map. Presence is not an instance either — it is D6's first-class non-durable `Signal`, a
  compiler-provided source moved by *connections* rather than by events, which its own module
  documentation states as the one thing that makes it unusual. So nothing in the tree is a
  non-durable fold, where two things looked like one. Found by doubting the sentence rather than by
  a gate, which is [`docs/08`](docs/08-roadmap.md) §8.5.6's second direction of decay and the one
  nothing outside `pending_security.rs` catches.

- **2026-08-16 — `beck fmt` keeps comments, and the editor can format because of it.**
  The lexer skipped ordinary `#` comments, so formatting a file deleted every one of them — which
  is why `textDocument/formatting` was withheld rather than missing: a formatter an editor runs on
  save must not delete what somebody wrote. Comments are now collected from the source **by
  position, in the pass that already collected documentation**, which is what keeps a comment at
  column zero from closing an indented block, and it is one pass rather than two because what
  separates the two kinds is one decision about `#` and `##`. Three positions, each attaching
  differently: above a node, at the end of its own line (found by a scan that skips string
  literals, since `"a # b"` is not a comment), and below it with nothing after — which attaches
  *backwards*, or the note at the bottom of a function body would move out of the block it was
  written in. Gated three ways over the tree: `roundtrip.rs` now parses the way `beck fmt` does
  rather than through the bare parser, so its idempotence property covers comments at all
  (**it caught ten programs immediately**), plus `formatting_keeps_every_comment` — **1,850
  comments across every program in the tree, none deleted** — and a fixture with a comment in every
  position the grammar allows, byte-identical after a format. `textDocument/formatting` is enabled
  in the same change so the fix has a caller: one edit for the document, an empty list when there
  is nothing to do, `null` for a file that does not parse. Two older defects surfaced on the way
  and are fixed with it: a doc comment was lost outright when an ordinary comment sat between it
  and its declaration, and a node reached through both `item` and `stmt` printed its comments
  twice. Deletes `DEFECTS.md::fmt-comments`; corrects [`docs/02`](docs/02-syntax.md) §2.2 and
  [`docs/65`](docs/65-the-editor-report.md) in place.
- **2026-08-16 — CI retries the toolchain download it cannot control.**
  `rustup target add wasm32-unknown-unknown` failed a run with `Connection reset by peer` part-way
  through a component download from `static.rust-lang.org`; rustup keeps the partial file and says
  "please try again" in as many words, so trying again is the whole fix. Three attempts with a
  growing pause, and the loop still fails when the failure is real — checked both ways by hand,
  because a retry that swallowed a genuine failure would be worse than the flake it replaced.

- **2026-08-16 — `ui:` has a vocabulary: an event the client cannot hear and an attribute HTML does
  not have are compile errors.**
  `ui:` turned any `name=value` into an attribute and any `on_x=` into `data-b-x`, knowing nothing
  about either — so `span(on_mouseenter=…)` shipped a dead attribute to a browser that listens for
  five events and passed every gate, and `cls="done"`, the spelling
  [`docs/01`](docs/01-vision-and-premise.md) §1.3's own sketch uses, silently lost a page its
  styling. `beck_macro::vocabulary` is now the table: the five events, the HTML and SVG attribute
  names, and the elements [`docs/12`](docs/12-standards-and-conformance.md) §12.4's accessibility
  checks will read. **B0217** refuses an event the client does not listen for and **B0218** an
  attribute HTML does not have, with `data_…` and `aria_…` admitted by prefix — the escape hatch for
  an attribute that is genuinely yours is HTML's own, so there was none to invent. A table in a
  crate rather than a check in the expander, because typed macros retire the compiler-provided `ui:`
  ([`docs/10`](docs/10-decisions.md) D22) and the second copy is the one that drifts. Two things
  make it more than a list: `client.rs::the_event_vocabulary_is_what_the_client_listens_for` reads
  `beck-patch.js`'s own registrations and compares the two sets **in both directions**, so an event
  the client drops is caught as well as one the compiler invents; and the suggestion is a rule —
  squashing the hyphens `ui:` writes and looking again turns `max_length` into `maxlength` and
  covers every attribute of that shape, with `cls` needing the one alias because it is *one* edit
  from `cols` and two from `class`. An unknown **element** is not refused, and the reason is in the
  module: a lowercase all-keyword call inside `ui:` is indistinguishable from a helper function.
  Gated by three rendered-diagnostic snapshots and two client tests, all four of which go red on the
  previous expander. Deletes `DEFECTS.md::ui-vocabulary`; item 2 of
  [`docs/104`](docs/104-styling-and-the-component-library.md) §104.11's cluster, and the **G** that
  §12.4's three checks were waiting behind.

- **2026-08-16 — `beck explain cost` counts what it prints, and says how often a capture moves.**
  The summary collected operators whose cost mentions `n entries copied` and the capture line was
  written after the count, so `corpus/27-review.beck` — the one program in the corpus that contains
  a join — was told **1 of 29** operators cost `O(n)` per event when two do, wrong in the
  reassuring direction. The tally is now derived from the same per-operator record the body is
  printed from, so the two cannot disagree, and it reports **2 of 29** with the two reasons named
  apart: an arrangement forced into a list is `docs/23` §23.8's constant factor, a per-element
  function that captured the state is a program that left the view algebra. The capture line also
  carries the **cadence** of what it captured — never, per subscription, or per event — traced back
  to a source in one pass over the plan's dependency order, so a captured `const`, a captured
  `session` and a captured *state* print three different sentences instead of one; §99.3's sweep
  found 18 capture sites of which only 3 are the expensive kind, and one of those is two hops from
  `#0`. Gated by `incremental.rs::the_tally_counts_every_line_the_report_prints`, which reads both
  numbers out of the printed text rather than recomputing either, and
  `a_capture_says_how_often_what_it_captured_moves`, which builds one program per cadence; both go
  red on the previous behaviour. Deletes `DEFECTS.md::cost-report-undercount`; item 2 of
  [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.9, the instrument every item below
  it is read through.

- **2026-08-16 — A patched-in chart is a chart: the client builds SVG in the right namespace.**
  `beck-patch.js` built every subtree with `document.createElement`, which only ever guesses HTML,
  while server-side rendering goes through the browser's own parser and gets it right. An `svg`
  built that way is not an `SVGElement`, so it lays out as nothing: a chart painted on first load
  and vanished the first time its data changed, which is the only reason to have drawn it. The
  client now uses `createElementNS`, taking the namespace **from the tag** where the tag opens one
  and **from the destination** otherwise, with `foreignObject` handing it back to HTML — and the
  second half is where the difficulty is, because a patch that adds a bar to an existing chart
  carries no `svg` tag of its own. Gated by
  `browser.rs::a_patched_in_chart_is_still_a_chart` over the new `examples/chart.beck`, the first
  program in the tree whose page is an SVG: two patches, and the assertion is the **laid-out width**
  of every `rect` rather than its namespace. Checked against three wrong versions — the original
  measures 0 on the first patch, a tag-only fix measures 0 too, and a fix with subtree inheritance
  but no destination measures `30,0` on the second. Deletes `DEFECTS.md::svg-namespace`; item 1 of
  [`docs/104`](docs/104-styling-and-the-component-library.md) §104.11's cluster.

- **2026-08-16 — A cancellation gate stops betting on the scheduler.**
  `concurrency.rs::a_sibling_blocked_in_an_outbound_call_is_stopped_in_the_call` asserts that a
  scope reaches a child *blocked in the host*, and what put the sibling inside its call was
  arithmetic — twenty fast fetches first, on the reasoning that this made it "provably inside".
  Nothing enforced it, so under load the sibling was cancelled by the step counter before it ever
  entered a call and the test failed on its own guard while cancellation was working. The host now
  **holds** the failing child's first call until the sibling is blocked (a condvar, with a backstop
  that goes red rather than hanging), and the sibling has 4,000 steps to take before its fetch — so
  the hazard is exercised every run rather than only on a busy machine. Checked both ways: with the
  latch removed the test fails deterministically with the message that was seen intermittently, and
  it passes with it. Deletes `DEFECTS.md::blocked-sibling-race`;
  [`docs/80`](docs/80-structured-concurrency-report.md) §80.14 is the property it guards.

- **2026-08-16 — `sin` and `cos` are computed here, correctly rounded, and no longer the host's.**
  IEEE 754 requires `sqrt` correctly rounded and requires **nothing** of the transcendentals, so
  three backends reaching three platform libms meant a `durable` fold that computed a sine could
  replay to a different state on a different machine — the one thing
  [`docs/10`](docs/10-decisions.md) D3 rests the data tier on. `beck_prim::math` computes them
  instead, and every backend calls it: the evaluator directly, the two native emitters through a
  new `beck_prim_f64` entry point that carries no arena because a function from a double to a
  double allocates nothing. The answer is **correctly rounded**, which makes the specification the
  mathematics rather than a vendored file — a later rewrite cannot change a bit of any replay — and
  the implementation performs **no rounded floating-point operation at all**: exact integer
  reduction over 1472 bits of 2/π, an integer series, one rounding at the end. Measured
  (`cargo test -p beck-prim --release --test transcendentals -- --nocapture`): ~640 ns a call
  against a platform libm's 11 ns, and the same cost at `10^300` as at 1, which is the shape that
  gate holds; 400 calls per run of `awfy/cd.beck` — the only program in the tree that calls either
  — is 0.1% of it. Ziv's fast path in front of it is
  [`docs/08`](docs/08-roadmap.md) §8.5.4 and changes no answer. Gated by
  `beck-prim/tests/transcendentals.rs`, which recomputes 4,000 arguments at 1408 bits by a
  deliberately different route — Bailey–Borwein–Plouffe rather than Machin, binary long division
  rather than a window into 2/π, a term recurrence rather than Horner — and by
  `the_host_libm_would_fail_this`, which asserts that **11 of 8,000** of those answers are ones
  glibc does not give, so a change back to `f64::sin` goes red rather than unnoticed; plus a
  structural gate per backend (`native.rs`, `cranelift.rs`) that the module names the library and
  no libm symbol. Closes F9 ([`docs/14`](docs/14-review-findings.md)) and
  `DEFECTS.md::libm-determinism`; [`adr/0031`](docs/adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md).
- **2026-08-16 — The host half of the native protocol becomes one definition, and three sweeps of
  dead code and stale links.** A codebase audit against the standards
  [`AGENTS.md`](AGENTS.md) already sets. `beck-clif` and `beck-llvm` each carried their own
  `Artifact::exchange` — 65 lines, byte-identical but for the comments — encoding arguments,
  decoding a trap, decoding a raise payload and reading the arena back. That is host code, so the
  argument that keeps the two *emitters* apart (a shared selection would make `cranelift.rs`'s
  agreement gate true by construction and therefore worth nothing) never reached it; `beck-clif`'s
  own manifest says the worker protocol is "one definition, not two", and `service.rs`'s header
  already claimed both backends call it for the host. Now they do:
  `beck_llvm::service::exchange` is the other direction of the module that holds
  `service::answer`, and a new trap code has one place to be forgotten rather than two. The same
  shape held for the two WebAssembly modules, whose buffer table and length-prefixed frame — a
  contract with two *pages*, `playground.js` and `beck-mode-b.js` — was written twice; it is now
  `beck-frame`, with the exports left in the modules that answer them so `playground.rs` and
  `mode_b.rs` keep counting each crate's `forbid(unsafe_code)` exception locally.
  `docs.rs::a_relative_link_out_of_a_rustdoc_page_lands_on_the_file_it_names` was found to skip
  exactly the files that had broken links: it filters to `src/` and to targets containing `docs/`,
  and under that scope all 304 links resolve, while the 150 in the harnesses were checked by
  nothing and **eleven named a file that does not exist**. That is
  [`82`](docs/82-the-edge-report.md) §82.10's pattern again — a gate written to the shape of the
  fix, its scope frozen where the fix was. `a_relative_link_in_a_harness_lands_on_the_file_it_names`
  is the second rule, counted from the file rather than from a rendered page because nothing
  renders a harness; it was confirmed red on all eleven before they were corrected. Seventeen
  `pub fn`s that nothing referenced are gone, which is dead code rather than API because every
  crate is `publish = false` — two of them documented callers that do not exist
  (`parse_expr_str` "used by `beck ast` and by tests", `Types::rows_equal` "used … `.becki`
  agreement and `--wire-compat`") and one, `Artifact::codegen_time`, was exported so §7.3's
  compile-time claim could be checked by something, and was checked by nothing.
  `cargo clippy --workspace --all-targets` and `cargo fmt --all --check` were clean before this
  change and are clean after it; `cargo test --workspace` is 1345 tests over 102 suites.

- **2026-08-16 — The ecosystem question gets a per-library answer, and the roadmap gets a sweep.**
  [`docs/105`](docs/105-the-ecosystem-answer.md) answers "what about NumPy and pandas" from two
  independent constraints: a bridged call carries an effect and `place.rs:760` makes a fold
  replay-pure, so the [`09`](docs/09-risks-and-open-questions.md) §9.2 sidecar cannot reach the data
  tier; and the libraries that most expand a language's utility are **notations**, which cannot be
  bridged at all because an RPC hop destroys the composition that was their value. §102.4 discards
  download rank as an instrument — it measures fan-in, and `requests` is outranked by three of its
  own dependencies — for the Stack Overflow survey [`08`](docs/08-roadmap.md) §8.6's ≥1% rule
  already runs on, which puts NumPy at 21.2% and pandas at 20.7%, second and third among all
  libraries in all languages. GitHub stars were tested as a third instrument and discarded with
  evidence: they rank TensorFlow 6× above NumPy and measure it at half NumPy's use, because a star
  is a one-time vote that never decays. §8.6.2 applies the ≥1% rule to libraries for the first time
  and gives **all 39 entries** of the survey's section a verdict — four had none anywhere, including
  the Electron/Tauri adjacency (15.4% together, and Beck already emits both halves), which is
  recorded as watch rather than scheduled. §102.4 also carries what has moved since the 2024 survey:
  pandas 3.0 defaults to PyArrow-backed strings and PyArrow is PyPI #95 at 56% of pandas' own
  downloads, so the ecosystem has corroborated the Arrow argument with its defaults; Polars is a
  fifth convergence on the same dataframe verbs at a ninth of pandas' volume; and LLM clients are a
  category that post-dates the survey entirely, with `litellm` at #46 above `pip` — bridged, and the
  response becomes an event, so a session replays without re-calling the model. So pandas is
  [`99`](docs/99-the-data-tier-means-of-combination.md)'s missing algebra, NumPy is a notation over
  a linked kernel, and charting is blocked on `beck-patch.js`'s `createElement`. A doc-versus-code
  sweep (§8.5.6) then found one document behind the code —
  [`42`](docs/42-security-assurance.md) called macro expansion fuel "absent" when `MAX_EXPANSION`,
  `B0214` and `macro_bomb.rs` have bounded it all along — and seven items no ordered list held,
  including deterministic `sin`/`cos`, which resolve to the host libm in all three backends, so two
  machines can fold one log to two states. All now have a position in §8.5.4, and the two that are
  **defects rather than absences** — the libm divergence and `beck explain cost` excluding an
  `O(n)` operator from its own tally — are entries in [`DEFECTS.md`](DEFECTS.md) with the gate each
  fix owes. Charting was ranked here and is fixed nowhere: [`104`](docs/104-styling-and-the-component-library.md)
  found the same `createElement` defect from the UI side and owns it. Documents only; nothing built.
- **2026-08-16 · #69 — The two decision registers get a decidable boundary, and ADR identities get
  a gate.** "Design decisions there, engineering decisions here" was a judgement about intent and
  went both ways at least six times — [`adr/0010`](docs/adr/0010-generic-arithmetic-through-a-prelude-trait.md),
  [`0011`](docs/adr/0011-identifiers-are-snake-case-in-the-python-surface.md),
  [`0013`](docs/adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md),
  [`0014`](docs/adr/0014-a-keyed-digest-is-the-one-declassifier.md),
  [`0017`](docs/adr/0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md) and
  [`0007`](docs/adr/0007-evaluator-stack-is-declared-not-discovered.md)/[`0012`](docs/adr/0012-the-front-end-counts-its-own-recursion.md)
  each state a rule a Beck program lives under. The rule is now **a D-number is a rule a Beck
  program lives under; an ADR is a choice only the compiler lives under**, tested by whether a user
  could observe it without reading our source. Nothing is moved: a record is immutable and cited by
  identity from `front_end_bound.rs`, `lib/README.md` and `AGENTS.md`, so relocating one would break
  the citations and the immutability that is the difference between the registers.
  `docs/adr/README.md` also stated "D1–D20 stays as is" while the file held D1–D29.
  **The defect the check was written for**: `0023-tls-and-the-signature-it-brings.md` was titled
  `ADR 0022` — a real record's number — since the day it was written, so a citation to 0022 landed
  on the wrong decision, and `docs.rs`'s numbering gate excludes `docs/adr/`.
  `docs.rs::an_adr_is_numbered_for_the_file_it_is_in_and_is_listed` now holds three properties —
  title agrees with filename, no two records claim one number, the index names every record — each
  proved red by perturbation before the fix went in.

- **2026-08-16 · #69 — Styling is decided, scheduled, and defects get a register.**
  [`docs/104`](docs/104-styling-and-the-component-library.md) measures the position: the stylesheet a
  running application serves is eight rules hard-coded in `beck-rt/src/css.rs` and `css:` has no
  parser, so three documents claiming otherwise are corrected in place. Tailwind styles a Beck page
  with no configuration and no language change (63 ms, 35 utilities, 7,047 bytes) and its *scanner*
  cannot be adopted — 71 Beck files that style nothing emit 15 rules extracted from English prose in
  comments, a typo and a computed class vanish at exit 0, and an application whose components are an
  imported module yields 1 utility of 12, which is fatal because a Beck package has no source tree
  to scan. [`docs/10`](docs/10-decisions.md) **D29** settles it: take the design system, refuse the
  delivery, a name Beck does not know is a diagnostic, and **on by default with `styles = none` to
  turn all of it off**, the switched-off path gated beside the switched-on one per §8.3. The eight
  items are scheduled in [`docs/08`](docs/08-roadmap.md) §8.5.4 with a class and a lane each, and
  Phase 3's exit table gains the question a developer actually asks. The component half needs no
  language feature — components compose as functions, may be generic over the application's own
  command, and `ui:` already emits SVG with `viewBox` and `aria-label` intact, which answers
  [`docs/09`](docs/09-risks-and-open-questions.md) §9.6 item 8 in favour of a library. Nothing is
  built. New file [`DEFECTS.md`](DEFECTS.md) — what is wrong right now, every entry naming the gate
  its fix owes, deleted by the change that fixes it, union-merged like this file — seeded with the
  three defects this audit found and one older one, that `beck fmt` deletes `#` comments.
- **2026-08-16 · #68 — `Core` compiles to WebAssembly, for the scalar subset.** A third emitter
  ([`docs/103`](docs/103-the-wasm-emitter-report.md), `beck-wasmgen`), over the same layout module,
  trap codes, monomorphiser and fixtures the two native backends use, with the binary format
  written by hand and no runtime taken as a dependency
  ([`adr/0030`](docs/adr/0030-the-webassembly-emitter-writes-its-own-bytes.md)). `beck native
  --backend wasm --out <dir>` writes `module.wasm` and a readable `module.wat` rendered from the
  same instruction list. The gate is `wasm_backend.rs`: **12,852 calls agreed with the tree-walker
  in a real WebAssembly engine** — value or failure *and its message*, reals crossing as bit
  patterns — plus a million-deep tail recursion proving `return_call` is a jump. It compiles **0 of
  the corpus's 195 definitions** and 58 of `awfy/`'s, because the heap is not laid out on this
  target, so [`adr/0022`](docs/adr/0022-mode-b-ships-the-backend-it-has.md) is **not** reversed and
  Mode B still ships the interpreter; `docs/93` §93.15, `docs/94` §94.15, `docs/12` §12.3 and
  `docs/08` corrected in place. The suite skips without a JavaScript engine and
  `BECK_REQUIRE_WASM_RUN=1` forbids the skip, which CI now sets.

- **2026-08-16 · #67 — A `parallel:` child blocked in an outbound call is stopped inside the
  call.** Cancellation rode the evaluator's step counter, and a child blocked in a socket takes no
  steps — so a scope whose first child failed waited out a sibling's ten-second timeout.
  `beck_core::net::Stop` is the deadline [`docs/80`](docs/80-structured-concurrency-report.md)
  §80.12 said belonged on the seam: the same question `burn` asks, as a predicate
  `Outbound::fetch` takes as a parameter (not a defaulted second method — an implementation that
  ignored it would be a gate that cannot fail) and the real client polls every 5 ms while an
  exchange is watched. `Stop::never()` keeps the unwatched path, which is every call outside a
  scope. Gated by a counter rather than a clock — the host says whether the scope reached it or it
  hit its own backstop (`concurrency.rs`) — and over a real socket that accepts and says nothing
  (`outbound.rs`). §80.14 is the section; the compiled half is still open, because a worker holds
  its pipe for a whole call ([`docs/93`](docs/93-the-native-backends-report.md) §93.15).

- **2026-08-16 · #66 — Macro bodies run Beck at compile time.** The template expander becomes an
  interpreter ([`docs/102`](docs/102-the-macro-interpreter-report.md)): bindings, `if`, `for`,
  `while`, lambdas, calls to the module's own `def`s and to the pure prelude, `node_*` reflection
  over syntax, and `splice([…])`. A `let` computes where it used to substitute. The gate is a
  **differential** — 24 pure expressions computed by the interpreter and by `beck-eval` and
  compared inside the program (`macro_interp.rs`) — and the sandbox stops being satisfied by
  construction, so `macro_sandbox.rs` enumerates the prelude and fails when an effectful primitive
  is reachable at compile time ([`docs/12`](docs/12-standards-and-conformance.md) §12.7's G-class
  companion). Three bounds, measured: 84 steps for the largest real macro body against a budget of
  a million (`B0215`), 1.9 MB of the declared 64 MiB at the recursion ceiling (`B0216`), and
  nothing at all for a module with no macros in it. `docs/02` §2.4 and `docs/12` §12.10 corrected
  in place; `docs/08` §8.5.4's first item becomes the list of what it unblocked.

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
