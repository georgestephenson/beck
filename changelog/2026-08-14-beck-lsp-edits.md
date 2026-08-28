- **2026-08-14 · #53 — `beck lsp` edits**: references, document highlight, prepare-rename, rename
  and inlay hints, every answer in `beck_core::editor` so a browser tab can ask too
  ([`docs/65`](../docs/65-the-editor-report.md)). A rename is verified by making the edit and
  re-analysing; 316 of the corpus's 325 names rename and every decliner is asserted. The largest
  real file (914 lines) analyses in 16.84 ms and renames in 19.03 ms (`measure_compile.rs`).
