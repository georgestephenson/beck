# 96 — Phase 3, part 64: who is here

**Built.** `presence()` — who is connected now, as a first-class non-durable `Signal`. It is
[`10`](10-decisions.md) D6's last row, [`08`](08-roadmap.md) §8.5.4's last unbuilt row of Wave 3,
and the row [`48`](48-identity-report.md) §48.5 and [`95`](95-oidc-relying-party-report.md) §95.6
have both carried as *not built* while everything around it was finished.

The feature is small. What is interesting about it is that it is the **first input to a view that
moves without an event**. The session is not a function of the log either, but it is fixed for the
life of a subscription; the accumulator moves only when the log does. The roster moves on its own,
and every decision below is that sentence applied somewhere: to the chokepoint, to the tier table,
to the shared dataflow, to Mode B, and to what a test can say.

## 96.1 What a program writes

```beck
here: Signal[Map[Str, Int]] = presence()
page: Signal[Html] = per_session(map2(combine, board, here), view)
```

The roster is a map from actor to how many connections that actor holds.

Not a declared model, and that is a decision rather than a shortcut. `corpus/15-presence.beck` has
had `here: Map[Str, Int]` in it since the corpus was written — it is what a program that had to
fake this reached for, and it folded join and leave *commands* to get there. The ordering is the
key's and therefore a function of the value, which is what [`54`](54-ordering.md) says any
collection reaching the wire has to be. And every
question a page asks of it is a `Map` primitive that already exists: `map_len` for how many,
`map_keys` for who, `map_contains` for whether one particular person is here. A `Presence` model
would have added a type, a docgen entry and a wire shape, and bought nothing.

`corpus/32-here.beck` is the program, and like every file there it carries **no placement
annotations**:

```text
name                 tier     kind       effects
proposals            server   signal     {ingress}
events               data     signal     {}
board                data     signal     {durable}
here                 server   signal     {cap.presence}
page                 client   signal     {}
```

The crossing is a crossing like any other, with the id a resumable subscription is keyed by —
`beck explain flow` on the same file:

```text
tier crossings — each is one subscription, resumable by (id, seq) (§4.3)
  proposals → events  server → data    carries Proposal       7b85aaa2a9b255ab
  board → page·map2   data → client    carries State          1083d8a20f09d182
  here → page·map2    server → client  carries Map[Str, Int]  21f424dc9b8e6efa
```

## 96.2 The atom is a capability, and that is F16 taken literally

`presence()` performs **`cap.presence`**. No new effect atom.

[`14`](14-review-findings.md) F16 is one line and has been open since the review:

> **F16** Presence signals (D6) leak who-is-online; gate behind a capability like any other view.

So it is a capability, which does three things at once:

* **It places the roster.** §3.3's table gives `cap.*` to the server and to no other tier, so the
  line above is derived rather than annotated — and a `presence()` on the data tier or in a browser
  is a placement error with a reason attached rather than a runtime surprise.
* **It keeps the roster out of a fold.** `Effect::breaks_replay` is true of every atom except the
  ambient ones, `partial` and `raises`, so the machinery that has refused a clock inside a fold
  since Phase 2 refuses this without being told about it.
* **It is in the signature.** The row reaches `.becki`, `--wire-compat` and `beck explain place`,
  so "this program reads who is online" is a fact about the module rather than a line of code
  somebody has to find.

What it does *not* do is stop a program that wants the roster from having it. Nothing here is an
access-control system: the gate is that reading it is an authority-tier act, visible in the row,
and publishing it to a page is a **declared edge in the signal graph** that `beck explain flow`
prints. That is the same kind of gate the rest of this language has, and it is worth saying which
kind it is.

## 96.3 The one refusal that is new: the chokepoint may not read it

`B0515`. A `decide` whose accumulator argument is — or reaches — a `presence()` is refused.

This is the whole replay argument in one rule. §3.7 makes the log the only description of a
program's history; who was connected when an event was recorded is written down nowhere, so a
`validate` that decided from the roster would decide one thing today and another on replay, and the
log would stop being the whole history. The fix the diagnostic suggests is the one
`corpus/15-presence.beck` already implements: record the fact — propose a command when a client
arrives — and decide from the state that fold produces.

The check is **reachability in the graph**, not the shape of an argument, and `presence.rs` has the
program that says so: a `signal_map` between the roster and the chokepoint is still the roster.
Writing it the other way would have been a rule about `decide(proposals, here, validate)` and
nothing about `decide(proposals, signal_map(here, f), validate)`, which is
[`85`](85-what-the-generator-found-report.md) §85.7's pattern — a limit at the one production
somebody thought of.

