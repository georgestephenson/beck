# 118 — A scope stops its children, and which wasm can have threads

**Built.** A `parallel:` child that fails **stops the children after it**.
[`80`](80-a-scope-owns-its-children-report.md) §80.5 forecast the need — *"a backend that starts
them together needs a cancellation signal, and nothing designs one"* — and
[`117`](117-a-scope-runs-its-children-report.md) built that backend and left the forecast standing,
naming it §117.7's largest open item. This is the signal.

**"After it" is the whole finding, and it cost a defect to learn.** The obvious signal — any failing
child stops every sibling — passed its own gate and broke a test that had been green since
[`80`](80-a-scope-owns-its-children-report.md), eight times in forty runs and never when run alone:
two children that both raise became a race over *which error the scope reports*. §118.3 is that
story. Cancelling only the children an ordered join would never have reached makes this a change in
when work stops rather than in what a scope answers.

**Investigated, with the evidence, and the first answer was wrong.** The other half of the question
is whether the playground could run its children on threads too. "Wasm has no threads" turns out to
be false as stated — `wasm32-wasip1-threads` is a stable target whose module imports a real
`thread-spawn` — but it is **WASI, not a browser**, and the two places Beck compiles to wasm are
browser targets. There the answer is still no, and for three reasons of which the first is decisive:
`atomics` is unstable on the pinned channel. §118.5 separates the three facts, because a flat "no"
was an overclaim and a flat "yes" would be a worse one.
[`117`](117-a-scope-runs-its-children-report.md)'s ordered fallback stands, and it is correct rather
than degraded.

**What the signal costs the programs that never use it** is the part `AGENTS.md` asks for, because
it rides `Interp::burn` — the one path every evaluation step passes through, and the path
[`70`](70-the-evaluator-gets-fast-report.md) spent a chapter on. Measured at two sizes with the
check and without it: **388.3 / 380.1 ns per iteration against 385.0 / 374.7**, a difference of
about 1% that is **inside the variance between two runs of one build**, and flat across a 10× change
in size. §118.4 says what that does and does not establish.

---

## 118.1 What a cancellation signal has to be, and what it does not

The temptation is a mechanism: a token, a scheduler, a cooperative-yield protocol. None of that is
needed, because of what [`80`](80-a-scope-owns-its-children-report.md) already proved in the
*checker*. A child cannot name another (`B0398`) and cannot perform an effect another could observe
(`B0399`), so stopping one is not a coordination problem — nothing is waiting on it, and nothing can
tell whether it finished.

So the signal is a flag, and the only two questions worth answering are **where it is read** and
**what a stopped child's error means**.

## 118.2 It rides the step counter

Cancellation is read in `Interp::burn`, which is the one place every evaluation step already passes
through. That is deliberate and it is the cheap answer rather than the obvious one:

- **A checkpoint at call boundaries would miss a loop.** `enter()` is where depth is charged, and a
  call in *tail* position does not nest — [`27`](27-the-walls-come-down-report.md) §27.2 is the
  whole point — so a child spinning in an iterative loop would never reach one. Fuel is charged
  there; that is exactly why fuel is the right hook.
- **"The program is making progress" is when stopping it is both possible and worth doing.** A child
  blocked in `http_fetch` is not spending steps, and this does not interrupt it: it is stopped when
  the host answers and it takes its next step. §118.6 is what that leaves open.

The field is `Option<Arc<Cancel>>` and it is `None` for every interpreter that is not a `parallel:`
child — which is every one the runtime, the LSP and `beck test` build. So the cost to a program that
never writes `parallel:` is a branch on a discriminant that is `None` for the whole run, and
loop-invariant where it is not.

## 118.3 The finding: the obvious signal is wrong, and a flaky test is what said so

The first version was the obvious one — a flag any failing child sets, stopping every sibling. It
passed its own new gate, passed the concurrency suite when that suite was run alone, and **failed
eight times in forty runs of the whole suite**.

The failure was `a_childs_failure_joins_at_the_scope_and_the_earliest_child_wins`, which has been
in the suite since [`80`](80-a-scope-owns-its-children-report.md) and whose third case runs two
children that raise *different* errors and expects the earlier one. With a stop-everybody flag that
case is a race: whichever child got to its `raise` first cancels the other before it can raise, so
the scope answers `Second` or `First` depending on the scheduler. That is precisely the property
[`117`](117-a-scope-runs-its-children-report.md) §117.3 exists to keep — *"the error a scope reports
is a function of the program rather than of the scheduler"* — broken by the feature added to serve
it.

