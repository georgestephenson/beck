# 82 — The edge: what faces a stranger

**Built.** The four things that stand between a Beck deployment and somebody who was not invited:
the **websocket handshake** checks `Origin` against `Host` and the socket's limits are numbers this
project chose rather than numbers its library chose; a **per-actor write quota** is on by default at
the merge point; the **front end** answers every input with a diagnostic rather than an abort, after
grammar-aware fuzzing found three productions the recursion ceiling did not cover; and the
**generated pod** drops every capability, refuses privilege escalation, takes the runtime's default
seccomp profile and gets a read-only root filesystem unless the program's own effect row says it
writes a file.

They are one chapter because they close one list. [`42`](42-security-assurance.md) §42.6 asks what
an untrusted client can do to a running Beck app and answers with four bullets; §42.9 names
grammar-aware fuzzing as the method that finds the rest of the recursion class;
[`06`](06-kubernetes-and-packaging.md) §6.5 lists four pod defaults it calls *unavoidable*; and
[`14`](14-review-findings.md) F3 has been `APPROVED` and unbuilt since the review. §82.1 is the
scoreboard against all of that.

They are also one chapter because **three of the four ended on the same finding**, arrived at
independently: a claim in a document is not a claim anything checks, and a gate written by the
person who knew the gap tests the shape of the gap rather than the shape of the fix. §82.10 is that
finding stated once, with the four gates it is drawn from.

## 82.1 The scoreboard

| §42.6's question — what can an untrusted client do? | Then | Now |
|---|---|---|
| Open a socket from any page on any host | yes | **refused** (§82.2) |
| Send a 64 MiB message, hold 128 KiB of read buffer, grow an unbounded write buffer | yes | **bounded by this project's numbers** (§82.3) |
| Claim any identity | yes | **unchanged.** `DevIdentity` believes the client, deliberately ([`48`](48-identity-report.md)) |
| Spend the log without limit | yes | **bounded per actor** (§82.4), and that bound is worth what the actor is worth (§82.5) |
| §42.9 — reach past the front end's recursion ceiling | three ways | **refused with a span** (§82.6) |
| §6.5's four "unavoidable" pod defaults | one of four emitted | **four of four** (§82.8) |

Everything below is how, and what each one is not.

## 82.2 The handshake, and the three ways the origin rule could have gone

Nothing above the upgrade looked at `Origin`. A Beck app's page is served by the app itself and the
socket carries whatever identity the visitor's browser has, so a page on any other host could open
one, send a `hello` and a command, and be a subscriber. That is cross-site WebSocket hijacking, and
the reason to fix it while identity is still `DevIdentity` is that the fix is independent of who the
actor turns out to be: the same-origin check is about **which page asked**, not about who is asking.

`Origin` is set by the browser and cannot be forged by a script, which is why it answers the one
question worth asking: is the page requesting this socket the page this server rendered? The check
compares `Origin`'s authority to `Host` and answers `403` when they differ. Three decisions, each of
which could have gone the other way:

* **An absent `Origin` is allowed.** Non-browser clients do not send one — a script, a load
  generator, a future `beck` subcommand — and the attack this defends against *needs* a browser,
  which always sends one. Refusing an absent header would break every non-browser client for no
  security gain, because an attacker running their own client was never subject to a browser's rules
  in the first place.
* **The scheme is not compared.** Behind a TLS-terminating gateway — which is exactly what
  [`06`](06-kubernetes-and-packaging.md) §6.5's HTTPRoute is — the page is `https://app.example` and
  the request arriving here is plain HTTP. Comparing schemes would refuse every deployment this
  project generates.
