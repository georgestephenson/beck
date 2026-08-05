# 74 — Phase 3, part 43: what a call costs

**Built.** Four changes to the path a function call takes, and one that was written, measured and
thrown away. A call costs **287 ns → 174 ns**, and every benchmark in the tree is **17–27% faster**
than [`73`](73-closures-share-their-code-report.md) left it.

This is [`73`](73-closures-share-their-code-report.md)'s §73.7 list worked through, and the answer
to the question that list was really asking: *what should calling a function cost?* A frame and a
jump. It cost a name lookup, three allocations, a virtual call and a copy of the parameter list.

## 74.1 How a call is measured

Two programs, identical but for one extra call in the loop body:

```beck
def one(n: Int) -> Int:
    return n

def with_calls(i: Int, n: Int, acc: Int) -> Int:
    if i >= n:
        return acc
    return with_calls(i + 1, n, acc + one(1))
```

against the same loop with `acc + 1` in place of `acc + one(1)`. Two million iterations, minimum of
five whole-process runs each, and the *difference* divided by the iteration count — so process
start-up, parsing, checking and the loop's own arithmetic all cancel and what is left is one call.

The measurement is in the scratch directory rather than the tree, because it is a diagnostic and not
a gate: `scaling.rs` gates *shapes*, and this is a constant.

## 74.2 A definition's closure is built once

A definition **is** a lambda over the empty environment, so `Global("f")` evaluates to the same
closure every time it is reached — which is once per call of `f`. Building it again meant a lookup
of the name in the host's definition table, a copy of the parameter list, an `Arc` for the closure
and a clone of the environment.

`Interp` now memoises it, in both places a global is resolved — `step`'s transparent `Global` arm
and `Interp::global`. Only a `Closure` is cached: nothing else a global can evaluate to is
guaranteed to be the same value twice, and the cache is not the place to decide that.

## 74.3 A frame is one allocation, and its parent is shared

Binding arguments used to cost four allocations:

| | |
|---|---|
| the argument vector | unavoidable — `App` evaluates operands before it knows the callee |
| the binding vector | `params.zip(args).collect::<Vec<_>>()` |
| the `Arc<Vec<…>>` around it | `Arc<Vec<T>>` is *two* allocations and two pointer hops to reach a binding |
| an `Arc<Env>` for the parent | `Env::extend` cloned the closure's environment and boxed the clone |

Three of those are gone. `Env`'s frame is an `Arc<[(VarId, Value)]>`, built by collecting straight
from `params.iter().zip(args)` — a `Zip` of two exact-size iterators has a known length, so that is
one allocation and no intermediate vector. And a `Closure` holds its environment as `Arc<Env>`
already, so `Env::extend_shared` takes the parent by refcount rather than by copy. A call now
allocates twice: the argument vector, and the frame.

A frame never changes length, which is what makes the fixed-size form possible — a moved-out binding
is tombstoned in place ([`70`](70-last-use-moves-report.md)), not removed.

`CoreKind::Lam`'s parameter list is an `Arc<[VarId]>` for the same reason its body became one in
[`73`](73-closures-share-their-code-report.md): handing it to a closure is a refcount bump.

## 74.4 Two smaller ones

**The name hash.** The globals cache is keyed by name, so a call hashes a short string. The
standard library's hasher is SipHash, chosen so that an adversary cannot force collisions; nothing
here is adversary-supplied — the keys are the names the program itself declares, and the map is
rebuilt per evaluation. It is FxHash now, the multiply-and-rotate the Rust compiler uses for its own
interners. Worth about 2% on a call-heavy program and nothing outside the noise on the rest, which
is the honest size of it.

**The stub check.** `beck test` can replace a call to a named definition with a stub
([`21`](21-tests-in-beck-and-proof.md) §21.3), so every direct call asked the host whether this one
is intercepted — a virtual call through `dyn Host`, on the hottest path there is, answering "no" in
all but a handful of runs. `Host::intercepts` is a field test that gates it. It defaults to `true`
rather than `false`: a host that overrides `intercept` and forgets this one is slower than it needs
to be, which is the harmless direction.

That one is worth 12 ns of the 174 — 6% of a call, for a branch.

## 74.5 What it buys

Release, minimum of five, the two binaries run **interleaved** so that machine drift falls on both:

| | [`73`](73-closures-share-their-code-report.md) | now | |
|---|---|---|---|
| `clbg/pidigits.beck` | 1.576 s | **1.151 s** | **−27.0%** |
| `lib/decimal.beck` | 0.369 s | **0.281 s** | **−24.0%** |
| `clbg/fasta.beck` | 0.495 s | **0.379 s** | **−23.6%** |
| `awfy/richards.beck` | 2.185 s | **1.693 s** | **−22.5%** |
| `awfy/json.beck` | 0.144 s | **0.112 s** | **−21.9%** |
| `clbg/knucleotide.beck` | 0.877 s | **0.693 s** | **−21.0%** |
| `awfy/deltablue.beck` | 0.081 s | **0.064 s** | **−20.9%** |
| `awfy/havlak.beck` | 4.130 s | **3.442 s** | **−16.7%** |

And the call itself, by the §74.1 measurement: **287 ns → 174 ns**, a 39% reduction. The three
increments measured on the same harness were 191 ns after §74.3's frame work, 185 ns after the
hash, and 174 ns after the stub check; the rest is §74.2.

Together with [`73`](73-closures-share-their-code-report.md), a call is now about a fifth of what it
cost two reports ago.

## 74.6 What was built, measured and thrown away

**An argument stack.** The remaining two allocations are the argument vector and the frame, and the
frame's is the one that has to happen. So: push arguments onto one buffer owned by the interpreter,
innermost call last, and drain them into the frame — one allocation per call, no `unsafe`, and the
buffer reaches its high-water mark once and stays there. It is the obvious next move and it is
written up in [`73`](73-closures-share-their-code-report.md) §73.7 as "one allocation per call
frame".

It made a call **20% slower** — 185 ns to 220 ns. The bookkeeping costs more than the allocation it
removes: a `RefCell` borrow per argument pushed (argument evaluation nests, so the borrow cannot be
held across it), a drop guard to put the stack back however the arm is left, and a `Drain` where
there had been a `Vec` by value. It is reverted.

Worth recording rather than quietly dropping, for two reasons. The first is that it is the second
time on this branch that the arithmetic of "fewer allocations is faster" came out the other way —
[`70`](70-last-use-moves-report.md) found the same thing about moving values out of frames, and
fixed it by moving only what is worth moving. The second is that the version that *would* pay —
allocating the frame uninitialised and writing arguments into it directly — needs
`Arc::new_uninit_slice` and `assume_init`, and this workspace sets `unsafe_code = "forbid"`. That is
the constraint that makes two allocations per call the floor here, and it is a decision already
taken rather than an oversight.

## 74.7 How it is tested

Nothing new. None of this changes an answer, so the gate is that **all 48 suites still pass** — the
differential harness, the corpus, both SICP chapters against the book's own answers, all fourteen
Are We Fast Yet benchmarks against their published constants, eight Benchmarks Game ports against
the Game's published files, the standard library's property tests, and `tests_in_beck.rs`, which is
what would catch §74.4's stub gate if it were wrong.

`scaling.rs` is untouched and still passes: this is a constant coming down, not a shape changing.

## 74.8 What is not built

| | |
|---|---|
| One allocation per call frame | **not built, and measured** — §74.6. Two is the floor without `unsafe` |
| A global resolved to an index | **not built.** `Global` carries a name, so a call hashes it; an index assigned at link time would make it an array read. Worth a few nanoseconds of the 174 and touching thirty match sites, which is the wrong ratio today |
| One allocation per `let` | **not built.** `Env::extend` is the per-`let` path and still clones the environment and boxes the clone. Fewer of those than calls, and the loop owns its environment rather than sharing it, so the fix is not the same one |
| A smaller `Core` | **not built.** 152 bytes a node, 80 of it an inline `Ty` |
| Interned field names | **not built** |

## 74.9 What this establishes

**That the interpreter's constants were never measured until this branch, and they were the
majority of its cost.** [`25`](25-benchmarks-and-expressiveness.md) §25.3 put the evaluator at about
33× CPython. Between [`72`](72-space-and-constants-report.md), [`73`](73-closures-share-their-code-report.md)
and this, none of which changed an algorithm or an answer, the same programs run in about a third of
that time. Every one of the findings was reachable by asking what an operation *should* cost and
measuring whether it did — which is the standard [`AGENTS.md`](../AGENTS.md) now states, applied to
the implementation rather than to a program.

**And that the standard cuts both ways.** §74.6 is a change that every instinct said would be faster
and that measurement refused. A number decides it, in both directions.