Two other refusals came for free and are asserted rather than assumed: `durable(presence())` is
`B0502` ("`durable` must wrap a `fold`"), and `fold(step, init, presence())` does not typecheck,
because a fold takes a `Stream` and a roster is a `Signal`.

## 96.4 Mode B may not read it, for the same reason it may not read the session

`B0516`. `@render(client)` sends the browser the **accumulator** and lets it run the program's own
fold and view locally ([`94`](94-mode-b-report.md)). The roster is in neither the accumulator nor
the log — it is a fact the server holds about its own sockets — so a browser handed the state would
have nothing to render that part of the page from. Shipping the roster alongside would be a second
wire that nothing reconciles by `seq`, which is exactly the thing Mode B's correctness argument
rests on not having.

The refusal is separate from [`94`](94-mode-b-report.md)'s `B0514`, and the program in the harness
that triggers it is written so that **only** the new one can fire — its page does not read the
session. A program refused for two reasons proves neither.

## 96.5 The engine: a third source, and which side of §5.3's cut it is on

The view's shape changes from `(state, session) -> Html` to `(state, session, presence) -> Html`,
and the plan gains a third source beside `Op::State` and `Op::Session`. The engine's arm for it is
the session's arm, to the line: compare with what this engine last saw, and mark the operator
changed if it moved. Every role has three parameters whether or not the program reads the third,
because a role the runtime calls has one arity and two shapes would be two places to disagree.

**Everything downstream of the roster runs per subscriber**, and the reason is a *clock* rather
than a privacy rule. §5.3's shared dataflow is versioned by the log's `seq`
([`26`](26-arrangement-sharing-report.md), [`51`](51-arrangement-lifecycle-report.md)): a
subscriber renders at a version, and two subscribers at the same version must be handed the same
input. The roster moves when `seq` does not, so there is no version at which the shared side could
hold it. Sharing it needs a second clock, and §96.8 is that item, unbuilt and named.

The cost of that is real and is a property of the program rather than of the feature — which is
[`26`](26-arrangement-sharing-report.md)'s own finding, arriving from the other direction. On
`corpus/32-here.beck`, `beck explain incremental` says:

```text
  shared arrangement: nothing is read twice, so there is no prefix to share
  per subscriber:     page·map2, page  (one plan, these operators per connected session)
  in the plan:        14 of 34 operators read neither the session nor who
                      is connected, and the runtime holds those once for every
                      subscriber — one shared dataflow, advanced per event rather
                      than per connection (docs/26). The other 20 run per
                      subscriber.
```

That is a small program whose page is mostly the roster, so almost all of it is per subscriber. A
program that reads the roster in a footer and sorts a large public feed above it would be the
opposite, and `beck explain incremental` is where a developer sees which.

## 96.6 What a connection costs, at two sizes

The shape that would have been wrong is a page whose re-render on every connection walked the
accumulator: connections are the one input that moves without an event, so a roster change costing
`O(the accumulator)` would make connecting to a large application quadratic in the number of people
doing it.

Gated on the shape rather than on a rate, with the evaluator's own step budget as the instrument —
deterministic, and with no clock in it since [`70`](70-the-evaluator-gets-fast-report.md) made the
budget charge for work:

| roster | 200 notes | 1,600 notes |
|---|---|---|
| 2 connected | **112 steps** | **112 steps** |
| 8 connected | **298 steps** | **298 steps** |

The same two numbers at both accumulator sizes, to the step. Eight times the notes costs nothing;
four times the people costs 2.7×, which is the roster being walked and each name's note looked up
by key. `presence.rs`'s
`a_page_of_the_roster_costs_the_roster_and_not_the_accumulator` is the gate, and it holds a
*constant* at two sizes rather than a ratio, so a page that started walking the state would fail at
1,600 and pass at 200.

What a connection costs the process is one `O(actors)` rebuild of the published value under a
mutex taken twice per connection and never on a render or an event, plus one re-render for each
subscriber whose page reads the roster. The second of those is the fanout, and it is the cost §96.5
names.

**A program that never asks is never woken.** `Roles::view_reads_presence` is a compile-time fact,
so a subscription to a program whose page does not mention `presence` does not even hold a receiver
on the roster. The harness asserts the consequence rather than the flag: a connected
`examples/todo.beck` client is sent **no frame at all** when a second client arrives. What such a
program does still pay is the bookkeeping — every connection joins the roster and leaves it
whatever the program reads — and that is deliberate rather than overlooked: it is one mutex and one
rebuild per *connection*, never per event and never per render, and it keeps "how many are
connected" a fact the process has rather than one it would have to start collecting.

## 96.7 The roster is bounded, and that is §84.4 one subsystem over