* **There is no allowlist.** A Beck app serves its own page (§5.2's first paint), so same-origin is
  a *description* of the architecture rather than a policy chosen over alternatives. A deployment
  that genuinely needs a cross-origin client has nothing to configure, and
  [`43`](43-threat-model.md) §43.4 records that as the part still absent rather than leaving it
  implied.

`Origin: null` — a sandboxed iframe, a `file://` page — has no authority, matches no host, and is
refused. That is the answer it should get and it falls out of the rule rather than being a case.

**`beck-cli/tests/runtime_edge.rs` is the first test in the project to drive `beck-rt`'s HTTP
edge.** Every harness that touches a session goes through `beck_rt::session::run` over an in-memory
duplex — which is what the `Socket` trait exists for, and is right for testing a subscription — and
the consequence is that nothing had ever exercised the handshake in front of it. That matters here
specifically: a refusal wired into `upgrade` and tested only as a pure function is a refusal one
refactor away from never being called. So the client is a `TcpStream` and a literal request, which
is what a browser sends, and the assertions are on the status line:

| request | answer |
|---|---|
| `Origin: http://<host>` — the server's own page | `101` |
| no `Origin` — not a browser | `101` |
| `Origin: https://evil.example` | `403` |
| `Origin: null` | `403` |

Both directions in one test on purpose. "Cross-origin is refused" is worth nothing without
"same-origin still works" beside it, and a check that refused everything would satisfy the first
assertion alone. The rule itself is tested where it is a rule — six unit tests beside `same_origin`,
including the one that would catch the obvious wrong implementation: `app.example.evil.test` must
not pass as `app.example`, which a `starts_with` or a `contains` would let through.

## 82.3 The socket's numbers, and the argument for each

What was there was `None`:

```rust
WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None)
```

so the limits were tungstenite's — bounded, and bounded well, but by somebody else's judgement about
somebody else's protocol.

| | was | is | why |
|---|---|---|---|
| `max_message_size` | 64 MiB | **256 KiB** | a client sends a `hello` naming a subscription and an actor, or a `Cmd` carrying one value of the program's own `union Command`. The largest field either can hold is text a person typed into a form, and 256 KiB is around a hundred pages of it |
| `max_frame_size` | 16 MiB | **256 KiB** | a message that fits needs no larger frame |
| `read_buffer_size` | 128 KiB | **8 KiB** | **eagerly allocated per connection.** §5.3 makes per-subscriber memory a number this project reports rather than hopes about, and the library's default is tuned for high read load — a Beck client sends a few hundred bytes when somebody clicks something |
| `max_write_buffer_size` | unbounded | **8 MiB** | it only grows past `write_buffer_size` when writes are *failing*, so this is backpressure against a client that has stopped reading rather than a ceiling on what a healthy one is sent |

`write_buffer_size` is left at 128 KiB: batching outgoing patches is what it is for, and it is a
threshold rather than a per-connection allocation. Outgoing patches are unaffected —
`max_message_size` and `max_frame_size` bound what is *read*, which is the half an untrusted client
controls.

The read-buffer figure is arithmetic rather than a measurement, and is labelled as such: the library
documents that buffer as eagerly allocated, so a thousand connections hold 128 MB of it at the old
default and 8 MB at the new one. Nothing here has measured RSS at a thousand connections, and
§82.11 lists that as owed rather than done. `socket_limits()` has a test asserting each number *and*
asserting each is tighter than the library's default, so a drift back to `None` is a decision rather
than an edit.

## 82.4 A quota goes at the merge point, and its table has to be bounded

F3 splits "events are forever" in two. Channel (a) — rejected garbage — was closed by §3.7's rule
that only validated events are durably logged, so a refused command leaves nothing behind. Channel
(b) is validated spam from a legitimate session, permanent by design, and the only place to stop it
is before it becomes an event. So the charge is in `App::propose`, before the proposal enters the
ingress queue:

```rust
let at = Instant(self.config.clock.now_millis());
if !self.limit.admit(&actor, at.0) { … }
```

Two details, both consequences of decisions this project already took. **Before the queue, not in
the sequencer**: a proposal nothing will admit should not occupy a slot in a bounded channel, and
refusing after queueing would make a quota into a slower queue. **From `at`, not from a second
clock**: §3.7 makes the merge point "the one place time enters", and F11 makes the clock a
dependency rather than an ambient call, so a process given a stated clock has a stated quota and
`beck_core::clock`'s "exactly one place" test stays true.

**The table is bounded, which is the part that is easy to get wrong.** The obvious implementation is
a map from actor to a counter, and that map is unbounded memory keyed by a string the client
chooses — the same denial of service the quota exists to prevent, moved one level down and made
harder to see. So the counters are **sharded**: 1,024 buckets, an actor hashed into one, and no
per-actor allocation ever. 16 KiB for the life of the process, whatever arrives.
`ten_thousand_actors_allocate_nothing` is the assertion, and it needs no measurement because there
is no per-actor storage to have grown. Two consequences, both deliberate:

* **Two actors can share a bucket, and therefore a budget.** That is *why* the limit is generous
  rather than tight — a shared bucket must still be ample for both and still bite a script.
* **The hash is keyed per process.** `RandomState` is the standard library's answer to precisely
  this question — it is what stops a caller choosing colliding `HashMap` keys — and it seeds itself
  from the OS. Without a key, sharing a bucket stops being an accident an operator accepts and
  becomes a way to spend somebody else's budget on purpose. Using it rather than minting a key by
  hand also keeps the workspace's `forbid(unsafe)` intact: the first version of this module derived
  a key from a heap address and needed `unsafe` to do it.

**600 events a minute** — ten a second sustained, over a one-minute window. The number comes from
what a *person* can produce: a fast typist committing a todo per keystroke does not reach it, and
any UI that batches at all is nowhere near. It is deliberately far above interactive use and far
below what a script does in a second, which is the gap F3 asks to be closed.
`the_quota_is_on_by_default_and_generous` asserts the shipped default rather than a configured one,
so turning it off by accident is a failing test. A refusal is counted as `throttled`, apart from
`rejected` and `unauthenticated`: "you may not do that", "who are you" and "not that often" are
three different things for an operator watching an attack, and one number covering all three tells
them nothing.

**The first thing the quota refused was this project's own scaling harness**, which proposes two
thousand events from one actor as fast as the machine will take them. That is the number doing
exactly what it should, on the first script it met. `scaling.rs` now asks for `Quota::unlimited()`
and says why in a comment — what it measures is the *shape* of a fold, and the quota is not the
thing under test. It is the general shape of living with a limit calibrated against human
behaviour: **a benchmark trips it before an attacker does**, and every harness that drives the merge
point has to say which of the two it is.

## 82.5 What "per-actor" is actually worth

**It binds an actor, so it is worth exactly what an actor is worth.**

Under `DevIdentity` — the default, deliberately ([`48`](48-identity-report.md)) — the actor is the
claim the client sent. An attacker who rotates names therefore does not exhaust one bucket; they
spread across all of them. The total is still bounded, at `1,024 × 600` events a minute rather than
`600`, which is a bound rather than the bound anybody wanted.

That is not a defect in the quota and it is not fixed by making the quota cleverer. It is the
composition of two things this project has documented separately and never multiplied together, and
`rotating_actor_names_is_bounded_by_the_table_rather_than_by_the_limit` is that multiplication as a
test: 200,000 proposals under distinct names, and the assertion is that no more than
`BUCKETS × limit` were admitted. The test also asserts the buckets fill *roughly evenly*, because a
hash that concentrated would make the ceiling unreachable and the test vacuous.

The seam that fixes it exists: a `SignedIdentity` mints an actor the client cannot choose, and then
one actor is one bucket. This chapter does not build that — it records that F3's value is a function
of which provider is configured, which is a sentence neither [`14`](14-review-findings.md) F3 nor
[`48`](48-identity-report.md) contains and both imply. The finding generalises to **any per-actor
structure**, and the ones built since cite it: presence, subscription accounting, anything keyed by
a name the client supplies.

## 82.6 The front end answers, and the three productions that did not

The property asserted over generated programs is one line, and it is the only one worth asserting
about arbitrary input:

> **The front end answers.** For every generated program, it either accepts it or produces
> diagnostics — never an abort, never a panic, never a failure to terminate.

Not "it compiles". Most generated programs are nonsense, and a generator that only produced valid
ones would be testing the wrong half.

The forecast was §42.2's, quoting the Scriban advisory (GHSA-p6q4-fgr8-vx4p): **a limit added at the
one production somebody thought of is bypassed through a different one.** That sentence had been in
the tree since Wave 0 as a warning. It was also a description. Each of the three was refused
correctly for one shape and aborted the process for another, which is what made them findable: the
generator varies the *production* while holding the depth, so a shape that survives is the control
for the one that does not.

**One — the type grammar was not counted at all.** `Parser::type_expr` recurses in four places and
never called `enter`. `list[list[list[…]]]` 80,000 deep aborted; `((((…))))` 80,000 deep was refused
with a span, because parens recurse through `primary`, where the counter is. A whole production with
no counter, which is the Scriban shape in its plainest form.

**Two — the counter was released before the recursion happened.** `Parser::primary` enters, reads a
leaf, and *leaves*. The recursion that makes `g(g(g(…)))` deep happens afterwards, in `postfix`'s
loop, through `call_args` → `expr` → `postfix`. So the depth returned to **zero at every level** and
80,000 nested calls aborted while 80,000 nested parens did not. This is the subtler one: the counter
was present, on a function on the path, and still saw nothing — because it measured the wrong
interval.

**Three — an iterative parser builds a deep tree without recursing.** `1 + 1 + 1 + …` is
left-associative, so the Pratt loop `expr_bp_from` reads it **without recursion at all** and builds
a left-leaning tree of the same depth. No recursion counter can see that, because there is no
recursion; the depth is real in the tree and shows up later, in whatever walks or drops it. At
120,000 terms the macro expander's own ceiling happened to catch it (`B0213`) — the wrong counter,
for the wrong reason, and only by luck of ordering. At 300,000 the process died before anything
could report. This is the one that changes how to think about the class: **a recursion counter
counts recursion done; what the stack cares about is tree depth built**, and those are the same
number only when the parser is recursive.

| | the fix |
|---|---|
| `type_expr` | enters and leaves, like every other production |
| `postfix` | enters and leaves around the **whole chain**, not just the leaf. A nested expression now spends two levels of the ceiling rather than one, which is affordable at 256 against a corpus whose deepest expression is 11 |
| `expr_bp_from` | counts its **iterations** against `MAX_BLOCK`, and refuses with `B0122`. Not a recursion counter: a bound on how long a flat run may be, which is the same thing `MAX_BLOCK` already means for a block of sequential bindings |

A fourth was found before the generator existed, by reading [`64`](64-compile-speed-report.md) §64.4,
and is fixed here because a generator reaching 120,000 would have found it too: **a flat block of
sequential bindings** recursed once per statement with nothing counting, so a debug build aborted at
12,000 and a release build at 100,000 — "which programs compile depended on how the compiler was
built", precisely the property
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) says a ceiling must never have.
`MAX_BLOCK` is 2,048, refused with `B0389`, and `the_block_ceiling_fits_the_declared_stack` measures
the checker at **6.8 KiB a statement** in an unoptimised build — so the ceiling costs 14 MiB, and
28 MiB with the doubled margin the parser's and the evaluator's tests also apply, against a declared
64 MiB. A million-term operator chain and a 400,000-deep call are now both diagnostics with spans.

