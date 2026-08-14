# 81 — Phase 3, part 50: `fs` is two atoms

**Built.** `fs(path)` is now `fs.read(path)` and `fs.write(path)`, so a `parallel:` scope may have
children that read files and still refuses children that write them.

This is [`80`](80-structured-concurrency-report.md) §80.12's first open item, taken the day after
it was named. [`27`](27-the-walls-come-down-report.md) is the precedent for the shape of this
report — a wall written by one change and closed by the next, recorded when it is found rather than
at the end of whatever finds it.

## 81.1 The finding, restated

[`80`](80-structured-concurrency-report.md) §80.2 gave a `parallel:` scope one rule about
effects: no child may perform one another child could observe. Deriving that list meant asking, atom
by atom, "could a second child tell this one had run?" — and every atom answered except one.

`fs(path)` named a resource without saying what was being done to it. A child *writing* a file is
exactly the interference the rule exists to refuse; a child *reading* one is not. With one atom
there was no way to say the first without saying the second, so the rule refused the pair and §80.2
recorded the cost in the open:

> Two children reading two files is something this form should allow and cannot.

It is the only atom in §3.2's list with that shape. `net.out` and `net.in` are two; `external.read`
and `external.write` are two; `durable` names an operation rather than a resource; `cap.X` is an
authority. §3.8's escape hatches had already been split, for the same reason and without anybody
writing down that it was a reason.

## 81.2 The split

One variant becomes two, and everything that reads it was two lines:

```text
Effect::Fs(path)   →   Effect::FsRead(path)
                       Effect::FsWrite(path)
```

* **`Tier::discharges`** needed no change at all. `server` discharges everything but `dom`, `client`
  discharges a closed list neither is on, and `data` discharges a closed list neither is on. The
  table was never about the operation.
* **`Effect::breaks_replay`** needed none either, and both atoms still break it. A fold that reads a
  file is not a function of the log any more than one that writes it — the file can change between
  replays, which is §3.7's rule and is about *reading* in the first place.
* **`is_auto_stubbable`** takes both. §21.3's "genuinely external" is a statement about the
  boundary, and both cross it.
* **`observable_order`** — the checker's `parallel:` rule — takes only `FsWrite`. **This one line is
  what the split was for.**

The atom's *spelling* follows §3.8's, so a reader who has seen one has seen the other:
`fs.read(/var/lib/beck)`, `external.read(legacy)`. Nothing new had to be taught to the parser, whose
effect-atom reader has reassembled dotted heads generically since Phase 2.

## 81.3 What it buys

```beck
def both(p: Str) -> Int:
    return parallel:
        a = load_profile(p)      # uses fs.read(profiles)
        b = load_settings(p)     # uses fs.read(settings)
        a + b
```

compiles, and publishes
`def both(p: Str) -> Int uses fs.read(profiles), fs.read(settings), spawn`. Change either child to
`fs.write` and it is `B0399` again. Both directions are asserted, because the whole value of the
split is that the two answers now differ
(`concurrency.rs::two_children_may_read_files_and_may_not_write_them`).

The refusal is still coarse in one way worth naming: two children writing **different** paths are
refused as surely as two writing the same one. The atom carries a path, so the checker could compare
them — and deliberately does not, because two paths that differ as strings can be the same file, and
a rule that is right about `/a` versus `/b` and wrong about `/a` versus `/a/../a` is worse than one
that is simply conservative. A scope that genuinely needs two writers can do them in the tail.

## 81.4 The spelling that used to work

`fs(path)` is not accepted, and it is the spelling a reader arrives with — from §3.2 as it stood
until this change, or from habit. `B0305`'s ordinary message is "`fs(profiles)` is neither an effect
nor a row", which is true and useless, so the atom gets a note of its own:

```text
error[B0305]: `fs(profiles)` is neither an effect nor a row
  = note: `fs` is two atoms: write `fs.read(profiles)` or `fs.write(profiles)`. One name for both
          could not say whether a mount needs to be writable, or whether two children of a
          `parallel:` scope may touch it at once
```

Nothing in the tree had to change to accommodate it: no `.beck` or `.becki` file in this repository
writes `fs(…)`, so the split broke no program. It changed exactly one test constant — the row the
placement-property suite fuzzes over, which now carries both.

## 81.5 What this corrects

**[`80`](80-structured-concurrency-report.md) §80.12 said §6.5 derives a volume's mount options
from this atom. It does not.** That was written from
[`06`](06-kubernetes-and-packaging.md) §6.5's worked example — "No filesystem mounts beyond the
volume, `readOnlyRootFilesystem: true`" — which is a *design* for what the derivation will say, and
read as though it were built. `beck-infra`'s derivation reads three atoms: `ingress`, `durable` and
`net.out`. Nothing there has ever looked at `fs`.

That does not change the conclusion — a resource atom that cannot say read from write is wrong for
§6.5 whenever §6.5 gets there, and it was already wrong for `parallel:` today — but the report
claimed a built derivation that does not exist, and the standard here is that "built" and "designed"
are different claims. §81.6 is the corrected version.

## 81.6 What is not built

| | |
|---|---|
| Any primitive that touches a file | **not built**, and this is the honest limit of the change. Nothing in the prelude reads or writes a path, so `fs.read` and `fs.write` reach a row only through a `uses` clause a program declares. That was equally true of `fs(path)` before the split — the atom has been in the vocabulary since Phase 2 with nothing able to perform one — so this is a vocabulary that is now correct rather than a capability that is now available |
| A mount derived from the atom | **not built.** [`06`](06-kubernetes-and-packaging.md) §6.5 describes it and `beck-infra` does not do it (§81.5). What the split changes is that the derivation now *can* be written: `fs.read(p)` is a `readOnly: true` mount and `fs.write(p)` is not, which is a distinction the old atom could not express and which is the reason not to defer the split until the derivation is wanted |
| Path comparison in the `parallel:` rule | **not built**, deliberately (§81.3) |

## 81.7 What this establishes

**That the vocabulary is worth auditing against a question rather than a list.** `fs` had been one
atom for three phases and nothing was wrong with it, because nothing had asked it a question it
could not answer. The question was not "is this list complete" — it was "could a second child tell
this one had run", asked of each atom in turn, and it took one afternoon to find the one atom that
could not answer. [`80`](80-structured-concurrency-report.md) §80.2's table is that audit, and
this is what it was worth.

**And that a report can be wrong in the direction of generosity.** §80.12's justification for
deferring the split cited a derivation that does not exist. The split was still right and the
argument for it still holds — but it held for one reason rather than two, and the second was read
out of a design document as though it were a measurement. [`67`](67-sqlite-report.md) §67.3 is the
same failure with a number in it — a 26× that turned out to be a durability setting rather than an
engine — and the check that catches both is the same one, which is to go and look at what the code
does rather than at what the document says it will.
