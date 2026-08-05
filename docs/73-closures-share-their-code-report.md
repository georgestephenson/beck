# 73 — Phase 3, part 42: a closure shares its code

**Built.** One line, and it is the largest single improvement this project has measured:

```rust
CoreKind::Lam { params, body } => Value::Closure(Arc::new(Closure {
    params: params.clone(),
-   body: (**body).clone(),   // a deep copy of the whole function body
+   body: Arc::clone(body),   // a refcount bump
    env: env.clone(),
}))
```

**Every benchmark in the tree is between 56% and 72% faster.** Nothing about the language changed.

## 73.1 What it was doing

A `lam` node evaluates to a closure, and a closure held its body **by value**. `Core` is a tree of
`Box`es, so `(**body).clone()` is a deep copy: one allocation per node of the function being
referred to.

That happens once per *call* of a named function. `f(x)` is `App { func: Global("f"), args }`; the
callee is evaluated as an operand, `Global` resolves to the definition's `lam`, and evaluating a
`lam` builds a closure — so calling a 50-node function copied 50 `Core` nodes before running any of
them.

**Measured by holding the work still and growing the body.** 20,000 calls to a function whose
executed path is one comparison, with the rest of its body behind a condition that never fires:

| unexecuted bindings in the callee | before | after |
|---|---|---|
| 0 | 42.0 ms | **18.8 ms** |
| 20 | 226.8 ms | **20.8 ms** |
| 60 | 606.1 ms | **21.8 ms** |

Before: 2.1 µs, 11.3 µs, 30.3 µs per call — **the cost of calling a function was proportional to
how much code it contained**. After, it is flat, and what remains of the slope is the larger tree's
cache footprint rather than any copying.

The first version of that measurement was wrong and is worth recording: it grew the body with
ordinary bindings, which the callee then *executed*, so it measured work rather than size. The
dead-branch version is the one that isolates it.

## 73.2 What it buys

Release, median of five, peak resident set per process:

| | before | after | |
|---|---|---|---|
| `lib/decimal.beck` | 1.106 s | **0.307 s** | **−72.2%** |
| `clbg/pidigits.beck` | 4.389 s | **1.329 s** | **−69.7%** |
| `awfy/json.beck` | 0.387 s | **0.120 s** | **−69.0%** |
| `awfy/havlak.beck` | 10.159 s | **3.476 s** | **−65.8%**, and 29.1 → 23.0 MB |
| `clbg/knucleotide.beck` | 2.143 s | **0.753 s** | **−64.9%** |
| `awfy/deltablue.beck` | 0.153 s | **0.068 s** | **−55.6%** |

Memory falls too, by 0.4–6 MB, because the copies were allocations.

This is a **constant**, not an asymptote: every program was paying it on every call, and the
programs that call small functions in tight loops — which is all of them — were paying it most.
[`72`](72-space-and-constants-report.md) shrank a `Value` from 48 bytes to 16 and bought 5%; this
shares one pointer instead of copying a tree and buys 65%. Both were found the same way, and the
larger one was found second.

## 73.3 Why it survived four reports about performance

Because it is invisible to every question those reports asked. It is not an asymptotic defect: cost
per call is constant *for a given function*, so measuring one program at two sizes — this branch's
own rule — shows a clean straight line. It is not visible to `--fuel`, even after
[`72`](72-space-and-constants-report.md) made the budget count work, because the copy is not work a
*primitive* does over a length a caller chose; it happens between nodes.

What found it was asking the question [`AGENTS.md`](../AGENTS.md) now demands of an operation and
asking it of an *implementation detail* instead: *what should calling a function cost?* A frame and
a jump. It cost a deep copy of the callee, and the way to see that was to make the callee bigger
without making it do more.

## 73.4 What the `Arc` costs

`CoreKind::Lam`'s body is `Arc<Core>` rather than `Box<Core>`, so the compile-time passes that
rewrite a lambda body — type resolution, placement, [`70`](70-last-use-moves-report.md)'s liveness
— go through `Arc::make_mut`. They run once, on a program nothing else holds, so the
copy-on-write never copies; if a future pass ever runs after a closure exists, it will copy rather
than mutate somebody's running code, which is the safe direction.

A `Core` node is 8 bytes larger where a lambda holds its body, and there are as many lambdas as
there are definitions. That is the whole price.

## 73.5 How it is tested

Nothing new, deliberately: this changes no answer, so the gate is that **all 48 suites still pass**
— the differential harness, the corpus, SICP's two chapters against the book's own answers, all
fourteen Are We Fast Yet benchmarks against their published constants, eight Benchmarks Game ports
against the Game's published files, and the standard library's property tests. A closure that shared
the wrong code would fail nearly all of them.

The scaling gates in `scaling.rs` are untouched and still pass, which is the other half of the
claim: this is a constant coming down, not a shape changing.

## 73.6 What this corrects

- **[`72`](72-space-and-constants-report.md) §72.6's list of remaining constants was missing the
  biggest one.** It named the `Core` node's size, the per-call frame allocation and interned field
  names; the per-call *body copy* is larger than all three together and was not on it.
- **Every wall-clock number in [`69`](69-standard-library-imports-report.md)–[`72`](72-space-and-constants-report.md) is now
  historical.** They were measured on an evaluator that copied a function body per call. The
  *shapes* those reports establish are unaffected — a quadratic is a quadratic at any constant — and
  the ratios between before and after within each report still hold, because both sides paid this.

## 73.7 What is not built

| | |
|---|---|
| A closure cache for globals | **not built.** Evaluating `Global("f")` still builds a fresh `Closure` — three `Arc` bumps and one allocation — where the value is the same every time and could be memoised per name. Worth measuring now that the copy is gone, because it is what remains |
| One allocation per call frame | **not built**, per [`72`](72-space-and-constants-report.md) §72.6. A call still allocates the argument vector, the binding vector, the `Arc` around it and an `Arc` for the parent environment |
| A smaller `Core` | **not built.** 152 bytes a node, 80 of it an inline `Ty` |
| Interned field names | **not built** |

## 73.8 What this establishes

**That the tree-walker's reputation was partly this.** [`25`](25-benchmarks-and-expressiveness.md)
§25.3 measured the evaluator at about 33× CPython and every report since has said the numbers are
about scaffolding. A third of that scaffolding was one `clone` — and the reason to say so plainly is
that "the interpreter is a placeholder" is exactly the sentence
[`AGENTS.md`](../AGENTS.md)'s standard forbids using as an explanation for a bad number.
