# 64 — Phase 3, part 33: the compile-speed budget, and the quadratic it found on its first run

**Built.** [`compiler/crates/beck-cli/tests/measure_compile.rs`](../compiler/crates/beck-cli/tests/measure_compile.rs)
— the front end's cost, per phase and per program — and
[`compile_speed.rs`](../compiler/crates/beck-cli/tests/compile_speed.rs), the gate.

[`25`](25-benchmarks-and-expressiveness.md) §25.9 schedules "compile-speed budgets" for Phase 3 and
[`13`](13-testing.md) §13.7 lists them among the numbers every merge answers to. Both were
outstanding. They are the second-to-last item on §25.9's Phase 3 row; only the CLBG harness is left.

The budget found a **quadratic** in the front end within an hour of existing, which is the argument
for the whole exercise and is stated first for that reason.

## 64.1 What a compile-speed budget can honestly be

§13.7's own rule rules out the obvious design: *"a gate that flakes gets deleted"*, and a wall-clock
threshold on a shared CI runner flakes. So the gate asserts a **shape** rather than a rate, which is
what [`scaling.rs`](../compiler/crates/beck-cli/tests/scaling.rs) already does for the fold
([`19`](19-phase-1-report.md) §19.4 item 3):

> **cost per declaration does not grow with the number of declarations.**

That is the regression a compile-speed budget is actually for. A constant factor is a nuisance
somebody notices; an exponent is a wall that arrives without warning at whatever module size crosses
it. Three axes, because a module grows three ways and each is a different quadratic:

| axis | the quadratic it catches |
|---|---|
| **width** — `n` top-level definitions | re-resolving every declaration per declaration |
| **width with an edge each** — `n` definitions, each calling the last | summing the whole dependency graph per node |
| **depth** — `n` sequential local bindings in one body | re-walking the enclosing scope per binding |

A program that grows along one is flat along the others, so none is a duplicate of another — and the
second exists because the first would not have caught what §64.2 found. `wide` has no edges at all.

## 64.2 The finding: placement was quadratic, and it was the *explanations*

The per-phase table said it immediately. Along the width axis, parse, expand, check and the security
pass are each **flat** per declaration — 3.4 µs, 1.0, 4.9, 0.11, unchanged from 200 declarations to
3,200 — and `place` went 12.5 → 22.7 → 44.9 µs, doubling every time `n` doubled.

It was not the solver's search. `place::solve` computes, for every node, what the program would cost
with that node moved to each of the three tiers — the number `beck explain place <file> <name>`
prints. Each of those was a **full re-sum of every node and every edge**:

```rust
for t in [Tier::Client, Tier::Server, Tier::Data] {
    let mut probe = assign.clone();
    probe[i] = t;
    candidates.push((t, total_of(&probe)));   // O(n + e), n times over
}
```

Three sweeps per definition, so `O(n × (n + e))` — computed unconditionally, for every program,
whether anybody ever asked for an explanation or not.

The fix is the observation that moving one node changes its own cost and the cost of the edges
touching it, and nothing else. With an incidence list the probe is `O(degree(i))`, and summed over
all nodes that is `O(n + e)`:

| definitions | before | after | |
|---|---|---|---|
| 1,600 | 0.142 s | 0.062 s | |
| 3,200 | 0.454 s | 0.125 s | |
| 6,400 | 1.434 s | 0.245 s | |
| **12,800** | **6.155 s** | **0.583 s** | **10.6×** |

Release build, median of three, `beck check` on a generated module. The ratio grows with `n`, which
is what removing an exponent looks like as against removing a constant factor.

**Nothing it prints changed.** `beck explain place` over all 57 checked-in Beck programs is
byte-for-byte identical either side of the change, and so are the per-candidate cost breakdowns —
`beck explain place <file> <name>` for every named definition in the corpus and the examples, 2,817
lines of them. That is the same evidence [`53`](53-are-we-fast-yet-report.md) §53.5's
short-circuiting fix was held to, and it is the only kind that settles a change to a cost model.

The one place delta arithmetic could differ from a re-sum is saturation, and `Cost` is an `i64`
against costs bounded by `FORBIDDEN = 10⁹`: a program would need 10¹⁰ nodes at the forbidden cost
to reach it.

