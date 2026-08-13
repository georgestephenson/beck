# 98 — Phase 3 report, part 66: the playground, and what a tab turns out to be

**Built.** [`17`](17-playground.md)'s rungs A and B: the compiler, compiled to WebAssembly and
answering in the browser, with source on the left and what it derives on the right; and the whole
*application* in the same tab — a log, a fold, and two client iframes speaking the patch protocol
over a `MessageChannel` — with a `seq` scrubber that folds the log again from genesis. It is one of
the two bullets [`08`](08-roadmap.md) Phase 3 listed as untouched when this was started, and the last
one that is a *product* rather than a backend.

The interesting half is not the WebAssembly, and it is not the page. It is what §17.2 claims and how
much of it survives contact: **"by the differential harness's own guarantee, rung-B behaviour *is*
the deployed behaviour."** That sentence is only true if the tab runs the deployed code, and
`beck-rt` does not cross to `wasm32-unknown-unknown` — it holds Postgres, redb, SQLite, TLS and a
multi-threaded reactor. So the work was mostly not in `crates/beck-play/`: it was in deciding which
half of the runtime is *about the machine* and moving the other half somewhere both hosts can reach
it. That is §98.2, and it is the only structural change here.

## 98.1 Rung A, and the drift it found on the way in

§17.1 lists what a visitor gets with zero servers: "type checking with real diagnostics, macro
expansion … **inferred placement per definition**, generated dataflow/SQL plans, generated
Kubernetes objects, effect signatures, and `beck explain`". All of it is there, as thirteen tabs —
eleven derived answers, the two surfaces, and diagnostics when there are any — and
`crates/beck-play/src/analysis.rs` is 169 lines because every one of them is a single call into the
compiler.

It was not 169 lines when it was written. Three of `beck explain`'s answers — the placement table,
the wire id, and a type's flow — were **`println!`s in `main.rs`**, so a playground wanting to show
them had two options: call a private function that did not exist, or render the same data a second
time. The second option is how a language playground ends up disagreeing with its own compiler about
what a program means, quietly, in the direction nobody checks.

They are now [`beck_core::place::report`], [`beck_core::split::wire_report`] and
[`beck_core::secure::flow_report`], returning `String`, and `main.rs` prints what they return. The
gate is not that they exist:

```text
playground.rs::the_playground_shows_what_the_command_line_shows
```

which runs the `beck` binary and the playground over the same source under the same module name and
compares the **bytes**, for nine sections, over three programs. It goes red the day somebody renders
a placement table twice. (The same module name matters and is worth saying: a tier crossing's id is
content-derived from the module (§4.3), so `beck explain flow todo.beck` and a playground holding
that text legitimately print different digests. Comparing them anyway would be asserting that two
different modules are one.)

Two rules the page follows that are not in §17.1. **A program that does not compile derives
nothing**: diagnostics become the first and only tab, because there is no placement for a program
that has no types, and a stale table beside a red error teaches a visitor something false. And **a
library is a library rather than three errors** — a module with no merge point is what a visitor
pasting three definitions into an empty editor has written, and `beck check` has answered "ok: … a
library: no merge point, so there is nothing to run" since [`86`](86-getting-started.md) found the
same thing. It gets its placement and its signatures, and not the sections that are questions about
an application: a plan is a plan *of* a view, and a NetworkPolicy is derived from what a deployment
would do.

[`beck_core::place::report`]: ../compiler/crates/beck-core/src/place.rs
[`beck_core::split::wire_report`]: ../compiler/crates/beck-core/src/split.rs
[`beck_core::secure::flow_report`]: ../compiler/crates/beck-core/src/secure.rs

## 98.2 The tab is a host, and `beck-host` is what made that a true sentence

§17.2's table is three implementations of the runtime's interfaces, and the third is a browser:

| Runtime interface | Production | DST | **Playground** |
|---|---|---|---|
| Clock | OS | simulated | supplied by the page |
| Network | Tokio/websocket | simulated | `MessageChannel` |
| Log storage | Postgres/redb | simulated | an array in the worker |

The design says a browser is a third *host*. What it does not say — because it was written before
there was a runtime to look at — is that `beck-rt` had never been divided along that line. The
sequencer's rules and its `tokio::select!` were in one function; the wire's message types and the
websocket that carries them were in one crate; the bridge to a compiled program sat beside a
Postgres client.

So `crates/beck-host/` is the half that is **program-shaped rather than machine-shaped**:

