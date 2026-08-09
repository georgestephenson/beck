# 94 — Phase 3 report, part 62: the browser renders, and what that costs it the right to know

**Built.** A component may say `@render(client)`, and the wire stops carrying a rendering of the
state and starts carrying the state. The browser holds the accumulator, runs the program's own
`validate` and the program's own fold speculatively, renders the same view the server would have
rendered, and reconciles by `seq`. [`08`](08-roadmap.md)'s "**Mode B client**: per-component WASM
(view + fold + signal kernel), optimistic application with `seq` reconciliation" — one of the four
bullets [`08`](08-roadmap.md) §8.6 has listed as untouched since Phase 3 began.

The interesting half is not the WebAssembly. It is what a component has to give up to earn the
mode: **a Mode B page may not be a function of who is asking**, because Mode B sends the browser
the state and a page that filters by identity is a page whose state is not that browser's to hold.
That is §94.2, it is a compile error with the reason attached, and it is the only new refusal here.

What this is not is codegen. The kernel is `beck-eval` compiled to `wasm32-unknown-unknown` —
[`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) is that decision, with its cost measured
rather than estimated in §94.6. And nothing here has been loaded by a real browser: §94.7 says so
first, because "built", "runs" and "measured" are three different claims and this report makes two
of them.

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
everything else follows without a design decision: the client can render because it has the input
to the view; it can guess because it has the input to the fold; the server does less because it is
not rendering; and the browser has been given data it did not have before, which is §94.2.

The implementation is correspondingly small at the seam. `beck_rt::session` gained exactly one
branch — `Feed::Dom` renders per subscriber and diffs two pages, `Feed::Data` diffs two states —
and resumption, acknowledgement, `up_to_date` and the command channel are the same code either way.
A rendering mode is a choice about what a frame contains, not about how a subscription works.

## 94.2 A Mode B page may not be a function of who is asking

A view has the shape `(state, session) -> Html`. If it reads the session it renders a different
page for different actors *from the same state* — which is to say it is filtering, scoping or
hiding by identity. `examples/todo.beck`'s view does exactly that:

```python
def view(s: State, session: Session) -> Html:
    todos = mine(s, session)          # filter_list(…, lambda t: t.owner == session.actor)
    return render(todos, remaining(todos))
```

Running that view on the client requires giving the client `State` — every user's todos — and
letting the filter run in the browser. **The page would still look right.** Every user would see
their own todos, and every user's browser would be holding everybody's. That is the worst shape a
disclosure can have, because nothing about the running system looks wrong.

So the compiler refuses it:

```text
error[B0514]: `page` renders differently for each session, so it cannot render on the client
  --> todo.beck:143:1
    |
143 | @render(client)
    | ^^^^^^^^^^^^^^^ `@render(client)` sends the browser the state, not the page
    |
    = note: This page is a function of the session as well as of the state: it filters, scopes or
            hides by identity. A client that rendered it locally would first have to be given the
            state it filters — including everything the filter removes.
    = help: render this component on the server (the default), or make the page a function of the
            state alone
```

The fact it turns on is `Roles::view_is_per_session`, which the slicer already computed for
[`26`](26-arrangement-sharing-report.md)'s fanout analysis — "where a program reads the session is
what decides its fanout cost" — so a program cannot be per-session for the sharing analysis and not
for this one. It is a placement rule rather than a lint, checked in `split` and refused before the
program compiles.

What survives the rule is exactly the class [`10`](10-decisions.md) D5 named for Mode B: "typeahead,
drag-and-drop, editors, anything marked `optimistic` or `offline`" — pages that are the same
function of the same state for everybody. `examples/board.beck` is the one written for this report,
and its comment says why it is a board rather than a todo list.

### The check that turned out not to be needed

The obvious second refusal is §3.5's `Sendable`: Mode B puts the accumulator on the wire, so a
`secret[T]` in it would cross. It was written, and then deleted, because it is **already
discharged**: a durable fold's state must be *storable* (`B0411`), storable is strictly stronger
than sendable ([`beck_core::secure`](../compiler/crates/beck-core/src/secure.rs)), and the
accumulator is what crosses. A secret cannot reach a Mode B client because it cannot reach the log.

A composition is a fragile thing to rely on silently, so `mode_b.rs` asserts it — the refusal is
`B0411`, and `storable ⟹ sendable` is checked directly — rather than leaving a future edit to one
half free to break the other.

## 94.3 Optimism is a property of what crosses, not of a component

D5 says the browser applies the expected event "speculatively — legitimate because it runs the
*same pure fold* the server runs". Taking "the same fold" literally is what forces the design:

* The client holds **confirmed** (the accumulator at `seq`, moved only by a data patch) and derives
  **optimistic** (confirmed, plus every pending command's events folded on top). The optimistic
  state is never stored, because a guess that is kept is a guess that has to be un-kept.
* Reconciliation is therefore not an operation. When a data patch moves `seq` past the position the
  server gave a command, that command stops being pending and the same derivation produces the
  corrected page. A guess that was right costs an empty patch; a guess that was wrong is corrected
  by the same code path. Neither is a special case, and there is no rollback.
* An ack alone confirms nothing. `mode_b.rs` asserts that too — the page does not move on an ack,
  and the pending command is retired by the *state* that includes it.

This also settles a question the design left implicit: **optimism is not an extra feature layered
on Mode B, it is the same fact stated twice.** A client can only guess the next state if it holds
the state the fold is *of*. A client holding a projection — one session's filtered list — could not
apply an event to it without a second, different fold that no program writes. Which is why §94.2's
rule and optimism have the same precondition, and `beck explain render` reports them together.

One consequence worth naming: because `validate` is in the bundle, **the browser refuses what the
server would refuse, with the program's own `Rejection` value and no round trip.** That is not a
duplicated rule — it is the same rule, run early. Authority stays at the chokepoint; the client's
copy is advice to the person typing.

## 94.4 What actually ships to the browser

Two artefacts, and keeping them apart is the whole of §94.6's honesty:

**The bundle** — `beck_core::bundle`, the component's slice: `view`, `validate`, the fold, the
initial state, and every definition those four reach transitively. That closure *is* the slice, and
it is why the bundle is smaller than the program. It carries no types, no signal graph, no test and
no `Placed`: the program was checked on the way in, and the client checks nothing.

Two decisions live in a hand-written mirror type rather than in a `derive`, so that they are
reviewable:

* **Types are erased.** The evaluator never reads `Core.ty` — it dispatches on values — so carrying
  resolved types would roughly double the payload to say something the only consumer cannot use. A
  *compiling* client backend needs them, and that is bundle format 2 rather than a field somebody
  adds quietly.
* **Spans are kept.** Three integers, and the difference between "the fold failed" and "the fold
  failed at `board.beck:47`" in a browser console.

A primitive is encoded as its number in the table, which is compact and silently means something
else if the table changes — so a bundle carries `shape_id`, a digest of every primitive's name and
number. A kernel from a different compiler refuses the bundle instead of executing a `str_len` that
used to be a `list_len`. Same rule as `beck_core::repr`'s `FORMAT` for the log: a misread log is
worse than an unreadable one.

**The kernel** — `crates/beck-wasm`, a `wasm32-unknown-unknown` module with four exports
(`beck_alloc`, `beck_free`, `beck_load`, `beck_call`) and a length-prefixed byte buffer between
them. No `wasm-bindgen`, no generated glue. It is the same program for every Beck application,
because it is a backend rather than a compilation of anything.

**No `unsafe` code**, which took one idea: a buffer is a `Vec<u8>` the module keeps in a table keyed
by the address of its own allocation. The host writes through linear memory — that is what linear
memory is — and Rust reads its own `Vec` back out of the table. Nothing is reconstructed from an
integer. The four `#[allow(unsafe_code)]` are on `#[no_mangle]`, which rustc classifies as unsafe
because two libraries exporting one symbol is undefined at link time; `beck-wasm` therefore denies
where the other nine crates forbid, and `mode_b.rs` gates the extent — no `unsafe` block, no
`unsafe fn`, four allows, all on exports, every other crate still inheriting the workspace lint.

## 94.5 Hydration is free, and the reason is a theorem

The document is still server-rendered: first paint is SSR, as in Mode A. The client then loads the
kernel and the bundle, takes its first frame, and **adopts its own first render as what the DOM
already shows, without emitting a patch.**

That is legitimate because the server-rendered page is `view(state, session)` at some `seq`, and the
client holds the same `view` and — once its first frame arrives at that same `seq` — the same
state. Same function, same input, same page. Nothing has to be reconciled because nothing can
differ.

`mode_b.rs` asserts that as *equality of `Html` values* rather than as similarity of markup:

```text
the_browser_renders_what_the_server_would_have_sent
the_two_modes_agree_on_every_state_a_log_can_reach     (40 commands, accepted and refused)
```

which is checkable precisely because both sides execute the same `Core`. It is the gate every other
claim in this report rests on, and it is the one that would go red first if the kernel and the
server ever stopped being the same program.

## 94.6 What it costs

`cargo test --release --test measure_mode_b -- --nocapture`, on the container this was written in:

| | bytes | brotli | against §5.1's 150 KB |
|---|---:|---:|---|
| The bundle — `examples/board.beck`, 10 definitions, 252 `Core` nodes | 4,872 | **1,753** | 1.1% |
| The kernel — every Beck application, whatever the program | 723,354 | **179,401** | **116.8%** |

Read the two rows separately, because the budget answers a different question for each. "< 150 KB
brotli for a typical Mode-B component bundle" was written about compiled output, where the component
*is* the download. Here the marginal cost of a component is 1.7 KB and the kernel is a fixed,
program-independent, cacheable download that every component of every Beck application shares. The
kernel is 17% over a budget written for something else; the component is 1% of it. Neither number
is flattering on its own and neither is the whole answer, so both are here.

`wasm-opt -Oz`, which §5.1's release path calls for, has **not** been run — it is not installed on
this machine and the number above is what `--release` produces. So 179,401 is a ceiling on the
kernel rather than a measurement of the best it can do.

And what one event costs on the wire, for the same program, at two sizes — because one size cannot
tell a cost that is a function of the change from one that is a function of the collection:

| cards on the board | Mode A (bytes) | Mode B (bytes) |
|---:|---:|---:|
| 100 | 240 | 177 |
| 1,000 | 242 | 177 |

Both are flat, which is what both modes claim. The Mode B frame is smaller here because a moved
card is one `Set` at a path, where the DOM patch is a removal and an insertion carrying markup —
but that is one program's shape and not a law, and a page that renders very little of a large state
would invert it.

What is **not** measured: interaction latency, because that needs a browser (§94.7); memory in the
browser; the kernel's throughput against the server's, which is the same evaluator on a different
target and would mostly measure WebAssembly.

## 94.7 What building it found, and it had nothing to do with Mode B

The first version of the wire-cost table above read:

| cards | Mode A (bytes) | Mode B (bytes) |
|---:|---:|---:|
| 100 | 25,784 | 177 |
| 1,000 | **257,084** | 177 |

A quarter of a megabyte for one card moving. A DOM patch that is a function of the *page* rather
than of the change contradicts the whole of §5.1, so the number was a design question rather than a
fact to write down — and the answer was two `SetText` ops, each carrying a whole `<section>` as
**escaped text**.

`examples/board.beck` assembles its page out of a function: `for c in columns(): column(b, c)`. The
`ui:` macro lowers a child that is not an element through `html_text`, whose own documentation says
the case is "an ordinary function call producing text **or Html**" — and `html_text` called
`.display()` on it. A view composed out of functions returning `Html` therefore rendered those
functions' output into its parent as markup-shaped text: `&lt;section class="column"&gt;…`.

The fix is one arm — a child that is already a tree is spliced, not stringified — and the effect on
the number is the table in §94.6: **242 bytes, flat**. The effect on the language is larger than
that: before it, a `view` could not be factored into functions at all. `corpus/25-thread.beck` has
been composing `render_comment(r)` that way since [`27`](27-walls-report.md), and its page was
wrong in the same way with nothing asserting otherwise.

Two things about this are worth keeping. The defect was **older than everything in this report** and
was found by measuring a *cost*, not by testing a behaviour — no assertion in the tree looked at
enough of the page to see it, and `expect page contains "…"` passes happily on escaped markup. And
the gate that stops it coming back is a snapshot (`expect page matches snapshot` in
`examples/board.beck`), which is [`66`](66-page-snapshots-report.md)'s argument arriving on time:
"`contains` asserts one string somebody thought to name; a snapshot asserts every attribute".

### And the second one, which is what "no browser has run it" costs

Wiring the Mode B document meant reading the Mode A one, and the served page has **never contained
the element its JavaScript looks for**. Both clients open with

```js
const root = document.getElementById("b-root");
if (!root) return;
```

and `beck run`'s document put `data-b-seq` and `data-b-actor` on `<body>` with no `#b-root`
anywhere. The thin client has therefore returned immediately, in every browser, since Phase 1: no
socket, no patches, no interactions. Every test in this workspace passes because none of them runs
JavaScript — the differential harness applies patch ops with a Rust client, and the subscription
harness speaks the protocol directly.

The document now wraps the rendered page in `<div id="b-root" …>`, which is also the right shape
for a different reason: a patch path is child indices *from the frame root*, and the body's other
children are the two script tags. `http.rs` gates the agreement between the served document and the
served residue, which is as far as a test that cannot execute JavaScript can go — the rest is the
browser in CI that §94.8 still owes.

## 94.8 What is not built

- **No codegen.** The kernel interprets `Core`; §5.1's "compiled to WASM (GC proposal where
  available; Perceus-style refcounting fallback)" is a backend, and
  [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) records why this is the seam it arrives
  at rather than a rewrite.
