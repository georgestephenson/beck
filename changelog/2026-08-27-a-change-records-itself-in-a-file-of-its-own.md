- **2026-08-27 — A change records itself in a file of its own, and the changelog is assembled.**
  `DEFECTS.md::union-merge-is-local-only`, deleted here.
  [`CHANGELOG.md`](../CHANGELOG.md) was one flat list every branch prepended a bullet to, kept
  mergeable by `merge=union` in [`.gitattributes`](../.gitattributes) — a driver **GitHub reads
  nowhere**, neither for the mergeability it reports on a pull request nor for the merge its button
  performs. Since every change is required to record one, *every* pull request open across another
  one's merge was reported as conflicting, and the honest response to that message — resolve it by
  hand — was the one thing a flat list with no topic headings was shaped to make unnecessary. What
  has gone is the reliance rather than the report: a change now adds one file to
  [`changelog/`](../changelog/README.md) and edits nothing else, and `beck doc changelog` assembles
  that directory into the same newest-first list under the file's hand-written head. The 99 entries
  that were in the list are the first 99 files, checked as an unchanged multiset; the only thing
  that moved is the order within a date, which is the file name's now rather than whichever merge
  landed first.
  **The gate is the half the entry said would be forgotten.** Not "the union driver keeps both
  bullets", which passed the whole time the defect stood: two branches each recording a change
  merge cleanly **in a tree with no `.gitattributes` at all**, which is the configuration GitHub
  runs — modelled by removing the file and not by `core.attributesFile`, which names the *global*
  file and would have left the driver in force. Its negative half is what gives it teeth, and the
  truth table is the defect's own: the same two branches prepending bullets the old way conflict
  there, and merge once the driver is in the tree. `beck doc changelog --check` is the second gate,
  and catches what the first cannot see — an entry written straight into the assembled list is an
  entry no file holds. It does not demand a complete list, because a check that did would make
  reassembling part of recording a change and put every branch back on one line of one file: the
  assembly may lag and may not disagree. What survives of the defect is
  `DEFECTS.md::defects-entries-share-one-file`, the same mechanism on the register itself, which
  two branches reach only when both find a defect.
