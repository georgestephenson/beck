# 55 — Phase 3, part 24: bignums, and the last floor of the tower

**Built.** Arbitrary-precision integers, written in Beck, in
[`compiler/lib/bignum.beck`](../compiler/lib/bignum.beck) — with the numeric coercions
[`08`](08-roadmap.md) §8.5.4 pairs them with, and no new primitive and no new dependency.

[`25`](25-benchmarks-and-expressiveness.md) §25.7 item 5 ordered the numeric tower: "reals first,
then rationals and bignums". The reals landed in [`32`](32-numeric-tower-and-polymorphism-report.md),
the rationals in [`41`](41-generic-arithmetic-report.md), and this is the floor underneath both. It
is the last untouched item of Wave 2 that is a *number*.

## 55.1 Why it is a file and not a primitive

[`lib/README.md`](../compiler/lib/README.md) draws one line: **a host's table or grammar is a
primitive, composition is a file**. `str_upper` is a Unicode table, `json_parse` is somebody else's
grammar, `digest` is a hash somebody else specified. Schoolbook long multiplication is none of
those. A carry, a borrow and a trial quotient are arithmetic on `Int`, and `Int` is something the
language has.

So the line says this is a file, and taking `num-bigint` would have been faster to run and would
have been the admission the directory exists to avoid making. That argument was easy to write for
`money.beck`, where the alternative was silly. It is worth more here, because here the alternative
was *reasonable*: `num-bigint` is a good crate, and choosing against it is the first time the
division has cost something real.