- **No browser has run it.** The shim (`beck-mode-b.js`), the routes (`/beck-kernel.wasm`,
  `/beck-bundle.bpk`) and the mode-dependent `<script>` tag are written and served; the kernel is
  driven natively in `mode_b.rs` and over a real subscription socket. Nobody has opened a tab.
  [`21`](21-tests-in-beck-and-proof.md) §21.4 already said what closes this — "Phase 3's Mode B will
  need a browser in CI" — and it is still owed. §94.7's second finding is what that gap costs.
- **No offline.** D7's rung 2 "falls out of Mode B + determinism" and it does not fall out yet:
  there is no local log, no queued command surviving a reload, no service worker. The kernel carries
  `init` so a client with no server has a state to render, which is the first brick and not the
  wall.
- **One component per program**, because a program has one `page`. The bundle format is
  per-component and carries the component's name; what does not exist is a *second* `Signal[Html]`
  for it to be about, so "a page mixes modes freely; the boundary is per component subtree" (§5.1)
  is still unbuilt in the language rather than in this.
- **The mode is declared, never inferred.** §5.1 says a component is promoted "inferred from those
  requirements" — `optimistic`, `offline`, a latency budget — and none of those are syntax.
  `@render(client)` is the whole surface. A wrong inference here would ship a state to a browser,
  so a declaration is the conservative default; `beck explain render` prints the counterfactual so
  the choice is informed rather than guessed at.
