# 98 — The playground

**Built.** [`17`](17-playground.md)'s rungs A and B: the compiler compiled to WebAssembly and
answering in the browser, with source on the left and eleven derived answers on the right; and the
whole *application* in the same tab — a log, a fold, and two client iframes speaking the patch
protocol over a `MessageChannel` — with a `seq` scrubber that folds the log again from genesis. Plus
an editor that highlights, completes and squiggles from the same module `beck lsp` answers from; a
log that survives a reload; a share link that carries the program and names its digest; and Mode B
running in the client iframe.

**The interesting half is never the WebAssembly.** Twice over, the work turned out not to be in the
playground at all:

- §17.2 claims that "by the differential harness's own guarantee, **rung-B behaviour *is* the
  deployed behaviour**". That is only true if the tab runs the deployed code, and `beck-rt` does not
  cross to `wasm32-unknown-unknown` — it holds Postgres, redb, SQLite, TLS and a multi-threaded
  reactor. So the work was deciding **which half of the runtime is about the machine** and moving the
  other half somewhere both hosts can reach it (§98.2).
- An editor's answers were in `beck-cli`, where a browser cannot reach them, so plugging the
  playground into the LSP meant first **making the LSP not be the place those answers live**
  (§98.4).

Both are the same shape, and it is the shape a playground is *for*: a second consumer of the
compiler is the thing that finds where the compiler's answers are only reachable by one caller.

---

## 98.1 Rung A, and the drift it found on the way in

§17.1 lists what a visitor gets with zero servers: "type checking with real diagnostics, macro
expansion … **inferred placement per definition**, generated dataflow/SQL plans, generated Kubernetes
objects, effect signatures, and `beck explain`". All of it is there, as thirteen tabs, and the module
that produces them is 169 lines because every one of them is a single call into the compiler.

**It was not 169 lines when it was written.** Three of `beck explain`'s answers — the placement
table, the wire id, and a type's flow — were **`println!`s in `main.rs`**, so a playground wanting to
show them had two options: call a private function that did not exist, or render the same data a
second time. **The second option is how a language playground ends up disagreeing with its own
compiler about what a program means, quietly, in the direction nobody checks.**

