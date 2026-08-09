# Phase 3 report, part 64 — the route is a field of the session

**Built**, except for the one item that has nothing to be about yet, which §96.7 says so about.

[`08-roadmap.md`](08-roadmap.md) §8.6 has carried one line since Phase 3 began that nothing had
touched:

> Client polish for both modes: router, forms, lazy routes, focus/scroll preservation, devtools
> extension showing signal graph, patch traffic and pending state.

[`94`](94-mode-b-report.md) §94.8 listed it as "the rest of §8.6's client bullet, untouched" and
§94.11 repeated it. This is that line, and the interesting half is not any of the five items. It is
what the first one found: **`Session` had been one word for two facts**, and the rule that decides
where a component may render was about only one of them.

## 96.1 A route is not a router

The obvious way to add routing to a language is to add routing to the language: a route table, a
matcher, a `route` form, a path-parameter syntax. None of that is here, and the reason is that a
Beck page is already a pure function of two things and one of them is the connection.

A route is a field of `Session`:

```
model Session {actor: Str, claims: Map[Str, Str], path: Str}
```

That is the whole feature. `view(state, session)` may read `session.path` the way it already reads
`session.actor`, and "which page is this" becomes an ordinary `if` over an ordinary `Str`, written
in the program rather than configured beside it:

```beck
def shown(t: Task, path: Str) -> Bool:
    if path == "/done":
        return t.done
    if path == "/active":
        return not t.done
    return True
```

Everything downstream already knew what to do with it. The route reaches the page through
`beck_core::edge::session`, which is the one constructor both tiers build a `Session` with. It makes
the page per-session, which §3.8's fanout analysis and §5.3's shared cut already knew how to read.
The incremental engine already recomputes what is downstream of `Op::Session` when the session
changes — a comment in `plan.rs` had called the session "constant for the life of one subscription",
which stopped being true and cost nothing, because `Op::Session` has compared the value it was handed
against the one it held since it existed, and `Cell::rebuilt` has been contagious downstream since
[`26`](26-arrangement-sharing-report.md) §26.6 for exactly the case of a subscriber whose session
moved. Not one line of the engine, the splitter, the plan or the fold changed.

What did have to exist is the edge: something has to *tell* the program where the browser is.

* **The document.** Every `GET` that is not one of the runtime's own paths renders the program at
  that path. So a pasted URL and a reload are server-rendered at the route they name, before any
  JavaScript runs — which is the difference between a router and a single-page application that
  corrects itself after first paint. `beck_rt::http::reserved_routes` is the list a program's routes
  may not be, published as a function because "read the router" is not an answer.
* **The `hello` frame.** A subscription states its route when it opens, not afterwards. A route
  established by a second message would leave every reconnection — and every cold start with the
  network down — rendering the root's page until that message arrived.
* **A `nav` frame**, `{"t":"g","path":"/done"}`, for a route that changes while the socket is open.

And in the browser, one `click` listener over `a[href]`. The link in
[`examples/routed.beck`](../compiler/examples/routed.beck) is an ordinary anchor with nothing in it
that knows a router exists; a browser executing no JavaScript at all follows it to a page the server
renders. That is the property worth having, and it is a consequence of routes being real URLs rather
than a decision anybody made about progressive enhancement.

One thing the router **narrowed without anybody noticing**, which is why it is here rather than in a
list of extras. [`94`](94-mode-b-report.md) §94.13's cold start caches the shell so that a Mode B tab
reloading with no network gets a page: the worker caches `/`, because "that is what a reload asks
for". With routes it is not. A tab that had navigated to `/active` and then reloaded offline asked
for a document the cache had never held, and got the worker's "never been here before" page — for a
route it was perfectly able to render, from state it was already holding. The fix is one branch and
it is the same observation the mode rests on: in Mode B the route is the *client's* to render, so one
cached document answers for every route. Caching a document per route would have been caching one
file under several names and would still have missed the route nobody had visited.

