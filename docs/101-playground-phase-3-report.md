# 101 — Phase 3 report, part 69: the four things the playground would not do

**Built.** [`98`](98-playground-report.md) §98.7 listed what the playground was not, and four of its
items were one sentence each: no IndexedDB, so a reload started from `init`; no content-addressed
sharing, so an edit was not addressable; no Mode B in the tab, so a `@render(client)` program was
*named* rather than run; and "a `<textarea>`, with no highlighting, no completion and no inline
diagnostics — which is odd given [`65`](65-lsp-report.md) built an LSP over this same front end. The
seam is there; nothing is plugged into it."

All four are now built. Three of them are small; the fourth is the interesting one, and it is
interesting for the reason §98.2 was: the work was not in the playground. An editor's answers were
in `beck-cli`, where a browser cannot reach them, so plugging the playground into the LSP meant
first making the LSP not be the place those answers live. That is §101.1, and it is the only
structural change here.

## 101.1 The editor: one module, two consumers, and a keyword table that cannot drift

[`04`](04-compiler-architecture.md) §4.6 has said since Phase 1 that "there is no separate language
server implementation to drift". That was true of the *compiler* and false of the *editor*: `beck
lsp` held the name index, the UTF-16 conversions, the word-under-the-caret rule and the message
assembly, and anything else that wanted them had to write them again. A playground written that way
would have been a highlighter in JavaScript disagreeing with the compiler about where a string ends.

So the answers are [`beck_core::editor`] now, and `beck lsp` translates:

| Answer | What produces it | Who asks |
|---|---|---|
| Highlighting | `beck_syntax::lexer::lex` and `KEYWORDS` | `semanticTokens/full`, and the page's paint layer |
| Inline diagnostics | the diagnostics the checker pushed | `publishDiagnostics`, and the page's squiggles |
| Hover | `beck_core::iface::render_item` | `textDocument/hover`, and the page's completion detail |
| Completion | the checked program's own definition table | `textDocument/completion`, and the page's popup |
| Definition | the span the checker recorded | `textDocument/definition` |

The gate is not that the module exists:

```text
playground.rs::the_playground_and_the_language_server_answer_the_same_questions
```

which runs the **`beck lsp` binary** over stdio and the playground module over its own boundary, on
one source, and compares every token — position, length and category — and every completion. Two
encodings of one answer (the protocol's line/column deltas, the page's flat offsets) are converted
to a common shape here and nowhere else, which is the only way two encodings can be compared at all.
It goes red the day somebody colours a keyword in the page's own JavaScript.

Four decisions inside it are worth stating because each could have gone the easy way.

**A keyword is what the parser says it is.** The lexer does not distinguish `def` from any other
identifier — the parser does, in `at_kw`/`eat_kw` — so a highlighter needs a list, and a list is a
second place for the truth to live. `beck_syntax::lexer::KEYWORDS` is that list and
`the_keyword_table_is_the_one_the_parser_matches` reads every `_kw("…")` out of the parser's own
source and asserts the two sets are equal. A keyword the parser gains and the table does not is a
red test rather than a word an editor quietly stops colouring.

**A comment is recovered from the lexer's gaps rather than re-scanned.** Comments are *skipped* by
the lexer — making them tokens would put the layout rule "a comment-only line has no indentation" at
risk — so `tokens()` walks the spans the lexer did claim and reads a `#` in the space between two of
them as a comment to the end of its line. That keeps one scanner: a `#` inside a string literal is
inside a token, so the gap scanner never sees it, and `a_hash_inside_a_string_is_not_a_comment` is
the assertion that says so.

**Every offset that crosses a boundary is UTF-16.** A `<textarea>` counts its value in UTF-16 code
units and the compiler counts in bytes, and the two agree until somebody writes an emoji in a string
— at which point a squiggle lands on the wrong word and nothing says why.
`beck_core::editor::utf16_offset` and `byte_of_utf16` are the conversion, they live where the text
is, and the playground's boundary is where they are applied. Neither the page nor the test harness
implements it a second time.

