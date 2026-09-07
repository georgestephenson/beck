## `an-imported-impl-is-visible-only-if-its-trait-was-imported-first`

**What is wrong.** Whether an imported `impl` can be called depends on the **order the `import`
lines are written in**. Three modules — one declaring a trait, one implementing it for its own type,
one calling the method — compile or do not compile depending on which of the first two the third
names first:

```beck
# vocab.beck
trait Labelled:
    def label(self) -> Str
```
```beck
# thing.beck
import vocab

model Thing:
    n: Int

impl Labelled for Thing:
    def label(self):
        return str(self.n)
```
```beck
# root.beck
import thing
import vocab

def shown(t: Thing) -> Str:
    return t.label()
```

`beck check root.beck` reports two errors — `B0320: return type mismatch: expected \`Str\`, found
\`Unit\`` and `B0387: \`Thing\` does not implement \`Labelled\``, the second labelled
"\`label\` needs an \`impl Labelled for Thing\`". Swap the two `import` lines and it compiles.
Nothing else changes.

**Why it is a defect rather than an absence.** Three ways over.

1. `beck iface thing.beck` **publishes the impl** — `impl Labelled for Thing` is a line in the
   contract — so the diagnostic asks for something the module it names already offers, and offers
   in the file the compiler read.
2. Nothing gives `import` an order. D23 fixes where a name resolves *from* (the root's directory,
   then the standard library) and says nothing about the sequence, because a module's contract is
   derived from its body rather than from its position.
3. It is silent. The impl is dropped with no diagnostic at the point it is dropped, and the error
   surfaces one module later as an absence.

**Where it is.** `check/traits.rs::import_traits` registers an imported module's traits and then its
impls, and skips an impl whose trait it has not seen:

```rust
for i in impls {
    let head = i.head();
    let Some(decl) = self.traits.get(&i.trait_name).cloned() else {
        continue;
    };
```

`check/mod.rs` calls it once per import, in the order `project.rs` collected them — which is the
order `imports_of` read them out of the file. So a module whose impls name a trait declared in an
import that has not been reached yet loses them. The shape of the fix is two passes over all the
imports, traits before impls, rather than one pass per module; the `continue` then means what it
looks like it means, which is "no such trait anywhere", and deserves a diagnostic.

This is what `lib/json.beck` meets in practice: a program that puts `import json` after an
`import` of its own domain module cannot call `to_json()` on anything that module derived, and
`docs/86` §86.9's two-file program is written in the order that works.

**The gate a fix owes**, and it is the ordering that has to be gated rather than the call. Positive:
the three modules above compile **with the imports in either order**, and the method dispatches to
the same implementation in both — a fix that only made one order work has moved the bug rather than
removed it. Negative: a program importing a module whose impl names a trait **nothing** declares
still fails, and now with a diagnostic naming the trait, because the `continue` above is currently
carrying two meanings and only one of them is a bug.
