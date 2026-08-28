- **2026-08-17 — `main` merged down, and the union driver's reach written down where it is relied
  on.** This branch reported as conflicting on GitHub while `git merge` on a clone resolved it
  silently: the only conflict was [`CHANGELOG.md`](../CHANGELOG.md), the file
  [`.gitattributes`](../.gitattributes) sets `merge=union` on, and **GitHub reads neither that file nor
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
