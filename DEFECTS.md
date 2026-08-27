# Defects

**What is wrong right now.** [`CHANGELOG.md`](CHANGELOG.md) is what has been fixed; this is what has
not. An entry is **deleted by the change that fixes it**, in the same commit, and the CHANGELOG
bullet for that change is where it goes on record. So this file is always the current list and never
a history — git holds the history, as it does for everything else in this repository
([`AGENTS.md`](AGENTS.md)).

**What belongs here: something that behaves wrongly.** Silent, misleading, or contrary to what a
document says. **What does not: something that is merely absent.** A feature nobody has built is a
line in [`docs/08`](docs/08-roadmap.md) §8.5, which is the only place that holds an order; putting
absences here would turn the register into a second roadmap that disagrees with the first.

**Every entry names the gate a fix owes.** This project has repeatedly shipped fixes behind gates
that could not have failed ([`docs/82`](docs/82-the-edge-report.md) §82.10), and the cure is to write
down *what would have to go red* while the defect is still in front of you. A fix that lands without
its gate has not been fixed; it has been made invisible.

**Ids are slugs, not numbers, and are never reused.** Entries are deleted, so a number would imply a
sequence that does not survive.

This register was opened alongside [`docs/104`](docs/104-styling-and-the-component-library.md), seeded
with what that audit found plus one older defect already recorded in a report. It has never been
*complete* and is not meant to be read as a survey: it is what somebody wrote down. Anything you find
that meets the admission rule above belongs here, whether or not you are the one to fix it.

---

## `a-typed-macro-nested-in-its-own-argument-is-charged-for-output-nobody-gets`

**What is wrong.** A `typed macro` call whose argument is another `typed macro` call is expanded
twice — once in the probe that infers the argument, once inside whatever the enclosing macro wrote
— so nesting `d` deep costs `2^d` expansions, and **each one is charged against F17's module-wide
production budget**. The budget is defined as a bound on what expansion *produces*
([`docs/42`](docs/42-security-assurance.md) §42.6), and the probe's output is thrown away, so this
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

**Why it is not just slow.** [`docs/102`](docs/102-the-macro-interpreter-report.md) §102.9 recorded
the `2^d` as a known cost "charged honestly against" the budget. The charge is not honest: it counts
work whose output is discarded. The exponent is real either way and
[`AGENTS.md`](AGENTS.md)'s rule applies — an exponential in the compiler is a design question, not a
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

---

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

---

## `defects-entries-share-one-file` — a pull request that records a defect reads as conflicting

**What is wrong.** This file is one list, and a branch that finds a defect adds a section to it. Two
branches that each add one are editing the same file, and on GitHub that is reported as a conflict:
[`.gitattributes`](.gitattributes) sets `merge=union` here so that `git merge` on a clone keeps both
sections, but **GitHub reads no merge driver** — neither for the mergeability it reports on a pull
request nor for the merge its button performs. So the report is the misleading one again: a reviewer
told the branch "has conflicts that must be resolved" cannot tell this file from a real disagreement
in the compiler, and resolving it by hand is the work the union driver was added to make
unnecessary.

**Why it is smaller than what it is left over from.** The changelog had the same defect and it fired
on *every* pull request, because every change records one; this fires only when two open branches
both find a defect, which is rare. That is a difference in how often, not in kind. The changelog's
fix was to stop relying on the driver — a change records itself in a file of its own under
[`changelog/`](changelog/README.md), so two branches never write the same line — and the same shape
is available here: one file per defect, assembled into this list. Deletion is the half worth
checking, because it is this file's own rule that an entry leaves in the change that fixes it, and a
deleted file merges against another branch's added file with nothing to resolve, where union merge
does not resolve a delete against an edit at all.

**The gate a fix owes.** The changelog's gate is
`docs.rs::two_branches_recording_a_change_merge_with_no_gitattributes`, and this one is its twin,
written the same way: two branches each **recording a defect**, and a third case that two branches
each **fixing** one — deleting entries — merge cleanly **in a tree with no `.gitattributes` at all**,
which is the configuration GitHub runs. Model the absent driver by removing the file and not by
configuration: `core.attributesFile` names the *global* file and does not suppress the one in the
tree, so a gate written that way passes for the wrong reason. And keep the negative half: two
branches recording a defect the way they do today must **conflict** in that same tree, or the gate
cannot tell the two shapes apart. Checked while this entry was written — with the file absent, two
branches each adding a section here conflict, and with it present they merge — so the gate goes red
today and green on a fix.