## 96.2 `Session` was one word for two facts

Here is the wall. [`94`](94-mode-b-report.md) §94.2's rule is that **a Mode B page may not be a
function of who is asking**, because Mode B sends the browser the state rather than the page, so a
page that filters by identity is a page whose state is not that browser's to hold. `B0514` enforced
it, and what it tested was `Roles::view_is_per_session` — whether the page reads *the session*.

Put the route on the session and that rule refuses every routed page in Mode B. Which is wrong, and
not marginally: a page that renders by route is not hiding anything from the browser it is running
in, because that browser chose the route and already holds the state.

So the two halves of a `Session` are not the same kind of thing:

| field | says | verified by | may a Mode B page read it |
|---|---|---|---|
| `actor` | **who** is asking | the identity provider ([`48`](48-identity-report.md), [`95`](95-oidc-relying-party-report.md)) | no |
| `claims` | what the provider said about them | the same | no |
| `path` | **where** they are | nobody, and nothing should | yes |

`B0514` now asks which fields the view can observe, and the answer comes from the view's own code:
`beck_core::render::SessionUse`. The analysis is one sentence long — **a `Session` can only be
observed by having a field read off it, so collect every field read whose base is `Session`-typed
anywhere the view can reach** — and it is sound without tracking where the value flows, because flow
does not create an observation. Wherever the record ends up, reading it is still a `Field` over a
`Session`-typed base, and every definition it could end up in is in the closure the walk covers.
Types are what make it cheap: a field read needs a concrete record type, so a `Session` handed to a
generic definition cannot have anything read off it there — the parameter is a rigid variable and
`x.actor` does not check.

What flow *could* hide is an observation that is not a field read: an equality, a digest, a session
stored inside a value that crosses. Those are the conservative cases and they are named rather than
ignored — a `Session` reaching a primitive, or the inside of a constructed value, is `Identity`.

The refusal is better for it, because it can say what is allowed:

```
error[B0514]: `page` renders differently for each *actor*, so it cannot render on the client
  = note: This page reads `session.actor`: it filters, scopes or hides by identity. A client that
          rendered it locally would first have to be given the state it filters — including
          everything the filter removes. Reading `session.path` is allowed and is not this: the
          browser chose the route and already holds the state.
```

**Eligibility and fanout stopped being the same answer**, and that is the sentence to carry forward.
`examples/routed.beck` is `per_session` — two people on two routes see two pages, so the operators
below the session are theirs and §5.3's cut is unchanged — *and* it renders in a browser. Before
this, one fact answered both questions, and it answered the second one wrongly for a whole class of
page.

## 96.3 What a navigation costs, in each mode

The same program, both ways. `examples/routed.beck` has no `@render(client)`; adding that one line
is the whole of the difference, which is what makes "the router is the same in both modes" a test
(`browser.rs`) rather than a claim.

**Mode A.** The client sends 24 bytes and the server answers with the difference between two pages —
111 bytes on the state the gate builds. Not a page: a diff, because a navigation is an ordinary
change and goes through the same `Feed::advance` an event does. There is nothing in `beck-rt` that
knows what a route is; `session.rs` sets a string and re-renders.

**Mode B.** The kernel moves `session.path` and re-renders from the state it already holds, so the
page changes with no round trip. The server is told anyway — 24 bytes, answered with nothing —
and *not* for the page, which it is not rendering. It is so that the `Session` the server hands
`validate` is the one this client's own `validate` saw. Both frames travel on the one socket, so the
navigation precedes every command proposed from the page it produced; that ordering is free and it
is the only reason a Mode B nav needs a wire message at all.

One thing in the kernel had to learn a second question. `Client::repaint` skips a render when the
state a page was derived from has not changed — [`94`](94-mode-b-report.md) §94.14's optimisation,
worth half an interaction. A navigation changes nothing but the session, so under that guard a route
change was the one interaction Mode B rendered nothing for. `paint(force)` is the fix and the
`force` has exactly one caller.

