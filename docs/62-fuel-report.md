# 62 — Phase 3, part 31: the budget is a default, not a ceiling

**Built.** `beck test --fuel`, and with it the three Are We Fast Yet benchmarks that could not run at
the size their suite measures at now do.

[`61`](61-deltablue-report.md) §61.3 ended by naming this as owed:

> Three is enough to stop calling it a finding and start calling it a missing feature. **`beck test`
> needs a `--fuel`, or a program needs a way to declare its own budget.**

## 62.1 What was wrong

The evaluator stops a call after 50,000,000 steps (`interp.rs::DEFAULT_FUEL`). That is a
runaway-program backstop and it is right: a tree-walker with an accidental infinite loop should stop
rather than run until somebody notices.

What made it wrong was that **nothing could raise it.** A backstop that cannot be raised is a
ceiling, and the difference only shows up when a program legitimately wants to be above it. Three of
the fourteen benchmarks in [`awfy/`](../compiler/awfy/) do:

| | Refused at | Reported in |
|---|---|---|
| `mandelbrot` | size 500 | [`53`](53-are-we-fast-yet-report.md) §53.3 |
| `havlak` | the published `find_loop_iterations = 50` | [`59`](59-havlak-report.md) §59.3 |
| `deltablue` | the published `n = 12,000` | [`61`](61-deltablue-report.md) §61.3 |

Each of those was reported as a fact about the *benchmark*. Three of them is a fact about the
**evaluator**, and that is the whole content of this change: nothing about the budget was wrong
except that it was the only one available.

## 62.2 What it is

One flag, and one builder behind it.

`Evaluator::with_fuel` sets the per-call step budget; `beck_eval::backend_with_fuel` is the backend
built with one; `beck test --fuel N` is how a person says it. The default is unchanged and is named
rather than inlined — `beck_eval::DEFAULT_FUEL` — so the flag's default *is* the backstop rather
than a second number that could drift from it.

**Per call, not per process.** `constant` and `function` each build a fresh `Interp`, so the budget
bounds one evaluation. That was already true; it is now written down, because "how much fuel does a
test get" has an answer only if you know what a call is.

`beck run` does **not** take the flag. A fold runs per event and each event is its own call, so a
long-running application never accumulates against the budget — and a fold that cannot finish one
event is the case the backstop is for.

## 62.3 What it buys, measured

All three benchmarks now run at the size their suite measures at. Release build, and the budget
needed is stated rather than rounded up silently:

| | at the suite's size | `--fuel` |
|---|---|---|
| `deltablue`, `n = 12,000` | **1 m 51 s** | 2,000,000,000 |
| `havlak`, 50 discarded runs | **1 m 32 s** | 4,000,000,000 |
| `mandelbrot`, size 500 | **43.4 s** | 4,000,000,000 |

Those are not published numbers and are not compared to anything —
[`25`](25-benchmarks-and-expressiveness.md) §25.9's rule survives this change unaltered. They are
here because "it fits now" is a claim that needs a number behind it, and because the *ratio* is the
useful part: the default budget is roughly an eightieth of what DeltaBlue's published configuration
needs, so no smaller default would have helped.

**The gates are unchanged.** `awfy/`'s files still run their reduced configurations, because a suite
that took three minutes per benchmark is a suite nobody runs. What changed is that the reduction is
now a *choice about the gate* rather than a limit of the tool, and
[`59`](59-havlak-report.md) §59.3's two tests — the answers do not depend on the parameter, and each
discarded run does the whole job — are what make that choice honest.

## 62.4 `mandelbrot` reaches the suite's *default* size, and that is worth more than the other two

`deltablue` and `havlak` now run at the configuration the suite **measures** at. `mandelbrot` does
something stronger: at size 500 it produces **191**, which is `Mandelbrot.java`'s own
`verifyResult` at the size Are We Fast Yet's published results are about.

[`53`](53-are-we-fast-yet-report.md) §53.3 had to record this as a caveat — the port verified at
size 1, which the suite publishes a value for but does not measure at, and §53.6 listed "the suite's
default sizes for `mandelbrot` and `nbody`" as not reached. Half of that is now reached, and it took
43.4 s and a flag rather than any change to the port.

The gate still runs size 1, for §62.3's reason. What changed is that the smaller size is a decision
about how long `cargo test` should take rather than a statement about what the language can do.

## 62.5 How it is tested

`awfy.rs::the_fuel_budget_is_a_default_rather_than_a_ceiling` asserts **both** directions on the same
program: a loop long enough to exhaust the default is stopped without the flag, and completes with
it. Both, for [`20`](20-phase-2-report.md)'s reason — a flag that raises a ceiling has to be shown
raising it *and* shown that the ceiling is still there without it, or it is indistinguishable from
having removed the ceiling.

That test uses a synthetic loop rather than a benchmark on purpose: it takes about a hundred seconds,
where the smallest benchmark that would demonstrate the same thing takes ninety.

## 62.6 What is **not** built

| | Status |
|---|---|
| A program-declared budget | **not built.** `--fuel` is a person's decision at the command line; a `@budget` on a definition, or a manifest, would be the program's. Nothing needs one yet |
| `--fuel` on `beck run` | **not built**, per §62.2, and the reason is that a fold is a call per event |
| A budget on the *depth* ceiling | **not built**, and deliberately: `Interp::with_max_depth` only lowers, because the number that is safe is a property of the host stack rather than of the program ([`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)). Fuel is not like that — it bounds work, not memory |
| A default that is not 50,000,000 | **unchanged.** §62.3's ratio is why: nothing between the old default and what a benchmark needs would have helped anybody |
| Reporting how much fuel a run used | **not built.** A program that ran out is told so; a program that nearly did is not |

## 62.7 What this corrects

- **[`61`](61-deltablue-report.md) §61.3's owed item is built**, and with it
  [`59`](59-havlak-report.md) §59.6's and [`53`](53-are-we-fast-yet-report.md) §53.3's.
- **[`53`](53-are-we-fast-yet-report.md) §53.3's sentence about `mandelbrot` needs reading
  differently.** "Nothing exposes that budget to a caller, so the size is out of reach of `beck
  test`" was true when written and is the thing this change removes. The size is reachable; the
  gate still does not run it, and §62.3 says why that is now a different decision.
- **The generated CLI reference gains a flag**, regenerated and checked in.

## 62.8 What Phase 3 is still not

Unchanged from [`61`](61-deltablue-report.md) §61.8. The standard-library bullet is done, Are We
Fast Yet is complete, and CLBG, the compile-speed budgets and the Felleisen table are outstanding
under [`25`](25-benchmarks-and-expressiveness.md) §25.9.

The exit criterion — an outside developer building a non-trivial app from documentation alone — is
not met and is not closer. Seven bullets of the fourteen remain untouched, identity has its seam and
not its relying party, and [`26`](26-arrangement-sharing-report.md) §26.9 still names them one at a
time.
