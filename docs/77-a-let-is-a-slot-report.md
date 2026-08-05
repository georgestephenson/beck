# 77 — Phase 3, part 46: a `let` is a slot

**Built.** A binding cost **134 ns** and now costs **32 ns**. Every benchmark in the tree is
**4.2% to 8.2% faster**, and the one program that binds nothing does not move — which is the
control, and the reason to believe the rest.

[`76`](76-the-record-and-the-read-report.md) §76.7 named the next item as making a variable *read*
an indexed load. It measured the wrong thing. The read was never the expensive half.

## 77.1 What a binding cost, and how that was found

[`76`](76-the-record-and-the-read-report.md) left "a variable read that is an index" as the largest
thing not built, on the strength of `Env::read` being 3.3% of the instructions `awfy/richards.beck`
executes. Before writing a calling convention on the back of that, the thing to do — after §76.4 —
was to measure the wall clock rather than the profile. So: the same loop with 0, 8 and 24 bindings
in its body, 300,000 iterations.

| bindings in the loop body | before |
|---|---|
| 0 | 130 ms |
| 8 | 441 ms |
| 24 | 1,092 ms |

**134 ns per binding**, flat — linear, so not the scope chain's depth, which would have curved. It
is 77% of what a whole function call costs
([`74`](74-the-cost-of-a-call-report.md): 174 ns), for the privilege of naming a value.

Where it went was three allocations. `Env::extend` built a vector for the one binding, converted it
into an `Arc<[_]>`, and boxed a clone of the enclosing environment to be its parent:

```rust
Env { frame: bindings.into(), parent: Some(Arc::new(self.clone())) }
```

[`74`](74-the-cost-of-a-call-report.md) §74.3 took the *call* path from four allocations to two and
said so; [`75`](75-what-the-profiler-said-report.md) §75.4 looked at the `let` path, counted
40,388 of its allocations against `bind`'s 137,785 in one run of `awfy/json.beck`, and called it
"a third-order term". That was a count of *how often*, in a program that happens not to bind much,
and it was never a measurement of *how much*.

## 77.2 What replaces it

A call already allocates a frame. It now sizes that frame for the parameters **and for every
binding the body is going to make**, so a `let` writes into a slot that already exists:

```rust
if let Err(v) = env.put_one(*var, v) {      // a store and a counter
    owned = Some(env.extend(vec![(*var, v)]));   // …or the old path, if it cannot
}
```

`beck_core::frames` is the pass that counts. It runs where
[`70`](70-last-use-moves-report.md)'s liveness runs — once, on the finished program, before any
backend sees it — and writes the number on the lambda's `Core::locals`.

**The counting rule is the safety argument.** It sums what runs in sequence and takes the *maximum*
over what cannot both run, so every binding that can be live at once has a slot of its own and no
slot is ever written twice within a call. Two arms of a `match` share a reservation because only one
of them runs; a nested lambda contributes nothing, because its body runs in a frame its own call
makes.

## 77.3 Why a closure cannot be hurt by this

A frame is now written after it exists, which is exactly the thing a captured environment must not
allow. `Arc::get_mut` is the whole guard: building a closure clones the `Env`, which shares the
frame's `Arc`, so from that moment `put_one` refuses and the `let` chains a scope as it always did.
One live capture disables reservations for the rest of that body — conservative, and cheaper than
being clever.

Getting the count *wrong* is safe in one direction and merely slow in the other. Too few slots and
the evaluator falls back to chaining, which is what also happens to every program built by something
that never runs the pass: a synthesised test body, a splitter's generated module. That is why the
fallback is kept rather than asserted away — and it is why the mutation that under-reserves does not
fail a test, which is the correct outcome rather than a gap.

## 77.4 How it is tested

A new suite, `beck-cli/tests/frames.rs`, five tests, none of them about speed. They assert what a
*program* would see if either half of §77.2 were wrong — which is a closure quietly answering with
somebody else's value:

