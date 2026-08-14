# The same four benchmarks, in six languages

[`docs/25`](../../docs/25-benchmarks-and-expressiveness.md) §25.9 rule 2 held every comparative
claim back "until the second backend exists". It exists ([`docs/93`](../../docs/93-the-native-backends-report.md)),
and this directory is the comparison — the first place in this repository where a Beck number is
put beside another language's.

It is a **microbenchmark suite, and a narrow one**. Read the caveats before the table.

| | |
|---|---|
| [`bench.beck`](bench.beck) | The original. Read by `measure_native.rs` *and* `measure_xlang.rs`, so the two harnesses cannot disagree about what was measured |
| [`bench.c`](bench.c) | C. Built twice: wrapping `int64_t`, and `__builtin_*_overflow` for Beck's semantics |
| [`bench.rs`](bench.rs) | Rust, `checked_*` — Beck's semantics in a safe language |
| [`bench.js`](bench.js) | JavaScript, warmed before timing |
| [`bench.py`](bench.py) | Python |
| [`bench.rb`](bench.rb) | Ruby |
| [`escapes_variants.c`](escapes_variants.c) | Not a port: the diagnostic that says whether a gap on the mandelbrot is *code generation* or *semantics*, by changing only the semantics with the compiler held fixed |

Run it with `cargo test --release --test measure_xlang -- --nocapture`. A language the machine does
not have prints a skip and is left out of the table.

## The rules these ports are held to

1. **The answers must agree.** Every implementation computes `832040`, `500000500000`, `3688` and
   `2220064`, and `measure_xlang.rs` **asserts it**. That is the gate; the times are only printed
   ([`docs/13`](../../docs/13-testing.md) §13.7 — a timing threshold on a shared runner cannot be
   held honestly). It is also what makes the ports trustworthy: six independent implementations
   agreeing on four answers is a much stronger statement than six files that look similar.
2. **Same algorithm, idiomatic spelling.** `sum_to`, `xor_sweep` and the mandelbrot's three drivers
   are tail-recursive in Beck and are **loops** in every port, including the C one — that is what a
   tail call is, and Python and Ruby cannot recurse a million deep. `fib` stays recursive
   everywhere, because tree recursion is the point of that one.
3. **Process startup is excluded, everywhere.** Each port times the computation from inside itself
   and reports a median. Beck's number *includes* its pipe round trip, because that is a real cost
   of every call into this backend; the round trip is printed on its own line so it can be
   subtracted.
4. **A number is not a language.** Every row says what its integer arithmetic actually is, because
   the column is not comparing like with like.

## What this does not measure, stated first

- **The scalar subset only.** These are arithmetic and recursion over `Int`, `Float` and `Bool` —
  which is the whole of what the native backend compiles, and therefore the most flattering ground
  it has. A Beck program with a record or a list in it still runs on the tree-walker;
  [`awfy/`](../awfy/README.md) and [`clbg/`](../clbg/README.md) are the whole-program suites and
  they measure exactly that.
- **Different integer semantics down the column.** Beck, Rust and checked C detect overflow;
  wrapping C wraps; JavaScript uses `f64` (every value here is under 2⁵³, so it is exact, but it is
  not integer arithmetic and there is nothing to check); Python and Ruby use arbitrary-precision
  integers, which is *more* work than a machine word. Only the middle three are like-for-like with
  Beck, and the wrapping-C row is there to price the checking rather than to be beaten.
- **One machine, one run.** These are wall-clock medians on whatever ran them. A number here is
  comparable to another number from the same run and to nothing else — the discipline
  [`awfy/README.md`](../awfy/README.md) already applies.
- **Nothing about compile time**, deployment, memory or startup. `measure_native.rs` prints what
  `clang -O2` costs; nothing here does.

## What `escapes_variants.c` is for

The mandelbrot loop was once 3× slower in Beck than in C, and the useful question was *which* 3×.
That file answers it by holding the compiler fixed and changing only the semantics: plain IEEE, the
order-key comparison [`docs/27`](../../docs/27-the-walls-come-down-report.md) §27.8's
structural equality on reals requires, and the per-operation normalisation the backend used to
emit. When the `key` row lands on Beck's number, the code generation is at parity with clang and
what is left is the price of the language's own rule about reals — which is a design question, and
a different one from "the backend is slow".
