# 94 — The client

**Built.** [`08`](08-roadmap.md) Phase 3's two client bullets — Mode B and the polish — in one
chapter: `@render(client)`, a bundle that is the component's slice, a `wasm32` kernel, data patches
instead of DOM patches, reconciliation by `seq`, an offline queue that survives a cold start with the
server switched off, freshness-typed pending state, a size gate in CI, routing, forms, focus and
scroll preservation, and a devtools panel. Chromium runs all of it. What the bullets still owe is
**codegen** and **lazy routes**, and §94.15 says why neither is a small remainder.

The interesting content is not the WebAssembly. It is three rules and one measurement.

**A Mode B page may not be a function of who is asking** (§94.2), because Mode B sends the browser
the *state* rather than the page, so a page that filters by identity is a page whose state is not
that browser's to hold. **A `Session` was one word for two facts** (§94.3), and that refusal had been
testing the wrong one: where a browser *is* is not who it belongs to, and eligibility and fanout
stopped being the same answer. **A server cannot say it is saving** (§94.5) — the first rule in this
subsystem that refuses a component for rendering on the *server*, because a server renders what it
has recorded and would answer `Confirmed` at every position of every log.

And the measurement (§94.12): an interaction on a thousand-card board is **13 ms of browser CPU, 97%
of it `view`**, growing with the board rather than with the change. That is the number that says
codegen is the wrong thing to reach for first.

---

## 94.1 The one row in §5.1's table that decides everything

[`05`](05-tier-lowering.md) §5.1 gives two rendering modes over one source, and six rows of
differences. Five of them are consequences of the second:

| | **Mode A — thin** | **Mode B — local** |
|---|---|---|
| `view` runs on | server | client |
| **Wire carries** | **DOM patches** | **data patches (state diffs)** |
| Optimistic UI | no | yes — the same fold runs locally, reconciled by `seq` |
| Server cost | per-session view state | per-session subscription only |

Mode A sends a *rendering* of the state. Mode B sends the **state**. Once that is taken literally,
everything else follows without a design decision: the client can render because it has the input to
the view; it can guess because it has the input to the fold; the server does less because it is not
rendering; and the browser has been given data it did not have before, which is §94.2.

The implementation is correspondingly small at the seam. `beck_rt::session` gained exactly one
branch — one feed renders per subscriber and diffs two pages, the other diffs two states — and
resumption, acknowledgement and the command channel are the same code either way. **A rendering mode
is a choice about what a frame contains, not about how a subscription works.**

## 94.2 A Mode B page may not be a function of who is asking

A view has the shape `(state, session) -> Html`. If it reads the session it renders a different page
for different actors *from the same state* — which is to say it is filtering, scoping or hiding by
identity. `examples/todo.beck`'s view does exactly that: `filter_list(…, lambda t: t.owner ==
session.actor)`.

Running that view on the client requires giving the client `State` — every user's todos — and letting
the filter run in the browser. **The page would still look right.** Every user would see their own
todos, and every user's browser would be holding everybody's. That is the worst shape a disclosure
can have, because nothing about the running system looks wrong.

```text
error[B0514]: `page` renders differently for each *actor*, so it cannot render on the client
    = note: This page reads `session.actor`: it filters, scopes or hides by identity. A client that
            rendered it locally would first have to be given the state it filters — including
            everything the filter removes. Reading `session.path` is allowed and is not this: the
            browser chose the route and already holds the state.
