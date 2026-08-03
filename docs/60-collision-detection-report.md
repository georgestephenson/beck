# 60 — Phase 3, part 29: CD, and two gaps in the prelude

**Built.** [`compiler/awfy/cd.beck`](../compiler/awfy/cd.beck) — the fourth of Are We Fast Yet's five
macro-benchmarks, verified against the suite's own number: **42 collisions** between two aircraft
over 200 frames. Are We Fast Yet is thirteen of fourteen.

Two primitives were added to get there, and they are the more interesting half of this change.

## 60.1 What it is

Two hundred frames of aircraft positions, each frame reduced to a voxel map, with every pair of
motions sharing a voxel tested for a collision by solving a quadratic. It is in the suite because of
what it is built on: a **red-black tree with parent pointers**, rebalanced on insert *and on delete*,
which is the most mutation-shaped data structure anywhere in Are We Fast Yet.

The tree is an **arena** — `Map[Int, RbNode]` with integer handles and `-1` for null — which is
[`57`](57-richards-report.md) §57.2's answer to a mutable object graph applied to a structure that is
nothing but pointers. Every rotation and both fixups are the original's, with `x.parent.left = y`
becoming a rebuilt node in the map.

Two decisions in the port are worth naming.

**The tree is not generic, and that is a port decision rather than a limitation.**
`RedBlackTree<K extends Comparable<K>, V>` is instantiated at three value types in one program.
A generic tree *is* expressible in Beck — [`36`](36-parameterised-types-report.md)'s parameterised
types and [`39`](39-bounds-report.md)'s bounds are exactly that — and the port uses a `union Key` and
a `union Held` instead because one program's worth of variants says in one place what three
instantiations would say in three. Said here because a reader holding the Java beside it will notice.

**`Vector2D.compareTo` is transcribed with its NaN branch intact.** Its `compareNumbers` ends with
`if (a == a) return 1; return -1;`, which only fires for values that compare equal to nothing. No
position in this benchmark is one. It is here because dropping a branch from a comparator is how a
port stops being one.

## 60.2 The two gaps, which is what the suite was for

`CD.java` needs three things Beck's prelude did not have:

| | |
|---|---|
| `Math.cos`, `Math.sin` | `Simulator.simulate` puts the aircraft on a sine and a cosine |
| `(int)` on a `double` | `voxelHash` truncates a position into a voxel index |

[`32`](32-numeric-tower-and-polymorphism-report.md) built the reals with `sqrt` and nothing else, and
three phases of programs never asked for more. That is not a decision anybody took — a numeric tower
with a square root and no sine is a gap — and neither is the missing half of `float`: there was a
`Int -> Float` and no way back.

So `sin`, `cos` and `trunc` are primitives. All three are the host's in
[`lib/README.md`](../compiler/lib/README.md)'s sense: `sin` is somebody else's polynomial and
truncation is the format's own rule about which way a real falls. `trunc` is toward zero — IEEE
754's `roundTowardZero`, which is what every language with this conversion means — and saturating
rather than wrapping, because a wrap turns a large real into a small integer of the wrong sign.

**This is the third time Are We Fast Yet has found something the language was missing**, after
[`53`](53-are-we-fast-yet-report.md) §53.5's short-circuiting `and` and its bitwise operators. The
pattern is worth stating: a suite written against a *common subset* of language features is a list
of the things every language is assumed to have, and running it is how you find out which of them
you do not.

## 60.3 What it costs

`beck test awfy/cd.beck` is **2.7 s** in a debug build — cheap, because two aircraft is the smallest
configuration the suite publishes a value for and the tree never holds more than a handful of
entries. `CD.java`'s `verifyResult` carries seven sizes; this is the smallest, and the larger ones
are not run.

No comparative claim, unchanged: [`25`](25-benchmarks-and-expressiveness.md) §25.9 holds those until
there is a second backend.

## 60.4 The tree is tested as a tree

Every other benchmark in this directory is verified only through its answer, which is right when the
answer is the benchmark. Here the *structure* is the benchmark, and a red-black tree can be wrong in
a way that still produces 42: a rebalancing that never fires leaves a correct binary search tree with
the wrong shape.

So `cd.beck` also asserts the tree directly — thirty-two keys inserted in an order chosen to force
every rebalancing case, walked in order, then one removed and walked again. It is the only file in
`awfy/` with a test that is not the suite's own, and the reason is that the suite's own test cannot
see the thing this benchmark exists to exercise.

## 60.5 What is **not** built

| | Status |
|---|---|
| DeltaBlue | **not ported**, and it is the last one. About 1,140 lines of Java over twelve files, verifying against **nothing** — its oracle is the assertions inside the planner — over a cyclic object graph |
| The larger CD configurations | **not run.** `verifyResult` publishes seven sizes up to 1,000 aircraft; this runs the smallest |
| A generic red-black tree | **not built**, per §60.1. Expressible, and not what this port needed |
| Trigonometry beyond `sin` and `cos` | **not built.** No `tan`, no inverses, no `atan2`, no `exp`, no `log`, no `pow`. What was added is what the benchmark needed, and the gap this found is wider than what fills it |
| Rounding modes for `trunc` | **not built.** Toward zero only; `lib/decimal.beck` has the three rounding rules for the exact case |
| The CLBG harness | **not stood up**, unchanged |

## 60.6 What this corrects

- **[`58`](58-json-report.md) §58.5's "none of them is blocked by the language" was wrong about
  CD.** It needed `sin`, `cos` and a truncation, and none of the three existed. The sizing was right
  about everything else — the red-black tree was the work — but the blocking question was answered
  from the Java's *shape* rather than from its arithmetic, and arithmetic is where the gap was.
- **[`32`](32-numeric-tower-and-polymorphism-report.md)'s reals gain two operations and a
  conversion**, per §60.2. Nothing about IEEE 754 changed; what changed is how much of it is
  reachable.
- **[`55`](55-bignums-report.md) §55.3's coercion table gains a row.** It lists `Int -> Big`,
  `Big -> Int`, `Big -> Float` and named every one of them as deliberate. `Float -> Int` was missing
  from the language, not from the table.
- **[`08`](08-roadmap.md) §8.4's Phase 3 row moves again.** Are We Fast Yet is thirteen of fourteen.

## 60.7 What Phase 3 is still not

Unchanged from [`59`](59-havlak-report.md) §59.8. The standard-library bullet's library half is done
and its harness half is thirteen of fourteen. The exit criterion — an outside developer building a
non-trivial app from documentation alone — is not met and is not closer.

Seven bullets of the fourteen remain untouched, identity has its seam and not its relying party, and
[`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a time.