## 96.4 Forms

`on_submit` on a `form:`, and one new hole in a handler's template:

```beck
form(on_submit=Add(id=Id(value="$id"), text="$field:text")):
    input(name="text", placeholder="what needs doing?", autofocus="on")
    button: "add"
```

No compiler change. The `ui:` macro has turned `on_<event>=` into `data-b-<event>` since Phase 1, so
a form is a form and `submit` is an event the residue listens for. What fires it is the *browser's*
submit — a button, or Enter in a single-line field — so the page a keyboard and a screen reader
already know how to drive is the page the program wrote, rather than a `keydown` handler this
project invented. `on_input` and `on_change` arrived with it for the same reason: the distinction
between "as it changes" and "on commit" is the browser's, and reusing it costs one listener each.

`$field:name` reads a named control out of the form being submitted. Writing it found something the
two existing holes had hidden: **the filler only looked at the top level of a command**. Every
command in the tree happens to flatten — `Id(value="$id")` is a newtype and crosses as a string — so
a hole one level down had never been written, and a command whose field is a record would have put
the literal `"$id"` in the log. The filler recurses now, through objects and arrays alike.

## 96.5 The caret, and the list somebody had scrolled

A patch that replaces an ancestor of the focused element destroys it, and the browser's answer to
"what is focused now" is `body`. A whole-frame replace is what a `Reset` frame carries — a
reconnection whose gap the log could not answer — so this is not a corner case, it is what happens
to anybody who was mid-sentence when their train came out of a tunnel.

`beck-patch.js` now records the caret before applying a patch and puts it back if the element it was
in did not survive: the child-index path from the frame root, the selection range, and the element's
own scroll. It refuses to restore into a *different* element that happened to take the same
position — tag, `data-b-k`, `name` and `id` have to match, which is the program's own answer to what
an element is where there is one and the tag where there is not.

Scroll offsets are kept for subtrees a `replace` is about to rebuild, by position within the
replaced subtree, and restored where the position still holds something scrollable. That is best
effort by construction and the report says so rather than the code implying otherwise: a subtree
that was replaced is one whose shape may have changed. `insert`, `remove` and `move` need nothing —
they do not rebuild the container, so the browser keeps its scroll itself, which is what the `move`
op was added for in the first place.

**The cost is proportional to the patch and not to the page.** Nothing is walked except the subtrees
a replace destroys, and the caret is one lookup. A version that scanned the document for scrolled
elements would have made every patch cost the size of the page, which is the shape this project
spends its gates refusing.

## 96.6 The panel, and why it is not an extension

§8.6 asks for "a devtools **extension** showing signal graph, patch traffic and pending state". What
is here is the three things and not the extension, and the difference is deliberate rather than a
shortcut: an extension is a second artefact with its own distribution, its own permissions and its
own release pipeline, and **nothing in this repository could run one** —
[`94`](94-mode-b-report.md) §94.12's browser gate drives a page over the DevTools protocol. A panel
the server serves is testable by the same harness that tests the client, and it is the same residue:
no framework, no CDN, nothing the network policy this program derives would refuse.

It is loaded on request (`?devtools`, and the switch it leaves behind), so a page that does not want
it pays nothing. It is appended to `body` rather than into `#b-root`, which is not tidiness: a patch
path is a child index from the frame root, and a panel inside it would be counted by every path in
every patch.

The three things:

* **Patch traffic** — frames and ops applied, bytes in and out, frames sent, navigations, and
  whether the socket is up. Counted in `beck-patch.js` where they happen.
* **Pending state** — in Mode A, the commands sent and not yet answered; in Mode B, the commands
  *applied* and not yet confirmed, which is the difference between what the browser is showing and
  what the server has agreed to. Both modes publish it under one vocabulary, so the panel is one
  panel and not two.
