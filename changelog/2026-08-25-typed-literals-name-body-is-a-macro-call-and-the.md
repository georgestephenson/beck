- **2026-08-25 — Typed literals: `name"body"` is a macro call, and the first one is a date.**
  [`docs/02`](../docs/02-syntax.md) §2.5, [`docs/102`](../docs/102-the-macro-interpreter-report.md)
  §102.10, [`docs/08`](../docs/08-roadmap.md) §8.5.4. A sigil lexes as one token and desugars to
  `name_sigil(raw="body")` — §2.3's table, minus a `span=` argument it promised and nothing could
  consume — so what parses a body is an ordinary macro and nothing in the front end knows a sigil
  exists. The body is **raw** (no escape processed, which is what `regex"^\d{4}$"` needs), it
  carries the span *inside* the quotes so `refuse(msg, raw)` underlines the body rather than the
  expression, and a sigil with no macro behind it names the sigil that was written instead of the
  `_sigil` name nobody typed. Two readers were what was actually missing: `node_lit` reads a
  literal node's value (`node_head` reads a symbol's and refuses a literal, and nothing read the
  other half), and `str_to_int` is now a compile-time builtin — **total, and refuses** — which is
  the answer indexing had already settled for an `Option`-returning primitive in a sandbox with no
  unions. `lib/dates.beck` gains `date"YYYY-MM-DD"`, checked at compile time by `is_valid`, the
  same function `parse_date` calls: `date"2026-02-30"` is a compile error pointing at the ten
  characters. The point is the effect row rather than the parse — `parse_date` raises, so one
  hard-coded date put `raises(CalendarError)` on the signature holding it. Also fixed: a macro that
  refused was reported **twice**, because the failed call was left for the checker to fail to
  resolve; what replaces it is `<refused>`, spelled so no program can write it, carrying a fresh
  type variable so nothing cascades at the call either. F17's second half is closed by the work
  arriving rather than by anything being built for it — a typed literal's parser *is* a macro body
  — and [`docs/42`](../docs/42-security-assurance.md) §42.6, [`docs/43`](../docs/43-threat-model.md)
  §43.4 and [`docs/14`](../docs/14-review-findings.md) say so. §3.5's "no injection / no XSS" row named
  `sql"…"`/`html"…"` as its mechanism and now names the ones that deliver it, which is the finding:
  those two examples are the two things this language already removed the need for. Gates:
  `macro_interp.rs` (the desugaring, the raw body, the caret width inside the literal, the missing
  macro's note, one error not two, both readers refusing, and `date"…"` across a module boundary)
  and `macro_bomb.rs` (a sigil that does not terminate and one that produces too much).
