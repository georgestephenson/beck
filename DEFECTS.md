# Defects

**What is wrong right now lives in [`defects/`](defects/), one file per defect.** That directory is
the register; this file is the rules it is kept by. [`CHANGELOG.md`](CHANGELOG.md) is what has been
fixed, and git holds the history, as it does for everything else in this repository
([`AGENTS.md`](AGENTS.md)).

**A file per defect, because the register is state rather than history.** A defect is recorded by
adding `defects/<id>.md` and **deleted by the change that fixes it**, in the same commit, with that
change's changelog entry as the record. Two branches recording a defect write two different files
and two branches fixing one delete two different files, so neither merge has anything to resolve —
on a clone and on the forge alike. The list this replaced was one file every branch appended a
section to, kept mergeable by a `merge=union` driver that git honours and **GitHub does not read**,
so every pull request open across another one's merge was reported as conflicting.

That is also why the register is the directory and not a file assembled from it. The changelog can
be assembled and lag behind its entries, because it is history and a late entry is merely late. A
register that lagged would go on naming a defect somebody had already fixed, which is the one thing
"what is wrong right now" may not do. `ls defects/` is the current list because there is nothing
between the list and the truth.

**What belongs here: something that behaves wrongly.** Silent, misleading, or contrary to what a
document says. **What does not: something that is merely absent.** A feature nobody has built is a
line in [`docs/08`](docs/08-roadmap.md) §8.5, which is the only place that holds an order; putting
absences here would turn the register into a second roadmap that disagrees with the first.

**Every entry names the gate a fix owes.** This project has repeatedly shipped fixes behind gates
that could not have failed ([`docs/82`](docs/82-the-edge-report.md) §82.10), and the cure is to write
down *what would have to go red* while the defect is still in front of you. A fix that lands without
its gate has not been fixed; it has been made invisible.

**Ids are slugs, not numbers, and are never reused.** Entries are deleted, so a number would imply a
sequence that does not survive. The id is the file's name and the heading it opens with — `##` and
the id in backticks, which `docs.rs::every_defect_is_a_file_named_for_the_entry_it_holds` holds
together. Code and documents cite a defect as `` `DEFECTS.md::<id>` ``, which is a name rather than
a path and stays readable long after the entry has been deleted by its fix.

This register was opened alongside [`docs/104`](docs/104-styling-and-the-component-library.md), seeded
with what that audit found plus one older defect already recorded in a report. It has never been
*complete* and is not meant to be read as a survey: it is what somebody wrote down. Anything you find
that meets the admission rule above belongs here, whether or not you are the one to fix it.