**The fix is to stop the children *after* the failure, and only those.** What is stored is not
"somebody failed" but the **lowest index that has failed**, and a child stops only when a child
before it has. That set is exactly the one an ordered join would never have reached: under one, a
failure at child `i` means the children after `i` never ran and the children before `i` had already
finished. So cancellation becomes a change in *when work stops* and not in *what the scope answers*,
which is the only version of this feature that can be correct.

Two things about how this was caught are worth keeping:

- **It passed alone and failed in company.** Sixty runs of the test on its own were green; the
  failures only appeared when the rest of the suite was competing for cores. A gate that is only run
  in isolation would have shipped this.
- **The test was written for the ordered join** and was, at the time, a case that could not fail —
  [`117`](117-a-scope-runs-its-children-report.md) §117.3 noticed that it had become load-bearing
  when the children started running together, and said so. One report later it earned that
  description by catching the thing it was newly able to catch.

## 118.3.1 Why the signal is a chain

A scope may nest inside a scope, and a grandchild has to stop when an enclosing scope's earlier
child fails. One link cannot say that, so each scope keeps the scope it is inside **and its own
index within it**, and asking walks the chain. It is as deep as the scopes are nested, which
`a_scope_nests_inside_a_scope` puts at two.

A stopped child's error is also discarded rather than answered with: it did not fail, it was not
allowed to finish. That is belt and braces given the rule above — a stopped child is never the
earliest failure — and it is what makes the message a program sees honest.

## 118.4 What it costs, and what that number is worth

From `measure_concurrency.rs::what_the_cancellation_check_costs_a_program_without_a_scope`, release,
median of nine, on a program with no scope in it at all — the one that pays for this and gets
nothing:

| iterations | with the check | without it |
|---|---|---|
| 40,000 | 388.3 ns | 385.0 ns |
| 400,000 | 380.1 ns | 374.7 ns |

**The honest reading is that this harness cannot resolve it.** The difference is about 1%, and an
earlier run of the *same* build measured 410.8 ns at the smaller size — so the gap between the two
columns is smaller than the gap between two runs of one column. What the two sizes *do* establish is
the shape: the cost per iteration does not grow with the number of iterations, which is what
"loop-invariant branch" predicts and what a check that had been put somewhere worse would violate.

Saying "about 1%, and below what this can measure" is the claim. "Free" would be a stronger claim
than the evidence supports.

## 118.5 Can wasm have threads? Yes — but not the wasm Beck runs in

The question was asked properly, and the first answer was wrong. "Wasm has no threads" is false as
stated: **`wasm32-wasip1-threads` is a stable Rust target and it really spawns.** The evidence is
the compiled module rather than the documentation — the same probe built for each target:

| target | what `thread::Builder::spawn` compiles to |
|---|---|
| `wasm32-unknown-unknown` | the string `operation not supported on this platform` — `std`'s `Unsupported` stub |
| `wasm32-wasip1-threads` | an import named **`thread-spawn`** — the WASI call |

So the honest shape of the answer is three separate facts, and only the first two constrain anything
today.

**One: wasm32 is the only *bitness*.** `rustup target list` has no `wasm64` on stable at all;
memory64 is a nightly target. That part of the question has a flat answer.

**Two: the wasm Beck actually runs is the browser's, and there the answer is still no.** The two
places this repo compiles to wasm — `beck-wasm` for a Mode B client and `beck-play` for the
playground — are browser targets, so they are `wasm32-unknown-unknown`, and the workflow builds only
that. Threads there need all three of: the `atomics` target feature, which `rustc` answers is
*"not stably supported"* on the pinned 1.94.1; a `std` rebuilt with it, which is `-Z build-std` and
therefore nightly; and `SharedArrayBuffer` in the page, which needs COOP/COEP headers
[`98`](98-playground-report.md)'s server does not set, plus a worker per thread. The first of those
alone is decisive, because `rust-toolchain.toml` pins a channel and [`07`](07-dependencies.md)
treats that as a dependency like any other.

