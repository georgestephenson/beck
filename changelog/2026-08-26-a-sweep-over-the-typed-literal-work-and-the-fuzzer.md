- **2026-08-26 — A sweep over the typed-literal work, and the fuzzer gets the shape that eats text.**
  [`docs/13`](../docs/13-testing.md), [`docs/82`](../docs/82-the-edge-report.md) §82.11,
  [`docs/11`](../docs/11-language-tour.md) §11.9. `grammar_fuzz.rs` gains `Shape::Sigils`: ten shapes
  built *structure* out of the grammar's own punctuation, and a sigil is the first production that
  consumes **arbitrary text** — a raw body the lexer takes wholesale and a macro reads with
  `node_lit` — which is exactly the class `docs/13` calls CVE-shaped. Generated with a real macro
  behind it so the body reaches the interpreter rather than stopping at `B0340`, over an alphabet
  that includes `$` and `$*`, and added to the exhaustive ceiling walk beside the property test.
  §8.5.6's sweep over the same work found four documents behind the code: **`docs/11` §11.9 showed
  four escape hatches in present tense and not one of them parses** — `external store`, `extern def`
  and `python_service` are all `B0307`, and `sql"…"` has its notation and not its macro — so the
  section now says so, in the same shape §11.10 beside it already used. `docs/105`'s regex row said
  "unblocked" when the notation is built and what is missing is a regex *engine*; `docs/104`'s `css:`
  row and `docs/82`'s fuzzing row were each one step behind.