* **The signal graph** — the one thing the browser cannot know, because a Mode A client is never
  sent a program. `/beck-signals.json` is the running program's own graph with
  [`beck_core::incremental`]'s verdicts, which is what `beck explain incremental` prints, plus the
  plan's operator counts. Built once per process, for [`67`](67-sqlite-report.md)-adjacent reasons
  that are really [`88`](88-read-models-and-pgwire-report.md)'s: a view of a thing is cheapest to
  keep right by being the thing.

What the endpoint deliberately does **not** carry is the accumulator. A Mode A page is precisely the
part of the state its viewer may see, and an endpoint that handed a browser the rest would be a
disclosure with a friendly name. The gate asserts the absence rather than the presence.

The residue grew, and here is the number rather than an adjective. Mode A's two files were 9,184
bytes and are 20,803; gzipped, 3,910 → 7,970. The panel is a further 7,125 bytes and is not loaded
unless it is asked for. These files carry more comment than code and there is no minifier anywhere
in this project, so the gzip figure is an upper bound on what a build step would ship and the raw
one is not a measurement of anything.

## 96.7 Lazy routes, and why there is nothing to be lazy about

Not built, and the reason is not effort.

Lazy routes mean not shipping code for a route you are not on. In **Mode A** there is no code to
ship: the browser receives a rendering, and the residue is the same 20 KB for every route of every
program. In **Mode B** the browser receives the component's slice — and
[`94`](94-mode-b-report.md) §94.8's third bullet is that a program has **one component**, because it
has one `page`. There is no second `Signal[Html]` for a bundle to be about, so "the boundary is per
component subtree" (§5.1) is unbuilt in the *language*, and splitting one component's bundle by
route would be splitting the only thing there is.

The honest ordering is therefore: per-component rendering first, in the language; lazy routes are a
consequence of it and not a feature that can precede it. This is written here rather than left to be
rediscovered, and it is the one item of §8.6's client bullet that this report does not close.

## 96.8 The gates, and what makes each go red

`crates/beck-cli/tests/client.rs`, 12 tests, and `browser.rs` gained 5 — the split is that focus and
scroll are facts about a DOM, so they are gated where there is one.

| gate | goes red when |
|---|---|
| `a_page_may_be_a_function_of_where_the_browser_is_and_not_of_who_holds_it` | `routed.beck` stops compiling in Mode B, or `todo.beck` starts |
| `a_read_of_the_session_through_a_helper_is_still_a_read` | the analysis stops following a call |
| `a_session_that_is_compared_rather_than_read_is_identity` | the conservative case is dropped |
| `the_page_a_route_renders_is_the_route_the_viewer_names` | the route stops reaching `view` |
| `a_navigation_on_an_open_socket_produces_the_new_routes_page` | a `nav` stops re-rendering, or starts re-rendering when the route did not move |
| `a_hello_carries_the_route_so_a_deep_link_never_renders_the_wrong_page_first` | the first frame is the root's page |
| `every_binding_a_page_emits_is_one_the_residue_captures` | a program binds an event no client listens for |
| `a_handlers_holes_are_named_in_one_place_and_filled_at_any_depth` | the filler stops descending |
| `the_reserved_routes_are_the_ones_this_process_answers` | the published list and the router disagree |
| `the_signal_graph_a_panel_draws_is_the_programs_own_and_carries_no_state` | the endpoint starts carrying the accumulator |
| `mode_a_follows_a_link_and_submits_a_form` (browser) | a link stops being followed, a form stops submitting, or the DOM stops being the server's page for that route |
| `a_route_is_server_rendered_before_any_script_runs` (browser) | a deep link becomes a correction after first paint |
| `mode_b_navigates_without_a_round_trip` (browser) | the local render stops agreeing with the server's |
| `a_patch_that_rebuilds_the_page_keeps_the_caret_and_the_scroll` (browser) | the caret or the scroll is lost |
| `mode_b_cold_starts_at_a_route_it_has_never_asked_for` (browser) | the shell stops answering for a route the cache has never held |
| `the_devtools_panel_shows_the_signal_graph_the_traffic_and_the_pending_state` (browser) | the panel stops rendering, or moves inside the frame |
| `a_route_change_is_a_local_render_and_agrees_with_the_server` (`mode_b.rs`) | the kernel's short-circuit swallows a navigation again |