```

What survives the rule is exactly the class [`10`](10-decisions.md) D5 named for Mode B: "typeahead,
drag-and-drop, editors, anything marked `optimistic` or `offline`" — pages that are the same function
of the same state for everybody.

**The obvious second refusal turned out not to be needed.** Mode B puts the accumulator on the wire,
so a `secret[T]` in it would cross. That check was written and then deleted, because it is **already
discharged**: a durable fold's state must be *storable* (`B0411`), storable is strictly stronger than
sendable, and the accumulator is what crosses. A secret cannot reach a Mode B client because it
cannot reach the log. A composition is a fragile thing to rely on silently, so the suite asserts it —
the refusal is `B0411`, and `storable ⟹ sendable` is checked directly — rather than leaving a future
edit to one half free to break the other.

`B0514` has a sibling: [`48`](48-identity-report.md)'s `B0516` refuses a Mode B page that reads
`presence`, because who is connected is a fact the server holds about its own sockets. **The two
refusals are the two kinds of thing a browser handed the accumulator would not have — who is asking,
and who is connected.** Where the browser *is* is not one of them, which is §94.3.

## 94.3 `Session` was one word for two facts, and a route is not a router

The obvious way to add routing to a language is to add routing to the language: a route table, a
matcher, a `route` form, a path-parameter syntax. None of that is here, and the reason is that a Beck
page is already a pure function of two things and one of them is the connection.

**A route is a field of `Session`**: `model Session {actor: Str, claims: Map[Str, Str], path: Str}`.
That is the whole feature. `view(state, session)` may read `session.path` the way it already reads
`session.actor`, and "which page is this" becomes an ordinary `if` over an ordinary `Str`, written in
the program rather than configured beside it.

Everything downstream already knew what to do with it. The route reaches the page through the one
constructor both tiers build a `Session` with. It makes the page per-session, which §3.8's fanout
analysis and §5.3's shared cut already knew how to read. The incremental engine already recomputes
what is downstream of the session node when the session changes — a comment in `plan.rs` had called
the session "constant for the life of one subscription", which stopped being true and cost nothing,
because that node has compared the value it was handed against the one it held since it existed, and
a rebuild has been contagious downstream since [`23`](23-incremental-views-report.md) §23.16 for
exactly the case of a subscriber whose session moved. **Not one line of the engine, the splitter, the
plan or the fold changed.**

What did have to exist is the edge — something has to *tell* the program where the browser is. Every
`GET` that is not one of the runtime's own paths renders the program at that path, so a pasted URL
and a reload are server-rendered at the route they name **before any JavaScript runs**, which is the
difference between a router and a single-page application that corrects itself after first paint. A
subscription states its route when it opens rather than afterwards, because a route established by a
second message would leave every reconnection rendering the root's page until that message arrived.
And a `nav` frame carries a route that changes while the socket is open. In the browser it is one
`click` listener over `a[href]`: the link in `examples/routed.beck` is an ordinary anchor with
nothing in it that knows a router exists, and a browser executing no JavaScript at all follows it to
a page the server renders. **That is a consequence of routes being real URLs rather than a decision
anybody made about progressive enhancement.**

### The wall

`B0514` tested whether the page reads *the session*. Put the route on the session and that rule
refuses every routed page in Mode B — which is wrong, and not marginally: a page that renders by
route is not hiding anything from the browser it is running in, because that browser chose the route
and already holds the state.

So the two halves of a `Session` are not the same kind of thing:

| field | says | verified by | may a Mode B page read it |
|---|---|---|---|
| `actor` | **who** is asking | the identity provider ([`48`](48-identity-report.md)) | no |
| `claims` | what the provider said about them | the same | no |
| `path` | **where** they are | nobody, and nothing should | yes |

`B0514` now asks which fields the view can observe, and the answer comes from the view's own code.
**The analysis is one sentence long** — a `Session` can only be observed by having a field read off
it, so collect every field read whose base is `Session`-typed anywhere the view can reach — and it is
sound without tracking where the value flows, **because flow does not create an observation**.
Wherever the record ends up, reading it is still a field read over a `Session`-typed base, and every
definition it could end up in is in the closure the walk covers. Types are what make it cheap: a
field read needs a concrete record type, so a `Session` handed to a generic definition cannot have
anything read off it there. What flow *could* hide is an observation that is not a field read — an
equality, a digest, a session stored inside a value that crosses — and those are named as the
conservative cases rather than ignored.

**Eligibility and fanout stopped being the same answer, and that is the sentence to carry forward.**
`examples/routed.beck` is per-session — two people on two routes see two pages, so §5.3's cut is
unchanged — *and* it renders in a browser. Before this, one fact answered both questions, and it
answered the second wrongly for a whole class of page.

**Nothing verifies a route, and nothing should.** `session.path` is the client's own statement about
itself and it reaches `validate` exactly as the actor does — but the actor is what a provider minted
and the route is what a browser typed. A program that used it for authority would be making
[`82`](82-the-edge-report.md) §82.5's mistake in a new place. What the
architecture guarantees instead is narrower and worth stating: **the route cannot reach a fold**,
because an `Envelope` carries the actor's name and nothing else — so no replay can depend on where
anybody was browsing.

### What a navigation costs, in each mode

The same program both ways: `examples/routed.beck` has no `@render(client)`, and adding that one line
is the whole of the difference, which is what makes "the router is the same in both modes" a test
rather than a claim.

**Mode A.** The client sends 24 bytes and the server answers with the difference between two pages —
111 bytes on the state the gate builds. Not a page: a diff, because a navigation is an ordinary
change and goes through the same advance an event does. **There is nothing in `beck-rt` that knows
what a route is**; `session.rs` sets a string and re-renders.

**Mode B.** The kernel moves `session.path` and re-renders from the state it already holds, so the
page changes with no round trip. The server is told anyway — 24 bytes, answered with nothing — and
*not* for the page, which it is not rendering. It is so that the `Session` the server hands
`validate` is the one this client's own `validate` saw. Both frames travel on one socket, so the
navigation precedes every command proposed from the page it produced.

## 94.4 Optimism is a property of what crosses, not of a component

D5 says the browser applies the expected event "speculatively — legitimate because it runs the *same
pure fold* the server runs". Taking "the same fold" literally is what forces the design:

- The client holds **confirmed** (the accumulator at `seq`, moved only by a data patch) and derives
  **optimistic** (confirmed, plus every pending command's events folded on top). The optimistic state
  is never stored, **because a guess that is kept is a guess that has to be un-kept.**
- Reconciliation is therefore not an operation. When a data patch moves `seq` past the position the
  server gave a command, that command stops being pending and the same derivation produces the
  corrected page. A guess that was right costs an empty patch; a guess that was wrong is corrected by
  the same code path. **Neither is a special case, and there is no rollback.**
- An ack alone confirms nothing. The page does not move on an ack, and the pending command is retired
  by the *state* that includes it.

This settles a question the design left implicit: **optimism is not an extra feature layered on Mode
B, it is the same fact stated twice.** A client can only guess the next state if it holds the state
the fold is *of*. A client holding a projection — one session's filtered list — could not apply an
event to it without a second, different fold that no program writes. Which is why §94.2's rule and
optimism have the same precondition.

One consequence worth naming: because `validate` is in the bundle, **the browser refuses what the
server would refuse, with the program's own `Rejection` value and no round trip.** That is not a
duplicated rule — it is the same rule, run early. Authority stays at the chokepoint; the client's
copy is advice to the person typing.

## 94.5 Freshness, and the rule that points at the server

[`03`](03-type-and-effect-system.md) §3.7 has carried one sentence since before there was a compiler:
"`Signal[T]` carries a freshness dimension (`confirmed | pending(n)`) that UI code can render
("saving…") — staleness is typed, not pretended away."

The value is that sentence's own two cases:

```beck
union Freshness:
    Confirmed
    Pending(n: Int)