**A half-typed name still completes.** `compile_or_library_str` returns no program when the text has
an error, and the most common state of a file being typed into is exactly that — so the name table
is empty precisely when somebody asks for a name. `Editor::completing_from` borrows the previous
analysis's [`Index`] and marks the result `stale`; the diagnostics stay this text's. That is not
[`98`](98-playground-report.md) §98.1's rule being bent: a stale *derived answer* beside a red error
teaches something false about the program, and a stale completion list is a list of names that were
there a keystroke ago, with the consumer told.

### What this changed about `beck lsp`, which was not the point

Two things, both improvements, both worth stating rather than discovering:

* **The server gained `completionProvider` and `semanticTokensProvider`** — the same answers the
  page shows, over the protocol, with the legend published from `TokenKind::legend()` so an editor
  and the playground colour the same categories. `lsp.rs::it_offers_completions_and_semantic_tokens`
  holds them to the protocol's own encodings.
* **A file that imports now checks.** `Editor::of` goes through `beck_core::project::check_project`
  with a loader that serves the text as the root module and the standard library as everything else,
  so `import bignum` resolves in an editor and in the tab. Before this, `beck lsp` reported
  `cannot find add_big in this scope` for every name in such a file. An imported name is indexed,
  described and offered; it has **no span**, because its declaration is in a module this document is
  not showing, and `definition` declines rather than pointing at a plausible-looking byte range.

There is still no directory: a language server resolving a relative path off a URI is a decision
[`65`](65-lsp-report.md) did not take and this does not take either. A file that imports a module
*beside it on disk* still does not check in the editor.

### The page

A `<textarea>` over a `<pre>`, sharing one set of metrics, with the textarea's text transparent and
the caret coloured. The textarea is what knows about undo, selection, IME and screen readers, and
none of that is worth reimplementing to get colour. The paint layer is one sweep over the source cut
at every boundary a token *or* a diagnostic introduces, so a squiggle that starts inside a string is
still one span and the two never fight over a character.

Highlighting is **not** debounced and the analysis still is: one costs a lex and needs no program,
the other costs a check. §101.5 is the measurement that says that distinction is real rather than
tidy.

## 101.2 The log a reload survives

§17.2's storage row says IndexedDB. The tab's log is still a `Vec` — what changed is that it can be
handed over: `Tab::records(after)` returns `beck_host::Envelope::encode` bytes, which is exactly what
redb, SQLite and Postgres write, and `Tab::restore` reads them back and folds them. A tab keeping its
history in IndexedDB is therefore keeping **records**, not a rendering of them.

**The key is the wire id**, and that is the whole design. Two sources whose event types agree share a
wire id and can legitimately read each other's history; a change to those types is a new id and a new
log. That is §4.3's rule rather than one this page invented — editing a comment keeps your history,
changing an event's shape starts a fresh one, and neither needs a migration the playground does not
have.

Two rules `restore` enforces rather than assumes:

* **Only into a tab that has not run.** A restore after a subscription has rendered would be
  rewriting history under a client that had already seen it.
* **Dense `seq`s from 1.** The contract every fold in this repository depends on. A store that
  dropped a record would otherwise produce a state no history could have reached, silently.

The oracle for the round trip is the tab that produced the records:
`a_log_kept_by_the_page_is_the_log_the_tab_had` compares the page at *every* position, which is the
same comparison the scrubber gate makes and for the same reason. In a browser,
`the_playground_keeps_its_log_across_a_reload` clicks twice, reloads, and asserts the application
comes back **at 2** — and then clears it with the button, because a log that cannot be forgotten is a
playground that cannot be started over.

What is not here: no snapshots and no compaction, so a restore is a fold of the whole log (§101.5
measures it), and the page stores the whole log on every command rather than appending one record —
tens of events is what a playground session is, and a cursor and a row per event would be a store's
worth of machinery for a saving nobody in a tab can measure. A browser's storage quota is the only
bound. And a page that will not store — a private window, a disabled store — says so once and keeps
working, which is the posture [`94`](94-mode-b-report.md) §94.13 took for the same problem.

## 101.3 A share link is the program, and it names itself

§17.4 says "a share link is a digest; forks are new digests", and that sentence describes a link
*resolved* through a CDN. Resolving one needs something to resolve against — the registry
[`16`](16-packages-and-ecosystem.md) describes and Phase 3 does not have. So the link carries the
program and names its digest:

```text
https://play.beck.dev/#p=b3a71c2e5f9d04a8.eJxLy…
                         └ the first 16 hex digits of BLAKE3 over the source
                                           └ the source, DEFLATE'd and base64url'd
```

Three properties, and they are the ones §17.4 wanted. It is **content-addressed**: the digest is
`beck_core::digest::of`, the same BLAKE3 a Beck program's own `digest()` computes, so one program is
one link wherever it was written and a fork is a new link because a fork is different bytes. It is
**self-certifying**: `unpack` recomputes the digest and refuses a mismatch with a constant-time
comparison, so a link truncated by a chat client is an error rather than a *different program*
opening under a name somebody trusted. And **nothing is sent anywhere**: it is a fragment, which is
the one part of a URL a browser does not put in the request.

What it is not is short — a link is proportional to the program (§101.5). §17.4's embeds, and its
"a bug report arrives as a digest", want the resolver; this is the half that works with no server.
Nor is it private: a fragment is not sent to a server and is still in whatever it was pasted into.

DEFLATE rather than raw base64 because base64 of source is 4/3 of the source and a program is mostly
repetition of a small vocabulary. `flate2` is already this workspace's compressor
([`adr/0025`](adr/0025-deflate-so-the-image-build-needs-no-tools.md)) with a `miniz_oxide` backend that
is Rust rather than a vendored zlib, which is the only reason a compressor crosses to
`wasm32-unknown-unknown` without a build-tooling decision. Decompression is bounded at 1 MiB: a
fragment is attacker-controlled input.

## 101.4 Mode B in the tab, which took one seam and one branch

§98.7 called this "a second module in a second frame, and a piece of work rather than a flag". It was
both of those, and the work divided into exactly two pieces.

**The seam.** `beck-mode-b.js` fetched `/beck-kernel.wasm` and `/beck-bundle.bpk` from the origin it
was served from. A playground frame has no origin of its own and no server behind it, so
`beck-patch.js` grew `beck.asset(name)` beside `beck.dial` — the same shape, for the same reason:
`asset` returns a promise of a `Response`, a deployment's default is `fetch("/" + name)`, and the
playground's shim answers the bundle from memory and the kernel from the directory the page was
deployed to. `beck.shell` joins it: a `srcdoc` frame may not register a service worker and now
declines rather than failing a registration nobody reads. The residue is otherwise unmodified, and
`the_playground_serves_the_runtimes_own_residue` asserts the bytes.

**The branch.** A subscription carries DOM patches or data patches, and which one is the program's
rendering mode — one branch, in `beck_rt::session` and now in the tab. Mode A diffs two *pages* and
sends `beck_core::diff` ops; Mode B diffs two *states* and sends `beck_core::delta` ops, and the
rendering happens in the iframe, in the kernel, from the bundle the tab derived from the program it
is itself running. `the_tab_and_the_server_send_the_same_data_frames` is the differential — a real
subscription over the socket harness against a `Tab`, comparing the encoded accumulator and then
every delta. It is the half of §17.2's guarantee that could not be made while the tab served one
mode.

Two things fell out that were not the point:

* **`beck play --out` writes two modules.** The kernel is the playground's second artefact, on the
  same reserved name a deployment serves it under.
* **A route is part of the session in the tab.** Mode B sends `{"t":"g"}` when it navigates locally,
  so that the `Session` the server hands `validate` is the one the client's own `validate` saw — and
  a tab that ignored it would run the program against a session no deployment builds. Handling it
  meant giving a subscription a path, which in Mode A makes a link in a playground iframe behave the
  way a link in a deployment does. [`100`](100-client-polish-report.md) made the route a field of the
  session; this is the tab catching up with it.

## 101.5 What it costs

`cargo test --release --test measure_play -- --nocapture`, on the container this was written in.

**The download.** The page is nine files now rather than eight — `beck-mode-b.js` joined it — and
there is a second module, which a visitor pays for only if they run a `@render(client)` program:

| | bytes | compressed |
|---|---:|---:|
| The page — nine files | 75,636 | 24,409 (gzip) |
| `beck-play.wasm` — the compiler and the tab server | 3,053,243 | 985,632 (gzip) |
| `beck-kernel.wasm` — Mode B's kernel, fetched by a Mode B iframe | 791,863 | — |