- a closure keeps what it captured, and bindings made after it do not reach it;
- three closures from three calls each keep their own binding;
- the arms of a `match`, which share a reservation, do not see each other's bindings;
- an inner binding of the same name shadows the outer one, now that both live in one array;
- a body of 64 bindings — past any reservation — still answers, through the fallback.

Plus `frames.rs`'s own unit tests for the counting, and the other 48 suites, which is where the real
coverage is: SICP's two chapters build closures for a living, and a frame that leaked a binding
would fail them long before it failed anything written here.

## 77.5 What it buys

The binding itself, by the §77.1 measurement:

| bindings in the loop body | before | after |
|---|---|---|
| 0 | 130 ms | 126 ms |
| 8 | 441 ms | **201 ms** |
| 24 | 1,092 ms | **356 ms** |

**134 ns → 32 ns**, a little over four times cheaper.

And the tree, release, minimum of nine, interleaved:

| | [`76`](76-the-record-and-the-read-report.md) | now | |
|---|---|---|---|
| `clbg/knucleotide.beck` | 0.526 s | **0.483 s** | **−8.2%** |
| `clbg/pidigits.beck` | 0.886 s | **0.819 s** | **−7.6%** |
| `clbg/fasta.beck` | 0.311 s | **0.287 s** | **−7.5%** |
| `lib/decimal.beck` | 0.204 s | **0.190 s** | **−6.7%** |
| `awfy/json.beck` | 0.076 s | **0.073 s** | **−4.7%** |
| `awfy/havlak.beck` | 2.128 s | **2.029 s** | **−4.6%** |
| `awfy/deltablue.beck` | 0.045 s | **0.043 s** | **−4.4%** |
| `awfy/richards.beck` | 1.227 s | **1.176 s** | **−4.2%** |

`fib(30)` is **unchanged** at 0.82 s, and that is the control worth having: it binds nothing, so it
should not move, and it does not.

**The cost is memory**: reserving slots a branch may not take raises peak resident set by 3–6% —
21.3 → 21.8 MB on `havlak`, 11.9 → 12.5 MB on `knucleotide`. Taking the maximum over exclusive
branches rather than the sum was worth having and did not change those numbers much; what costs is
long straight-line bodies, which is also what benefits.

## 77.6 What this corrects

- **[`76`](76-the-record-and-the-read-report.md) §76.7 pointed at the wrong half of `Env`.** It
  named the read; the write was four times dearer and is what this fixes. The read is still a walk.
- **[`75`](75-what-the-profiler-said-report.md) §75.4 dismissed this as third-order**, from an
  allocation *count* on a program that binds little. A count of how often is not a measurement of
  how much, and §76.4's rule — an instruction profile ranks candidates, the wall clock decides —
  wants a third clause: a candidate you have only counted has not been measured at all.
- Every wall-clock number in [`69`](69-standard-library-imports-report.md)–[`76`](76-the-record-and-the-read-report.md)
  is historical, as before. The shapes are unaffected.

## 77.7 What is not built

| | |
|---|---|
| A variable read that is an index | **still not built.** A read scans a frame and then walks the parent chain; with a body's bindings in one frame the chain is shorter, but neither half is `O(1)`. Making it one is `Core::locals` extended to a slot per binding and a `(hops, slot)` on every `Var` — the same pass, more of it |
| Fields placed by a compile-time permutation | **not built**, per [`76`](76-the-record-and-the-read-report.md) §76.7 |
| Interned field names | **not built** |

## 77.8 What this establishes

**That "measure it at two sizes" applies to the implementation and not only to the program.** The
rule [`AGENTS.md`](../AGENTS.md) states was written about a program's asymptotics; the same
instrument — hold the work still, vary one thing, measure twice — is what turned "a `let` is
probably fine" into a number, and the number was 77% of a function call. Two reports had already
looked straight at this line and read it as third-order, because both were counting rather than
timing.