- **No router, no lazy routes, no focus/scroll polish, no devtools.** The rest of §8.6's client
  bullet, untouched.
- **No `wasm-opt`**, no size gate in CI. `mode_b.rs` asserts a *ceiling* of 8 MB — enough to catch
  the kernel becoming a different kind of object, and deliberately not a budget, because a budget
  that fails on a machine without `brotli` is a flaky gate ([`13`](13-testing.md) §13.7).

## 94.9 The gates, and what makes each go red

`crates/beck-cli/tests/mode_b.rs`, 19 tests:

| What would break it | The test |
|---|---|
| The kernel and the server stop being the same program | `the_browser_renders_what_the_server_would_have_sent`, and the same over 40 commands |
| A page that reads the session is allowed onto the client | `a_page_that_reads_the_session_cannot_render_on_the_client` |
| `storable ⟹ sendable` stops holding, or the fold's state stops needing to be storable | `a_secret_cannot_reach_a_mode_b_client_because_it_cannot_reach_the_log` |
| A guess is retired by an ack rather than by the state | `a_guess_is_retired_by_the_state_that_confirms_it_and_not_before` |
| The client stops running the program's own `validate` | `a_command_the_program_refuses_never_reaches_the_page_or_the_wire` |
| A data patch becomes a function of the collection | `a_data_patch_costs_the_change_and_not_the_state` |
| A patch is applied to a state it was not derived from | `a_data_patch_against_a_state_the_client_does_not_have_fails_loudly` |
| A client adopts a page the DOM is not showing, or rebuilds one it is | `a_client_adopts_the_page_it_was_rendered_and_rebuilds_any_other` |
| A Mode B subscription sends DOM patches, or a Mode A one sends state | `a_mode_b_subscription_carries_the_state_and_the_browser_renders_it` |
| The `unsafe` exception grows past four export attributes | `the_wasm_boundary_is_the_only_exception_to_forbid_unsafe` |
| The kernel stops building for the browser | `the_kernel_builds_for_the_browser` (skips, loudly, without the target; `BECK_REQUIRE_WASM=1` forbids the skip) |

