## `a-published-interface-naming-a-trait-cannot-be-read-back`

**What is wrong.** `beck iface` writes a `.becki` that `beck check` then **refuses**. Any module
that implements a trait declared elsewhere, or publishes a bounded `def`, produces a contract the
compiler cannot read:

```beck
# priced.beck
trait Priced:
    def price(self) -> Int
```
```beck
# goods.beck
import priced

model Book:
    cost: Int

impl Priced for Book:
    def price(self):
        return self.cost

def total[T: Priced](xs: list[T]) -> Int:
    return list_sum(map_list(xs, lambda x: x.price()))
```

```console
$ beck iface goods.beck
wrote goods.becki
$ beck check root.beck            # root.beck imports goods and priced, in either order
error[B0383]: cannot find trait `Priced`
  --> goods.becki:10:1
   |
10 | impl Priced for Book
   | ^^^^
error[B0383]: cannot find trait `Priced`
  --> goods.becki:13:10
   |
13 | def total[T: Priced](xs: list[T]) -> Int
   |          ^^^^^^^^^^^
```

Delete `goods.becki` and the same program compiles and runs from source. The interface is what
downstream is *supposed* to see ([`project.rs`](../compiler/crates/beck-core/src/project.rs): "the
published interface, if one is checked in, is what downstream sees — not what this module happens
to compile to today"), so checking one in makes the program stop building.

**Why it is a defect rather than an absence.** The tool refuses its own output. `beck iface` is the
command §3.6's separate compilation is built on and the thing `docs/86` tells a reader to run, and
there is no spelling of these two declarations that survives the round trip — the file it writes is
the file it rejects.

**Where it is.** [`iface.rs`](../compiler/crates/beck-core/src/iface.rs)'s `Interface::parse`
checks the `.becki` **as a module with no imports**:

```rust
let program =
    crate::check::check_module_with(&node, crate::check::Mode::Interface, &[], diags);
```

so `expand_impl` and the bound resolver look for `Priced` in an empty trait table and report
`B0383`. The writer is the other half: a `.becki` carries no `import` line, so even a non-empty
list would have nothing to say which module to take the trait from. Both halves have to move — the
writer emitting what the contract depends on, or the reader resolving a trait name against the
project the way [`check/mod.rs`](../compiler/crates/beck-core/src/check/mod.rs)'s import passes now
do.

Not to be confused with the ordering defect this was found beside, which was in the *reader* of an
interface's impls and is fixed: there the trait was declared in the program and reached too late,
and here it is never reachable at all.

**The gate a fix owes.** Positive: `beck iface goods.beck` followed by `beck check root.beck`
compiles and runs, with the `.becki` checked in and in **both** import orders — and the method
still dispatches, because an interface that parses but drops the impl would pass a compile-only
gate. Negative, and it is the half that would be forgotten: a `.becki` naming a trait that the
importing program genuinely does not have still reports something, rather than the fix turning
`B0383` into silence — `B0388` is what an unresolvable trait means once the reader can resolve the
resolvable ones.
