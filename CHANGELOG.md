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