**Two gates had to move, and neither was wrong before.** `compile_speed.rs`'s depth axis measured
6,400 sequential bindings, which is now a program the front end refuses — so the gate would have
been measuring error recovery. Its three axes now name their own sizes and the depth one runs
100 → 1,600, which is not a weakening: the gate asserts a *shape* (cost per declaration must not
grow with the count), sixteen times as many is the measurement whatever the absolute numbers are,
and **the ceiling is a safety property that cannot move to suit a benchmark while the ratio is
arbitrary and can.** `front_end_bound.rs` asserted that 5,000 sequential bindings *must compile*,
which was correct for the world §64.4 described, where the stack was what bounded the axis; it now
asserts the property that replaced it — under the ceiling compiles, over it is `B0389` with a span,
and the number does not move with the profile. That test **fired**, which is the good case and worth
saying next to §82.10's four that did not.

**And the ceiling's own tests needed the declared stack, which CI found and this machine did not.**
`MAX_BLOCK` is sized against `beck_diag::depth::STACK_BYTES` — 64 MiB — and a default test thread has
2 MiB, so the first version of `a_block_past_the_ceiling_is_refused_with_a_diagnostic` called the
checker directly and aborted the test binary. It is the same lesson
[`80`](80-structured-concurrency-report.md) §80.11 recorded when `roundtrip.rs` overflowed on
`awfy/havlak.beck`: **a harness is a caller of the front end, and a caller has to honour the
declaration.** That is three harnesses that have had to learn it separately, which is an argument
for the entry point making it hard to get wrong rather than for remembering.