The page grew from 30,243 bytes to 75,636 ([`98`](98-playground-report.md) §98.6 is the before): the
editor, the store, the link and Mode B's residue. `brotli` is not installed on this machine, so the
compressed column is gzip and is a **ceiling** on what a CDN would send; `wasm-opt -Oz` has not been
run, for the same reason [`94`](94-mode-b-report.md) §94.6 gives.

**The editor**, which is the cost a person pays per keystroke:

| program | lines | tokens | highlight µs | completion µs |
|---|---:|---:|---:|---:|
| `counter.beck` | 103 | 485 | 59 | 1,160 |
| `25-thread.beck` | 171 | 1,115 | 154 | 2,938 |
| `todo.beck` | 191 | 1,191 | 175 | 2,404 |

**Twenty times apart, and that is the design.** Highlighting is a lex and needs no program, so it
runs on every keystroke undebounced; completion is a check — the same one the analysis pays for —
and is asked for. Neither grows faster than the source does: 2.3× the tokens costs 2.6× the lex.
Against a 250 ms debounce and a person's typing, a millisecond is two to three orders of magnitude of
headroom — natively; a browser will be slower by a factor these numbers do not establish.

**The log a page keeps:**

| events | record bytes | bytes per event | restore µs | of which preparing the program |
|---:|---:|---:|---:|---:|
| 100 | 2,700 | 27.0 | 340 | 145 |
| 1,000 | 27,873 | 27.9 | 1,586 | 161 |

A record is 27 bytes for this program because it is `postcard` over an `Envelope` rather than JSON,
and it is the same encoding redb and Postgres hold. Per-event cost is flat. A restore is preparing
the program once and then folding the whole log, and the second column is there because without it
the two rows look sublinear in a way a fold cannot be: the fold itself is 195 µs and 1,425 µs, which
is the linear growth a scrub has and for the same reason. A thousand-event session is 28 KB in
IndexedDB and under two milliseconds to come back.

**A share link:**

| program | source | link characters | ratio |
|---|---:|---:|---:|
| `counter.beck` | 3,155 | 1,731 | 0.55× |
| `board.beck` | 5,732 | 2,812 | 0.49× |
| `todo.beck` | 6,789 | 3,368 | 0.50× |

Half the source, and proportional to it — which is the honest shape of a link that carries a program
instead of naming one. `todo.beck` is a 3.4 KB URL: fine in a browser, awkward in a chat client that
truncates, and exactly the thing a resolver would fix.

## 101.6 What is still not built

- **Rung C.** Untouched, and Phase 4's ([`08`](08-roadmap.md) §8.7). Nothing here compiles against a
  restricted effect budget.
- **The playground is still not a Beck app.** §17.5 and D15 say it should be; it is a page of
  JavaScript with two Rust modules under it. What would make it one is the registry and the site
  tier.
- **A resolvable share link.** §17.4's digest-that-resolves, its embeds and its docs-as-demos all
  need something to resolve *against*, and that is the registry (§101.3).
- **One log, one program.** Loading a program replaces the running one. No forking a session, no two
  programs side by side, no way to seed a log from a `given` block. The stored log is per wire id, so
  two programs' logs coexist in the browser — but only one runs at a time.
- **No incremental analysis.** Every keystroke that gets past the debounce re-checks the whole file
  ([`64`](64-compile-speed-report.md) §64.6 is why that is defensible and §65.4 is where it stops
  being).
- **The editor is a `<textarea>` still**, and deliberately: what it gained is highlighting,
  completion and squiggles. No rename, no signature help, no folding, no multi-file anything.
- **`beck lsp` still has no directory** (§101.1), so a module importing a file beside it on disk does
  not check in an editor.

## 101.7 The gates, and what makes each go red