```

Two variants rather than a count, because `Pending(n=0)` and `Confirmed` would be one fact with two
spellings and every page would have to know which one it had been handed. `n` sits on the pending
variant for the reason a `Some` carries its value: it exists only when there is something to count.

What it is *not* is a dimension on every signal's type, and that is the one place this reads §3.7
more narrowly than §3.7 is written. `freshness()` is a **source** in the signal vocabulary, beside
`presence()`, so a page reads the freshness of *the render it is part of* — "is any of this a guess"
— and not the freshness of each value it is built from. A per-signal dimension would let a page say
that one list is speculative while the header beside it is not; that is a stronger thing and it is
not built. What is built is what "saving…" needs, which is what the sentence gives as its example.

The implementation is the smallest one available, because `presence()` had already paid for the
shape: a `Prim`, a vertex in the signal graph, a parameter the slicer substitutes into the sliced
view, and a value the renderer supplies at the edge. **No capability** — `presence()` performs
`cap.presence` because a roster says something about other people, and that capability is also what
pins it to the server; a client counting its own unacknowledged commands says something about nobody
else.

### A server cannot say it is saving, because it has nothing to save

```text
error[B0518]: `page` reads `freshness`, so it cannot render on the server
    = note: freshness is a client's account of the commands it has proposed and not yet had
            confirmed. The server holds the log: what it renders is confirmed by definition, so this
            page would render `Confirmed` at every position of every log and its other branch would
            be unreachable.
    = help: render this component in the browser — `@render(client)`, which is what makes a guess
            possible in the first place — or take `freshness` out of its page
```

**It is the first rule here to refuse a component for rendering on the *server*.** Every previous one
asks whether the browser may be given something; this asks whether the server can answer something.
And it is worth being precise about why it is a refusal rather than a lint: rendering `Confirmed` on
the server is not wrong — it is *true*, and it is what the SSR of a Mode B page is rendered with.
What is wrong is a program written as though the other branch could happen. **A "saving…" indicator
that no log can ever show is not a mistake the type checker would otherwise catch**, because the
program is perfectly well-typed; it is a mistake about which tier is executing, which is the class of
thing this compiler is supposed to refuse.

**And the chokepoint may not read it either.** `B0515` refuses a `validate` that decides from the
roster, because who was connected when an event was recorded is written down nowhere and a replay
would decide differently. Freshness gets the same rule (`B0517`) for a sharper version of the same
reason: **on replay nothing is in flight at all**, so a chokepoint that read it would accept a command
today and refuse it on the way back. Both are checked by *reachability in the graph* rather than by
the shape of `validate`'s argument, because the value reaches the chokepoint through whatever `map2`
a program cares to write.

## 94.6 What actually ships to the browser

Two artefacts, and keeping them apart is the whole of §94.11's honesty.

**The bundle** — the component's slice: `view`, `validate`, the fold, the initial state, and every
definition those four reach transitively. That closure *is* the slice, and it is why the bundle is
smaller than the program. It carries no types, no signal graph, no test and no placement: the program
was checked on the way in, and the client checks nothing.

Two decisions live in a hand-written mirror type rather than in a `derive`, so that they are
reviewable. **Types are erased** — the evaluator never reads a node's type, it dispatches on values,
so carrying resolved types would roughly double the payload to say something the only consumer cannot
use; a *compiling* client backend needs them, and that is a format bump rather than a field somebody
adds quietly. **Spans are kept** — three integers, and the difference between "the fold failed" and
"the fold failed at `board.beck:47`" in a browser console.

A primitive is encoded as its number in the table, which is compact and silently means something else
if the table changes — so a bundle carries a digest of every primitive's name and number, and a
kernel from a different compiler refuses the bundle instead of executing a `str_len` that used to be
a `list_len`. Same rule as the log's format field: **a misread log is worse than an unreadable one.**

**The kernel** — `crates/beck-wasm`, a `wasm32-unknown-unknown` module with four exports and a
length-prefixed byte buffer between them. No `wasm-bindgen`, no generated glue. It is the same
program for every Beck application, because it is a **backend** rather than a compilation of anything
— which is why it is a fixed download in §94.11 rather than a per-component one.
[`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) is that decision.

**No `unsafe` code**, which took one idea: a buffer is a `Vec<u8>` the module keeps in a table keyed
by the address of its own allocation. The host writes through linear memory — that is what linear
memory is — and Rust reads its own `Vec` back out of the table. Nothing is reconstructed from an
integer. The four `#[allow(unsafe_code)]` are on `#[no_mangle]`, which rustc classifies as unsafe
because two libraries exporting one symbol is undefined at link time; `beck-wasm` therefore *denies*
where the other crates *forbid*, and the suite gates the extent — no `unsafe` block, no `unsafe fn`,
four allows, all on exports.

## 94.7 Hydration is free, and the reason is a theorem

The document is still server-rendered: first paint is SSR, as in Mode A. The client then loads the
kernel and the bundle, takes its first frame, and **adopts its own first render as what the DOM
already shows, without emitting a patch.**

That is legitimate because the server-rendered page is `view(state, session)` at some `seq`, and the
client holds the same `view` and — once its first frame arrives at that same `seq` — the same state.
Same function, same input, same page. **Nothing has to be reconciled because nothing can differ.**

The suite asserts it as *equality of `Html` values* rather than as similarity of markup, which is
checkable precisely because both sides execute the same `Core`:
`the_browser_renders_what_the_server_would_have_sent`, and the same over 40 commands accepted and
refused. It is the gate every other claim in this chapter rests on, and the one that would go red
first if the kernel and the server ever stopped being the same program.

## 94.8 Forms, the caret, and the list somebody had scrolled

**Forms are `on_submit` and one new hole.** No compiler change: the `ui:` macro has turned
`on_<event>=` into `data-b-<event>` since Phase 1, so a form is a form and `submit` is an event the
residue listens for. What fires it is the *browser's* submit — a button, or Enter in a single-line
field — so **the page a keyboard and a screen reader already know how to drive is the page the
program wrote**, rather than a `keydown` handler this project invented. `on_input` and `on_change`
arrived with it for the same reason: the distinction between "as it changes" and "on commit" is the
browser's, and reusing it costs one listener each.

`$field:name` reads a named control out of the form being submitted. Writing it found something the
two existing holes had hidden: **the filler only looked at the top level of a command.** Every
command in the tree happens to flatten — a newtype crosses as a string — so a hole one level down had
never been written, and a command whose field is a record would have put the literal `"$id"` in the
log. The filler recurses now.

