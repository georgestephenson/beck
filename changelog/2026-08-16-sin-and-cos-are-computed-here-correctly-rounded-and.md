- **2026-08-16 — `sin` and `cos` are computed here, correctly rounded, and no longer the host's.**
  IEEE 754 requires `sqrt` correctly rounded and requires **nothing** of the transcendentals, so
  three backends reaching three platform libms meant a `durable` fold that computed a sine could
  replay to a different state on a different machine — the one thing
  [`docs/10`](../docs/10-decisions.md) D3 rests the data tier on. `beck_prim::math` computes them
  instead, and every backend calls it: the evaluator directly, the two native emitters through a
  new `beck_prim_f64` entry point that carries no arena because a function from a double to a
  double allocates nothing. The answer is **correctly rounded**, which makes the specification the
  mathematics rather than a vendored file — a later rewrite cannot change a bit of any replay — and
  the implementation performs **no rounded floating-point operation at all**: exact integer
  reduction over 1472 bits of 2/π, an integer series, one rounding at the end. Measured
  (`cargo test -p beck-prim --release --test transcendentals -- --nocapture`): ~640 ns a call
  against a platform libm's 11 ns, and the same cost at `10^300` as at 1, which is the shape that
  gate holds; 400 calls per run of `awfy/cd.beck` — the only program in the tree that calls either
  — is 0.1% of it. Ziv's fast path in front of it is
  [`docs/08`](../docs/08-roadmap.md) §8.5.4 and changes no answer. Gated by
  `beck-prim/tests/transcendentals.rs`, which recomputes 4,000 arguments at 1408 bits by a
  deliberately different route — Bailey–Borwein–Plouffe rather than Machin, binary long division
  rather than a window into 2/π, a term recurrence rather than Horner — and by
  `the_host_libm_would_fail_this`, which asserts that **11 of 8,000** of those answers are ones
  glibc does not give, so a change back to `f64::sin` goes red rather than unnoticed; plus a
  structural gate per backend (`native.rs`, `cranelift.rs`) that the module names the library and
  no libm symbol. Closes F9 ([`docs/14`](../docs/14-review-findings.md)) and
  `DEFECTS.md::libm-determinism`; [`adr/0031`](../docs/adr/0031-transcendentals-are-computed-here-and-correctly-rounded.md).
