# 0008 — Numeric operators are resolved from their operands, not from a type class

**Superseded by [0010](0010-generic-arithmetic-through-a-prelude-trait.md).**

**Context.** Adding reals meant `+`, `-`, `*`, `/`, unary `-` and `abs` had to work at two numeric
types. Beck has no traits — they are parsed and not checked, and have been since Phase 1 — so there
is no `Num` to bound a type variable by. A `(a, a) -> a` scheme would let `Bool + Bool` typecheck,
which is why Phase 1 already resolved `+` ad hoc to pick `Str` concatenation.

**Decision.** Extend that resolution rather than introduce a partial type class. Whichever of the
two operands and the expectation first resolves to `Int` or `Float` decides the operator; an
expression with nothing known about it defaults to `Int`, so programs written before reals existed
mean what they meant. Mixing the tiers is an error, not a promotion — no implicit widening.

Rejected: a built-in `Num` constraint on type variables. It would have to be special-cased in
unification, in `.becki` publication and in `--wire-compat`, and every one of those is a place a
real trait system will have to go. A closed list pretending to be a type class is the thing that
makes traits harder to add later.

**Consequences.** No signature can say "either numeric tier", so a function needed at both is
written twice — `sicp/ch1.beck` carries `square: Int -> Int` and `square_real: Float -> Float`, and
says why where a reader will see it. A user's own numeric type cannot join the resolution at all,
which is what `sicp/refusals/rational.beck` records: SICP §2.1.1's exact rationals are blocked on
traits rather than on arithmetic. `abs` referenced *as a value* rather than applied gets the `Int`
form, because there is no operand to resolve from, and nothing diagnoses that.

Full argument: [`docs/32-numeric-tower-and-polymorphism-report.md`](../32-numeric-tower-and-polymorphism-report.md)
§32.3 and §32.9.
