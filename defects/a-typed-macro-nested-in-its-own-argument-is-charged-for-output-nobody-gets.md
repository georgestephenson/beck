## `a-typed-macro-nested-in-its-own-argument-is-charged-for-output-nobody-gets`

**What is wrong.** A `typed macro` call whose argument is another `typed macro` call is expanded
twice — once in the probe that infers the argument, once inside whatever the enclosing macro wrote
— so nesting `d` deep costs `2^d` expansions, and **each one is charged against F17's module-wide
production budget**. The budget is defined as a bound on what expansion *produces*
([`docs/42`](../docs/42-security-assurance.md) §42.6), and the probe's output is thrown away, so this
charges a program for code it does not contain. Measured on a macro that produces three nodes
(`$x + 0`), `beck check` at increasing nesting depth:

| depth | 13 | 14 | 15 |
|---|---|---|---|
| wall | 97 ms | 175 ms | 321 ms |
| result | ok | ok | **`B0214`** |

Doubling per level, and `2^15 × 3 ≈ 98,000` against a 100,000-node budget is where it lands. The
program that is refused has a total expansion of about **forty-five nodes**.

Reproduce it with the fixture the numbers came from — `wrap` nested `d` deep, timed through
`beck check`:

```beck
typed macro wrap(x):
    t = node_ty(x)
    if t.name != "Int":
        refuse("only Int")
    return quote:
        $x + 0

def f() -> Int:
    return wrap(wrap(wrap(...wrap(1)...)))   # d of them
```

**Why it is a defect rather than an absence.** The refusal is wrong on its own terms and its message
says so: "macro expansion produced too much … the budget is 100000 nodes for the whole module", on
a module whose macros produced forty-five. A reader has no way to reach the real cause from that
sentence, and the second diagnostic makes it worse — once the budget is spent every later expansion
produces nothing, so the macro's own `refuse` fires on a type it can no longer see, and the program
is told it has two problems, neither of which it has.

**Why it is not just slow.** [`docs/102`](../docs/102-the-macro-interpreter-report.md) §102.9 recorded
the `2^d` as a known cost "charged honestly against" the budget. The charge is not honest: it counts
work whose output is discarded. The exponent is real either way and
[`AGENTS.md`](../AGENTS.md)'s rule applies — an exponential in the compiler is a design question, not a
number to write down.

**What a fix owes.** Two gates, because the halves fail on different days.

1. **The refusal.** A nesting-depth sweep in `compile_speed.rs`'s shape form — budget charged per
   nesting level must not grow with the level — with the fifteen-deep fixture above compiling. Red
   today.
2. **The hole it must not open.** `macro_bomb.rs`'s doubling typed macro, unchanged and still
   refused. Anything that stops charging the probe has to keep that one red-when-removed, which is
   the trap: the probe's charge is what §102.9 found the first version deleting, and a fix that
   rolls the charge back reopens exactly that.

The likely shape is memoising the expansion on `(call span, the argument types the body reads)` —
a body reads `node_ty` of its arguments and the syntax it was given, and nothing else the checker
knows, so equal types are the same expansion. Note that this fixes the **charge** and not the
asymptotics: the doubling is in `Checker::expr`, not in the expander, because the probe checks the
argument and the real pass checks it again. Making *that* linear means reusing the probe's `Core`,
which cannot be done naively — the probe rolls back the effect row on purpose, and a macro that
keeps its argument must be charged for the argument's effects.