## 64.3 What the gate does not claim: a residual, and it is *not* fixed

The width-with-edges axis still measures **×2.45 per declaration across a 16× increase**. Linear
would be about ×1, and the other two axes measure ×1.23 and ×0.88.

That is not the quadratic §64.2 removed — a quadratic measures ×16 over that range, and this one
measured ×6.00 before the fix. It is roughly `n^1.35`, and the per-phase table says most of it is
**`check`** rather than `place`: 7.3 → 11.5 µs per declaration along a call chain, where the same
phase is flat when the definitions do not call each other.

It is recorded rather than chased. What a 6,400-deep call chain costs the checker is a question
worth an investigation of its own, and this report has no measurement of *why* — only that it is
there, that it is not an exponent, and that no checked-in program is anywhere near the shape that
provokes it (the longest chain in the tree is a few dozen). The gate's bound is set above it
deliberately, and §64.5 says what that costs.

## 64.4 The second finding: `MAX_NESTING` does not count a flat block

Found the same way and by accident: the measurement suite overflowed its own thread.

A body of sequential local bindings is **flat**. `v0 = …` followed by `v1 = …` is not one inside the
other, so a 3,200-binding body sits at nesting level 2 and the ceiling
[`42`](42-security-assurance.md) §42.2 installed never sees it. The front end still recurses once
per binding, because a block is a chain in `Core` whatever it looks like in source — measured at
roughly 650 bytes of host stack each.

What follows from that, and the second row is the sharper half:

| profile | largest flat body measured to compile | aborts at |
|---|---|---|
| release | 50,000 (25,000 in 0.32 s) | 100,000 |
| **debug** | **6,000** | **12,000** |

The abort is `thread 'beck-eval' has overflowed its stack`, SIGABRT, exit 134, no diagnostic. And
**the limit depends on the build profile**, which is precisely the property
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) and
[`27`](27-the-walls-come-down-report.md) §27.2 established the *evaluator* must not have: a recursion ceiling
should be a counted number with a diagnostic, so that "does this program compile" is a question
about the program. Here it is a question about how the compiler was built — an eight-fold
difference between a `cargo test` and a `cargo test --release`.

This is [`42`](42-security-assurance.md) §42.2's defect reached along an axis its ceiling does not
count, and §42.11's two-part gate — "a nesting one past the ceiling is a *diagnostic*; and the
declared stack holds the ceiling" — holds only for the axis the counter measures. It is **not fixed
here**. A 12,000-binding function body is not a program anybody writes, but that was equally true of
3,785 nested parentheses, and §42.2 is the report that says so.

`front_end_bound.rs::a_flat_block_is_bounded_by_the_declared_stack_rather_than_by_the_nesting_ceiling`
asserts what does hold, at 5,000 bindings — a size chosen from the *debug* limit, because a test
whose passing depended on `--release` would be measuring the profile rather than the compiler. It
runs through the binary for that file's own reason: the failure being ruled out is a process abort,
and nothing inside a process can catch one.

## 64.5 The gate, and why its bound is where it is

**4.0× per-declaration growth across a 16× increase in declarations**, and both numbers came from
measurement rather than from a round-number instinct:

| | measured |
|---|---|
| the defect §64.2 removed | **×6.00** — caught, with half again to spare |
| width, fixed | ×1.23 |
| width with an edge per definition, fixed | ×2.45 — §64.3's residual |
| depth | ×0.88 |

The range is 16× rather than `scaling.rs`'s 8× because 8× did not leave enough room: over that
range the same defect measures ×3.11 against a bound of 3.0. A gate that catches the bug it was
written for by 4% is a gate that will not catch the next one. Widening the *range* separates the
signal from the bound, where tightening the bound would only move it closer to the noise.

The three axes are measured in **one test, in sequence**, and that is not tidiness. The first draft
was three `#[test]`s, libtest ran them concurrently, and the contention reported ×2.13 on an axis
that measures ×1.40 alone — §13.7's flake, caught before it was committed rather than after.

## 64.6 What the table says about programs that exist

The other half of the suite runs the whole front end over every checked-in Beck program — 57 files,
10,837 lines across the corpus, the SICP chapters, the Are We Fast Yet ports, the standard library
and the examples.