What it costs is [`55.6`](#556-what-is-not-built), stated rather than discovered.

## 55.2 The representation, and the three decisions in it

**A sign, and a list of base-10,000 limbs, least significant first.**

**Base 10,000** so that two things are true by inspection rather than by argument: a product of two
limbs plus a carry is about 10⁸, nowhere near an `Int`'s range — and a limb is exactly four decimal
digits, so rendering and parsing are *grouping* rather than division. A larger base is faster and
makes both of those a calculation the reader has to trust.

**Canonical form** — no leading zero limbs, and zero is the empty list with a positive sign. That is
what makes `==` value equality rather than a function nobody remembers to call, and it is why
`normalise` is the only constructor: a negative zero would compare unequal to a positive one, so a
library whose zero has two values is a library whose tests pass and whose folds do not.

**A magnitude layer that knows nothing about signs.** Everything from `add_limbs` to `divmod_limbs`
works on unsigned lists, and `sub_limbs` is only ever called with the larger first. A sign rule
applied halfway through a borrow is the classic way to get subtraction wrong, and the way not to is
to have nowhere to apply it.

`impl Num for Big` puts it in the tower: `+`, `-`, `*` and `/` are SICP §2.5.1's four method names,
resolved through the prelude trait ([`41`](41-generic-arithmetic-report.md)). It is the **third**
floor added from outside the compiler after the rationals and money, and the first where the type is
a number in the ordinary sense — which matters, because a tower whose new floors can only ever be
domain types is not a tower.

Only `div` can fail, so `raises(BigError)` is inferred from it alone and `a + b` stays pure. That is
[`47`](47-effect-polymorphic-traits-report.md)'s per-impl row doing precisely the job it was built
for, and the checker enforces it in the direction that is easy to get wrong: a `try:` over an
expression that cannot fail is `B0392`, so the absence of `try:` on the addition lines is checked
rather than tidy.

## 55.3 Coercion, and the decision not to make it implicit

§8.5.4 pairs "bignums" with "numeric coercion", and the coercion half is a **decision** more than it
is code. Every conversion is a named function and none of them is implicit:

| | |
|---|---|
| `big(n)` | an `Int` as a `Big`. **Total**, including the most negative `Int` |
| `big_to_int(b)` | `Option[Int]` — narrowing is a question with a "no", not a failure |
| `big_to_real(b)` | lossy, and a different function so a reader has to mean it |
| `big_of_str` / `render_big` | text, with the reader raising per [`46`](46-standard-library-report.md) §46.2 |

[`41`](41-generic-arithmetic-report.md) refuses `1 + 1.0` on purpose, and adding a bignum is exactly
the moment that decision comes under pressure — an `Int` that silently widened on overflow would be
convenient. It would also make the cost of arbitrary precision invisible, and `Int` arithmetic is
checked (`docs/32` §32.3) *precisely* so that overflow is a message rather than a wrong answer.
Silent widening would trade a loud, cheap failure for a quiet, expensive success. So: no.

The one detail worth reading the code for is **the most negative `Int`**, which is where a
conversion written the obvious way has exactly one input it cannot take. `abs(n)` of it overflows —
there is no positive `Int` that large — so `big` peels limbs off the *signed* value, relying on `%`
following the sign of the dividend, and `big_to_int` accumulates *negatively* for a negative value
rather than negating at the end. Both directions round-trip. It is also, incidentally, a number with
no literal: `9223372036854775808` is not an `Int`, so the text of it is not a token, and the test
reaches the value by arithmetic on both sides.

## 55.4 How it is known to be right

Three layers, weakest first, and the third is the one that matters.

**Constants.** 25!, 30!, 2⁶⁴, 2¹⁰⁰, 2¹²⁸ and a thirty-digit product. 20! is checked against the
language's own `Int` arithmetic because it is the largest factorial an `Int` holds; the rest were
computed by Python's arbitrary-precision integers, which is an outside oracle and is said to be one.

**`property` blocks — this implementation against the one the language already has.** For every pair
the generator produces, `big` arithmetic and `Int` arithmetic agree on the sum, the difference and
the product; division and remainder reassemble the dividend; and an `Int` round-trips through a
`Big` and through its own rendering. That checks the carries, the borrows and the sign rules on
inputs nobody chose, and it costs three declarations.

**A cross-check against `i128`, over 400 pairs.**
`stdlib.rs::a_bignum_multiplies_and_divides_the_way_i128_does` builds operands around 10¹⁸ — past
`Int`, so the `property` blocks cannot reach them — multiplies, divides and takes the remainder of
each pair in Beck and in Rust, and compares the rendered decimal. 1,200 assertions.

Be exact about what that establishes, because [`50`](50-collections-and-dates-report.md) §50.3 was
exact about the same shape for the calendar and the limit is the same one: **it is not two
independent algorithms.** It is one claim — "this is the product of those two integers" — checked
against a different implementation of it, on a different evaluator, in a different language. What it
catches is a transcription error, a carry dropped at a boundary, and a sign rule that is wrong in a
case nobody wrote a constant for. What it does not establish is that the algorithm is right in
general; that is somebody else's proof, and long multiplication has one.

The generator is a fixed xorshift rather than a random source, because a cross-check that fails on
Tuesdays is a cross-check nobody keeps.

## 55.5 One finding, and it is a comment rather than a language

The parser produced a wrong number, and the cause was not the parser.

```beck
group = str_slice(digits, start, at)
```

**`str_slice`'s third argument is a count, not an end index.** `str_slice("héllo", 1, 1)` is `"é"`.
The signature is `(Str, Int, Int) -> Str` and reads either way, the compiler's own comment beside it
did not say, and the generated reference cannot say — a primitive's parameters have no names in it,
which is a gap in [`34`](34-generated-documentation-report.md)'s reference rather than an oversight
in this file.

Nothing had ever exercised the ambiguity. **Every other call site in the tree passes a start of zero
or a count that clamps to the end of the string**, and under both readings those give the same
answer. This is the first caller with a non-zero start and a real count, and it got a plausible
wrong number rather than an error — which is the worst available outcome and the reason this is
written down.

Worse, the comment that *was* there said the wrong thing about a second question:

> Indices are byte offsets into UTF-8

They are **characters** — Unicode scalar values. `str_len` counts `chars`, `str_slice` skips and
takes `chars`, and `str_index_of` converts a byte offset to a character index with its own comment
saying it does so "to agree with `str_len` and `str_slice`". The behaviour is right, it is
consistent, and `stdlib.rs::string_positions_are_characters_everywhere_or_nowhere` has asserted it
since [`46`](46-standard-library-report.md). The sentence three lines above the code contradicted a
passing test.

So the fix is a comment, and the finding is about comments: this is the **second** documentation
defect this week that nothing could catch — [`53`](53-are-we-fast-yet-report.md)'s rustdoc link
depths were the first — and both were wrong in the same way, by being *plausible*. The link-depth
one now has a gate. This one does not, and cannot easily get one: no test can check that a sentence
describes the code beside it. What it can have, and now has, is the sentence stating the thing a
signature cannot.

## 55.6 What is **not** built

| | Status |
|---|---|
| Sub-quadratic multiplication | **not built.** This is schoolbook: multiplication is O(n²) and division is O(n²) with a fourteen-step binary search inside each limb. No Karatsuba, no Toom-Cook, no FFT. Fine for the hundreds of digits a program actually asks for; wrong for cryptography, which is not what this is for — [`52`](52-crypto-and-identifiers-report.md)'s digests are the host's |
| A number for any of it | **none**, and [`46`](46-standard-library-report.md) §46.6's reason is unchanged: the tree-walker is 33× CPython, so a measurement of this would measure the interpreter. `beck test lib/bignum.beck` takes 0.85 s in a debug build, which is a fact about the gate and not about the library |
| Knuth's algorithm D | **not built**, and this is the thing to replace first if any of it is ever a bottleneck. The binary search for a trial digit is fourteen comparisons where the estimate-and-correct is one multiply and a rare fixup; it is here because it is *obviously* right, which for a first division is the trade to make |
| Arbitrary-precision **decimal** | **not built**, and it is now the only thing left of §8.5.4's Wave 2 list. `money.beck` is fixed-scale integer money and says so; a `Decimal` is a `Big` mantissa and a scale, which this file makes possible and does not contain. Its one real design question is what `/` does when the quotient does not terminate |
| A rational over `Big` | **not built.** `sicp/ch2.beck`'s `Rational` is over `Int`, so `one_third() + one_third()` is exact and a long chain of them still overflows. Rewriting it over `Big` is now expressible; whether SICP's chapter should be the thing that does it is a separate question |
| `Ord` for `Big` | **not built**, deliberately — `compare_big` is a function. [`54`](54-ordering.md) recommends against the trait, and `Big` is the exact case that document is about: `<` on it would compare the **sign flag** first and answer backwards, so the library exposes an ordering rather than relying on the derived one |
| Bit operations, `gcd`, modular arithmetic, primality | **not built.** [`53`](53-are-we-fast-yet-report.md) §53.5 records that Beck has no bitwise operators at all, so a shift here is `mul_small` by a power of two |

## 55.7 What this corrects

- **[`08`](08-roadmap.md) §8.5.4's Wave 2 loses another item.** "Arbitrary-precision decimal,
  bignums and numeric coercion" was the untouched remainder after
  [`52`](52-crypto-and-identifiers-report.md); the bignums and the coercion are built, and the
  decimal is what is left.
- **[`46`](46-standard-library-report.md) §46.6's last "untouched" row moves.** "Bignums and numeric
  coercion — untouched, and §8.5.4 puts them here" is now built.
- **[`25`](25-benchmarks-and-expressiveness.md) §25.7 item 5 is discharged.** "The numeric tower —
  reals first, then rationals and bignums" is complete: [`32`](32-numeric-tower-and-polymorphism-report.md),
  [`41`](41-generic-arithmetic-report.md), and this.
- **[`54`](54-ordering.md) gains its first concrete instance.** `Big` is a type whose derived order
  is wrong — the sign flag sorts before the magnitude — and the answer taken here is §54's
  recommendation rather than the trait: expose an ordering function, and say why in the code.
- **The prelude's comment on string positions was wrong**, per §55.5. Corrected in
  `prelude.rs`; the behaviour it describes never changed.

## 55.8 What Phase 3 is still not

Unchanged from [`53`](53-are-we-fast-yet-report.md) §53.8 except where this touches it. The
standard-library bullet is now everything *except* arbitrary-precision decimal, plus a benchmark
harness that is nine-fourteenths of one suite. The exit criterion — an outside developer building a
non-trivial app from documentation alone — is not met and is not closer.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
