# 0010 — Numeric operators resolve through a prelude trait

**Supersedes [0008](0008-numeric-operators-resolved-ad-hoc.md).**

**Context.** 0008 resolved `+`, `-`, `*` and `/` from their operands and was explicit that this was
temporary: it rejected a built-in `Num` constraint because "it would have to be special-cased in
unification, in `.becki` publication and in `--wire-compat`, and every one of those is a place a
real trait system will have to go." Those places now have one — traits
([`27`](../27-the-walls-come-down-report.md)), bounds ([`27`](../27-the-walls-come-down-report.md)) and a module boundary
([`27`](../27-the-walls-come-down-report.md)) — so the reason for the refusal has expired. The
consequence 0008 named is what forced the issue: "a user's own numeric type cannot join the
resolution at all", which is `sicp/refusals/rational.beck` and SICP §2.1.1.

**Decision.** A prelude trait `Num`, with SICP §2.5.1's four operation names — `add`, `sub`, `mul`,
`div`. An operator whose operands are neither `Int` nor `Float` nor `Str` resolves through it. The
argument for the shape is the book's: §2.5.1 builds generic arithmetic by hand as operations each
type installs an implementation for, and that is a trait.

Three options were written out in [`27`](../27-the-walls-come-down-report.md) §27.10 and this is the first.
Rejected: **a name the compiler knows and the program declares** — cheaper by a day, and it makes a
program that declares an unrelated `Num` behave strangely for reasons nothing explains. Rejected:
**operators stay closed**, with §2.1.1 written as `add_rat(x, y)` — which is the reading the refusal
file argued against, because a language whose third numeric floor reads differently from its first
two has not abstracted anything.

`Num` is built as a `TraitSig` in `prelude.rs` and enters through `import_traits`, the path
[`27`](../27-the-walls-come-down-report.md) built for imported traits. There is no prelude source
to parse and no span belonging to a file that does not exist.

**Consequences.** Dispatch happens only where there is an implementation to dispatch to, so the rule
0008 set is unchanged everywhere it already had an answer: `1 + true` is still a mismatch, `1 + 1.0`
is still refused, and an operand that is still a unification variable still defaults to `Int`. What
changes is a *declared* type with no implementation, which now names the cure rather than the
symptom.

`Int` and `Float` do not implement `Num`; they go through the primitives. A tower whose ground floor
is a dictionary call would make every existing program slower to prove a point.

0008's other consequence stands: no signature says "either numeric tier", so `sicp/ch1.beck` still
carries `square` and `square_real`. `Num` is one trait and not a tower with coercion — SICP §2.5.2's
raising an `Int` to a `Rational` is not built, for the same reason 0008 refused promotion.

The prelude now contains a trait, which is a new kind of thing to argue about. What earns a place
there is not written down, and [`27`](../27-the-walls-come-down-report.md) §27.1 item 2 says the next
addition should have to answer for it.

Full argument: [`docs/27-the-walls-come-down-report.md`](../27-the-walls-come-down-report.md).
