# The register, one file per defect

**What is wrong right now is the files beside this one**, one per defect, named `<id>.md` and
opening with `` ## `<id>` ``. [`DEFECTS.md`](../DEFECTS.md) holds the rules they are kept by:
what belongs here, what is a line in [`docs/08`](../docs/08-roadmap.md) §8.5 instead, and why every
entry names the gate its fix owes.

An entry is added by the change that finds the defect and **deleted by the change that fixes it**,
with that change's [changelog](../changelog/README.md) entry as the record. Two branches recording a
defect write two different files and two branches fixing one delete two different files, so neither
merge has anything to resolve — on a clone and on the forge alike.

**`ls` here is the current list, and nothing stands between the list and the truth.** There is no
assembled copy to fall behind: the changelog can lag its entries because it is *history* and a late
entry is merely late, but a register that lagged would go on naming a defect somebody had already
fixed, which is the one thing "what is wrong right now" may not do.

## Why this file exists

So that the directory survives being **empty**, which is what a register is for and what git cannot
store: a directory with no files in it is not a thing a commit holds, so the register would
disappear the first time everything in it was fixed — taking with it the two links that name it and
the listing every gate reads. This file is not an entry and is skipped by the one that checks that
each entry is named for what it holds.