## 82.7 The sizes are the method

§42.1 ran 600 iterations of byte-level mutation over `compiler/corpus/*.beck`, found nothing, and
said why that was not reassuring:

> random mutation cannot *generate structure*, so the one crash class the front end actually has is
> precisely the one this method is blind to.

A mutated file is a *slightly wrong* file. The crash class is a *deeply nested* or *very long* one,
and no amount of flipping bytes in a 40-line program produces 80,000 nested calls. So the generator
does not mutate: it **builds** programs from the grammar, with the recursive productions
parameterised by depth and the flat ones by length.

The sizes are where the first version of this harness went wrong, and it is the part worth carrying
forward. It generated up to 3,000 — comfortably past `MAX_NESTING` (256) and `MAX_BLOCK` (2,048),
and comfortably short of the aborts those ceilings replaced, which §42.2 measured at 3,785 nested
parens and §64.4 at 12,000 flat bindings. **It passed, and meant nothing.** A ceiling is cheap to
test just past, because the counter stops immediately; the failure worth finding is the one where no
counter stops it. Raising the sizes to 120,000 turned three green tests into a process abort within
a minute. **A fuzzer calibrated against the limits you built tests that your limits work. A fuzzer
calibrated against the failures you had tests whether you have found all the places they happen.**

§42.11's row names `cargo-fuzz`, which needs libFuzzer and therefore nightly; this workspace pins
stable 1.94.1, and taking a nightly toolchain for one harness is a larger decision than one test
should make. `proptest` is already a dev-dependency, already used by `manifest_properties.rs`, and
shrinks failures to a minimal case. The substitution is honest because **the generator is the
contribution, not the driver**: what found these three was knowing which productions exist and
generating each independently at a size past the stack, not coverage feedback. What `cargo-fuzz`
would add is finding the productions nobody thought to enumerate, which is a real difference and is
§82.11's first row.

