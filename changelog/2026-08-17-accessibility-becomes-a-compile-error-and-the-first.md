- **2026-08-17 — Accessibility becomes a compile error, and the first run refused every example in
  this tree with a text input.** [`docs/12`](../docs/12-standards-and-conformance.md) §12.4's first
  three checks, which that section had carried as **chartered** for as long as it existed: `B0219`
  an `img` with no alt text, `B0220` a button with no accessible name, `B0221` a form control with
  no label. The design claim — a typed `ui:` tree makes WCAG checkable at compile time in a way no
  template language can — was true and unexercised; these are three of it. What they found is the
  argument for them: `todo`, `board`, `editor` and `routed` had each labelled their input with a
  **placeholder and nothing else**, which is WCAG 3.3.2's commonest real failure, and all four are
  fixed. Which element needs what is `beck_macro::vocabulary::NAMING`, a table beside `ELEMENTS`
  rather than three tag names in the expander, held there by a gate that goes red on a misspelled
  tag — a check written against `"imag"` would never fire and no suite of correct programs could
  notice ([`docs/82`](../docs/82-the-edge-report.md) §82.10). Two limits are stated rather than hidden:
  an `id` is accepted as evidence of a `label(for=…)` in another function, and a user helper sharing
  an element's name is checked as that element, which is `B0218`'s existing limit. The escape hatch
  is `a11y_exempt="reason"`, stripped before the page is emitted; §12.4 asked for
  `@a11y(exempt, reason=…)` and is corrected in place, because an annotation inside a `ui:` block
  would be new parser syntax for one hatch. Gated by three `tests/ui/` snapshots, the acceptance
  half (every way of naming a control, and the exemption) in `beck-macro`, and the two vocabulary
  gates. This closes the ledger item [`docs/08`](../docs/08-roadmap.md) §8.5.4 scheduled behind the
  `ui:` vocabulary's **G**, which is that ordering paying for itself. One more stale figure fell out
  of it: §12.3 said **137** diagnostic codes and there are **145**, three of them these — the same
  decay direction as the corpus-wide numbers below, in the document that states the rule about it.