**Three: there is a wasm tier where this *will* matter, and it is not built.**
[`05`](05-tier-lowering.md) §5.4 and [`07`](07-dependencies.md) §7 name **Wasmtime** for server-side
WASM — "edge/serverless/multi-tenant placement". Nothing in this repo emits it yet. When something
does, `wasm32-wasip1-threads` is the target on which a `parallel:` scope keeps its threads, and this
section exists so that whoever builds it does not have to find that out again.

**What stands, unchanged, is the fallback.** The playground running a scope's children in order is a
*correct* implementation of the form for §118.1's reason — the order is unobservable, and one order
is an order. It loses the overlap, never an answer, which is a different thing from a feature that
does not work there. And a Mode B **client** cannot contain a scope at all: `spawn` is
`Tier::Server`'s alone and `B0401` refuses one pinned to the browser.

## 118.6 The gate

**`concurrency.rs::a_failing_child_stops_its_siblings`** — a count, not a clock, and two-sided:

- The failing child works for five host calls and *then* raises, so the sibling has provably started.
- The sibling would call its peer **400** times if nothing stopped it. The assertion is that it
  called it **more than zero** times — so this is cancellation and not a child that never ran — and
  **fewer than 200** — so it was stopped. It reaches **4**.
- The scope's answer must be the **raise**, never the cancellation.

**Checked by making it fail**, which is [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md)
§84.5's discipline: with the signal disabled the sibling reaches its peer **400 times out of 400**
and the gate goes red on the second assertion. With cancellation it reaches 4 or 5.

**And the ordering gate was run forty times, not once.** §118.3's defect was invisible to a single
run, so the check that it is gone is forty runs of the whole suite: **8 failures out of 40 before,
0 out of 40 after**. A concurrency fix verified by one green run is not verified.

## 118.7 What this does not establish

- **Nothing about a child blocked in the host.** Cancellation is read where steps are spent, so a
  child waiting on `http_fetch` is stopped when the call returns and not while it is in flight. A
  scope whose first child fails still waits for a sibling's outbound call to come back. Interrupting
  *that* is a property of the [`net`](../compiler/crates/beck-core/src/net.rs) seam — a deadline, or
  a request that can be abandoned — and it is a different change with a different design in front of
  it.
- **Nothing about undoing what a stopped child did.** A cancelled child may already have performed
  an effect, and nothing rolls it back. `B0399` means no sibling could have observed it, and the
  scope fails regardless — but "the request was sent and the answer discarded" is a real outcome and
  is said here rather than discovered.
- **Nothing about cancelling from outside.** There is no way for a caller, a deadline or a
  disconnected client to stop a scope; the only thing that sets the flag is a sibling's failure.
- **Nothing about the server-side WASM tier** (§118.5). `wasm32-wasip1-threads` would keep a
  scope's threads and nothing in this repo emits that target, so the claim is about what a future
  tier *could* do rather than about anything that runs. Nothing about the compiled backends either, where
  [`116`](116-the-host-answers-back-report.md) §116.10's one-pipe-one-lock still means two children
  that reach compiled code serialise.

## 118.8 What this corrects

- [`117`](117-a-scope-runs-its-children-report.md) §117.3's *"That backend now exists and the signal
  still does not"*, and §117.7's *"Nothing about cancellation … the largest single item"*. Both were
  true when written and are what this closes.
- [`80`](80-a-scope-owns-its-children-report.md) §80.5's table row — *"A child that is cancelled when
  a sibling fails — **not built**"* — with the forecast's reasoning intact: it said a backend
  starting children together would need a signal, and it did.
- **`concurrency.rs`'s module documentation** named `two_children_actually_overlap` as the gate for
  the concurrency claim. There are two gates now, and the second is the one about stopping.
- **This report's own first implementation** stopped every sibling rather than the ones after the
  failure, which made a scope's answer scheduler-dependent (§118.3). It never shipped; the record is
  here because the gate that caught it is the reason to keep running suites in company rather than
  alone.
- **This report's own first draft** said flatly that wasm cannot have threads, which is false:
  `wasm32-wasip1-threads` is stable and spawns. The claim that survives is narrower and is §118.5 —
  the *browser* targets cannot, on a pinned stable channel. Recorded here rather than quietly edited,
  because the difference between "wasm cannot" and "the wasm we compile to cannot" is exactly the
  kind of overclaim [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5 is about.