**The caret.** A patch that replaces an ancestor of the focused element destroys it, and the
browser's answer to "what is focused now" is `body`. A whole-frame replace is what a `Reset` frame
carries — a reconnection whose gap the log could not answer — so this is not a corner case, **it is
what happens to anybody who was mid-sentence when their train came out of a tunnel.**

The client records the caret before applying a patch and puts it back if the element it was in did
not survive: the child-index path from the frame root, the selection range, and the element's own
scroll. It refuses to restore into a *different* element that happened to take the same position —
tag, key, `name` and `id` have to match, which is the program's own answer to what an element is
where there is one and the tag where there is not. Scroll offsets are kept for subtrees a `replace`
is about to rebuild and restored where the position still holds something scrollable; that is best
effort by construction and this says so rather than the code implying otherwise. `insert`, `remove`
and `move` need nothing — they do not rebuild the container, which is what the `move` op was added
for in the first place.

**The cost is proportional to the patch and not to the page.** Nothing is walked except the subtrees a
replace destroys, and the caret is one lookup. A version that scanned the document for scrolled
elements would have made every patch cost the size of the page, which is the shape this project
spends its gates refusing.

## 94.9 The panel, and why it is not an extension

Phase 3 asks for "a devtools **extension** showing signal graph, patch traffic and pending state".
What is here is the three things and not the extension, and the difference is deliberate rather than
a shortcut: an extension is a second artefact with its own distribution, its own permissions and its
own release pipeline, and **nothing in this repository could run one** — the browser gate drives a
page over the DevTools protocol. A panel the server serves is testable by the same harness that tests
the client, and it is the same residue: no framework, no CDN, nothing the network policy this program
derives would refuse.

It is loaded on request, so a page that does not want it pays nothing. It is appended to `body`
rather than into the frame root, which is not tidiness: a patch path is a child index from that root,
and a panel inside it would be counted by every path in every patch.

- **Patch traffic** — frames and ops applied, bytes in and out, frames sent, navigations, and whether
  the socket is up.
- **Pending state** — in Mode A, the commands sent and not yet answered; in Mode B, the commands
  *applied* and not yet confirmed, which is the difference between what the browser is showing and
  what the server has agreed to. Both modes publish it under one vocabulary, so the panel is one
  panel and not two.
- **The signal graph** — the one thing the browser cannot know, because a Mode A client is never sent
  a program. The endpoint serves the running program's own graph with its incremental verdicts, which
  is what `beck explain incremental` prints.

**What the endpoint deliberately does not carry is the accumulator.** A Mode A page is precisely the
part of the state its viewer may see, and an endpoint that handed a browser the rest would be a
disclosure with a friendly name. The gate asserts the *absence* rather than the presence.

The residue grew, and here is the number rather than an adjective: Mode A's two files were 9,184
bytes and are 20,803; gzipped, 3,910 → 7,970. The panel is a further 7,125 bytes and is not loaded
unless asked for. These files carry more comment than code and there is no minifier anywhere in this
project, so the gzip figure is an upper bound on what a build step would ship and the raw one is not
a measurement of anything.

## 94.10 Offline, and what it costs

[`10`](10-decisions.md) D7 rung 2 is "what Beck v1 ships": a Mode B component holds a local copy of
its state and queues commands while offline, replaying them on reconnect. D7 also predicts how much
work it should be — "falls out of Mode B + determinism" — and **that prediction is the interesting
thing to check**, because it is a claim that no new agreement between the two sides is needed.

It very nearly held. The client half is three things: a snapshot of the *confirmed* state, its `seq`
and the commands still in flight (never the optimistic state — a guess restored as a fact could not
be corrected); `localStorage` under a key carrying the program's **wire id** and the actor, so a tab
coming back to a new program cannot restore a copy of the old one, with the kernel refusing a
mismatch as well because a key is a convention and a check is a rule; and a queue flushed on **every**
socket open, not the first. The server half is nothing at all, which is the part D7 got right:
`App::propose` has de-duplicated by command id since Phase 0.

**Except that the reply to a retry was "rejected".** The sequencer remembered the ids it had seen and
answered a repeat with `rejected.push((p.reply, "duplicate".into()))`, under a comment reading
"Idempotency by envelope identity: a retry after a reconnect is safe (§4.3)". The comment describes
the intent exactly; the code beneath it tells the client its command was **refused**. A Mode B client
replaying an offline queue would take every already-landed command back off the page — **the user
watches their work vanish, one card at a time, on reconnect.** An idempotent operation has to be
idempotent in its *answer*, so the sequencer now remembers `(id, seq)` and replies with the position
the first attempt got. The retry is an ack. Nothing had ever retried before, because the thin
client's outbox only survives a disconnection and not a reload.

**And a drained server never let go.** The second defect was found trying to *test* the first. Taking
the browser offline with Chromium's network emulation does not close an already-open websocket, so
the "offline" client cheerfully kept talking. Stopping the server did not work either: `http::serve`
stopped accepting on shutdown and left every connection it had already accepted running for the life
of the process. §5.2 lists "graceful drain (finish folds, snapshot, **hand off subscriptions**)" and
the third clause had no implementation. `App::drain` is a watch every subscription selects on, and
`serve` sets it when it stops accepting — which is also what a *deploy* looks like, and is the reason
the clause is in §5.2 in the first place.

**The cold start needs a service worker.** A queue survives a *reconnect*; it does not survive a
reload with nothing listening, because the document, the scripts and the kernel all come from the
server — the local copy is fine and unreachable, and the browser shows its own error page. So the
worker caches the shell, network-first so a live server always wins and a deploy is never hostage to
a cache, under a name carrying the program's wire id.