| What moved | What it is |
|---|---|
| `program.rs` → `beck_host::program` | `Runtime`: the compiled program's `validate`, fold, `view` and command decoder, prepared by a `Backend` |
| the log's records → `beck_host::record` | `Seq`, `Instant`, `Envelope`, `Pending`, `Snapshot`, and the bytes a store writes |
| `protocol.rs` → `beck_host::protocol` | `hello`/`c`/`ping` up, welcome/patch/ack/nack/up-to-date down |
| the sequencer's *decisions* → `beck_host::sequence` | de-duplicate, validate against the batch's own speculative state, check storable, fold |

`beck-rt` re-exports every one of them at the path it had, so no other file in the workspace
changed: `beck_rt::Runtime` is `beck_host::Runtime`, and `beck_rt::protocol::ClientMsg` is
`beck_host::protocol::ClientMsg`; the workspace is thirteen crates. What is left in `beck-rt` is
precisely what needs the machine — the queue, the durable append, the version watch, the snapshot
timer, the socket, the identity provider, the quota — and `app.rs`'s sequencer is now that list and
a call.

**The membership rule is a sentence and it is enforceable**: if it needs the machine, it stays
upstairs. The one place that bit was the clock. `beck_host::sequence` reads none, because
`std::time::Instant::now()` on `wasm32-unknown-unknown` is a panic rather than a number — so the one
step that costs real time takes a `Meter` the host supplies, `beck-rt` passes the histogram
`beck.fold.duration` has always been, and the tab passes `Untimed`. F11 asked for a supplied clock
three phases ago; this is the same argument arriving from the other direction, because a target
without a clock cannot cheat.

### The defect the extraction found, and it is in the *server*

Lifting the sequencer's inner loop out made one thing visible that reading it had not. A command
that produces several events folded them **one at a time into the shared speculative state** and
pushed each onto the batch as it went — and if the third one failed, the proposer was told the
command was refused while the first two stayed in `pending` and were appended.

That contradicts the log's own contract, three lines into `log.rs`: "a batch of events from one
command is appended **atomically at contiguous `seq`s** — no fold ever observes half a command's
consequences." `beck_host::sequence` folds a command's events into a copy and commits the copy only
when every one of them has landed, so a refused command adds nothing.

**This is not gated, and the reason is worth stating rather than hiding**: both failure paths are
unreachable for a program that compiled. An event that cannot be stored is what `secure::storable`
proves cannot exist (`B0411`), and a fold that fails mid-command is an evaluator error in code the
checker accepted. The fix is a correctness fix to a path no test can currently reach, and it is
recorded here so that the next person to make one of those paths reachable knows the behaviour was
chosen rather than inherited.

## 98.3 The client in the iframe is the client, and that cost one seam

§17.2 says the tab holds "the thin patch client in an iframe, speaking the identical patch/command
protocol over a `MessageChannel` instead of a websocket". The word doing the work is *identical*, and
the way to earn it is not to write a playground client.

`beck-patch.js` — the residue every Beck application serves — had `new WebSocket(...)` inside
`connect`. It now has a **transport seam**: `connect` dials through `beck.dial` when the page has
set one and through the websocket when it has not, and everything else about the connection is the
same code either way — the outbox, the backoff, the `hello` frame, and the rule that a null `seq` is
sent as an *absent* field because "I hold nothing" and "I hold the frame as of zero" are different
facts ([`94`](94-mode-b-report.md) §94.12).

So a client iframe loads `beck-patch.js`, then a **32-line** `beck-play-port.js` that sets
`beck.dial`, then `beck-thin.js` — unmodified, and unaware it is in a playground.
`playground.rs::the_playground_serves_the_runtimes_own_residue` asserts the bytes: the playground's
`beck-patch.js` and `beck-thin.js` *are* `beck_rt::PATCH_CLIENT` and `beck_rt::THIN_CLIENT`, and the
one file that is the playground's own contains no patch interpreter.

The iframe's document is assembled the way `beck run`'s is, too: the page the tab would have
rendered, inside `#b-root`, with `data-b-seq` on it — first paint is SSR, and the socket that opens
afterwards resumes from the position that render reflects. It is the same document because it is the
same claim.

## 98.4 The two demos §17.2 says nobody else can build

