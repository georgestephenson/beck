- **2026-08-14 · #54 — `str_trim`, `str_split` and `str_chars` compile**, and both old refusals
  were wrong about their own reason — `White_Space` is 25 code points, not case mapping's table,
  and "two loops" is what makes a split cheap
  ([`docs/93`](../docs/93-the-native-backends-report.md)). `examples/todo.beck` is the first program
  in the tree to compile whole. 812 → 850 across the two rounds; the text differentials reach
  4,872 calls, all three backends agreeing.
