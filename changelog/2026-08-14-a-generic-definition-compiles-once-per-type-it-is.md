- **2026-08-14 · #54 — A generic definition compiles, once per type it is used at** —
  monomorphisation as a shared backend pass, keyed on the whole type, with polymorphic recursion
  and undecided types refused by name ([`docs/93`](../docs/93-the-native-backends-report.md),
  [`docs/38`](../docs/38-literature-survey.md) §38.1). 850 → 870; refusals 223 → 208. Gated by
  `the_two_backends_agree_on_generics` and its Cranelift twin, with instantiations asserted by
  name.
