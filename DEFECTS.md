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

This register was opened alongside [`docs/104`](docs/104-styling-and-the-component-library.md), seeded
with what that audit found plus one older defect already recorded in a report. It has never been
*complete* and is not meant to be read as a survey: it is what somebody wrote down. Anything you find
that meets the admission rule above belongs here, whether or not you are the one to fix it.

---

## `the-gesture-measurement-asserts-on-a-clock` — a gate that can fail for a reason it is not about

**What is wrong.** `measure_mode_b.rs::what_a_gesture_costs_against_a_command` measures how long a
gesture takes against a command, at 100 cards and at 1,000, and **asserts** that the ratio is at
least 1.0 — a gesture must not cost more than a command. The measurement is a wall clock, the
margin is the 1.18–1.21× [`CHANGELOG.md`](CHANGELOG.md) records, and on a loaded runner it goes the
other way: observed at **0.87× at 100 cards** in a `cargo test --workspace` run, passing on the same
tree when run alone.

**Why it is a defect rather than a flaky test to re-run.** [`docs/13`](docs/13-testing.md) §13.7 is
this project's own rule — "a timing gate on a shared runner cannot be held honestly, and a gate that
flakes gets deleted" — and every other measurement suite obeys it by *printing* rather than
asserting. This one asserts, so it is the one place a red build says nothing about the tree. That is
the misleading direction: a person who sees it fail has to decide whether the routing regressed or
the runner was busy, and the honest answer is usually the second, which is how a gate stops being
read at all.

**Why the bound is thin rather than generous.** 1.0× is the right *shape* — a gesture that cost more
than a command would falsify D30's design — but the measured margin over it is 18%, which is inside
the noise of a debug build on a shared machine. Widening the bound would make it weaker without
making it deterministic.

**The gate a fix owes.** The instrument, not the threshold: what separates a gesture from a command
is that the local path skips `validate` and the state derivation, and that is **countable** —
`beck_core::engine::Work` and the evaluator's step budget both see it, and neither has a clock in
it. So the fix is the same one `scaling.rs` uses everywhere else, and its gate is that the assertion
survives being run under load. The negative half, which is the one that would be forgotten: with the
local routing removed so a gesture goes through the chokepoint, the gate must still go **red** — a
counter-based version that measured the wrong two things would pass either way.

## `a-bounded-impl-parameter-is-refused-by-the-compiler-that-suggests-it`

**What is wrong.** `impl[T: ToJson] ToJson for list[T]` — an impl whose type parameter carries a
bound — is refused with `B0310: cannot find type \`T\``, pointing at the `list[T]` in the impl's own
header. The unbounded form `impl[T] ToJson for list[T]` is accepted, so the bound is what breaks it.

**Why it is a defect rather than an absence.** The compiler *tells you to write it*. An unbounded
impl whose method calls a trait method on `T` reports `B0386: \`T\` is not known to implement
\`ToJson\`` with `help: bound it: \`[T: ToJson]\``, and taking that advice produces a different
error about a type parameter that is written three characters to the left. A suggestion that does
not compile is worse than no suggestion: it reads as the compiler contradicting itself, and the
person following it has no way to tell which of the two messages is the true one.

**What is actually broken, in three parts**, found while writing §2.4's `derive` and each confirmed
by fixing it in isolation:

1. `check/mod.rs`'s `typaram_names` and `bind_decl_typarams` read a parameter with `Node::as_var`,
   which answers `None` for the `(annot T ToJson)` a bounded parameter parses to — so the parameter
   is dropped from scope entirely. `check/traits.rs::typaram_name` is the function that reads both
   and is not used by either.
2. `expand_bounds` — the rewrite that turns a bound into a dictionary parameter — runs over the
   items **as written**, and an impl's methods do not exist until `expand_impls` has synthesised
   them one line earlier. So a method that fixes (1) still cannot call anything through its bound.
3. `trait_call` resolves a method to the impl's mangled global and applies it directly, without the
   dictionary-passing path `BindKind::Global` takes for a bounded `def`. Supplying the dictionary
   means matching the impl's target against the receiver to learn what `T` is, which is a piece of
   dispatch rather than a repair.

The first two are one line each and the third is not, which is why this is written down rather than
half-fixed: an impl that compiles and whose calls cannot is a worse state than the one above.

**The gate a fix owes**, and it has to be both halves. Positive: a program with
`impl[T: Ord] Ranked for list[T]` whose method calls the bound's own method **compiles and runs**,
with a call at a concrete element type. Negative, and this is the half that would be forgotten: the
same program with the bound removed still reports `B0386` and still suggests the bound — because a
"fix" that silently made an unbounded parameter satisfy every trait would pass the first half and
delete the check.

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