Two tests, and the split matters. The property test samples shapes and sizes; the enumerated one
walks every shape across every ceiling and both sides of it, because a random `n` in a range of
40,000 may never land on 256 or 2,048 and those are exactly where this class of failure lives. A
third asserts the refusals are the **counted** ones rather than any diagnostic at all — without it, a
parse error from a file so large the lexer gave up would satisfy "the front end answers" and mean
nothing.

Two smaller notes, because both are about verification rather than about the front end. A stack
overflow **aborts the test binary**, so it prints `fatal runtime error` and `error: test failed` and
*not* `test result: FAILED` — a check that greps for the usual failure strings misses it entirely,
and the exit code is the only reliable signal. And the type-grammar counter moved which pass refuses
a deep type: `a_type_past_the_ceiling_is_a_diagnostic_rather_than_an_abort` expected the *checker's*
`B0390` and now meets the *reader's* `B0121` a stage earlier. Its sibling for expressions already
said "whichever pass reaches it first", which is the right shape for both — the property in the name
is a claim about the front end, not about which half of it.

## 82.8 The pod's defaults, and the one that was not derivable until an atom split

[`06`](06-kubernetes-and-packaging.md) §6.5 has named four since the design was written:

> Non-obvious defaults that should be *unavoidable*, because they are what separates "generated
> YAML" from "production-grade generated YAML": non-root + read-only root filesystem + dropped
> capabilities + `seccomp: RuntimeDefault` …

