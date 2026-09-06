- **2026-08-27 — The defect register is the directory, and no merge driver is left.**
  `DEFECTS.md::defects-entries-share-one-file`, deleted here — the residue of the changelog's fix
  the same day. [`DEFECTS.md`](../DEFECTS.md) was one list every branch that found something
  appended a section to, kept mergeable by the last `merge=union` line in
  [`.gitattributes`](../.gitattributes), and so reported as conflicting on GitHub for the same
  reason and in the same place: a driver git honours and **the forge reads nowhere**. A defect is
  now [`defects/<id>.md`](../defects/), added by the change that finds it and deleted by the change
  that fixes it.
  **It is the directory and not a file assembled from it, and that difference is the finding.** The
  changelog can lag behind its entries because it is *history*, and a late entry is merely late.
  A register is *state*: one that lagged would go on naming a defect somebody had already fixed,
  which is the one thing "what is wrong right now" may not do. `ls defects/` is exact because
  nothing stands between the list and the truth, so there is no assembler here and no
  `--check` to write.
  The gate is the twin of the changelog's, with the half its entry named as the one worth having:
  two branches each **recording** a defect and two branches each **fixing** one — deleting entries,
  which is this register's own rule — merge cleanly **in a tree with no `.gitattributes` at all**,
  against the negative control that two branches appending sections to one list conflict there.
  Union merge could never have held the deleting half: it does not resolve a delete against an edit.
  Two more hold the shape a citation depends on: `defects/<id>.md` opens with `` ## `<id>` ``, so
  the `DEFECTS.md::<id>` a comment cites still finds it, and `.gitattributes` names no merge driver
  at all — the two merge gates model the driver as absent, so only that one says the tree has
  stopped relying on it. Checked by adding one back, which turns it red naming the line.
  **And `beck doc changelog --check`, written that morning, was wrong in a way its own second entry
  found.** It compared the assembled list against a *tail* of the directory, on the assumption that
  what an assembly has not caught up with is the newest entries. Entries sharing a date are ordered
  by file name, so a change recorded today lands wherever its name falls among today's — as often
  after an already-assembled entry as before it — and the second entry written under this scheme
  sorted after the first and turned the check red on a correct branch. It would have done that to
  most of the branches now open. The relation is a **subsequence**: each entry the entry its file
  says, in the order the directory gives, any of them allowed to be missing. Reordering, editing and
  an entry no file holds all still fail, each naming the file, which was checked one mistake at a
  time.