Plus `beck_core::delta`'s own round-trip tests — every patch this module produces, applied, has to
reproduce the value it was derived from — and `examples/board.beck`'s seven tests in Beck, one of
which is the snapshot §94.7 ends on.

## 94.10 What this corrects, elsewhere

- [`05`](05-tier-lowering.md) §5.1's "**v0.1 ships Mode A only** … with Mode B in Phase 3" is
  discharged, and its size budget is answered in two parts rather than one (§94.6).
- [`10`](10-decisions.md) D5's "How the choice is made" says a component is promoted "when it
  declares `optimistic`, `offline`, or a latency budget the round trip can't meet — or when the
  placement solver's cost model says the crossing is cheaper as data than as patches". Built:
  `@render(client)`, explicitly, with the cost printed beside it and no inference. D5 also did not
  anticipate §94.2's refusal, which is the one thing this work adds to the decision rather than
  implementing from it.
- [`08`](08-roadmap.md) §8.6's Mode B bullet is built except for the pieces §94.8 names; the client
  polish bullet is untouched.
- [`24`](24-incremental-views-report.md) §24.9 item 2 — "the plan is where the client work attaches.
  Mode B needs a per-component kernel" — is answered, and the answer is that the kernel needed the
  *slice* rather than the plan: the dataflow plan is a Mode A object, and a Mode B client evaluates
  the view directly.
- The `ui` macro's `html_text` no longer stringifies an `Html` child (§94.7). Any page composed out
  of functions is a different page than it was, in the way it was always meant to be.
- The served document contains `#b-root` (§94.7). [`18`](18-phase-0-report.md)'s thin client works
  against a document Phase 1's runtime never produced, so "the thin client applies patches in the
  browser" has been an untested claim rather than a false one — but it was not true of anything
  this repository serves, and now it is.

## 94.11 What Phase 3 is still not

Unchanged except for this bullet: **no LLVM backend and no native codegen**; **no playground**;
identity's OIDC relying party, `managed()` provisioning, the claims mapping and presence
([`48`](48-identity-report.md) §48.5); three of the supply-chain bullet's four pieces
([`92`](92-sbom-report.md) §92.5). **Client polish** is untouched, and Mode B now has a browser-half
that no browser has run.

The exit criterion is a claim about a person, and no outside developer has read the guide
[`86`](86-getting-started.md) published.
