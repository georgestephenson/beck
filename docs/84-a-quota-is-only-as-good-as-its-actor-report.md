# 84 — Phase 3, part 53: a quota is only as good as its actor

**Built.** F3's per-actor write quota, on by default with generous limits, enforced at the merge
point on the injected clock — so validated spam from one abusive session stops becoming permanent
storage.

[`14`](14-review-findings.md) F3 has been `APPROVED` and unbuilt since the review: "quotas are **on
by default** with generous limits, overridable per command type." The decision was taken; only the
build was missing. It is the third of [`42`](42-security-assurance.md) §42.6's four bullets to
close, after [`83`](83-the-runtime-edge-report.md) took two.

Two things in this report are worth more than the quota. §84.4 is what the bound is actually worth,
which is less than the word "per-actor" suggests. §84.5 is that **both of the gates guarding this
gap stayed green through the change that closed it**, and why.

## 84.1 Where it goes, and why there

F3 splits "events are forever" in two. Channel (a) — rejected garbage — was closed by §3.7's rule
that only validated events are durably logged, so a refused command leaves nothing behind. Channel
(b) is validated spam from a legitimate session, permanent by design, and the only place to stop it
is before it becomes an event.

So the charge is in `App::propose`, before the proposal enters the ingress queue:

```rust
let at = Instant(self.config.clock.now_millis());
if !self.limit.admit(&actor, at.0) { … }
```

Two details, both consequences of decisions this project already took:

* **Before the queue, not in the sequencer.** A proposal nothing will admit should not occupy a slot
  in a bounded channel; refusing after queueing would make a quota into a slower queue.
* **From `at`, not from a second clock.** §3.7 makes the merge point "the one place time enters",
  and F11 makes the clock a dependency rather than an ambient call. The window is computed from the
  same instant that goes on the envelope, so a process given a stated clock has a stated quota and
  `beck_core::clock`'s "exactly one place" test stays true.

## 84.2 The table is bounded, which is the part that is easy to get wrong

The obvious implementation is a map from actor to a counter. That map is **unbounded memory keyed
by a string the client chooses** — the same denial of service the quota exists to prevent, moved one
level down and made harder to see.

So the counters are **sharded**: 1,024 buckets, an actor hashed into one, and no per-actor
allocation ever. 16 KiB for the life of the process, whatever arrives.
`ten_thousand_actors_allocate_nothing` is the assertion, and it needs no measurement because there
is no per-actor storage to have grown.

Two consequences, both deliberate:

* **Two actors can share a bucket, and therefore a budget.** That is *why* the limit is generous
  rather than tight — a shared bucket must still be ample for both and still bite a script.
* **The hash is keyed per process.** `RandomState` is the standard library's answer to precisely
  this question — it is what stops a caller choosing colliding `HashMap` keys — and it seeds itself
  from the OS. Without a key, sharing a bucket stops being an accident an operator accepts and
  becomes a way to spend somebody else's budget on purpose.

Using `RandomState` rather than minting a key by hand also keeps the workspace's `forbid(unsafe)`
intact. The first version of this module derived a key from a heap address and needed `unsafe` to do
it; the standard library had the right facility all along.

## 84.3 The numbers

**600 events a minute** — ten a second sustained, over a one-minute window.

The number comes from what a *person* can produce: a fast typist committing a todo per keystroke
does not reach it, and any UI that batches at all is nowhere near. It is deliberately far above
interactive use and far below what a script does in a second, which is the gap F3 asks to be closed.
`the_quota_is_on_by_default_and_generous` asserts the shipped default rather than a configured one,
so turning it off by accident is a failing test.

A refusal is counted as `throttled`, apart from `rejected` and `unauthenticated`: "you may not do
that", "who are you" and "not that often" are three different things for an operator watching an
attack, and one number covering all three tells them nothing.

**The first thing the quota refused was this project's own scaling harness**, which proposes two
thousand events from one actor as fast as the machine will take them. That is not a bad sign; it is
the number doing exactly what §84.3 says it should, on the first script it met. `scaling.rs` now
asks for `Quota::unlimited()` and says why in a comment — what it measures is the *shape* of a fold,
and the quota is not the thing under test. It is worth recording because it is the general shape of
living with a limit calibrated against human behaviour: **a benchmark trips it before an attacker
does**, and every harness that drives the merge point has to say which of the two it is.

## 84.4 What the bound is actually worth

**It binds an actor, so it is worth exactly what an actor is worth.**

