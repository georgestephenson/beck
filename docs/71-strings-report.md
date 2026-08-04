# 71 — Phase 3, part 40: a string that knows its own length

**Built.** `Value::Str` carries its character count and whether it is ASCII
([`core::Text`](../compiler/crates/beck-core/src/core.rs)), and holds a `String` rather than an
`Arc<str>` so that `+` can push into it. **Both of the quadratics
[`70`](70-last-use-moves-report.md) §70.6 measured are gone**: building text and walking it by
character index are linear.

That section named this as what it did not fix:

> `Value::Str` is an `Arc<str>`: no spare capacity, and `+` is `format!("{x}{y}")`, so it allocates
> and copies both sides. `str_len` is `chars().count()` and `str_slice` skips characters, so both
> are `O(n)` in the string rather than in the answer. […] The fix is a representation […] That is a
> change of its own, and it is the largest performance item this project now knows about.

## 71.1 The representation

```rust
pub struct Text {
    bytes: String,   // owned, with capacity: `+` can push into it
    chars: usize,    // characters, not bytes — `str_len`'s answer, in O(1)
    ascii: bool,     // every character is one byte, so a character index is a byte index
}
```

Both facts are computed **once, when the string is built**, and that is work the construction was
already doing: it had to copy the bytes, and `is_ascii` is a scan of the same bytes that answers
`chars` for free when it says yes. A non-ASCII string pays one more pass, once, instead of paying it
on every `str_len`.

`Text` compares, orders and hashes **as its bytes**, which is not decoration: a `Map` keyed by
strings has to keep its order and the state digest a replay reproduces has to keep its value, so the
two cached fields must be invisible to every question a program can ask.

Three primitives change, and nothing else does:

| | before | after |
|---|---|---|
| `a + b` | `format!("{a}{b}")` — allocate, copy both sides | `push_str` into `a` when [`70`](70-last-use-moves-report.md)'s analysis proves nobody else holds it, and the copy otherwise |
| `str_len(s)` | `chars().count()` — `O(n)` | the cached count — `O(1)` |
| `str_slice(s, i, n)` | `chars().skip(i).take(n)` — `O(i + n)` | a byte range when the string is ASCII — `O(n)`; the old walk when it is not, because nothing else is possible without an index nobody has asked to pay for |

`+` pushing in place is the *same* mechanism as `list_append`: `Str` joins `List` and records in
`worth_moving`, so a last read hands the string over and `Arc::try_unwrap` finds it unshared. Neither
half works alone — [`70`](70-last-use-moves-report.md) built the analysis and this gives it a
second thing to be useful for.

## 71.2 What it buys

Release build, median of three, startup subtracted. **Per doubling**, which is the number that says
what shape it is:

| | before | after |
|---|---|---|
| building a string by `+` (n = 8,000 → 64,000) | ×2.09, ×2.64, **×3.47** — heading for ×4 | **×2.01, ×2.01, ×1.93** |
| at n = 64,000 | 232 ms | **86 ms** |
| walking a string by index (n = 4,000 → 32,000) | ×2.16, ×2.44, **×2.71** | **×1.87, ×1.97, ×1.99** |
| at n = 32,000 | 197 ms | **96 ms** |

Both are linear now, and the constant fell too: in a debug build the scanning loop costs **23,802 ns
a character against 52,414** at n = 1,000, because `str_len` in the loop guard stopped walking the
string.

**On today's programs it is neutral**, which is the same story as §70.4 and for the same reason —
the strings in this tree are small and the ones that are not are built by `str_join`, which was
always linear:

| | before | after | |
|---|---|---|---|
| `awfy/json.beck` | 0.376 s | 0.366 s | −2.7% |
| `lib/bignum.beck` | 0.261 s | 0.257 s | −1.5% |
| `clbg/pidigits.beck` | 4.378 s | 4.385 s | +0.2% |
| `clbg/revcomp.beck` | 2.457 s | 2.470 s | +0.5% |
| `clbg/fasta.beck` | 1.284 s | 1.294 s | +0.8% |
| `clbg/knucleotide.beck` | 2.135 s | 2.197 s | +2.9% |

`fasta` is the interesting row: it produces 10,245 characters and does not move, because it builds
them with `str_join`. A program that reaches for the obvious `done + piece` instead used to fall off
a cliff and now does not — which is the whole point, and is worth more than a percentage.

## 71.3 The gate, and the size it had to reach

`scaling.rs::text_costs_the_same_per_character_however_long_it_gets`, beside the fold's and the
accumulator's, and a **shape** rather than a rate for [`13`](13-testing.md) §13.7's reason.

The first version compared 1,000 characters against 8,000, the same 8× spread the other two gates
use, and **passed against the old evaluator** — which makes it not a gate. The reason is worth
keeping: a `memcpy` is fast next to an evaluator step, so a copy per append is invisible until the
string is long enough for the copying to overtake the interpretation. Measured on the old code, the
per-character cost rose ×2.18 over 8× and **×4.07 over 16×**; the fixed one is ×0.71 over the same
16×, because start-up amortises. So the gate spans 16× and keeps the 3× bound, and it was checked in
both directions: it fails at **5.5×** against the old evaluator and passes at 0.71× against this one.

## 71.4 What this corrects

- **[`70`](70-last-use-moves-report.md) §70.6's two string findings are fixed**, and §70.9's "a
  string representation that can be appended to — **not built**" is built.
- **[`70`](70-last-use-moves-report.md) §70.4's list of what a move is worth is longer.** It said a
  move pays for a `List` and a record; a `Str` is the third, and it pays for the same reason.

## 71.5 What is not built

| | |
|---|---|
| A character index for non-ASCII text | **not built.** `str_slice` still walks a string with a multi-byte character in it. The fix is a chunked index, paid for on construction, and nothing in this tree is asking for it yet — every benchmark and every library file here is ASCII |
| A rope | **not built**, and probably never needed: `+` in a loop is linear now, and `str_join` was always the right way to build from parts |
| `str_index_of`, `str_split`, `str_replace` | **unchanged**, and each is `O(n)` in the string because the *answer* is: they search it. `str_index_of` returns a character index and computes it by walking, which is the one remaining place a character index costs more than a byte one |
| A byte string | **not built**, and it is still what `clbg/mandelbrot` needs ([`68`](68-clbg-report.md) §68.6) — `Text` is UTF-8 exactly as `Arc<str>` was |

## 71.6 What this establishes

**That the last-use analysis was worth building for more than one structure.** The list fix needed
it; the string fix needed it *and* a representation, and the two together are what let a pure
language's most obvious loop — `done + piece` — cost what a mutable one costs.

Three quadratics have now been found and removed in this branch: long division's trial-digit search
([`69`](69-standard-library-imports-report.md) §69.6), the list accumulator
([`70`](70-last-use-moves-report.md)), and both halves of text. Each was found by asking what an
operation *should* cost and measuring at two sizes, which is now
[`AGENTS.md`](../AGENTS.md)'s standing rule rather than three separate accidents.
