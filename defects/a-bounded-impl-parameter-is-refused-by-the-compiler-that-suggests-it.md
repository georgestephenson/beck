## `a-bounded-impl-parameter-is-refused-by-the-compiler-that-suggests-it`

**What is wrong.** `impl[T: ToJson] ToJson for list[T]` — an impl whose type parameter carries a
bound — is refused with `B0310: cannot find type \`T\``, pointing at the `list[T]` in the impl's own
header. The unbounded form `impl[T] ToJson for list[T]` is accepted, so the bound is what breaks it.

**Why it is a defect rather than an absence.** The compiler *tells you to write it*. An unbounded
impl whose method calls a trait method on `T` reports `B0386: \`T\` is not known to implement
\`ToJson\`` with `help: bound it: \`[T: ToJson]\``, and taking that advice produces a different
error about a type parameter that is written three characters to the left. A suggestion that does
not compile is worse than no suggestion: it reads as the compiler contradicting itself, and the
person following it has no way to tell which of the two messages is the true one.

**What is actually broken, in three parts**, found while writing §2.4's `derive` and each confirmed
by fixing it in isolation:

1. `check/mod.rs`'s `typaram_names` and `bind_decl_typarams` read a parameter with `Node::as_var`,
   which answers `None` for the `(annot T ToJson)` a bounded parameter parses to — so the parameter
   is dropped from scope entirely. `check/traits.rs::typaram_name` is the function that reads both
   and is not used by either.
2. `expand_bounds` — the rewrite that turns a bound into a dictionary parameter — runs over the
   items **as written**, and an impl's methods do not exist until `expand_impls` has synthesised
   them one line earlier. So a method that fixes (1) still cannot call anything through its bound.
3. `trait_call` resolves a method to the impl's mangled global and applies it directly, without the
   dictionary-passing path `BindKind::Global` takes for a bounded `def`. Supplying the dictionary
   means matching the impl's target against the receiver to learn what `T` is, which is a piece of
   dispatch rather than a repair.

The first two are one line each and the third is not, which is why this is written down rather than
half-fixed: an impl that compiles and whose calls cannot is a worse state than the one above.

**The gate a fix owes**, and it has to be both halves. Positive: a program with
`impl[T: Ord] Ranked for list[T]` whose method calls the bound's own method **compiles and runs**,
with a call at a concrete element type. Negative, and this is the half that would be forgotten: the
same program with the bound removed still reports `B0386` and still suggests the bound — because a
"fix" that silently made an unbounded parameter satisfy every trait would pass the first half and
delete the check.