Routing narrowed that without anybody noticing, which is why it is here rather than in a list of
extras. The worker cached `/`, "because that is what a reload asks for". **With routes it is not.** A
tab that had navigated to `/active` and then reloaded offline asked for a document the cache had
never held, and got the worker's "never been here before" page — for a route it was perfectly able to
render, from state it was already holding. The fix is one branch and it is the same observation the
mode rests on: in Mode B the route is the *client's* to render, so one cached document answers for
every route. Caching a document per route would have been caching one file under several names and
would still have missed the route nobody had visited.

**What it costs.** Persisting is `O(state)` per write, coalesced onto a trailing 200 ms timer so a
burst costs one write. That is honest but not good: a thousand-card board is ~100 KB of JSON per save.
What would make the cost a function of the *change* is an append-only local log — which is the shape
of D7's own later rungs, and is not built.

What this is *not* is a local-first application: there is one writer, the queue is a queue rather
than a log, and a second device is a second replica of the same server's history. D7's later rungs —
CRDT-valued types, peer-to-peer — are unchanged and unbuilt.

## 94.11 Sizes, and the budget that had never been weighed

`cargo test --release --test measure_mode_b -- --nocapture`:

| | bytes | brotli | against §5.1's 150 KB |
|---|---:|---:|---|
| `board.beck` — 10 definitions, 252 `Core` nodes | 4,875 | **1,753** | 1.1% |
| `editor.beck` — 5 definitions, 143 `Core` nodes | 2,843 | **1,083** | 0.7% |
| The kernel — every Beck application, whatever the program | 794,224 | **194,377** | 126.5% |

**Read the rows separately, because the budget answers a different question for each.** "< 150 KB
brotli for a typical Mode-B component bundle" was written about compiled output, where the component
*is* the download. Here the marginal cost of a component is under 2 KB and the kernel is a fixed,
program-independent, cacheable download every component of every Beck application shares. Two
component rows rather than one, because a table with one row in it cannot say whether a number
belongs to the mode or to the program that happened to be measured — and `editor.beck` is the smaller
of the two *and* the one that reads freshness, which answers the obvious worry: the dimension is a
union and a `match`, not a runtime.

**`wasm-opt -Oz`, which §5.1's release path calls for, has not been run.** So the kernel's figure is a
ceiling rather than a measurement of the best it can do. The kernel also grew 8.5% compressed between
the first measurement and this one, and **an unattributed 15 KB on the download every application
shares is exactly the kind of number this project does not write down and leave**, so it was split:
building the same kernel from the commit a change starts at and from the change itself attributes
**+220 bytes compressed** to freshness — a union in the prelude, an entry in the primitive table, a
vertex in the signal graph and a fourth parameter on every sliced view — and the rest to the six
reports in between, which grew `beck-core` with read models, query fusion, routing, presence and the
playground's own surface.

And what one event costs on the wire, at two sizes, because one size cannot tell a cost that is a
function of the change from one that is a function of the collection:

| cards on the board | Mode A (bytes) | Mode B (bytes) |
|---:|---:|---:|
| 100 | 240 | 177 |
| 1,000 | 242 | 177 |

Both are flat, which is what both modes claim. The Mode B frame is smaller here because a moved card
is one `Set` at a path where the DOM patch is a removal and an insertion carrying markup — **but that
is one program's shape and not a law**, and a page that renders very little of a large state would
invert it.

### The gate, and the gate the threshold cannot be

The budget is enforced in CI, in the same job and the same shape as the thin-client budget that has
been enforced since Phase 1, over **every** Mode B example rather than one of them. It begins
`command -v brotli`, so a *missing* compressor fails the step instead of passing it at zero bytes,
and it runs under `shell: bash` for `pipefail` — without which a `brotli` that failed mid-pipe would
make the size the empty string and the comparison a shell error rather than a budget failure. Both
failure paths were run by hand rather than trusted.

**But a bundle is 1.1% of its budget, and a threshold with ninety times its headroom is a gate that
cannot go red.** §82.10's question — *what would have to be true for this to fail?* — has an
uncomfortable answer: a program ninety times the size of the board. Which is the wrong question,
because the budget is **per component**, and what makes it hold for a large application is not that
applications are small. It is that **a bundle is a function of the component's slice**. So that
property is gated directly, under `cargo test`, where no compressor is needed:

> `a_bundle_is_a_function_of_the_slice_and_not_of_the_program_around_it` — adding 10 and then 100
> definitions the component does not reach changes what the bundle carries not at all, and costs
> **under a byte each**.

Two sizes, because one cannot tell "does not grow" from "grows slowly". Under a byte rather than
zero, and the fraction of a byte is real and worth naming: variables are numbered across the whole
program, so a larger program numbers the *slice's own* locals higher and postcard spends a second
byte on a varint past 127. A hundred unreached definitions cost **five bytes between them** —
`O(log n)` in the program's size, four orders of magnitude away from what a genuinely carried
definition would cost.

## 94.12 What an interaction costs, and the render that was paid for twice

Everything above is a **size**. Mode B's claim is not a size — §5.1 promotes a component to the client
so that an interaction does not wait for the network — and that claim went unmeasured, with the first
version of this work giving a reason that turns out to be wrong: *"interaction latency, because that
needs a browser"*. It does not. `beck-wasm` is an `rlib` as well as a `cdylib`, so the kernel the
browser runs can be driven from a test and timed directly.

One card moved on a board of *n*:

| cards | derive | render | diff | the guess | its confirmation |
|---:|---:|---:|---:|---:|---:|
| 100 | 11.2 µs | **1,267.2 µs** | 45.1 µs | 1,324.7 µs | 15.6 µs |
| 1,000 | 30.6 µs | **13,667.0 µs** | 435.0 µs | 13,155.3 µs | 82.2 µs |

Ten times the board costs 9.9× the interaction. **`view` is 97% of it, and it is what grows** —
`derive` is a function of the pending queue rather than of the board, and `diff` is a twentieth of the
render it follows. So Mode B's wire is a function of the change and its **CPU is a function of the
collection**. Moving one card on a thousand-card board is 13 ms of browser CPU, which is a dropped
frame on this machine and several on a phone.

