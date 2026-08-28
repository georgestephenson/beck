- **2026-08-17 — Asked what the last three changes mean for Beck code that already exists, and
  found four false documents and one missing gate.** All 81 `.beck` files in the tree compile; the
  five that the accessibility checks refused were fixed with them. The exposure is Beck code *in
  documents*, which no gate reads. [`docs/11`](../docs/11-language-tour.md) §11.6 said "`ui:` checks
  neither attribute nor event names, so `cls=` compiles and reaches the browser as an attribute
  nothing reads" — false for as long as the vocabulary has existed — and
  [`docs/README.md`](../docs/README.md)'s index row said the same in summary; both corrected.
  [`docs/105`](../docs/105-the-ecosystem-answer.md) still described `27-review`'s nested-loop join as
  the cost being paid. Three comments in Beck programs forecast work that has since landed
  (`17-derived`, `22-shared`, `examples/todo`) and now state what is true instead of what was
  expected. **[`docs/01`](../docs/01-vision-and-premise.md)'s canonical example is deliberately not
  fixed**: it has not compiled since the surface settled, it is a faithful translation of the
  original sketch, and rewriting it would break the claim the section makes — so it says so, and
  points at `examples/todo.beck`, which is the same program in the language and is gated. The gate
  that would have caught the first two is now in `docs.rs`: a document showing a spelling the
  compiler refuses must name the diagnostic that refuses it, with the list read from
  `beck_macro::vocabulary`'s own alias tables rather than copied, so a new alias is covered the day
  it is added. It goes red on both documents as they stood.
