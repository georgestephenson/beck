# Defects

**What is wrong right now.** [`CHANGELOG.md`](CHANGELOG.md) is what has been fixed; this is what has
not. An entry is **deleted by the change that fixes it**, in the same commit, and the CHANGELOG
bullet for that change is where it goes on record. So this file is always the current list and never
a history — git holds the history, as it does for everything else in this repository
([`AGENTS.md`](AGENTS.md)).

**What belongs here: something that behaves wrongly.** Silent, misleading, or contrary to what a
document says. **What does not: something that is merely absent.** A feature nobody has built is a
line in [`docs/08`](docs/08-roadmap.md) §8.5, which is the only place that holds an order; putting
absences here would turn the register into a second roadmap that disagrees with the first.

**Every entry names the gate a fix owes.** This project has repeatedly shipped fixes behind gates
that could not have failed ([`docs/82`](docs/82-the-edge-report.md) §82.10), and the cure is to write
down *what would have to go red* while the defect is still in front of you. A fix that lands without
its gate has not been fixed; it has been made invisible.

**Ids are slugs, not numbers, and are never reused.** Entries are deleted, so a number would imply a
sequence that does not survive.

This register was opened alongside [`docs/104`](docs/104-styling-and-the-component-library.md) and is
**seeded rather than complete**: it holds what that audit found plus one older defect already
recorded in a report. Anything you find that meets the admission rule above belongs here, whether or
not you are the one to fix it.

---

## `non-durable-fold` — a decided construct is unbuilt, and what blocks it is a decision

**What is wrong.** [`docs/10`](docs/10-decisions.md) D1 provides for non-durable folds — "high-churn
ephemera get non-durable folds — same semantics, no log persistence" — and
[`docs/15`](docs/15-scale-and-distribution.md) assigns hot ephemeral state to them. A `fold` that is
not wrapped in `durable` does not make a signal graph, does not get a page, and does not run.

**Half fixed.** It used to report the program as *a library with no durable state*, which sends its
author to add the `durable` they deliberately left off. It now says what it is: `B0519`, naming the
construct, its status, and what stands in the way. The construct itself is still unbuilt.

**What stands in the way is not plumbing, and this is the finding.** An accumulator outside the log
is **not a function of the log**, and three things in this project rest on it being one:

- `beck-cli/tests/replay.rs` asserts `digest(replayed) == digest(live)`. A fold that is not
  replayed into makes the two differ by construction; one that *is* replayed into is not ephemeral,
  it is derived.
- [`docs/10`](docs/10-decisions.md) D3 rests on that digest — "replaying from the first event must
  always reproduce everything".
- [`docs/03`](docs/03-type-and-effect-system.md) §3.7 logs **every validated event**, so a fold over
  the one stream Beck has is reconstructible from the log whatever it is called. The *volume* half
  of D1's motivation — a cursor that moves a hundred times a second — is not addressed by an
  unlogged accumulator at all, because the events are what there are a hundred of.

So the construct needs an answer to **what the state digest covers**, and possibly a second,
un-journalled stream. Both are decisions rather than implementations, and they are D-numbers rather
than a branch.

**Two things that look like this construct and are not**, checked rather than assumed:

- `beck-rt/src/presence.rs` is a map mutated on connection join and leave. Its own module
  documentation states the distinguishing fact: it is "the only input to a view that moves
  **without** an event". It is D6's first-class non-durable `Signal` and a compiler-provided source,
  not a fold.
- `beck-rt/src/quota.rs` runs *before* an event exists, and is a **sharded** fixed table on purpose:
  a per-actor map is unbounded memory keyed by a name the client chooses, which is the denial of
  service it exists to prevent ([`docs/82`](docs/82-the-edge-report.md) §82.5). A fold would be that
  map.

Nothing in the tree is a non-durable fold, so there is no machinery to generalise.

**The gate a fix owes.** Unchanged, and the second is still the one that will be forgotten: a
program with a non-durable fold runs and its page reflects it, **and** the fold's state does not
appear in the log after a restart. A fix that only satisfies the first has built a durable fold with
a different spelling. `ui.rs::a_fold_nobody_wrapped_in_durable` holds the half that is done.

**Where it is argued.** [`docs/104`](docs/104-styling-and-the-component-library.md) §104.8, Wall 1.
The *language* question of where interface state should live is a decision rather than a defect and
is scheduled separately in [`docs/08`](docs/08-roadmap.md) §8.5.4 — and this entry now knows it is
the same decision rather than a separable one, because the client-local stream Wall 1 says does not
exist is the same missing thing.

---

## `class-list-recomputes` — the shape the styling design recommends turns a page into a recompute

