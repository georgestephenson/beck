- **2026-08-23 — A test stops writing generated pages into the source tree.**
  `beck-cli/tests/stdlib.rs::every_library_documents` ran `beck doc module` over every file in
  `lib/` with no `--out`, so each run wrote a page into the default output directory — which for a
  test binary is `crates/beck-cli/doc/`. Eleven HTML files had been checked in that way, by whoever
  next ran `git add -A`, and a twelfth arrived the moment `lib/json.beck` did. Nothing reads them:
  the published site is assembled into `site/` by `.github/workflows/docs.yml`, which passes its own
  `--out`. They are deleted, the directory is ignored, and the test passes `--stdout` — which also
  lets it assert that the page has something in it rather than only that the command exited zero.
  **A test that writes into the tree it is testing makes every diff after it suspect**, which is how
  this was found: the page for a new library module turned up in an unrelated commit.
