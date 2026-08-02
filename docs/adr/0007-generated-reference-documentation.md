# 0007 — Reference documentation is generated from the compiler, and checked in

**Context.** Everything a reference page wants to say about a Beck program is something the
compiler already computes: the signature comes from inference, the effect row from §3.2's
inference, the tier from §3.4's solver, and the module's contract from `Interface` (§3.6). The same
is true of the language's own reference — the diagnostic codes it can emit, the atoms a tier can
discharge, the schemes in the prelude, the CLI's own argument tree. None of it was published.
`docs/16` §16.2 promises the docs.rs model ("documentation generated from types and doc-comments
for every published version, automatically") and schedules it to Phase 4/5 with the Mere.

Hand-writing a reference for a language whose defining features are *inferred* guarantees drift:
a page saying `map_list` is pure stays wrong until someone reads it.

**Decision.** Four things, taken together.

1. **Doc comments in the language.** `##` in the Python surface, `;;` in the S-expression one.
   Collected from the source text and attached to nodes after parsing, rather than lexed as a
   token: layout treats a comment-only line as having no indentation, and putting that rule at risk
   for every file to serve a feature that only reads declarations is a bad trade. A doc comment is
   `Meta`, not a form — so it is outside `structurally_eq`, outside the memo key, and outside
   `Interface::digest`. Documenting a definition is not an API change.
2. **`beck doc`** generates a module's page (markdown, HTML, JSON) and the language reference.
3. **`docs/reference/` is checked in**, and `beck doc reference --check` fails when it differs
   from what the compiler generates. A generated file nobody sees in a diff is a generated file
   nobody notices going wrong — `docs/20` §20.4 item 8 is what that costs.
4. **The site is published from `main`** by `.github/workflows/docs.yml`: the reference, two
   module pages, and rustdoc for the nine crates.

**Alternatives rejected.**

- *`##` as a lexer token.* Correct in principle, and it would let a doc comment attach structurally
  rather than by position. Rejected for the layout risk above; the position rule is exact for
  declarations, which is the whole of what `beck doc` reads.
- *A Markdown dependency for the HTML.* `docs/07` lists mdBook (MPL-2.0) for the eventual book. A
  reference page needs less than a book does, and the pages are generated from a model rather than
  from Markdown, so the renderer writes HTML directly. The cost is named in the module docs: prose
  in a doc comment is escaped and paragraph-broken, not parsed as Markdown.
- *Generating the error index by scanning the source.* The prose at each diagnostic site is
  formatted at the call site and cannot be lifted mechanically. The index is written, and a test
  asserts the *set* of codes agrees with the compiler in both directions — which is the property
  that actually goes stale.

**Consequences.**

- A new diagnostic code fails `cargo test` until it has an index entry, and a removed one fails
  until its entry goes. The index cannot silently drift; its prose can, and only its prose.
- A change to `Tier::discharges`, to the prelude, or to the CLI changes `docs/reference/` and must
  land with it. That is one extra `beck doc reference --out ../docs/reference` in such a change.
- `beck fmt` now preserves `##` comments and still discards `#` ones. That distinction is the
  reason to use `##`, and it is asserted by a test.
- **`missing_docs` is deliberately not enabled.** 863 public items across the nine crates carry no
  `///`, CI denies warnings, and the lint would fail the build until every one of them had a
  sentence — most of which would be the restatement-of-the-name kind that makes documentation
  worth less, not more. The number is published in `docs/31` so the decision stays visible and
  reversible; a crate-by-crate opt-in is the path if it is taken.