### The interpreter is not why, and neither is the missing code generator

The obvious reading is that this is what §94.15's "no codegen" costs, and that a compiled `view`
would fix it. **That is not what the numbers say**, and two checks say so.

The first is the compiler's own account of the program: `beck explain incremental
examples/board.beck` reports that 1 of the view's 18 operators updates from the change itself, 17 are
recomputed when an input moves, and the page's children are still assembled in full every time.

The second is measuring the incremental engine against the same interaction. The engine the *server*
renders Mode A through takes 15.0 ms cold and **22.2 ms warm** on the thousand-card board, against the
kernel's 13.7 ms recompute. **Maintaining one operator of eighteen does not pay for the delta
machinery around the other seventeen.** The server pays this too; Mode B is not discarding an
advantage the server has.

So the shape is `view` being a pure function of the whole state, and **both backends have it**. A code
generator would divide the constant — perhaps by a lot — and leave the growth exactly where it is.
That does not make it worthless; it makes it the wrong thing to reach for first, and it makes
[`23`](23-incremental-views-report.md) §23.8's open problem — children assembled in full — the thing
Mode B most needs and the thing it cannot fix from inside Mode B.

### One thing here *was* Mode B's, and it was half the cost

An interaction was paying for **two** full renders. The client proposes, renders its guess, and shows
it. Then the server's data patch confirms that command, the repaint runs again — and by the argument
that makes optimism correct in the first place, the state it derives is *equal* to the one the guess
was derived from. Same state, same `view`, same page. **The second render was 13 ms of work with a
known answer, and it ended in a diff returning nothing.**

The client now keeps the state it last rendered from beside the page it rendered, and returns early
when they agree. The confirmation costs 82 µs instead of 13,155 — **~150× cheaper at a thousand
cards**, and an interaction end to end is halved. The two fields are one struct rather than two fields
on the client, because the shortcut's whole safety is that they agree; kept apart they could be
updated apart, and that failure is a stale page rather than a compile error.

**That shortcut then had to learn a second question, twice.** It asks whether the *state* moved. A
route change moves the session and not the state, so under the guard a navigation was the one
interaction Mode B rendered nothing for. And a confirmation is precisely the case where the state does
not move and the **freshness** does: `Pending(1)` becomes `Confirmed`, and a page that renders
"saving…" has to be repainted for it — which would have made the one interaction freshness exists for
the one interaction Mode B renders nothing for. So the shown page carries the freshness it was
rendered at, and that comparison is asked **only of a component that reads the answer** — a
compile-time fact from the splitter rather than a guess about the view's body. Comparing it
unconditionally would have handed every program in the tree back the second render, to show nobody
anything.

**What this measurement does not claim.** It is the kernel's cost compiled for this machine, not in
WebAssembly, which will be slower by some factor this does not establish — the *ratios* and the
*shape* carry across and the absolute microseconds do not. It is one program and one interaction. And
the confirmation still grows, 5.3× for ten times the board, because establishing that two states are
equal walks the map; it is 0.6% of an interaction, so it was left alone rather than made `O(change)`
with the shared-subtree walk `PMap::diff` already has. **That is a known cost with a known fix, not an
unexamined one.**

## 94.13 What building it found

Beyond §94.10's two offline defects and §94.12's double render, six more — and the interesting thing
about most of them is that they were **older than the work that found them**.

**A view could not be factored into functions.** The first wire-cost table read 25,784 bytes at 100
cards and **257,084 at 1,000** — a quarter of a megabyte for one card moving. A DOM patch that is a
function of the *page* rather than of the change contradicts the whole of §5.1, so the number was a
design question rather than a fact to write down, and the answer was two ops each carrying a whole
`<section>` as **escaped text**. `board.beck` assembles its page out of a function, and the `ui:`
macro lowers a child that is not an element through `html_text` — whose own documentation says the
case is "an ordinary function call producing text **or Html**" — and `html_text` called `.display()`
on it. The fix is one arm: a child that is already a tree is spliced, not stringified. The effect on
the number is 242 bytes, flat. **The effect on the language is larger**: `corpus/25-thread.beck` had
been composing a page that way since [`27`](27-the-walls-come-down-report.md), and its output was
wrong in the same way with nothing asserting otherwise. The defect was found by measuring a *cost*,
not by testing a behaviour — no assertion in the tree looked at enough of the page to see it, and
`expect page contains "…"` passes happily on escaped markup. The gate that stops it coming back is a
snapshot, which is [`22`](22-phase-3-report.md)'s argument arriving on time.

**The served page had never contained the element its JavaScript looks for.** Both clients open with
`const root = document.getElementById("b-root"); if (!root) return;` — and `beck run`'s document put
its attributes on `<body>` with no `#b-root` anywhere. **The thin client has therefore returned
immediately, in every browser, since Phase 1**: no socket, no patches, no interactions. Every test in
the workspace passed because none of them ran JavaScript. The document now wraps the rendered page in
that element, which is also the right shape for a different reason: a patch path is child indices
from the frame root, and the body's other children are the script tags.

Then three that only a browser could find, in the first run of the browser suite:

**The patch protocol sorted attributes.** An element's attributes crossed in a `serde_json::Map`,
which is a `BTreeMap`, so a rebuilt element carried them alphabetically where the server-rendered
document had them in source order. Nothing was *wrong* with the page; it simply was not the same
page, and no assertion had ever compared the two. The wire form is pairs now, in the order the
program wrote them — which §4.4's binary mirror already used, so this is the JSON form catching up.

**First paint was not free at position zero.** §5.1 promises "First paint: free — it *is* the SSR
output". The thin client resumes from a `seq` attribute, and the protocol read `seq = 0` as "I have
nothing; send me the world" — so a document rendered from an empty log was immediately replaced by a
full frame the browser had just been given. `Hello.seq` is now optional: **absent** means "I hold
nothing" and `Some(0)` means "I hold the frame as of zero", **which are different facts that had been
spelled the same way since Phase 0.** A Mode B client sends the absent form, because it holds the
*state* and starts holding none.

