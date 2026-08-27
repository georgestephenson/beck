# The changelog, one file per change

A change records itself here, in a file of its own, and edits nothing else. That is the whole
design: two branches recording a change write two different files, so their merge has nothing to
resolve — on a clone and on the forge alike.

What it replaced was one flat list every branch prepended a bullet to, kept mergeable by a
`merge=union` driver in [`.gitattributes`](../.gitattributes). Git honours that driver. **GitHub
reads no merge driver** — neither for the mergeability it reports on a pull request nor for the
merge its button performs — so the driver was in force exactly where nobody was looking and absent
where everybody was, and every pull request open across another one's merge was reported as
conflicting. A reviewer reading that report had no way to tell the changelog from a real
disagreement in the compiler.

## Writing one

Name the file `YYYY-MM-DD-a-few-words.md` for the day it merges, and put one entry in it, in the
shape [`CHANGELOG.md`](../CHANGELOG.md) describes:

```markdown
- **2026-08-27 — What changed, said in one line.**
  What it measured, what gate holds it, and the design document it derives from. A few lines; an
  entry that wants sub-bullets has outgrown the file it is going into.
```

**Links are written from this directory** — `](../docs/08-roadmap.md)`, not `](docs/08-roadmap.md)`
— so an entry reads correctly where it lives. Assembling strips the `../` so that it reads correctly
at the root too, and `docs.rs::every_relative_link_in_the_markdown_lands_on_a_file_that_exists`
checks both files.

## Assembling

From `compiler/`, `beck doc changelog` writes [`CHANGELOG.md`](../CHANGELOG.md): the head above
`## Unreleased` kept as it is, every entry below it in order — by date, newest first, and by file
name within a date.

`beck doc changelog --check` holds what is checked in to what is here, and deliberately does not
demand that it be complete: every entry the list carries must be the entry its file holds, and the
newest may be missing. Demanding otherwise would make reassembling part of recording a change, and
then two branches would once more be adding lines at the same place in one file — the thing this
directory exists to stop. **So the assembled list may lag and may not disagree**, and assembling it
is a change of its own that lands on its own.