They return `String` now and `main.rs` prints what they return. **The gate is not that they exist**:
`the_playground_shows_what_the_command_line_shows` runs the `beck` binary and the playground over the
same source under the same module name and compares the **bytes**, for nine sections, over three
programs. It goes red the day somebody renders a placement table twice. (The same module name matters
and is worth saying: a tier crossing's id is content-derived from the module, so `beck explain flow
todo.beck` and a playground holding that text legitimately print different digests. Comparing them
anyway would be asserting that two different modules are one.)

Two rules the page follows that are not in §17.1. **A program that does not compile derives
nothing** — diagnostics become the first and only tab, because there is no placement for a program
that has no types, and **a stale table beside a red error teaches a visitor something false.** And **a
library is a library rather than three errors** — a module with no merge point is what a visitor
pasting three definitions into an empty editor has written. It gets its placement and its signatures,
and not the sections that are questions about an application: a plan is a plan *of* a view, and a
NetworkPolicy is derived from what a deployment would do.

## 98.2 The tab is a host, and that took dividing the runtime

§17.2's table is three implementations of the runtime's interfaces, and the third is a browser:

| Runtime interface | Production | DST | **Playground** |
|---|---|---|---|
| Clock | OS | simulated | supplied by the page |
| Network | Tokio/websocket | simulated | `MessageChannel` |
| Log storage | Postgres/redb | simulated | an array in the worker |

The design says a browser is a third *host*. **What it does not say — because it was written before
there was a runtime to look at — is that `beck-rt` had never been divided along that line.** The
sequencer's rules and its `tokio::select!` were in one function; the wire's message types and the
websocket that carries them were in one crate; the bridge to a compiled program sat beside a Postgres
client.

So `crates/beck-host/` is the half that is **program-shaped rather than machine-shaped**: the
compiled program's `validate`, fold, `view` and command decoder; the log's records and the bytes a
store writes; the protocol's message types; and the sequencer's *decisions* — de-duplicate, validate
against the batch's own speculative state, check storable, fold. `beck-rt` re-exports every one of
them at the path it had, so **no other file in the workspace changed**. What is left in `beck-rt` is
precisely what needs the machine: the queue, the durable append, the version watch, the snapshot
timer, the socket, the identity provider, the quota.

**The membership rule is a sentence and it is enforceable**: if it needs the machine, it stays
upstairs. The one place that bit was the clock. The sequencer's decisions read none, because
`Instant::now()` on `wasm32-unknown-unknown` is a panic rather than a number — so the one step that
costs real time takes a meter the host supplies, `beck-rt` passes the histogram it always had, and
the tab passes an untimed one. **F11 asked for a supplied clock three phases ago; this is the same
argument arriving from the other direction, because a target without a clock cannot cheat.**

### The defect the extraction found, and it is in the *server*

Lifting the sequencer's inner loop out made one thing visible that reading it had not. A command that
produces several events folded them **one at a time into the shared speculative state** and pushed
each onto the batch as it went — and if the third one failed, **the proposer was told the command was
refused while the first two stayed in `pending` and were appended.**

That contradicts the log's own contract, three lines into `log.rs`: "a batch of events from one
command is appended **atomically at contiguous `seq`s** — no fold ever observes half a command's
consequences." The decisions now fold into a copy and commit the copy only when every event has
landed, so a refused command adds nothing.

**This is not gated, and the reason is worth stating rather than hiding**: both failure paths are
unreachable for a program that compiled. An event that cannot be stored is what `secure::storable`
proves cannot exist, and a fold that fails mid-command is an evaluator error in code the checker
accepted. The fix is a correctness fix to a path no test can currently reach, and **it is recorded so
that the next person to make one of those paths reachable knows the behaviour was chosen rather than
inherited.**

## 98.3 The client in the iframe *is* the client

§17.2 says the tab holds "the thin patch client in an iframe, speaking the identical patch/command
protocol over a `MessageChannel` instead of a websocket". **The word doing the work is *identical*,
and the way to earn it is not to write a playground client.**

`beck-patch.js` — the residue every Beck application serves — had `new WebSocket(...)` inside
`connect`. It now has a **transport seam**: `connect` dials through `beck.dial` when the page has set
one and through the websocket when it has not, and everything else about the connection is the same
code either way. A second seam, `beck.asset(name)`, does the same for Mode B's kernel and bundle —
`beck-mode-b.js` fetched them from the origin it was served from, and a playground frame has no
origin of its own. A third, `beck.shell`, lets a `srcdoc` frame decline to register a service worker
rather than failing a registration nobody reads.

So a client iframe loads `beck-patch.js`, then a **32-line** shim that sets the seams, then
`beck-thin.js` — unmodified, and unaware it is in a playground.
`the_playground_serves_the_runtimes_own_residue` asserts the **bytes**: the playground's copies *are*
the runtime's constants, and the one file that is the playground's own contains no patch interpreter.
A deployment is unchanged — no `beck.dial`, so a websocket — and the browser suite's five Mode A and
Mode B tests against a deployment are what says so.

The iframe's document is assembled the way `beck run`'s is, too: the page the tab would have
rendered, inside the frame root, with its `seq` on it — first paint is SSR, and the socket that opens
afterwards resumes from the position that render reflects. **It is the same document because it is
the same claim.**

## 98.4 The editor: one module, two consumers, and a keyword table that cannot drift

[`04`](04-compiler-architecture.md) §4.6 has said since Phase 1 that "there is no separate language
server implementation to drift". **That was true of the *compiler* and false of the *editor*:** `beck
lsp` held the name index, the UTF-16 conversions, the word-under-the-caret rule and the message
assembly, and anything else that wanted them had to write them again. **A playground written that way
would have been a highlighter in JavaScript disagreeing with the compiler about where a string
ends.**

So the answers are `beck_core::editor` now, and `beck lsp` translates: highlighting from the lexer
and a keyword table, inline diagnostics from what the checker pushed, hover from the interface
renderer, completion from the checked program's own definition table, and definition from the span
the checker recorded.

**The gate is not that the module exists.**
`the_playground_and_the_language_server_answer_the_same_questions` runs the **`beck lsp` binary**
over stdio and the playground module over its own boundary, on one source, and compares every token —
position, length and category — and every completion. Two encodings of one answer (the protocol's
line/column deltas, the page's flat offsets) are converted to a common shape **here and nowhere
else**, which is the only way two encodings can be compared at all. It goes red the day somebody
colours a keyword in the page's own JavaScript.

Four decisions inside it, each of which could have gone the easy way:

**A keyword is what the parser says it is.** The lexer does not distinguish `def` from any other
identifier — the parser does — so a highlighter needs a list, **and a list is a second place for the
truth to live.** `the_keyword_table_is_the_one_the_parser_matches` reads every `_kw("…")` out of the
parser's own source and asserts the two sets are equal. A keyword the parser gains and the table does
not is a red test rather than a word an editor quietly stops colouring.

**A comment is recovered from the lexer's gaps rather than re-scanned.** Comments are *skipped* by
the lexer — making them tokens would put the layout rule "a comment-only line has no indentation" at
risk — so the token walk reads a `#` in the space between two claimed spans as a comment to the end
of its line. **That keeps one scanner**: a `#` inside a string literal is inside a token, so the gap
scanner never sees it, and there is an assertion that says so.

**Every offset that crosses a boundary is UTF-16.** A `<textarea>` counts its value in UTF-16 code
units and the compiler counts in bytes, and **the two agree until somebody writes an emoji in a
string** — at which point a squiggle lands on the wrong word and nothing says why. The conversion
lives where the text is, and neither the page nor the test harness implements it a second time.

**A half-typed name still completes.** There is no program when the text has an error, and **the most
common state of a file being typed into is exactly that** — so the name table is empty precisely when
somebody asks for a name. The editor borrows the previous analysis's index and marks the result
`stale`; the diagnostics stay this text's. That is not §98.1's rule being bent: a stale *derived
answer* beside a red error teaches something false about the program, and a stale completion list is
a list of names that were there a keystroke ago, **with the consumer told**.

**What this changed about `beck lsp`, which was not the point.** The server gained
`completionProvider` and `semanticTokensProvider`, with the legend published from the same enum so an
editor and the playground colour the same categories. And **a file that imports now checks**: the
editor goes through the project checker with a loader that serves the text as the root module and the
standard library as everything else, so `import bignum` resolves in an editor and in the tab. Before
this, `beck lsp` reported `cannot find add_big in this scope` for every name in such a file. An
imported name is indexed, described and offered; it has **no span**, because its declaration is in a
module this document is not showing, and definition **declines rather than pointing at a
plausible-looking byte range.** There is still no directory — a language server resolving a relative
path off a URI is a decision nobody has taken — so a file importing a module *beside it on disk*
still does not check in the editor.

**The page** is a `<textarea>` over a `<pre>`, sharing one set of metrics, with the textarea's text
transparent and the caret coloured. The textarea is what knows about undo, selection, IME and screen
readers, **and none of that is worth reimplementing to get colour.** The paint layer is one sweep
over the source cut at every boundary a token *or* a diagnostic introduces, so a squiggle that starts
inside a string is still one span and the two never fight over a character. Highlighting is **not**
debounced and the analysis still is — one costs a lex and needs no program, the other costs a check —
and §98.7 is the measurement that says that distinction is real rather than tidy.

## 98.5 The three demos §17.2 says nobody else can build

**Multiplayer in one tab.** Two iframes, two subscriptions, one log. ana clicks; the command goes up
her port to the worker, through the one merge point, into the array that is the log; every
subscription is advanced; and **the frame that reaches bo is what makes this a fanout rather than a
mirror.** The gate is in Chromium and asserts exactly that crossing. An idle subscriber whose page
did not change is sent **nothing**, which is the property the whole subscription design exists for
and is gated separately on `todo.beck`, whose view filters by the session: ana adds a todo, and bo —
who cannot see it and is not waiting on anything — is owed no frame at all.

**And who else is here.** Presence is the one input to a view that moves without an event, so it is
also the one place a tab could quietly answer a different question than a server. The runtime's
`view` renders against the viewer's *own* roster, which is right for `beck test` and wrong for an
application; the tab therefore keeps a roster built from the thing it already knows — its own
subscriptions — exactly as the server keeps a registry. **The bound the server's roster needs is not
here and does not need to be**: a server's roster is keyed by a name the client chose, and a tab has
as many connections as the page opened.

**Time travel.** The scrubber asks the tab for the page at a position, and the tab computes it by
folding the log **from genesis** with the program's own `apply_event`. **Not a recording and not an
undo stack**: D3's genesis-replay discipline, as something a visitor drags. The gate holds it to an
oracle that is the *other host* — the same commands through a real `App`, with its page captured
after each one — and the browser test drags the scrubber and then checks that the live clients did
not move, **because a scrubber that moved the application would be an undo.**

## 98.6 The log, the link, and Mode B

**A log a reload survives.** The tab's log is still a `Vec`; what changed is that it can be handed
over as the same `Envelope` bytes redb, SQLite and Postgres write. **A tab keeping its history in a
browser store is therefore keeping *records*, not a rendering of them.** The key is the **wire id**,
and that is the whole design: two sources whose event types agree share one and can legitimately read
each other's history; a change to those types is a new id and a new log. **That is §4.3's rule rather
than one this page invented** — editing a comment keeps your history, changing an event's shape
starts a fresh one, and neither needs a migration the playground does not have.

Two rules `restore` enforces rather than assumes: **only into a tab that has not run**, because a
restore after a subscription has rendered would be rewriting history under a client that had already
seen it; and **dense `seq`s from 1**, the contract every fold in this repository depends on, because
a store that dropped a record would otherwise produce a state no history could have reached,
silently. The oracle for the round trip is the tab that produced the records, compared at *every*
position. In a browser the gate clicks twice, reloads, asserts the application comes back **at 2**,
and then clears it — **because a log that cannot be forgotten is a playground that cannot be started
over.** Forgetting stops the session keeping anything more rather than emptying the store and
carrying on: a store that resumed at the next command would write a log beginning at seq 3, which the
restore rule refuses. *That test failed one run in three under parallel load and passed every time on
its own, which is how three defects in the page's store were found at once.*

**A share link is the program, and it names itself.** §17.4 says "a share link is a digest; forks are
new digests", and that sentence describes a link *resolved* through a CDN — which needs the registry
Phase 3 does not have. So the link carries the program and names its digest:

```text
https://play.beck.dev/#p=b3a71c2e5f9d04a8.eJxLy…
                         └ the first 16 hex digits of BLAKE3 over the source
                                           └ the source, DEFLATE'd and base64url'd
```

Three properties, and they are the ones §17.4 wanted. It is **content-addressed** — the digest is the
same BLAKE3 a Beck program's own `digest()` computes, so one program is one link wherever it was
written and a fork is a new link because a fork is different bytes. It is **self-certifying** —
unpacking recomputes the digest and refuses a mismatch with a constant-time comparison, so **a link
truncated by a chat client is an error rather than a *different program* opening under a name
somebody trusted.** And **nothing is sent anywhere**: it is a fragment, which is the one part of a
URL a browser does not put in the request. What it is not is short, or private: a fragment is not
sent to a server and is still in whatever it was pasted into. Decompression is bounded at 1 MiB,
because a fragment is attacker-controlled input.

**Mode B in the tab** took one seam and one branch. The seam is §98.3's `beck.asset`. The branch is
that a subscription carries DOM patches or data patches, and which one is the program's rendering
mode — one branch, in the server's session and now in the tab. `the_tab_and_the_server_send_the_same_
data_frames` is the differential: a real subscription over the socket harness against a tab,
comparing the encoded accumulator and then every delta. **It is the half of §17.2's guarantee that
could not be made while the tab served one mode.** Two things fell out that were not the point: the
kernel becomes the playground's second artefact, on the same reserved name a deployment serves it
under; and **a route is part of the session in the tab**, because Mode B sends a navigation frame so
that the `Session` the server hands `validate` is the one the client's own `validate` saw — a tab
that ignored it would run the program against a session no deployment builds.

## 98.7 What it costs

`cargo test --release --test measure_play -- --nocapture`. `brotli` is not installed on this machine,
so every compressed column is gzip and is a **ceiling** on what a CDN would send; `wasm-opt -Oz` has
not been run either.

| | bytes | compressed |
|---|---:|---:|
| The page — nine files | 75,636 | 24,411 |
| `beck-play.wasm` — the compiler and the tab server | 3,053,345 | 985,704 |
| `beck-kernel.wasm` — fetched only by a Mode B iframe | 791,863 | 270,447 |

The module is large and honestly so: it is the whole front end — parser, macro expander, inference,
placement, the splitter, the plan, the read-model derivation and the Kubernetes emitter — plus the
evaluator. **What it buys is that there is no server in the answer**, which is the entire point of
the rung.

**The editor**, which is the cost a person pays per keystroke:

| program | lines | tokens | highlight µs | completion µs |
|---|---:|---:|---:|---:|
| `counter.beck` | 103 | 485 | 59 | 1,160 |
| `25-thread.beck` | 171 | 1,115 | 154 | 2,938 |
| `todo.beck` | 191 | 1,191 | 175 | 2,404 |

**Twenty times apart, and that is the design.** Highlighting is a lex and needs no program, so it runs
undebounced; completion is a check. Neither grows faster than the source does. Against a 250 ms
debounce and a person's typing, that is two to three orders of magnitude of headroom.

**An interaction, and whether history makes it worse** — because the question a person asks after ten
minutes of clicking is whether the tab slows down:

| events in the log | one command (µs) | scrub to head (µs) | per event (ns) |
|---:|---:|---:|---:|
| 100 | 26 | 140 | 1,168 |
| 1,000 | 26 | 955 | 937 |

**Two shapes, and they are the shapes those two operations *are*.** A command is a fold and a render
of the **state**, so it does not grow with the log; a scrub is a fold **of the log**, so it grows with
the history, linearly. A thousand events is under a millisecond and a million would be a second; the
fix, if it is ever needed, is the snapshots the durable substrates already have and a tab does not.

**The log a page keeps**, and **a share link**:

| events | record bytes | per event | restore µs | of which preparing the program |
|---:|---:|---:|---:|---:|
| 100 | 2,700 | 27.0 | 340 | 145 |
| 1,000 | 27,873 | 27.9 | 1,586 | 161 |

| program | source | link characters | ratio |
|---|---:|---:|---:|
| `counter.beck` | 3,155 | 1,731 | 0.55× |
| `todo.beck` | 6,789 | 3,368 | 0.50× |

A record is 27 bytes because it is `postcard` over an `Envelope` rather than JSON, and it is the same
encoding redb and Postgres hold. The "preparing the program" column is there because without it the
restore rows look sublinear in a way a fold cannot be: the fold itself is 195 µs and 1,425 µs. And a
link is half the source and proportional to it — **which is the honest shape of a link that carries a
program instead of naming one.** `todo.beck` is a 3.4 KB URL: fine in a browser, awkward in a chat
client that truncates, and exactly the thing a resolver would fix.

Measured natively rather than in WebAssembly, exactly as the Mode B kernel's numbers were: the crate
is an `rlib` as well as a `cdylib`. **The ratios and the shapes carry across; the absolute
microseconds do not.**

## 98.8 The gates

`playground.rs`, and a set in Chromium. What would break each:

| | |
|---|---|
| The playground and the compiler disagree about a derived answer | `the_playground_shows_what_the_command_line_shows` — nine sections, three programs, byte for byte |
| The page and the language server answer differently | `the_playground_and_the_language_server_answer_the_same_questions` |
| A keyword the parser learns stops being coloured | `the_keyword_table_is_the_one_the_parser_matches` |
| A rejected program is given a placement anyway; a library is answered with errors | `a_program_that_does_not_compile_derives_nothing`, `a_library_is_a_library_rather_than_three_errors` |
| The tab stops being the deployed runtime, or sends a different frame | `the_tab_and_the_server_agree_on_every_state_a_log_can_reach`, `…_send_the_same_frames`, `…_send_the_same_data_frames` |
| An idle subscriber costs bytes, or a fanout stops reaching the other client | `a_command_moves_every_page_it_changes_and_no_others` |
| The tab answers "who is here" with the viewer alone | `presence_in_the_tab_is_who_is_connected_to_the_tab` |
| A retry is refused rather than acknowledged, or appended twice | `a_retried_command_is_acknowledged_and_appended_once` |
| The scrubber becomes a recording rather than a fold | `the_scrubber_renders_the_state_the_log_produces_at_every_position` |
| A kept log stops being the log the tab had; a restore rewrites history or folds a gapped log | `a_log_kept_by_the_page_is_the_log_the_tab_had`, `a_restore_into_a_running_tab_or_of_a_gapped_log_is_refused` |
| A share link opens a program other than the one it names | `a_share_link_carries_the_program_and_is_named_by_its_digest`, and four more |
| A client is handed a bundle the tab is not running | `the_bundle_the_tab_hands_over_is_the_program_it_is_running` |
| The playground forks the runtime's residue, or asks for a file nothing writes | `the_playground_serves_the_runtimes_own_residue`, `the_bundle_carries_everything_the_page_asks_for` |
| The `unsafe` exception grows past the three exports | `the_wasm_boundary_is_the_only_exception_to_forbid_unsafe`, counting both modules |
| **In Chromium** — a browser cannot get the compiler's answers out of the module; colours, squiggles or completion stop reaching a person typing; the worker, ports or iframes stop connecting; one client's command stops reaching the other's page; a reload starts from `init`; a share link opens the wrong program; a Mode B program stops running in the tab | seven tests in `browser.rs` |

CI runs both with `BECK_REQUIRE_WASM=1` and `BECK_REQUIRE_BROWSER=1`, in the job that already existed
for the client, so neither can skip there.

## 98.9 What is not built

- **Rung C.** Untouched, and Phase 4's: an ephemeral cluster per session, with the compiler as the
  first sandbox. Nothing here compiles against a restricted effect budget — a playground program can
  name `net.out` and the page will show the effect row and the NetworkPolicy it implies, **because it
  never runs anything that has one.**
- **The playground is not a Beck app.** §17.5 and D15 say it should be, and it is a page of JavaScript
  with two Rust modules under it. What would make it one is the registry and the site tier, and **the
  honest position is that this rung proves the *language* runs in a tab, not that this particular tab
  was written in it.**
- **A resolvable share link.** §17.4's digest-that-resolves, its embeds and its docs-as-demos all need
  something to resolve *against*, and that is the registry.
- **One log, one program.** Loading a program replaces the running one. No forking a session, no two
  programs side by side, no way to seed a log from a `given` block. The stored log is per wire id, so
  two programs' logs coexist in the browser — but only one runs at a time.
- **No snapshots and no compaction**, so a restore is a fold of the whole log; and the page stores the
  whole log on every command rather than appending one record — tens of events is what a playground
  session is, and a cursor and a row per event would be a store's worth of machinery for a saving
  nobody in a tab can measure. A browser's storage quota is the only bound, and a page that will not
  store says so once and keeps working.
- **No incremental analysis.** Every keystroke past the debounce re-checks the whole file.
- **The editor is a `<textarea>` still**, deliberately: what it gained is highlighting, completion and
  squiggles. No rename, no signature help, no folding, no multi-file anything. And `beck lsp` still
  has no directory, so a module importing a file beside it on disk does not check in an editor.

### What this corrects, elsewhere

- [`17`](17-playground.md) §17.1, §17.2 and §17.4 are **built**; §17.3 and §17.5 are not.
- **§17.6 is half right, and the half it got wrong is the interesting one.** It says "rung B lands
  with Mode B's WASM kernel in the same phase — the worker-server is the rung-0 platform compiled to
  WASM". The worker-server *is* the rung-0 platform, and the reason it could be is **not** that Mode
  B's kernel existed: Mode B's kernel is a bundle interpreter, and a tab server is a sequencer, a log
  and a differ, none of which are in it. **The rung rode a division of the runtime, not the kernel
  work.**
- **`beck-rt` gained a crate below it and lost none of its public paths.** `beck-host` inherits the
  rule `beck-rt` has carried since Phase 1: no dependency on a backend crate.
- **`Runtime::new_uuid` is gone.** It had no callers — the evaluator mints its own ids — and removing
  it is what let `beck-host` carry no `uuid` dependency, which matters because that crate's
  `wasm32-unknown-unknown` support is `wasm-bindgen`, and [`94`](94-the-client-report.md)'s "no
  `wasm-bindgen`, no generated glue" is a property worth keeping.
- **A command's events are all-or-nothing** (§98.2), which the log's contract already required and the
  sequencer did not do.
- **`beck lsp` is now a translator**: the answers it gives are `beck_core::editor`'s. It gained two
  capabilities and the ability to analyse a file that imports the standard library. Its "no Salsa,
  whole file every time" position is unchanged.
- **A measurement suite could deadlock on a large artefact**, and one had since the client work.
  Compressing wrote its input into a compressor's stdin and only then read the output — so a
  compressor that emits while it reads fills a 64 KiB pipe, blocks, and blocks the writer. `brotli
  -q 11` buffers a whole window and never hit it; `gzip -9` on a 2.6 MB module hit it every time,
  **which is how a machine without brotli found a bug a machine with it could not.**
