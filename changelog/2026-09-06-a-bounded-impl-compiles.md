- **2026-09-06 — An `impl` whose type parameter carries a bound compiles, and its calls do too.**
  `DEFECTS.md::a-bounded-impl-parameter-is-refused-by-the-compiler-that-suggests-it`, deleted here.
  `impl[T: Ord] Ranked for list[T]` was refused with `B0310: cannot find type `T`` pointing at the
  `list[T]` in its own header — and the compiler *asks for it*: the unbounded form reports `B0386`
  with `help: bound it: `[T: Ord]``, so following the advice produced an error about a type
  parameter three characters to the left. Three things, and the third is why the entry existed
  rather than a half-fix: `typaram_names` and `bind_decl_typarams` read a parameter with
  `Node::as_var`, which answers `None` for the `(annot T Ord)` a bounded one parses to, so the
  parameter was dropped from scope; `expand_bounds` ran over the items **as written**, and an
  impl's methods do not exist until `expand_impls` has synthesised them, so a method that got its
  parameter back still had no dictionary to call through; and `trait_call` applied the impl's
  mangled global directly, so an impl that compiled was then called with one argument too few.
  The third is dispatch — learning what `T` is means matching the impl's declared target against
  the receiver's type — and it is
  [`check/dispatch.rs`](../compiler/crates/beck-core/src/check/dispatch.rs), a new file, because
  [`docs/08`](../docs/08-roadmap.md) §8.5.5 keeps Lane A serial over `check/mod.rs` and `ty.rs`
  and names putting new logic beside them as the mitigation that works. `ty.rs` is untouched and
  `check/mod.rs` gains one `mod` line and two replacements smaller than what they replace.
  A dictionary whose own implementation is bounded is closed over rather than passed raw, so
  `impl[T: Show] Show for list[T]` composes at `list[list[Int]]`.
  The gate is `bounded_impls.rs`, both halves, and the second is the one that would have been
  forgotten: the suggested program **runs**, at a concrete element type — and the same program with
  the bound removed still reports `B0386` and still suggests the bound, because a fix that let an
  unbounded parameter satisfy every trait would pass the first half and delete the check.
  `traits.rs::a_declaration_cannot_bound_its_type_parameter` gained the smaller half: `model
  Box[T: Show]` is refused for the bound, and no longer *also* told that `T` is a type nobody
  declared.
