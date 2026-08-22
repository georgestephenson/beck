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