**What is wrong.** [`docs/104`](docs/104-styling-and-the-component-library.md) §104.4 asks programs
to write a class as a *list of alternatives* — "`class=["btn", "primary" if hot else "plain"]` is the
shape §104.4 asks programs to write" — because a list can be enumerated and a concatenation cannot.
The `ui:` lowering turns that list into `str_join`, and **`str_join` has no delta rule**: a change to
its input can change all of its output. So a view containing one reaches
`beck explain incremental` as `Recompute`, and the page stops being maintained by delta.

A list whose elements are **all literals** is folded at lowering time and costs nothing — that is
what makes `class=["mx-auto", "max-w-80", "p-4"]` free. The defect is the mixed list, which is
exactly the case the design document's own example is about.

**How it was found**, which is the part worth keeping: `examples/todo.beck` was restyled onto
utilities and its `li` was written the way §104.4 recommends. `incremental.rs::a_relational_view_could_be_maintained_by_delta`
— §3.8's own claim, that "`remaining` updates by ±1 per event, never by recount" — went red. **No
program in this tree had ever used a class list with a non-literal element**, so the surface item 3
added had never been used in the shape its own design document recommends, and nothing said what it
cost.

**The workaround, which is what the sketch does.** Two whole alternatives behind an `if` —
`"flex gap-2 items-baseline line-through" if t.done else "flex gap-2 items-baseline"` — which is one
`Str`, no join, both arms enumerable, and the page stays incremental. It is worse to read and it
repeats the shared classes, which is the cost of the workaround rather than an argument for it.

**The gate a fix owes**, and the second half is the one that will be forgotten: a program whose
`class=` is a list with one non-literal element renders the same page **and** its `page` signal is
`Verdict::Incremental`. A fix that only satisfies the first has changed nothing. `style.rs` holds
the enumeration and `incremental.rs::a_relational_view_could_be_maintained_by_delta` holds the
verdict; what does not exist is a program with the mixed shape for either of them to be about, and
writing one is the first half of the fix.

**Where the answer probably is.** Not a delta rule for `str_join` — there is none to have. Either
the attribute's value keeps its list shape all the way to `html_attr` and the *patch* protocol joins
it (which moves the seam §104.4 deliberately did not touch), or the plan learns that a `str_join`
whose arguments are a literal list plus one branch is two constant alternatives and folds it the way
the all-literal case is folded. The second is the smaller change and covers the documented shape.

---

## `arrayref-is-pinned-to-a-yanked-version` — the safe version of a dependency is the withdrawn one

**What is wrong.** `compiler/Cargo.lock` holds `arrayref 0.3.9`, which crates.io **yanked** on
2026-08-20, and [`deny.toml`](compiler/deny.toml) carries the only entry in its `advisories.ignore`
list to keep the `licences` job green. A yanked dependency is a dependency nobody upstream is
maintaining, and holding one deliberately is a state to leave, not to settle in.

**Why it is not simply updated**, which is the whole entry. `cargo-deny`'s own suggested fix —
`cargo update -p arrayref` — resolves to **0.3.10, which is malicious**. From the registry index:

| version | yanked | dependencies |
|---|---|---|
| `0.3.9` | yes | none at runtime (`quickcheck`, dev only) |
| `0.3.10` | no | **`proc-macro1 ^1.0.107`**, a normal dependency |

`proc-macro1` is one character from `proc-macro2`. It has exactly two published versions, 1.0.106
and 1.0.107, which are `proc-macro2`'s own latest two; it copies `proc-macro2`'s feature set
(`proc-macro`, `nightly`, `span-locations`) and its single normal dependency (`unicode-ident`); and
it declares `base64`, `rustls` and **`ureq`** as *build* dependencies, so an HTTP client and a TLS
stack are linked into a build script that runs at compile time. `arrayref` is about two hundred
lines of macros for taking a reference to a sub-array. It reaches this tree through `blake3`.

Running the suggested fix here produced a lock file 283 lines larger, pulling in `ureq`, `url`,
`webpki-roots` and the whole ICU stack, which is what made it obvious. **Nothing was built with it**
and the `.crate` for 0.3.10 was never downloaded.

**The gate, which exists.** What has to go red is the ignore entry **outliving its reason** — the
crate moves on, nobody deletes the line, and the next yank of the same crate is waved through by a
permission granted for something else. `cargo-deny` reports `yanked-not-detected` when an ignore
entry matches nothing, but as a *warning*, which no CI job reads; `deny.toml` now sets
`unused-ignored-advisory = "deny"`, so it fails the build instead. Verified by pointing the entry at
a crate that is not in the graph: `error[yanked-not-detected]` and `advisories FAILED`. So the day
`arrayref@0.3.9` leaves this lock file, the `licences` job stops until somebody deletes the entry —
which is this defect's own removal, enforced rather than remembered.

**What has to be true to delete this.** Any one of: crates.io yanks 0.3.10; `arrayref` publishes a
0.3.11 without the `proc-macro1` dependency; or `blake3` moves to something else. Until then the
withdrawn version is the safe one, and that inversion is the reason this entry is long.

---

