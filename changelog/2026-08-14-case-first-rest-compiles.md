- **2026-08-14 · #55 — `case [first, *rest]` compiles**, on both code generators — the last
  pattern form they refused, with the length tested before any element is read and the tail copied
  as the evaluator copies ([`docs/93`](../docs/93-the-native-backends-report.md)). Its old refusal had
  been false for three reports, and the corpus pass now holds every refusal against a list of
  sentences the backend may no longer say about itself. 889 → 905; refusals 189 → 173.