**There was no way to know the client was live.** Mode B's handlers are installed at the end of an
asynchronous load, and before that a click reaches nothing. The first version of the browser suite
waited for `window.beck`, which only proves a script tag ran, and then interacted into the void. The
residue now says so: a `data-b-ready` attribute carries the mode's letter and a bubbling event fires,
so a spinner, a devtools panel and a test all wait on the same signal.

**And a fourth red that was the suite's own fault**, worth recording: launching a browser per test put
six headless Chromiums on a machine that was also running the debug measurement suites, and under that
load a DevTools command went unanswered for thirty seconds. It passed run on its own — which is
exactly the gate [`13`](13-testing.md) §13.7 refuses to have. There is **one** browser now, behind a
lock, so the tests are serial; their pages stay isolated for free, because each serves on a port of
its own and a different port is a different origin. The suite also stopped taking fifty seconds and
started taking two.

**A document with no URL of its own has no route.** The playground's clients are `srcdoc` iframes, and
`about:srcdoc`'s `location.pathname` is the string `srcdoc` — so a residue that read the route
straight off `location` would put that on the `hello` frame, no program would ever match it, and
**every test in this workspace would still pass**. One predicate decides it, both modes ask it rather
than reading the location themselves, and the router installs no listeners at all where there is no
address bar to move.

The pattern across the browser findings is [`82`](82-the-edge-report.md)
§82.10's: none could have been caught by a test that does not execute the residue, and all had been
shipped for as long as the residue has existed.

## 94.14 The gates

`mode_b.rs` (30 tests), `client.rs` (12) and `browser.rs` (in Chromium, no new dependency — CDP is
JSON over a websocket and `tokio-tungstenite` was already here). The browser suite skips loudly
without a browser; `BECK_REQUIRE_BROWSER=1` forbids the skip, and CI sets it along with
`BECK_REQUIRE_WASM=1`.

| What would break it | The test |
|---|---|
| The kernel and the server stop being the same program | `the_browser_renders_what_the_server_would_have_sent`, and the same over 40 commands |
| A page that reads the actor is allowed onto the client, or a routed one is refused | `a_page_that_reads_the_session_cannot_render_on_the_client`, `a_page_may_be_a_function_of_where_the_browser_is_and_not_of_who_holds_it` |
| The session analysis stops following a call, or drops its conservative case | `a_read_of_the_session_through_a_helper_is_still_a_read`, `a_session_that_is_compared_rather_than_read_is_identity` |
| `storable ⟹ sendable` stops holding | `a_secret_cannot_reach_a_mode_b_client_because_it_cannot_reach_the_log` |
| A guess is retired by an ack rather than by the state | `a_guess_is_retired_by_the_state_that_confirms_it_and_not_before` |
| The client stops running the program's own `validate` | `a_command_the_program_refuses_never_reaches_the_page_or_the_wire` |
| A data patch becomes a function of the collection, or is applied to a state it was not derived from | `a_data_patch_costs_the_change_and_not_the_state`, `…fails_loudly` |
| A page reading `freshness` renders on the server, or a chokepoint decides from what is in flight | `a_page_that_reads_freshness_cannot_render_on_the_server`, `the_chokepoint_cannot_decide_from_what_is_in_flight` |
| The double-render shortcut swallows a confirmation, **or** every program starts paying for one again | `a_confirmation_repaints_a_page_that_reads_freshness_and_no_other` |
| The bundle starts being a function of the program rather than of the slice | `a_bundle_is_a_function_of_the_slice_and_not_of_the_program_around_it` |
| The `unsafe` exception grows past four export attributes | `the_wasm_boundary_is_the_only_exception_to_forbid_unsafe` |
| A deep link becomes a correction after first paint | `a_route_is_server_rendered_before_any_script_runs` (browser) |
| A local navigation stops agreeing with the server's page | `mode_b_navigates_without_a_round_trip` (browser) |
| The caret or the scroll is lost | `a_patch_that_rebuilds_the_page_keeps_the_caret_and_the_scroll` (browser) |
| An interaction stops working with the server gone, or a queued command is appended twice, or never | `mode_b_works_with_the_server_gone_and_catches_up_when_it_returns` (browser) |
| A tab cannot come back at all with the server stopped, or at a route the cache never held | `mode_b_cold_starts_with_the_server_gone`, `…_at_a_route_it_has_never_asked_for` (browser) |
| A local copy of another program is restored into this one | `a_local_copy_of_another_program_is_dropped` (browser) |
| The panel stops rendering, or moves inside the frame, or starts carrying the accumulator | `the_devtools_panel_shows_…` (browser), `the_signal_graph_a_panel_draws_…_carries_no_state` |
| The word "saving" never reaches a real DOM, or never leaves it | `mode_b_says_it_is_saving_before_the_server_has_heard_of_it` (browser) |

Four of these were checked by removing the fix and watching them fail, which is the only way to know
a gate is about the gap rather than about the code that closed it: commenting out the caret
restoration, dropping the replace-only filter on scroll capture, and removing each half of the
freshness repaint condition.

The last browser test reads the guessing page **synchronously**, in the same evaluation that
dispatches the key, and the reason is the claim itself: a Mode B interaction is a local fold and a
local render, so the guess and the word for it are both on the page before the function that sent the
command returns. **Polling for "saving" afterwards would be a race against the server's own answer,
and would pass just as well against a page that never said it at all.**

`examples/routed.beck` and `examples/editor.beck` assert their own behaviour in Beck, which needed
the test surface to be able to *say* a route — `session("ana", "/done")`, in `when` and in both page
expectations. A router the program cannot test would be half a feature.

## 94.15 What is not built