## `union-merge-is-local-only` — every pull request that touches `CHANGELOG.md` reads as conflicting

**What is wrong.** [`.gitattributes`](.gitattributes) sets `merge=union` on
[`CHANGELOG.md`](CHANGELOG.md) so that two branches each prepending a bullet under `## Unreleased`
do not conflict. Git honours it. **GitHub does not read the file at all** — neither the
`mergeable_state` it reports on a pull request nor the merge its button performs consults a merge
driver — so the driver is in force exactly where nobody is looking and absent where everybody is.
Since every change is required to add a bullet at the top of that list, and the list has no topic
headings on purpose, *every* pull request open across another one's merge is reported as conflicting.

**Why it is a defect rather than an inconvenience.** The report is not merely noisy, it is
**misleading in the direction that costs the most**: a reviewer reading "this branch has conflicts
that must be resolved" has no way to tell the one file the driver would have settled from a real
disagreement in the compiler, and the honest response to the message — resolve the conflict by hand
— is the one thing the flat-list design was built to make unnecessary. The comment in
`.gitattributes` asserted the conflict was solved; it is solved on a clone and not on the forge, and
that comment has been corrected in place.

**The workaround, which is not the fix.** Merge the base branch down into the branch locally, where
the driver applies, and push the merge. The pull request then has nothing left to merge and reports
clean. This works and is what has been done, but it puts a merge commit on every branch that
outlives one other merge, and it requires somebody to know why.

**The gate a fix owes**, and it is the half that will be forgotten: a real fix removes the *reliance*
on the driver rather than teaching the forge about it, most likely by giving each change its own
file so two branches never write the same line. So the gate is **not** "the union driver keeps both
bullets" — that passes today and pins the defect in place. It is that two branches each recording a
change merge cleanly **in a tree with no `.gitattributes` at all**, which is the configuration
GitHub runs: build the two branches, drop the file, merge, and assert no conflict.

**Model the absent driver by removing the file, not by configuration.** `core.attributesFile` names
the *global* attributes file and does not suppress the one in the tree, so a gate written that way
runs with the driver still in force and passes for the wrong reason — [`docs/82`](docs/82-the-edge-report.md)
§82.10's pattern, arrived at from the other direction. Checked both ways while this entry was
written: two branches each prepending a bullet conflict with the file absent and merge clean with it
present, so the gate goes red today and green on a fix.

---

## `corpus-wide-counts-drift` — adding a program silently falsifies six documents

**What is wrong.** Several documents quote a number derived from *the whole corpus*: how many
definitions the native backends compile and refuse
([`docs/93`](docs/93-the-native-backends-report.md), [`docs/08`](docs/08-roadmap.md) and
[`docs/README`](docs/README.md) — "941 definitions compiled against 137 refused"), how many the
WebAssembly emitter is measured against ([`docs/103`](docs/103-the-wasm-emitter-report.md),
`docs/README` — "0 of the corpus's 195 definitions"), and how the corpus places
([`docs/20`](docs/20-phase-2-report.md) — "353 placed definitions and signals", with a tier table).
**Adding one program to `compiler/corpus/` changes every one of them, and nothing says so.**

**It has already happened.** Those three figures were re-derived while `corpus/35-workload.beck` was
being added and came back **963/137**, **208** and **362** *before* the new program — so 941, 195 and
353 had been wrong since the corpus program before this one, in six places, through a merge. They now
read 968, 213 and 362, which is this tree.

**Why it is a defect rather than untidiness.** The numbers are the evidence for claims a reader acts
on: 941-of-1078 is what "the heap is whole" is worth, and the tier table is Phase 2's exit
measurement. A reader has no way to tell a figure that is current from one that is two corpus
programs old, and the failure is silent in the direction that flatters — a stale count is always the
smaller, older one. This is [`docs/82`](docs/82-the-edge-report.md) §82.10's shape in a document
rather than in a gate.

**Why it survives.** Every one of these numbers is printed by a **release-only measurement suite**
(`measure_phase2`, `measure_native`, `wasm_backend`), and those are run by a person who remembers to.
`docs.rs` gates that a link resolves, that a shell command runs and that every diagnostic the
vulnerability matrix names exists — it does not gate a number.

**The gate a fix owes**, and the hard part is not the assertion. It is that **a count has to be
findable in prose**: a test cannot grep for `941` and know which document meant which quantity. So
the fix is a marker convention — a quantity named where it is quoted, in a form a test can parse —
plus a `docs.rs` test that re-derives each named quantity and asserts the documents agree. The
re-derivation is cheap and needs no release build: compiled-versus-refused, corpus definitions and
the placement tier table are all compile-time facts about the corpus, which is why this is worth
gating rather than accepting. The gate goes red today only if it is written *before* the numbers
above are corrected; written after, it needs the marker convention to be exercised by at least two
documents quoting the same quantity, which the native count already does three times over.
