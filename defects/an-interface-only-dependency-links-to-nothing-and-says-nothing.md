## `an-interface-only-dependency-links-to-nothing-and-says-nothing`

**What is wrong.** A module reached through an `import` that has a `.becki` and **no** `.beck`
compiles, links, and then fails at run time with a bare `no such definition`. The compiler knows
the module has no implementation, has the diagnostic that says so, and does not use it:

```beck
# root.beck
import helper

def four() -> Int:
    return double(2)

test "no traits anywhere":
    expect four() == 4
```

with `helper.becki` beside it and `helper.beck` deleted:

```console
$ beck test root.beck
test "no traits anywhere" … FAILED
  no such definition: double

0 passed, 1 failed, 0 skipped
```

Nothing names `helper`, nothing says an interface has no bodies, and the failure arrives from the
linker as if the program had called something that does not exist.

**Why it is a defect rather than an absence.** `B0604` is exactly this diagnostic — "`{name}` has
an interface but no implementation", with the note "an interface is enough to compile against and
never enough to run" — and [`project.rs`](../compiler/crates/beck-core/src/project.rs) raises it
only when the interface-only module is the **root**:

```rust
let Some(module_src) = &src.module else {
    // Interface only: it can be checked against, but there is no code to link.
    if name == root {
        diags.push(Diagnostic::error("B0604", …));
    }
    continue;
};
```

A dependency takes the `continue` in silence. The message is written, the fact is in hand, and the
person gets a link error instead — which is [`docs/82`](../docs/82-the-edge-report.md) §82.10's
shape one level up: the thing that would have said so exists and is not reached.

Checking against an interface alone is legitimate — that is what §3.6's separate compilation is
for, and `beck check` on a library is a real thing to want. What is not legitimate is *linking* a
program whose code is missing and discovering it at run time.

**Where it is.** The `continue` above, in `check_project`. The distinction it does not make is
between checking a project and building one that has to run: `compile_project` slices and links,
and that is the point at which a definition with no body is a program that cannot run.

**The gate a fix owes.** Positive: the program above is refused, naming `helper` and saying an
interface has no bodies, rather than failing at run time. Negative, and it is the half that would
be forgotten: `beck check` on a **library** whose own dependency is interface-only still succeeds,
because compiling against a contract is the feature and refusing it would delete §3.6 — so the
refusal has to be at the point something is linked, not at the point something is read.