Two of them were checked by removing the fix and watching them fail, which is the only way to know a
gate is about the gap rather than about the code that closed it
([`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5's pattern): commenting out
`restoreCaret` reddens the caret half, and dropping the replace-only filter on scroll capture
reddens the scroll half.

`examples/routed.beck` asserts its own behaviour in Beck, which needed the test surface to be able
to *say* a route — `session("ana", "/done")`, in `when` and in both page expectations. A router the
program cannot test would be half a feature; the slot is one node with two shapes rather than a
second positional argument, because a form whose arity varies with which optional part was written
cannot be read back without guessing.

## 96.9 What this corrects, elsewhere

* [`94`](94-mode-b-report.md) §94.2 says a Mode B page "may not be a function of who is asking" and
  §94.8's last-but-one bullet says "no router, no lazy routes, no focus/scroll polish, no devtools".
  The first sentence is still exactly right and is now enforced as written; what changed is that
  `per_session` had been standing in for it, and a page that varies by route is not what it meant.
  Of the second, all but lazy routes are here (§96.7).
* [`94`](94-mode-b-report.md) §94.11 lists "client polish" as untouched. It is not.
* `Op::Session`'s doc comment in `plan.rs` said the session is "constant for the life of one
  subscription". It is not, and the engine was already right about it.
* `B0514`'s entry in the error index said "renders differently for each session". It says *actor*
  now, and names what is allowed.
* [`08`](08-roadmap.md) §8.6's client bullet is updated with what remains, which is one item.
* [`05`](05-tier-lowering.md) §5.1 says the router is "derived from route declarations" and that
  "navigation is just another command". There are no route declarations, and a navigation is not a
  command: a command is a proposal that becomes an event and reaches the log, and where a browser is
  is neither. The section carries the correction.
* [`94`](94-mode-b-report.md) §94.13's service worker caches `/` "because that is what a reload asks
  for". With routes it is not (§96.1), and the worker now answers a navigation it has no document
  for with the shell.
* [`03`](03-type-and-effect-system.md) §3.7 describes `Session` as what an identity subsystem mints.
  Two thirds of it still is; the third field is the client's own statement about itself, and the
  section says so.

Nothing in an earlier report is edited; this section is where the corrections are, per the rule in
[`AGENTS.md`](../AGENTS.md).

## 96.10 What is not built, and one thing that is deliberately absent

* **Lazy routes** (§96.7), waiting on per-component rendering in the language.
* **A devtools extension** (§96.6). A page panel instead, for a reason rather than a shortcut.
* **Query strings and fragments.** `session.path` is `location.pathname` and nothing else. A
  fragment never reaches a server, so carrying one would be a field whose value differs between the
  two rendering modes; a query string is a second vocabulary with its own parsing, and path segments
  are available today. Both are additive if a program ever needs them.
* **A scroll restoration that is exact.** §96.5 says what it is: exact for the caret, by position
  for the scroll.
* **Nothing verifies a route**, and nothing should. `session.path` is the client's own statement
  about itself and it reaches `validate` on the `Proposal`'s session exactly as the actor does — but
  the actor is what a provider minted and the route is what a browser typed. A program that used it
  for authority would be making [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.4's
  mistake in a new place. What the architecture guarantees instead is narrower and is worth stating:
  the route cannot reach a **fold**, because an `Envelope` carries the actor's name and nothing
  else — so no replay can depend on where anybody was browsing, which is the property
  [`95`](95-oidc-relying-party-report.md) §95.4 established for claims and which this inherits
  unchanged.
* **No `beck explain route`.** The signal graph endpoint and `beck explain render` both say which
  half of the session a page reads; a third command saying it a third way is not owed.
