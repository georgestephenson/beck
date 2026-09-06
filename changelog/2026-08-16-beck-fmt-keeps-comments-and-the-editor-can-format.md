- **2026-08-16 — `beck fmt` keeps comments, and the editor can format because of it.**
  The lexer skipped ordinary `#` comments, so formatting a file deleted every one of them — which
  is why `textDocument/formatting` was withheld rather than missing: a formatter an editor runs on
  save must not delete what somebody wrote. Comments are now collected from the source **by
  position, in the pass that already collected documentation**, which is what keeps a comment at
  column zero from closing an indented block, and it is one pass rather than two because what
  separates the two kinds is one decision about `#` and `##`. Three positions, each attaching
  differently: above a node, at the end of its own line (found by a scan that skips string
  literals, since `"a # b"` is not a comment), and below it with nothing after — which attaches
  *backwards*, or the note at the bottom of a function body would move out of the block it was
  written in. Gated three ways over the tree: `roundtrip.rs` now parses the way `beck fmt` does
  rather than through the bare parser, so its idempotence property covers comments at all
  (**it caught ten programs immediately**), plus `formatting_keeps_every_comment` — **1,850
  comments across every program in the tree, none deleted** — and a fixture with a comment in every
  position the grammar allows, byte-identical after a format. `textDocument/formatting` is enabled
  in the same change so the fix has a caller: one edit for the document, an empty list when there
  is nothing to do, `null` for a file that does not parse. Two older defects surfaced on the way
  and are fixed with it: a doc comment was lost outright when an ordinary comment sat between it
  and its declaration, and a node reached through both `item` and `stmt` printed its comments
  twice. Deletes `DEFECTS.md::fmt-comments`; corrects [`docs/02`](../docs/02-syntax.md) §2.2 and
  [`docs/65`](../docs/65-the-editor-report.md) in place.