One of the four was emitted. The reason the read-only root filesystem waited is worth the paragraph:
[`80`](80-structured-concurrency-report.md) §80.13 split `fs(path)` into `fs.read(path)` and
`fs.write(path)`, and with one `fs(path)` atom the emitter had exactly two choices and both were
wrong — hard-code `readOnlyRootFilesystem: true` and a program that writes a file gets a container
that refuses the write, or hard-code it `false` and every program that writes nothing, which is
every program in this repository, ships a writable root for no reason. An atom that could not say
which one the program was is why the field was simply absent. Now it is a function of the row:

```text
readOnlyRootFilesystem = not (the program performs fs.write(_))
```

Any path rather than a named one, deliberately. The flag is about the container's *root* filesystem,
and a program that writes anywhere needs it writable; matching paths against mount points is a
different question and §82.11 leaves it open. **The default is the secure one and the row is what
relaxes it**, which is the direction that fails safe: a program that writes a file and forgets to
declare it gets a loud failure at the point of the write rather than a container anybody can write
to. That is the same asymmetry §3.5 uses everywhere — an undischarged effect is a compile error, and
a *missing* declaration must never be the permissive answer.

The other three are not derived from anything, because nothing a Beck program can do needs a Linux
capability, needs to gain privileges partway through, or needs a syscall outside the container
runtime's default profile. They are applied to **every** container this emitter writes:

```yaml
securityContext:
  allowPrivilegeEscalation: false
  capabilities:
    drop:
      - "ALL"
  seccompProfile:
    type: "RuntimeDefault"
```

`revisionHistoryLimit: 2` comes with them. Two is enough to roll back to and enough to see what the
last rollout changed; the default is ten, and unbounded histories are how a cluster ends up holding
every ReplicaSet a deploy has ever made.

Three levels of testing, and the third is a gap. **The derivation moves**:
`the_root_filesystem_is_read_only_unless_the_program_says_it_writes` runs it three times — no
filesystem atom, `fs.read`, `fs.write` — and asserts `true`, `true`, `false`. The middle one is the
assertion that would have been impossible before the split, and the one that says the split was
worth taking: **reading a file is not a reason to make the root writable.** **The manifest set is a
reviewed diff**: the golden file moved by exactly these fields, and the snapshot gate is what made
that a decision rather than a surprise. **Nothing here has been applied to a cluster**:
`beck-infra/tests/conformance.rs` is the rung that would `kubectl apply` these objects and it skips
without one.

So what is established is that the emitter *writes* these fields and that the flag is a function of
the row. That the pod then starts is not established. The specific risk is the obvious one — a
read-only root filesystem breaks any process that writes to `/tmp`, and the usual remedy is an
`emptyDir` mounted there. Adding one pre-emptively would be a second unverified guess rather than a
fix, and the reason to think it is not needed is checkable without a cluster: on the deployed path
the runtime writes no file. `beck run` is given `--store postgres` or `--store memory`, both of
which keep the log outside the container's filesystem, and the only `std::fs` writes in `beck-rt`
are the file-backed log store (not reachable from either argument) and `beck test --update` (not a
server path). `k8s.rs` says that next to the `--store` argument, because the two decisions have to
stay true of each other.

## 82.9 A derived manifest claims about the program's image, not a dependency's

The substrate's container — Postgres — gets the three constants and **not** the read-only root
filesystem. It is somebody else's image; it writes its socket and its temporary files outside the
volume, and whether it does is not a fact any Beck effect row knows.

That is a real limit on the claim, and it is asserted in both directions rather than commented:
`every_container_drops_its_capabilities_and_refuses_privilege_escalation` checks the three on both
containers and checks that the substrate's `readOnlyRootFilesystem` is **absent**. A test that
asserts the absence is what stops somebody adding it later on the strength of the app container's
example.