| What would break it | The test |
|---|---|
| The page and the language server start answering differently | `playground.rs::the_playground_and_the_language_server_answer_the_same_questions` |
| A keyword the parser learns stops being coloured | `beck-syntax::lexer::the_keyword_table_is_the_one_the_parser_matches` |
| Highlighting or completion stops working on a file being typed into | `playground.rs::the_editor_answers_while_the_file_is_half_written` |
| A `#` inside a string starts a comment; a span stops covering its own bytes | `beck-core::editor::{a_hash_inside_a_string_is_not_a_comment, every_token_is_ordered_and_covers_its_own_bytes}` |
| The protocol's completion or token encodings drift | `lsp.rs::it_offers_completions_and_semantic_tokens`, `lsp.rs::semantic_tokens_are_deltas_from_the_previous_token` |
| A kept log stops being the log the tab had | `playground.rs::a_log_kept_by_the_page_is_the_log_the_tab_had` |
| A restore rewrites history under a running client, or folds a log with a hole in it | `playground.rs::a_restore_into_a_running_tab_or_of_a_gapped_log_is_refused` |
| A share link opens a program other than the one it names | `playground.rs::a_share_link_carries_the_program_and_is_named_by_its_digest`, `share.rs`'s four |
| A Mode B subscription starts sending DOM patches | `playground.rs::a_mode_b_subscription_carries_the_state_rather_than_the_page` |
| The tab and the server disagree about a data frame | `playground.rs::the_tab_and_the_server_send_the_same_data_frames` |
| A client is handed a bundle the tab is not running | `playground.rs::the_bundle_the_tab_hands_over_is_the_program_it_is_running` |
| A `g` frame stops moving the session | `playground.rs::a_client_that_navigates_is_a_client_somewhere_else` |
| The page asks a browser for a file nothing writes | `playground.rs::the_bundle_carries_everything_the_page_asks_for` |

And in Chromium, in `browser.rs`:

| What would break it | The test |
|---|---|
| Colours, squiggles or completion stop reaching a person typing | `the_playground_highlights_and_completes_in_the_browser` |
| A reload starts from `init` again | `the_playground_keeps_its_log_across_a_reload` |
| A share link opens the wrong program, or travels as a query | `a_share_link_opens_the_program_it_carries` |
| A `@render(client)` program stops running in the tab, or its fanout stops reaching the other frame | `the_playground_runs_a_mode_b_program_in_the_tab` |

CI runs both with `BECK_REQUIRE_WASM=1` and `BECK_REQUIRE_BROWSER=1`, in the job that already existed
for Mode B, so neither can skip there.

## 101.8 What this corrects, elsewhere

- [`98`](98-playground-report.md) §98.7 loses four of its seven items: IndexedDB, sharing, Mode B in
  the tab, and the editor. The other three — rung C, the playground not being a Beck app, and one
  log per tab — stand, and §101.6 says so.
- [`17`](17-playground.md) §17.2's storage row and §17.4 are **built**, with §101.2 and §101.3 saying
  what each does and does not deliver; §17.3 and §17.5 are still not.
- [`65`](65-lsp-report.md)'s server is now a **translator**: the answers it gives are
  `beck_core::editor`'s (§101.1). It gained two capabilities and the ability to analyse a file that
  imports the standard library, neither of which that report contemplated. Its "no Salsa, whole file
  every time" position is unchanged.
- [`94`](94-mode-b-report.md)'s kernel now has a second host. Nothing in `beck-wasm` changed;
  `beck-mode-b.js` gained the `beck.asset` seam and a `beck.shell` flag, and `browser.rs`'s five Mode
  A and Mode B tests against a deployment are what says a deployment is unaffected.
- [`100`](100-client-polish-report.md)'s route reaches the tab (§101.4).
- **`beck play --out <dir>` now needs the kernel built**, not only the playground module. The failure
  names the command, as the playground module's already did.
- `beck-play` takes one dependency it did not have — `flate2`, already in this workspace for the
  image build (§101.3).

## 101.9 What Phase 3 is still not

Unchanged except that the playground is now a place a person can be sent: a link opens the program,
the editor colours it and completes it, the log survives the reload, and both rendering modes run.
What is still missing is three of the supply-chain bullet's four pieces ([`92`](92-sbom-report.md)
§92.5) and the heap both code generators and Mode B's kernel wait on ([`94`](94-mode-b-report.md)
§94.8, [`97`](97-cranelift-report.md) §97.7).

The exit criterion is still a claim about a person — an outside developer building a non-trivial app
without asking the authors a question — and no outside developer has read
[`86`](86-getting-started.md). What this changes is the cost of the first ten minutes, not the
criterion, and this report is not going to claim otherwise.

[`beck_core::editor`]: ../compiler/crates/beck-core/src/editor.rs
[`Index`]: ../compiler/crates/beck-core/src/editor.rs
