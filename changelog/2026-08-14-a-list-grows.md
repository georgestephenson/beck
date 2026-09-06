- **2026-08-14 · #54 — A list grows**: `list_append` compiles via an immutable header over a
  shared data block, sound by the shape of the writes rather than by ownership analysis
  ([`docs/93`](../docs/93-the-native-backends-report.md)). 711 → 895 — the largest jump of these
  rounds — and refusals 707 → 523. Gated by `an_appended_accumulator_is_linear` and the
  differential's `forked` case.