The general rule this is an instance of: **a derived manifest may make claims about the program's
own image and not about a dependency's.** §6.5's promise is about what Beck generates from a Beck
program, and the Postgres image is a choice [`07`](07-dependencies.md) §7.8.1 made rather than a
consequence of anybody's effect row.

## 82.10 The gates that could not fail

Both `pending_security.rs` tests guarding the quota gap **stayed green through the change that
closed it.** That file's entire premise is the opposite:

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
says "a small number, because the point is that nothing refuses — not how fast it does not", which is
exactly right *as a statement about an absence* and exactly wrong as a tripwire. The honest form is
a **ratio or a shape**: propose more than any plausible limit, or assert that the head grows without
bound rather than that it reaches 200. What replaces both is in `runtime_edge.rs`, and it asserts on
the **log's head** rather than on identifiers: fifty proposals, a limit of five, forty-five refusals,
and a head that stops at five. "The proposal was refused" and "nothing was written" are different
claims, and F3 is about the second.

That is the fourth gate this project has found that could not fail, after
[`70`](70-the-evaluator-gets-fast-report.md) §70.7's three and
[`80`](80-structured-concurrency-report.md) §80.11's harness that was cited by name and never
written. The pattern across all four: **the gate was written by the person who knew the gap, and
tested the shape of the gap rather than the shape of the fix.** The defence is not more care; it is
to make the gate assert an *observable* — a log head, a printed file, a step count — rather than a
name or a number that seemed generous when it was written.

Three further statements of the same thing, which is why this chapter exists rather than four:

**A claim in a design document is not a claim anything checks.** §6.5 has listed four pod defaults
since it was written and the emitter produced one of them for as long as it has produced objects.
Nothing was wrong with the *derivation* — `tests/manifests.rs` checks that, and checks it well — and
the gap was a list of fields no test asked for, because no test asks for a field nobody has written.
One quarter of the claim was not even *expressible* until the `fs` atom split, which is the more
interesting failure: the missing test and the missing atom were the same absence seen from two ends.

**A warning quoted in a document is not a check.** §42.2 has quoted the Scriban advisory since Wave
0 — bound the recursion site, not one grammar rule — and the tree contained three violations of it,
one of which was a counter on the right function measuring the wrong interval. The project knew the
lesson well enough to write it down twice and could not apply it by reading.

**A list of absences is a better artefact than a list of work.** Neither handshake defect would have
been found by reading the code — the code was doing something reasonable, with a library's defaults
and no obviously missing line. They were found because somebody wrote down what was *not* there and
attached a test to it, and the test is what made the writing-down survive the months afterwards.
§42.6's fourth paragraph, which was prose rather than a test, is the control: it described a defect
that had been fixed and nobody noticed (§82.12).

## 82.11 What is not built