**Multiplayer in one tab.** Two iframes, two subscriptions, one log. ana clicks; the command goes up
her port to the worker, through the one merge point, into the array that is the log; every
subscription is advanced; and the frame that reaches **bo** is what makes this a fanout rather than a
mirror. The gate is in Chromium
(`browser.rs::the_playground_runs_the_application_and_two_clients_of_it`) and asserts exactly that
crossing: ana clicks, bo's `.count` reads `1`; bo clicks, ana's reads `2`.

An idle subscriber whose page did not change is sent **nothing**, which is the property the whole
subscription design exists for and is gated separately on `todo.beck`, whose view filters by the
session: ana adds a todo, and bo — who cannot see it and is not waiting on anything — is owed no
frame at all.

**And who else is here.** [`96`](96-presence-report.md) landed D6's presence signal while this was
being written, and it is the one input to a view that moves without an event — so it is also the one
place a tab could quietly answer a different question than a server. `beck_host::Runtime::view`
renders against the viewer's *own* roster, which is right for `beck test` and wrong for an
application; the tab therefore keeps a roster, built from the thing it already knows — its own
subscriptions — exactly as `beck_rt::App` keeps a registry. A second client arriving moves the first
client's page, and `playground.rs::presence_in_the_tab_is_who_is_connected_to_the_tab` is what says
so. The bound `beck_rt::presence` needs is not here and does not need to be: a server's roster is
keyed by a name the client chose, and a tab has as many connections as the page opened.

**Time travel.** The scrubber under the application asks the tab for the page at a position, and the
tab computes it by folding the log **from genesis** with the program's own `apply_event`. Not a
recording and not an undo stack: D3's genesis-replay discipline, as something a visitor drags.
`playground.rs::the_scrubber_renders_the_state_the_log_produces_at_every_position` holds it to an
oracle that is the *other host* — the same commands through a `beck_rt::App`, with its page captured
after each one — and the browser test drags the scrubber to 1 and to 0 and then checks that the live
clients did not move, because a scrubber that moved the application would be an undo.

## 98.5 What is actually in the tab

Two artefacts, and keeping them apart is the whole of §98.6's honesty.

**The module** — `crates/beck-play`, a `wasm32-unknown-unknown` build of the front end, the
evaluator, the infrastructure derivation and the tab server, with three exports (`beck_alloc`,
`beck_free`, `beck_call`) and a length-prefixed byte buffer between them. No `wasm-bindgen`, no
generated glue, and **no `unsafe`** — the same construction Mode B's kernel uses, for the same
reasons, gated the same way (`the_wasm_boundary_is_the_only_exception_to_forbid_unsafe`). It is the
same module for every program, because it is a compiler rather than a compilation of anything.

**The page** — eight files: an editor, a tab strip, two iframes, a worker, a transport, and two
stylesheets. `beck play --out <dir>` writes all of it plus the module, and that directory *is* the
deployment: §17.1's "costs a CDN" is not a figure of speech, and `beck play` with no `--out` serves
the same bytes on a port so that a browser can instantiate WebAssembly at all, which it cannot over
`file://`.

## 98.6 What it costs

`cargo test --release --test measure_play -- --nocapture`, on the container this was written in.

**The download**, which is what rung A costs a visitor:

| | bytes | compressed |
|---|---:|---:|
| The page — eight files | 30,243 | 10,575 (gzip) |
| The module — every program, whatever the source | 2,679,618 | 863,515 (gzip) |

`brotli` is not installed on this machine, so the compressed column is gzip and is a **ceiling** on
what a CDN would send; `wasm-opt -Oz` has not been run either, for the same reason
[`94`](94-mode-b-report.md) §94.6 gives. The module is large and honestly so: it is the whole front
end — parser, macro expander, inference, placement, the splitter, the plan, the read-model
derivation and the Kubernetes emitter — plus the evaluator. What it buys is that there is no server
in the answer, which is the entire point of the rung.

**An answer**, which is what rung A costs per keystroke:

| program | lines | µs |
|---|---:|---:|
| `counter.beck` | 103 | 2,139 |
| `25-thread.beck` | 171 | 3,180 |
| `todo.beck` | 191 | 3,253 |

Every section, from source, on every analysis — nothing is cached and nothing is incremental. Two
milliseconds against a 250 ms debounce, natively; a browser will be slower by a factor this does not
establish, and the margin is three orders of magnitude wide.

**An interaction**, and whether history makes it worse — because the question a person asks after
ten minutes of clicking is whether the tab slows down:

| events in the log | one command (µs) | scrub to head (µs) | per event (ns) |
|---:|---:|---:|---:|
| 100 | 26 | 140 | 1,168 |
| 1,000 | 26 | 955 | 937 |

Two shapes, and they are the shapes those two operations *are*. A command is a fold and a render of
the **state**, so it does not grow with the log — `counter`'s state is two integers, so what the
first column measures is the constant, and a program whose state grows would pay for that instead. A
scrub is a fold **of the log**, so it grows with the history, linearly, at about 900 ns an event. A
thousand events is under a millisecond and a million would be a second; the fix, if it is ever
needed, is the snapshots the durable substrates already have and a tab does not.

Measured natively rather than in WebAssembly, exactly as [`94`](94-mode-b-report.md) §94.14's kernel
numbers were: the crate is an `rlib` as well as a `cdylib`. The ratios and the shapes carry across;
the absolute microseconds do not.

## 98.7 What is not built

- **Rung C.** Untouched, and Phase 4's ([`08`](08-roadmap.md)): an ephemeral cluster per
  session, with the compiler as the first sandbox. Nothing here is that, and nothing here compiles
  against a restricted effect budget — a playground program can name `net.out` and the page will
  show the effect row and the NetworkPolicy it implies, because it never runs anything that has one.
- **Mode B in the tab.** The tab serves Mode A: the server renders and sends DOM patches. A
  `@render(client)` program *analyses* completely — `beck explain render`, the bundle's size, the
  refusal rule — and the page says so rather than serving it wrongly. Running one in the tab means
  the Mode B kernel inside the client iframe with its bundle over the port, which is a second module
  in a second frame and is a piece of work rather than a flag.
- **No IndexedDB.** §17.2's log-storage row says IndexedDB; the tab's log is an array, so a reload
  starts from `init`. Mode B's `localStorage` snapshot ([`94`](94-mode-b-report.md) §94.13) is the
  shape the answer would take and it is not wired here.
- **No sharing.** §17.4 is content-addressed share links — "a share link is a digest; forks are new
  digests" — and there is none. The playground has no URL state at all: an edit is not addressable.
- **The playground is not a Beck app.** §17.5 and D15 say it should be, and it is a page of
  JavaScript with a Rust module under it. What would make it one is the registry and the site tier,
  and the honest position is that this rung proves the *language* runs in a tab, not that this
  particular tab was written in it.
- **No editor.** A `<textarea>`, with no highlighting, no completion and no inline diagnostics —
  which is odd given [`65`](65-lsp-report.md) built an LSP over this same front end. The seam is
  there; nothing is plugged into it.
- **One log, one program.** Loading a program replaces the running one. No forking a session, no two
  programs side by side, and no way to seed a log from a `given` block.

## 98.8 The gates, and what makes each go red

`crates/beck-cli/tests/playground.rs`, 15 tests:

| What would break it | The test |
|---|---|
| The playground and the compiler start disagreeing about a derived answer | `the_playground_shows_what_the_command_line_shows` (nine sections, three programs, byte for byte) |
| A rejected program is given a placement anyway | `a_program_that_does_not_compile_derives_nothing` |
| A library is answered with errors, or given a deployment | `a_library_is_a_library_rather_than_three_errors` |
| The tab stops being the deployed runtime on any state a log can reach | `the_tab_and_the_server_agree_on_every_state_a_log_can_reach` |
| The tab starts sending a different frame than a subscription would | `the_tab_and_the_server_send_the_same_frames` |
| An idle subscriber starts costing bytes, or a fanout stops reaching the other client | `a_command_moves_every_page_it_changes_and_no_others` |
| The tab starts answering "who is here" with the viewer alone | `presence_in_the_tab_is_who_is_connected_to_the_tab` |
| A retry is refused rather than acknowledged, or appended twice | `a_retried_command_is_acknowledged_and_appended_once` |
| The scrubber becomes a recording rather than a fold | `the_scrubber_renders_the_state_the_log_produces_at_every_position` |
| The page asks a browser for a file the bundle does not carry | `the_bundle_carries_everything_the_page_asks_for` |
| The playground forks the runtime's residue | `the_playground_serves_the_runtimes_own_residue` |
| The `unsafe` exception grows past the three exports, or another crate takes the same liberty | `mode_b.rs::the_wasm_boundary_is_the_only_exception_to_forbid_unsafe`, which now counts both modules |
| The module stops building for the browser | `the_playground_builds_for_the_browser` (skips loudly; `BECK_REQUIRE_WASM=1` forbids the skip) |