| | |
|---|---|
| whole front end, all 57 | **67.5 ms**, about 160,000 lines/s |
| slowest program | `awfy/cd.beck`, 914 lines, **5.0 ms** |
| §13.7's keystroke→diagnostic (parse + expand + check, the prefix an editor reruns) | **4.7 ms** worst, **0.75 ms** median |

Those are not thresholds and are not compared to any other compiler. They are here because §13.7
lists keystroke→diagnostic latency as a budget and nothing had ever measured it, and because the
worst number in the tree being under 5 ms is the fact that decides whether the LSP
([`23`](23-incremental-views-report.md) §23.19) needs an incremental front end or can re-check the
file. On this evidence it can re-check the file.

## 64.7 What is **not** built

| | Status |
|---|---|
| A rate gate | **not built, deliberately**, per §64.1 and §13.7 |
| Incremental and clean *build* time | **not measured.** §13.7 lists both; they are `cargo` numbers about the compiler's own build rather than about the front end, and nothing here touches them |
| The `check`-phase residual on a call chain | **not fixed**, per §64.3 |
| The flat-block abort | **not fixed**, per §64.4 — and it is the profile-dependence rather than the size that makes it worth a line here |
| `criterion`/`divan`, fixed hardware runners | **not adopted.** §13.7 names them; a shape gate needs neither, and adopting a benchmarking framework to assert an exponent would be equipment without a question |
| The CLBG harness | **not stood up**, unchanged from [`53`](53-are-we-fast-yet-report.md) §53.7. It is now the *only* thing left on [`25`](25-benchmarks-and-expressiveness.md) §25.9's Phase 3 row, and §64.7.1 is why it was not attempted here |

### 64.7.1 Why CLBG was not attempted alongside this

It was tried and stopped at the first step, and the reason is worth recording because it is a
constraint on *how* the harness can be built rather than a judgement about whether to build it.

[`53`](53-are-we-fast-yet-report.md) establishes the rule the Are We Fast Yet ports obey and
[`awfy/README.md`](../compiler/awfy/README.md) states it: **each benchmark's verification constant
is the original suite's own, read out of its source.** A number invented here would defeat the whole
point of adopting somebody else's benchmark.

The Computer Language Benchmarks Game publishes its expected output per benchmark and per `N`, and
in this environment both its site and its repository are refused by the network policy:

```console
$ curl https://benchmarksgame-team.pages.debian.net/…/spectralnorm.html
curl: (56) CONNECT tunnel failed, response 403
```

So the ten benchmarks could be *written* here and not one of them could be *verified*, which would
produce a suite that measures Beck against numbers this repository made up. That is worse than an
absent harness, and it is the exact failure [`53`](53-are-we-fast-yet-report.md) §53.6 found by accident: a
benchmark with no oracle passes while doing nothing.

The harness is therefore owed **with its sources to hand** rather than owed generally. Nothing else
about it is blocked: `awfy/` is the shape to copy, `measure_awfy.rs` is the measurement half, and
[`55`](55-bignums-report.md)'s bignums are what `pidigits` needs and what would have been the
awkward one.

## 64.8 What this corrects

- **[`25`](25-benchmarks-and-expressiveness.md) §25.9's compile-speed budgets are built**, and its
  Phase 3 row is down to the CLBG harness alone.
- **[`13`](13-testing.md) §13.7's keystroke→diagnostic latency has a number** for the first time,
  and a way to produce it again.
- **A phase of the compiler was quadratic and nothing knew**, per §64.2. Three phases of programs,
  30 corpus files, 14 benchmarks and two SICP chapters all ran through it without the shape being
  visible, because every one of them is small enough that a quadratic and a linear cost the same.
- **[`42`](42-security-assurance.md) §42.11's gate covers one axis of two**, per §64.4.

## 64.9 What Phase 3 is still not

Unchanged from [`63`](63-felleisen-report.md) §63.9. The exit criterion — an outside developer
building a non-trivial app from documentation alone — is not met and is not closer. Seven bullets of
the fourteen remain untouched, identity has its seam and not its relying party, and
[`23`](23-incremental-views-report.md) §23.19 still names them one at a time.