| | |
|---|---|
| Identity that the client cannot choose | **not built by default.** `DevIdentity` believes the claim, deliberately ([`48`](48-identity-report.md)), which is what makes §82.5 the bound it is |
| A cross-origin allowlist | **not built** (§82.2), and recorded in [`43`](43-threat-model.md) §43.4 as the part of this that remains absent |
| A measurement of the read buffer | **not done.** §82.3's memory figures are the library's documented per-connection allocation times a connection count, not an RSS anybody observed. The measurement wants the fanout harness [`23`](23-incremental-views-report.md) built, pointed at real sockets rather than in-memory duplexes — a bigger change than any of this, and the second test of this edge |
| Anything about what happens *after* the handshake, beyond the write quota | **unchanged.** A client that passes the origin check and stays under the size limits can still open unlimited subscriptions (F15), whose `pending_security` test is still green — with the caveat §82.10 attaches to every grep-shaped test in that file |
| "Overridable per command type" | **not built.** F3's decision has that clause and this does not implement it: the quota counts events, not events-of-a-kind. Doing it properly means the *program* naming which commands are cheap, which is language surface rather than a config field |
| A quota that survives a restart | **not built.** The counters are in memory, so a process restart is a fresh window. For a rate limit that is the usual and correct behaviour; for F3's *volume* half it is not, and a volume quota over the life of an actor would have to be folded from the log rather than counted beside it |
| Per-actor crypto-shredding | **not built.** F3 names it as the abuse *cleanup* path, and it is a different mechanism from the quota that limits the damage |
| A cost model behind 600 | **not built.** It is argued from human typing speed (§82.4), not derived from what an event costs to store or to fold. F15 asks for "per-view cost budget from the solver's own estimates", which is the shape a derived number would take |
| Coverage-guided fuzzing | **not built** (§82.7). It finds the production nobody enumerated, which is the residual risk this harness has by construction: it tests the grammar somebody wrote down |
| The macro expander as a fuzzing target | **partly.** `Shape::Ui` reaches it and `B0213` shows its ceiling works, but nothing generates a *user* macro that expands into more of itself. F17's macro fuel is still unbuilt and still asserted absent in `pending_security.rs` |
| A corpus-seeded generator | **not built.** §42.11's row says "over the corpus", and this generates from the grammar instead. Seeding from real programs and mutating *structurally* — swap a subtree, duplicate a branch — is the version that would find semantic failures rather than depth ones |
| The same treatment for the evaluator | **not built.** This bounds what the *front end* will read; `beck test --fuel` bounds what the evaluator will run ([`62`](62-fuel-report.md)), and nothing generates programs to test that boundary |
| A **mount** derived from `fs.read(path)` / `fs.write(path)` | **not built.** The flag is derived; the volume is not. `fs.write(/var/lib/app)` says the root must be writable and does not say a volume should exist at that path — which needs a source (an `emptyDir`? a PVC? a ConfigMap?) that the atom does not name, and §6.5 does not either |
| The same hardening on the **Compose** platform | **not built.** Compose has `read_only`, `cap_drop` and `security_opt`, and the rung it serves is a laptop rather than a cluster. The parity claim between the two platforms is about the objects, not about the hardening |
| Resource requests and limits | **not built**, and §6.5 says why it is not a one-liner: "a genuinely hard inference problem", with a per-construct heuristic and `beck tune` planned. Emitting a guessed number would be worse than emitting none |
| Anti-affinity across zones | **not built.** `replicas` is 1, so a spread constraint would be a field with no effect. It becomes real when replicas do |
| Any of the pod hardening verified against a cluster | **not done** (§82.8) |

## 82.12 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`42`](42-security-assurance.md) §42.6 | Two of its four bullets are closed (§82.2, §82.3) and a third is bounded rather than closed (§82.4, §82.5). Its fourth paragraph — "`dash.html`'s `esc` escapes `&<>` only, and the graph renderer interpolates `class="${n.tier}"` into an attribute without it" — **was already false when it was written down as still-true**: `esc` escapes `&<>"'` and carries a comment saying why quotes are in the set, and the renderer writes `class="${esc(n.tier)}"`. The audit, since a claim of the form "every interpolation is safe" should say what it looked at: every `${…}` in `dash.html` is an `esc(…)` call, a number computed by the layout, an ISO timestamp, or a string literal chosen by a ternary. **The item rotted in the direction nobody watches for — it was fixed and the record was not**, which is the failure a `pending_security` test does not have and the argument for turning a paragraph into one |
| [`42`](42-security-assurance.md) §42.9, §42.11 | The trigger fired and the harness exists, with `proptest` rather than `cargo-fuzz` and the reason in §82.7 |
| [`06`](06-kubernetes-and-packaging.md) §6.5 | The word "unavoidable" was three-quarters aspirational and is now met, for the app container. §82.9 is the limit on it |
| [`14`](14-review-findings.md) F3 | Built, minus the per-command-type clause. **A per-actor bound composes with whichever identity provider is configured**, so a deployment on `DevIdentity` has a quota worth 1,024 times less than its configuration says — a sentence neither F3 nor [`48`](48-identity-report.md) contained |
| [`64`](64-compile-speed-report.md) §64.4 | Its 12,000-binding abort is refused at 2,048 with `B0389`, and its depth axis moved to 100 → 1,600 for the reason in §82.6 |
| The error index | `B0122` and `B0389` are new |
