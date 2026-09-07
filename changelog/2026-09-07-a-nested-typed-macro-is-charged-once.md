- **2026-09-07 — A typed macro nested in its own argument is charged for what it produces, and
  expanded once.**
  `DEFECTS.md::a-typed-macro-nested-in-its-own-argument-is-charged-for-output-nobody-gets`, deleted
  here. The checker infers a call's arguments inside a probe that rolls itself back, then checks
  the code the macro wrote — which contains those arguments — so a call whose argument was another
  typed macro call was expanded twice, nesting `d` deep cost `2^d` expansions, and **every one was
  charged** against F17's module-wide budget. That budget bounds what expansion *produces*
  ([`docs/42`](../docs/42-security-assurance.md) §42.6) and the probe's output is thrown away, so a
  fifteen-deep nest of a macro producing three nodes — forty-five nodes of program — was refused
  for producing ninety-eight thousand, and then told it had a second problem it did not have.
  An expansion is now remembered by **the call it came from and what the body was told about it**
  ([`check/expansion.rs`](../compiler/crates/beck-core/src/check/expansion.rs), a new file: `docs/08`
  §8.5.5 keeps Lane A serial over `check/mod.rs`, which is 78 lines *shorter* for this). The charge
  is per **use**, not per call site, which is the whole difference: a macro writing `[$x, $x]` really
  does produce `2^d` nodes and really does owe them.
  **It took the asymptotics with it**, which the entry did not expect: a probe wants a *type*, and
  the type of a call already answered is known, so a probe that meets one answers from memory rather
  than walking it. The entry's own fixture went from a `B0214` refusal at fifteen deep to 24 ms, and
  sixty deep now compiles in 147 ms. What still falls back to the old walk is an expansion whose type
  is not yet settled — reusing a type holding variables would carry one call site's inference into
  another's.
  Three gates in `macro_bomb.rs`, and two of them are the ones that would have been forgotten. The
  sweep is the shape [`compile_speed.rs`](../compiler/crates/beck-cli/tests/compile_speed.rs)
  established — hold the production per nesting level constant, grow the level, and the charge must
  stay flat — with the fifteen-deep fixture compiling beside it. The doubling typed macro is
  **unchanged and still refused**: memoising the charge per call site instead passes the sweep and
  turns that one red, which is how it was checked. And a bomb inside an argument a macro *discards*
  now compiles in 27 ms rather than being refused — free and exponential is the combination F17
  exists to prevent, so what bounds it is that the walk is linear, not that the meter runs out.
  **And the register emptied for the first time, which broke it.** `defects/` held both entries this
  branch deletes, and git does not store a directory — so the register vanished, along with the two
  documents that link to it and the listing every gate reads. It now holds a
  [`README.md`](../defects/README.md), as [`changelog/`](../changelog/README.md) does, and
  `docs.rs::every_defect_is_a_file_named_for_the_entry_it_holds` has its floor moved from *some
  entry exists* to *the listing found something*: an empty register is the state to be in, and it
  may not read as a broken listing.