Under [`crate::identity::DevIdentity`] — the default, deliberately
([`48`](48-identity-report.md)) — the actor is the claim the client sent. An attacker who rotates
names therefore does not exhaust one bucket; they spread across all of them. The total is still
bounded, at `1,024 × 600` events a minute rather than `600`, which is a bound rather than the bound
anybody wanted.

That is not a defect in the quota and it is not fixed by making the quota cleverer. It is the
composition of two things this project has documented separately and never multiplied together, and
`rotating_actor_names_is_bounded_by_the_table_rather_than_by_the_limit` is that multiplication as a
test: 200,000 proposals under distinct names, and the assertion is that no more than `BUCKETS ×
limit` were admitted. The test also asserts the buckets fill *roughly evenly*, because a hash that
concentrated would make the ceiling unreachable and the test vacuous.

The seam that fixes it exists: a `SignedIdentity` mints an actor the client cannot choose, and then
one actor is one bucket. This report does not build that — it records that F3's value is a function
of which provider is configured, which is a sentence neither [`14`](14-review-findings.md) F3 nor
[`48`](48-identity-report.md) contains and both imply.

## 84.5 The gates did not fire

Both `pending_security.rs` tests guarding this gap **stayed green through the change that closed
it.** That file's entire premise is the opposite:

> the day somebody builds one of these, its test goes red, and the person who built it has to come
> here and to the documents and say so.

Neither did. Why, in each case:

* `no_quota_limits_what_one_actor_can_write_to_the_log` grepped the workspace for `rate_limit`,
  `per_actor_quota` and `QuotaConfig`. What got built is `RateLimit`, `Quota` and `quota::admit`, so
  it matched nothing. **A name grep is a proxy for a control, and a proxy is defeated by naming** —
  not deliberately, just by somebody choosing different words later.
* `one_actor_may_fill_the_log_unchecked` was the behavioural half and sent **200** proposals. The
  limit eventually chosen is 600, so it passed under it. **A behavioural test for an absence has to
  be calibrated against a limit that does not exist yet**, which nobody can do.

The second is the more interesting failure, because it is not carelessness — the test's own comment
says "a small number, because the point is that nothing refuses — not how fast it does not", which
is exactly right *as a statement about an absence* and exactly wrong as a tripwire. The honest form
is a **ratio or a shape**: propose more than any plausible limit, or assert that the head grows
without bound rather than that it reaches 200.

What replaces both is in `runtime_edge.rs`, and it asserts on the **log's head** rather than on
identifiers: fifty proposals, a limit of five, forty-five refusals, and a head that stops at five.
"The proposal was refused" and "nothing was written" are different claims, and F3 is about the
second.

This is the fourth gate-that-could-not-fail this project has found, after
[`78`](78-a-record-is-a-permutation-report.md) §78.6's three and
[`80`](80-a-scope-owns-its-children-report.md) §80.6's harness that was cited by name and never
written. The pattern across all four: **the gate was written by the person who knew the gap, and
tested the shape of the gap rather than the shape of the fix.**

## 84.6 What is not built

| | |
|---|---|
| "Overridable per command type" | **not built.** F3's decision has that clause and this does not implement it: the quota counts events, not events-of-a-kind. Doing it properly means the *program* naming which commands are cheap, which is language surface rather than a config field, and it wants its own change |
| A quota that survives a restart | **not built.** The counters are in memory, so a process restart is a fresh window. For a rate limit that is the usual and correct behaviour; for F3's *volume* half — "rate/volume quotas" — it is not, and a volume quota over the life of an actor would have to be folded from the log rather than counted beside it |
| Per-actor crypto-shredding | **not built.** F3 names it as the abuse *cleanup* path, and it is a different mechanism from the quota that limits the damage |
| F15's subscription and connection quotas | **not built**, and its `pending_security` test is still green — with the caveat §84.5 now attaches to every grep-shaped test in that file |
| A cost model behind the number | **not built.** 600 is argued from human typing speed (§84.3), not derived from what an event costs to store or to fold. F15's design asks for "per-view cost budget from the solver's own estimates", which is the shape a derived number would take |

## 84.7 What this establishes

**That a control and its gate are written by the same person, and that is the problem.** Four times
now this project has found a test that could not fail, and every one was written alongside the thing
it was meant to watch, by somebody holding the right idea in their head at the time. The defence is
not more care; it is to make the gate assert an *observable* — a log head, a printed file, a step
count — rather than a name or a number that seemed generous when it was written.

**And that "per-actor" is a claim about identity, not about counting.** The quota was the easy half.
The half worth writing down is that a per-actor bound composes with whichever identity provider is
configured, so a deployment on `DevIdentity` has a quota worth 1,024 times less than its
configuration says. Neither document that owns half of that sentence contained it.