The obvious implementation is a map from actor to a count, and that map is unbounded memory keyed
by **a string the client chooses**. Under `DevIdentity` the actor is whatever the connection said it
was ([`48`](48-identity-report.md)), so a client opening sockets under fresh names would grow the
table until the process died. That is
[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.4's finding exactly, in a second
place, and finding it here rather than in a later hardening pass is the only thing that report's
existence is worth.

[`quota.rs`](../compiler/crates/beck-rt/src/quota.rs)'s answer — shard into a fixed table — is **not
available here**: a quota needs a number per actor and may let two actors share a bucket, while a
roster needs the actor's *name* and would be nonsense if two names collided. So the bound is a
capacity. Past `presence::Config::capacity` (4,096 by default) a new actor is **not recorded** and
`Registry::refused` counts it.

Presence therefore **under-reports rather than growing**, which is the failure this direction
should have: a page that says "127 here" when 200 are connected is wrong in a way that costs
nothing, and the opposite is a process that dies. An actor already in the roster is never refused,
whatever the capacity — the bound is on how many *names* are held, not on how many connections one
of them may open.

## 96.8 What is not built, and said first

* **The roster is not shared between subscribers.** §96.5 is the reason and it is a version rather
  than a design: a second clock on the shared dataflow — a `(seq, roster)` pair, or a render epoch
  with the `seq` riding along as a stamp — would let the operators above the session hold it once.
  Nothing in this change makes that harder, and the file that would change is `engine.rs` alone.
* **A roster change re-renders the page rather than patching it.** `map_keys` has no delta rule, so
  the operator that reads it is a recompute, which `beck explain incremental` prints. Eight people
  is 298 steps; eight thousand would be proportional. §96.6's gate is about the accumulator and
  says nothing about this.
* **It is per process.** Who is connected to *this* pod is what this reports. §15's partitioned
  deployment has no way to answer "who is connected to the application", and that is a Phase 4
  question about a fabric rather than a missing line here.
* **The served document is rendered before its own socket exists.** `http.rs` renders the first
  HTML from the roster as it stands, and the connection that document belongs to joins the roster a
  moment later when its websocket opens — so a first paint can say `0 here` and become `1 here` on
  the frame that follows. A `Fresh` subscription's first frame replaces the whole root, so the
  correction is immediate and needs no special case; it is written down because it is visible and
  because the fix is a different shape (the document handler would have to know which socket is
  about to arrive).
* **Nothing is resumable about it.** A reconnecting client is served the difference from the `seq`
  its last frame carried, and the roster is not a function of `seq`; the page it is sent reflects
  the roster *now*, which is the only thing "now" can mean. A patch frame produced by a connection
  carries the `seq` the page already had.
* **Mode B, per §96.4.** Refused rather than unimplemented.
* **No presence-specific metric is exported.** `Registry::here` and `Registry::refused` exist and
  are read by tests; §5.3's gauges are exported and these are not, which is a line of
  `telemetry.rs` somebody should write the day an operator asks.

## 96.9 The finding

The first version of the registry published its roster with `watch::Sender::send`, and **`send`
fails when there are no receivers — leaving the value it was given unpublished**.

Nothing subscribes to the roster until a program that reads one has a connection, and a connection
joins the roster *before* it subscribes. So the first client's own join was always lost: it was in
the map, and the value every render read was the empty one it had been constructed with. The
symptom was a first page that said `0 here` while somebody was looking at it.

`send_replace` is the fix, and it is one word. Both halves of the suite go red without it, which was
checked by putting `send` back: three of the four unit tests in `presence.rs` fail, and so does the
end-to-end `a_second_connection_moves_the_first_ones_page`. The one that stays green is the
*watcher* test — the only one that holds a receiver — which is the whole shape of the defect in one
line. A registry exercised only by a harness that subscribes first would have passed everything,
and nobody writing that harness would have thought about the order twice.
[`83`](83-the-runtime-edge-report.md) §83.4 is the same shape a suite earlier: a refusal tested only
as a pure function is one refactor away from never being called.

## 96.10 What this establishes, and what it does not

D6's last row is built: presence ships as a first-class non-durable `Signal`, it is the natural
demo of per-session fanout, and — per §96.5 — it is now also that fanout's permanent stress test,
because it is the only input that moves without an event.

What it does not establish is anything about scale. Thirteen tests, one corpus program, a bounded
registry and a two-size shape gate say that the feature is correct and that its cost is the
roster's rather than the accumulator's. Nobody has run this with a thousand connections, and §96.8's
first bullet is exactly the thing that would decide what happens when somebody does.

With this, **[`08`](08-roadmap.md)'s identity bullet has no unbuilt row left**, and Wave 3 is
finished.
