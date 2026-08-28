- **2026-08-16 — `ui:` has a vocabulary: an event the client cannot hear and an attribute HTML does
  not have are compile errors.**
  `ui:` turned any `name=value` into an attribute and any `on_x=` into `data-b-x`, knowing nothing
  about either — so `span(on_mouseenter=…)` shipped a dead attribute to a browser that listens for
  five events and passed every gate, and `cls="done"`, the spelling
  [`docs/01`](../docs/01-vision-and-premise.md) §1.3's own sketch uses, silently lost a page its
  styling. `beck_macro::vocabulary` is now the table: the five events, the HTML and SVG attribute
  names, and the elements [`docs/12`](../docs/12-standards-and-conformance.md) §12.4's accessibility
  checks will read. **B0217** refuses an event the client does not listen for and **B0218** an
  attribute HTML does not have, with `data_…` and `aria_…` admitted by prefix — the escape hatch for
  an attribute that is genuinely yours is HTML's own, so there was none to invent. A table in a
  crate rather than a check in the expander, because typed macros retire the compiler-provided `ui:`
  ([`docs/10`](../docs/10-decisions.md) D22) and the second copy is the one that drifts. Two things
  make it more than a list: `client.rs::the_event_vocabulary_is_what_the_client_listens_for` reads
  `beck-patch.js`'s own registrations and compares the two sets **in both directions**, so an event
  the client drops is caught as well as one the compiler invents; and the suggestion is a rule —
  squashing the hyphens `ui:` writes and looking again turns `max_length` into `maxlength` and
  covers every attribute of that shape, with `cls` needing the one alias because it is *one* edit
  from `cols` and two from `class`. An unknown **element** is not refused, and the reason is in the
  module: a lowercase all-keyword call inside `ui:` is indistinguishable from a helper function.
  Gated by three rendered-diagnostic snapshots and two client tests, all four of which go red on the
  previous expander. Deletes `DEFECTS.md::ui-vocabulary`; item 2 of
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.11's cluster, and the **G** that
  §12.4's three checks were waiting behind.