**Codegen — half of it, and not the half a component needs.** The kernel still interprets `Core`.
The third emitter §5.1 asks for exists ([`103`](103-the-wasm-emitter-report.md)): `Core` to
WebAssembly, held to the tree-walker by a differential run in a real engine. It compiles the
**scalar subset**, and a component's `view` is nothing but heap — records, a list, a string, an
`Html` tree — so it compiles **none of the corpus** and nothing loads its output.
[`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) is therefore not reversed; §103.6 is that
sentence as a measurement. And §94.12 still says this bullet is **not the leading cost** — a code
generator divides the constant and leaves the growth, because what grows is `view` being a pure
function of the whole state, which every backend shares.

**Lazy routes**, and the reason is not effort. Lazy routes mean not shipping code for a route you are
not on. In **Mode A** there is no code to ship: the browser receives a rendering, and the residue is
the same 20 KB for every route of every program. In **Mode B** the browser receives the component's
slice — and a program has **one component**, because it has one `page`. There is no second
`Signal[Html]` for a bundle to be about, so splitting one component's bundle by route would be
splitting the only thing there is. **The honest ordering is per-component rendering first, in the
language; lazy routes are a consequence of it and not a feature that can precede it.**

**The per-component boundary itself**, therefore. §5.1's "a page mixes modes freely; the boundary is
per component subtree" is unbuilt *in the language* rather than in the client.

**The mode is declared, never inferred.** §5.1 says a component is promoted "inferred from those
requirements" — `optimistic`, `offline`, a latency budget — and none of those are syntax.
`@render(client)` is the whole surface. A wrong inference here would ship a state to a browser, so a
declaration is the conservative default; `beck explain render` prints the counterfactual so the
choice is informed rather than guessed at.

**A devtools extension** (§94.9). A page panel instead, for a reason rather than a shortcut.

**Freshness is one value for a render, not a dimension on every signal.** A page can ask "is any of
this a guess" and not "is *this list* a guess". The stronger reading of §3.7 would need the pending
events' reach through the dataflow tracked per operator, which is the same machinery §5.3's engine
uses for change and is not wired to this. And **the count is of commands in flight, not of guesses
that survived**: a command whose events the client now skips is still counted, which is the reading a
person watching a spinner wants and means `Pending(n)` and the page's own content can disagree about
*n* for the moment before a refusal arrives.

**`beck test` cannot render a pending page.** The harness is the server's fold, so every page it
renders is `Confirmed` — correct rather than broken, and it means a program's "saving…" branch is
gated in Rust and not in Beck. Making it expressible means the test surface saying "proposed but not
confirmed", which is a fourth clause shape.

**No `wasm-opt`**, so the kernel's compressed number is a ceiling. The CI budget is on the
**component**, which is what §5.1's sentence budgets; the kernel keeps an 8 MB ceiling in
`cargo test`, which is deliberately not a budget, because a budget that fails on a machine without
`brotli` is a flaky gate.

**Query strings and fragments.** `session.path` is `location.pathname` and nothing else. A fragment
never reaches a server, so carrying one would be a field whose value differs between the two
rendering modes; a query string is a second vocabulary with its own parsing, and path segments are
available today. Both are additive if a program ever needs them.

**A scroll restoration that is exact.** §94.8 says what it is: exact for the caret, by position for
the scroll.

**Local-first**, in D7's later sense: one writer, a queue rather than a log, and a second device is a
second replica of the same server's history. CRDT-valued types and peer-to-peer are Phase 5's.

**A browser other than Chromium**, and a page more complicated than the board.

**No `beck explain route`.** The signal-graph endpoint and `beck explain render` both say which half
of the session a page reads; a third command saying it a third way is not owed.

### What this corrects, elsewhere

| Where | What |
|---|---|
| [`05`](05-tier-lowering.md) §5.1 | "v0.1 ships Mode A only, with Mode B in Phase 3" is discharged, and the size budget is answered in two parts rather than one. "The router is derived from route declarations" and "navigation is just another command" are both wrong: there are no route declarations, and a navigation is not a command — a command is a proposal that becomes an event and reaches the log, and where a browser is is neither. "Size budgets enforced in CI" is true of the component bundle as of this work; it was the only clause of that bullet that named CI and had none |
| [`05`](05-tier-lowering.md) §5.2 | "graceful drain (finish folds, snapshot, **hand off subscriptions**)" has its third clause |
| [`03`](03-type-and-effect-system.md) §3.7 | Describes `Session` as what an identity subsystem mints. Two thirds of it still is; the third field is the client's own statement about itself. Its freshness sentence is built, with §94.5's narrowing stated |
| [`04`](04-compiler-architecture.md) §4.3 | "A retry after a reconnect is safe" is now true of the *reply* as well as of the log |
| [`10`](10-decisions.md) D5 | Says a component is promoted "when it declares `optimistic`, `offline`, or a latency budget the round trip can't meet — or when the placement solver's cost model says the crossing is cheaper as data than as patches". Built as `@render(client)`, explicitly, with the cost printed beside it and no inference. D5 did not anticipate §94.2's refusal, which is the one thing this adds to the decision rather than implementing from it |
| [`18`](18-phase-0-report.md) | The thin client works against a document Phase 1's runtime never produced, so "the thin client applies patches in the browser" was an untested claim rather than a false one — but it was not true of anything this repository serves, and now it is. §18.5's resumption rule is unchanged; what changed is that position zero can be *claimed* |
| [`23`](23-incremental-views-report.md) | "The plan is where the client work attaches. Mode B needs a per-component kernel" is answered, and the answer is that the kernel needed the **slice** rather than the plan: the dataflow plan is a Mode A object, and a Mode B client evaluates the view directly |
| The `ui` macro | `html_text` no longer stringifies an `Html` child (§94.13). Any page composed out of functions is a different page than it was, in the way it was always meant to be |
| CI | `.github/workflows/compiler.yml` gains a job that installs the wasm target, builds the kernel and runs the browser and Mode B suites with the skips forbidden. Its "no user JavaScript in Mode A" step asserted *one* script tag; the residue is several files now, so it asserts the property instead — every script the document loads is one this server serves, and none of it is inline |