And in Chromium, in `crates/beck-cli/tests/browser.rs`:

| What would break it | The test |
|---|---|
| A browser cannot get the compiler's answers out of the module | `the_playground_compiles_in_the_browser` |
| A real diagnostic stops reaching the page | the same test's second half, which asserts an `error[B0…]` with a span |
| The worker, the ports or the iframes stop connecting | `the_playground_runs_the_application_and_two_clients_of_it` |
| One client's command stops reaching the other's page | the same test's two clicks |
| The scrubber stops folding, or starts moving the live clients | the same test's last three assertions |

CI runs both with `BECK_REQUIRE_WASM=1` and `BECK_REQUIRE_BROWSER=1`, in the job that already
existed for Mode B, so neither can skip there.

## 98.9 What this corrects, elsewhere

- [`17`](17-playground.md) §17.1 and §17.2 are **built**; §17.3, §17.4 and §17.5 are not, and §98.7
  says which parts of each.
- [`17`](17-playground.md) §17.6's "Rung B lands with Mode B's WASM kernel in the same phase — the
  worker-server is the rung-0 platform compiled to WASM" is **half right, and the half it got wrong
  is the interesting one**. The worker-server is indeed the rung-0 platform, and the reason it could
  be is not that Mode B's kernel existed — it is that `beck-rt` could be divided (§98.2). Mode B's
  kernel is a *bundle interpreter*; a tab server is a sequencer, a log and a differ, and none of
  those are in `beck-wasm`. The rung rode a division of the runtime, not the kernel work.
- [`08`](08-roadmap.md) Phase 3's playground bullet is built for rungs A and B; the "untouched" list
  loses it and keeps client polish.
- **`beck-rt` gained a crate below it** and lost none of its public paths (§98.2). `beck-host` depends on `beck-core` and nothing else that matters, and the rule
  `beck-rt` has carried since Phase 1 — no dependency on a backend crate — is inherited by it.
- **`Runtime::new_uuid` is gone.** It had no callers: the evaluator mints its own ids
  (`beck_eval::uuid_v7`), and the field on `Runtime` was a second source nothing read. Removing it
  is what let `beck-host` carry no `uuid` dependency, which matters because `uuid`'s
  `wasm32-unknown-unknown` support is `wasm-bindgen`, and [`94`](94-mode-b-report.md) §94.4's "no
  `wasm-bindgen`, no generated glue" is a property worth keeping.
- **`beck-patch.js` has a transport seam** (§98.3). A deployment is unchanged — no `beck.dial`, so a
  websocket — and `browser.rs`'s five Mode A and Mode B tests are what says so.
- **A command's events are all-or-nothing** (§98.2), which the log's contract already required and
  the sequencer did not do.
- **A measurement suite could deadlock on a large artefact**, and one that has been in the tree
  since [`94`](94-mode-b-report.md) did. `compressed()` wrote its input into a compressor's stdin
  and only then read the output — so a compressor that emits while it reads fills a 64 KiB pipe,
  blocks, and blocks the writer. `brotli -q 11` buffers a whole window and never hit it; `gzip -9`
  on this 2.6 MB module hit it every time, which is how a machine without brotli found a bug a
  machine with it could not. The write is on its own thread now, in `measure_play.rs` and in
  `measure_mode_b.rs`.
- `docs/reference/cli.md` gains `beck play`, regenerated rather than written.

## 98.10 What Phase 3 is still not

Unchanged except for this bullet: **the playground exists**, for the two rungs that need no cloud.
What is still missing is three of the supply-chain bullet's four pieces
([`92`](92-sbom-report.md) §92.5), client polish, and the heap both code generators and Mode B's
kernel wait on ([`94`](94-mode-b-report.md) §94.8, [`97`](97-cranelift-report.md) §97.7). Identity's
last row closed while this was being written ([`96`](96-presence-report.md)).

The exit criterion is a claim about a person — an outside developer building a non-trivial app
without asking the authors a question — and no outside developer has read [`86`](86-getting-started.md).
What a playground changes about that is not the criterion but the *first* thing such a person does:
Phase 3's table of the questions they ask in order now has a row above all of them — "can I see it?" —
and the answer is a URL rather than a `git clone` and a toolchain. That is a real change and it is
not the criterion, and this report is not going to claim it is.
