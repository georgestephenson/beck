# 102 — Phase 3 report, part 70: the page that says it is guessing, and a budget that had never been weighed

**Built.** [`08`](08-roadmap.md) Phase 3's Mode B bullet names four things. [`94`](94-mode-b-report.md)
built the kernel and the reconciliation and closed with a list; this is the two the list still had,
and after it what the bullet owes is codegen alone — the item
[`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) defers on purpose:

> **Mode B client**: per-component WASM (view + fold + signal kernel), optimistic application with
> `seq` reconciliation, **freshness-typed pending state**; **size budget CI gate** (< 150 KB brotli
> per component bundle).

The first is a language feature and the second is a shell script, and they are in one report because
they are the same question asked twice: *what is a claim worth when nothing can observe it?* A
browser that guesses without saying so is a page that lies for a few hundred milliseconds. A budget
in a design document that no job weighs is a number somebody wrote down.

The interesting half is again a refusal, and this one points the other way from every rule Mode B
has had so far. Every previous one asks whether the *browser* may be given something. This one asks
whether the **server** can answer something, and it cannot: a server renders what it has recorded,
so `freshness()` there is `Confirmed` at every position of every log. §102.2 is that rule.

## 102.1 The dimension, and the shape it actually takes

[`03`](03-type-and-effect-system.md) §3.7 has carried one sentence since before there was a
compiler:

> `Signal[T]` carries a freshness dimension (`confirmed | pending(n)`) that UI code can render
> ("saving…") — staleness is typed, not pretended away.

The value is `Freshness`, and it is the sentence's own two cases:

```beck
union Freshness:
    Confirmed
    Pending(n: Int)
```

Two variants rather than a count, because `Pending(n=0)` and `Confirmed` would be one fact with two
spellings and every page would have to know which one it had been handed. `n` sits on the pending
variant for the reason a `Some` carries its value: it exists only when there is something to count.

What it is *not* is a dimension on every signal's type, and that is the one place this work reads
§3.7 more narrowly than §3.7 is written. `freshness()` is a **source** in the signal vocabulary,
beside `presence()`:

```beck
saving: Signal[Freshness] = freshness()
page: Signal[Html] = map2(view, doc, saving)
```

so a page reads the freshness of *the render it is part of* — "is any of this a guess" — and not the
freshness of each value it is built from. A per-signal dimension would let a page say that one list
is speculative while the header beside it is not. That is a stronger thing and it is not built;
§102.7 says so rather than letting the sentence above imply otherwise. What is built is what
"saving…" needs, which is what the sentence gives as its example.

The implementation is the smallest one available, because `presence()` had already paid for the
shape. A source is a `Prim`, a vertex in the signal graph, a parameter the slicer substitutes into
the sliced view, and a value the renderer supplies at the edge — four small additions and no new
concept. `Roles::view` goes from three parameters to four; `beck_core::edge` gains the constructor
both sides build the value with, for the same reason it holds `session` and `envelope`: a browser
spelling a variant differently from a server is a page that differs from the one it is hydrating.

**No capability.** `presence()` performs `cap.presence` because a roster says something about other
people ([`14`](14-review-findings.md) F16), and that capability is also what pins it to the server.
A client counting its own unacknowledged commands says something about nobody else, so the row is
empty and what constrains the placement is the rendering rule below rather than authority.

## 102.2 A server cannot say it is saving, because it has nothing to save

`presence()` is refused to a Mode B page (`B0516`): who is connected is in neither the accumulator
nor the log, so a browser handed the state would have nothing to render it from. Freshness is the
mirror, and the argument runs in the other direction:

```text
error[B0518]: `page` reads `freshness`, so it cannot render on the server
   --> editor.beck:106:1
    |
106 | page: Signal[Html] = map2(view, doc, saving)
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ a server has nothing in flight
    |
    = note: freshness is a client's account of the commands it has proposed and not yet had
            confirmed. The server holds the log: what it renders is confirmed by definition, so this
            page would render `Confirmed` at every position of every log and its other branch would
            be unreachable.
    = help: render this component in the browser — `@render(client)`, which is what makes a guess
            possible in the first place — or take `freshness` out of its page
```

It is the first of this module's rules to refuse a component for rendering on the **server**, and it
is worth being precise about why it is a refusal rather than a lint. Rendering `Confirmed` on the server is not
wrong — it is *true*, and it is what the SSR of a Mode B page is rendered with. What is wrong is a
program written as though the other branch could happen. A "saving…" indicator that no log can ever
show is not a mistake the type checker would otherwise catch, because the program is perfectly
well-typed; it is a mistake about which tier is executing, which is the class of thing this
compiler is supposed to refuse.

So Mode A is unchanged for every program that does not ask, and a program that asks is told which
annotation makes the question answerable. `beck explain render` says the same thing before it can be
got wrong — a component that reads freshness gains a line of its own:

```text
freshness         read — this page renders `Pending(n)` while its own commands are in flight, and
                  `Confirmed` otherwise
```

### And the chokepoint may not read it either

`B0515` refuses a `validate` that decides from the roster: who was connected when an event was
recorded is written down nowhere, so a replay would decide differently. Freshness is the second
source that is not the log, and it gets the same rule (`B0517`) for a sharper version of the same
reason — **on replay nothing is in flight at all**, so a chokepoint that read it would accept a
command today and refuse it on the way back.

Both are checked by *reachability in the graph* rather than by the shape of `validate`'s argument,
because the freshness reaches the chokepoint through whatever `map2` a program cares to write.

## 102.3 The render that had to learn a second question

[`94`](94-mode-b-report.md) §94.14's finding was that an interaction paid for **two** full renders:
the client renders its guess, and then the server's data patch confirms exactly that guess and
`repaint` renders a page that — by the argument that makes optimism correct — cannot have changed.
The fix was to keep the state a page was rendered from beside the page, and return early when they
agree. It made a confirmation ~150× cheaper.

That shortcut asks whether the **state** moved. A confirmation is precisely the case where the state
does not move and the freshness does: `Pending(1)` becomes `Confirmed`, and a page that renders
"saving…" has to be repainted for it. Left alone, the shortcut would have made the one interaction
freshness exists for the one interaction Mode B renders nothing for — the same shape of defect
[`100`](100-client-polish-report.md) §100.3 found when a route change moved the session instead of
the state.

So `Shown` carries the freshness it was rendered at, and the comparison is asked **only of a
component that reads the answer** — `Bundle::reads_freshness`, which is a compile-time fact from the
splitter rather than a guess about the view's body. Comparing it unconditionally would have handed
every program in the tree back the second render §94.14 removed, to show nobody anything: a page
that does not mention its freshness parameter renders identical bytes for every value of it.

That is a conditional in a hot path, so the gate asserts both directions and would go red if either
were dropped — `a_confirmation_repaints_a_page_that_reads_freshness_and_no_other`, checked by
removing each half and watching it fail ([`84`](84-a-quota-is-only-as-good-as-its-actor-report.md)
§84.5's rule).

The bundle format is 2 rather than 1 because of that one boolean. postcard is not self-describing,
so a field added is a format changed, and [`94`](94-mode-b-report.md) §94.4 had already named
"bundle format 2" as where the next field goes rather than somewhere it could arrive quietly.

## 102.4 The budget, weighed

§5.1 says "Size budgets enforced in CI: < 150 KB brotli for a typical Mode-B component bundle".
§94.6 *measured* 1,753 bytes brotli for `board.beck` and §94.8 recorded that there was no gate, with
a reason: "a budget that fails on a machine without `brotli` is a flaky gate"
([`13`](13-testing.md) §13.7).

The reason was right about `cargo test` and wrong about CI, where the thin-client budget has been
enforced since Phase 1 by a step that begins `command -v brotli` — a *missing* compressor fails the
step instead of passing it at zero bytes. So the Mode B budget goes in the same job, in the same
shape, over **every** Mode B example in the tree rather than one of them:

```yaml
- name: Mode B component bundles (§5.1 budget < 150 KB brotli)
  shell: bash
  working-directory: compiler
  run: |
    command -v brotli
    cargo build --release -p beck-cli
    for f in examples/board.beck examples/editor.beck; do
      ./target/release/beck bundle "$f" --out /tmp/bundle.bpk
      raw=$(wc -c < /tmp/bundle.bpk)
      br=$(brotli -c -q 11 /tmp/bundle.bpk | wc -c)
      echo "$f: ${raw} B raw, ${br} B brotli (budget 153600)"
      test "$br" -lt 153600
    done
```

`shell: bash` is not decoration: it turns on `pipefail`, without which a `brotli` that failed
mid-pipe would make `br` the empty string and the comparison a shell error rather than a budget
failure — the same reason the thin-client step beside it carries the line.

Both failure paths were run by hand rather than trusted: a budget of 100 bytes fails the step, and a
`PATH` without `brotli` fails at `command -v` instead of passing at zero. (The step's numbers land a
byte above §102.5's, because it compresses a file where the measurement suite compresses a stream
and `brotli` knows the length in one case and not the other. Naming it is cheaper than a reader
finding two numbers for one artefact and wondering which is wrong.)

`beck bundle` is the new command, and its documentation says what it is *not* for. `beck run`
derives the bundle from the program it is executing, so a served slice cannot be of a different
program than the running one, and `beck build` deliberately writes none for the same reason. This
writes one for a measurement, and refuses a Mode A component rather than producing an empty file.

### The gate the threshold cannot be

A bundle is 1.1% of its budget. A threshold with ninety times its headroom is a gate that cannot go
red, and §84.5's question — *what would have to be true for this to fail?* — has an uncomfortable
answer: a program ninety times the size of the board. Which is the wrong question, because the
budget is **per component**, and what makes it hold for a large application is not that applications
are small. It is that a bundle is a function of the component's *slice*.

So the property is gated directly, under `cargo test`, where no compressor is needed:

> `a_bundle_is_a_function_of_the_slice_and_not_of_the_program_around_it` — adding 10 and then 100
> definitions the component does not reach changes what the bundle carries not at all, and costs
> **under a byte each**.

Two sizes, because one cannot tell "does not grow" from "grows slowly". Under a byte rather than
zero, and the fraction of a byte is real and worth naming: variables are numbered across the whole
program, so a larger program numbers the *slice's own* locals higher and postcard spends a second
byte on a varint past 127. A hundred unreached definitions cost **five bytes between them** — 0.05
each, `O(log n)` in the program's size. A definition that were genuinely carried would cost hundreds
of bytes, which is four orders of magnitude away from what this gate admits.

## 102.5 What it costs

`cargo test --release --test measure_mode_b -- --nocapture`, on the container this was written in:

| | bytes | brotli | against §5.1's 150 KB |
|---|---:|---:|---|
| `board.beck` — 10 definitions, 252 `Core` nodes | 4,875 | **1,753** | 1.1% |
| `editor.beck` — 5 definitions, 143 `Core` nodes | 2,843 | **1,083** | 0.7% |
| The kernel — every Beck application, whatever the program | 794,224 | **194,377** | 126.5% |

Two rows for the budget rather than one, and the point of the second is that it is a *different
program*: the budget is about a component, and a table with one row in it cannot say whether a
number belongs to the mode or to the program that happened to be measured. `editor.beck` is the
smaller of the two and it is the one that reads freshness, which is the answer to the obvious worry
— the dimension is a union and a `match`, not a runtime.

The kernel's row is **not** this report's number, and it is here because leaving it out would be
worse. §94.6 measured 724,031 bytes and 179,195 brotli; it is 794,224 and 194,377 now — 8.5% more
compressed. So the number is split rather than quoted, because an unattributed 15 KB on the download
every application shares is exactly the kind of number this project does not write down and leave.

### What this change costs the kernel, and what the six reports before it did

The same kernel built from the commit this branch starts at, on the same machine, and from this one
— `cargo build -p beck-wasm --release --target wasm32-unknown-unknown` in each tree, then
`brotli -c -q 11 target/wasm32-unknown-unknown/release/beck_wasm.wasm | wc -c`:

| | bytes | brotli |
|---|---:|---:|
| `e5d42cc` — before this change | 791,863 | 194,157 |
| this change | 794,224 | 194,377 |
| **the difference** | **+2,361** | **+220** |

220 bytes compressed, for a union in the prelude, an entry in the primitive table, a vertex in the
signal graph and a fourth parameter on every sliced view. **The other 15 KB since §94.6 is not
this**: it belongs to the six reports in between, which grew `beck-core` with read models, query
fusion, routing, presence and the playground's own surface — and this report makes no claim about
which of them, only that it has now been measured rather than assumed.

What freshness costs a page that reads it, at runtime, is one comparison per repaint of two values
that are `Confirmed` almost always, plus the render on a confirmation that used to be skipped. That
render is the *feature*: it is the page ceasing to say "saving". It is charged to the interaction it
belongs to, and it is the reason §102.3's conditional exists — so that a program that did not ask
for the feature does not pay for it.

## 102.6 The gates, and what makes each go red

`crates/beck-cli/tests/mode_b.rs` (30 tests, 7 of them new):

| What would break it | The test |
|---|---|
| `freshness()` stops reaching the view, or stops following the queue, or a confirmation stops repainting | `a_page_that_is_showing_a_guess_says_so_and_stops_when_it_is_confirmed` |
| `Pending` stops counting, and becomes "there is at least one" | `pending_counts_every_command_in_flight_and_not_just_that_there_is_one` |
| §94.14's shortcut swallows a confirmation, **or** every program starts paying for one again | `a_confirmation_repaints_a_page_that_reads_freshness_and_no_other` |
| A client that may not guess starts claiming to be pending | `a_client_that_may_not_guess_is_never_pending` |
| A page reading `freshness` is allowed to render on the server | `a_page_that_reads_freshness_cannot_render_on_the_server` |
| A chokepoint is allowed to decide from what is in flight | `the_chokepoint_cannot_decide_from_what_is_in_flight` |
| The bundle starts being a function of the program rather than of the slice | `a_bundle_is_a_function_of_the_slice_and_not_of_the_program_around_it` |

`crates/beck-cli/tests/browser.rs`, in Chromium:

| What would break it | The test |
|---|---|
| The word never reaches a real DOM, or never leaves it | `mode_b_says_it_is_saving_before_the_server_has_heard_of_it` |

That last one reads the guessing page **synchronously**, in the same evaluation that dispatches the
key, and the reason is the claim itself: a Mode B interaction is a local fold and a local render, so
the guess and the word for it are both on the page before the function that sent the command
returns. Polling for "saving" afterwards would be a race against the server's own answer, and would
pass just as well against a page that never said it at all.

And in Beck, `examples/editor.beck` asserts its own behaviour — including that a server-rendered page
says `saved`, which is §102.2's rule from the program's side. What a Beck test cannot assert is the
pending branch: `beck test` holds the log, and a log has nothing in flight. §102.7 has that as the
one gap this feature leaves in the in-language test surface.

## 102.7 What is not built

- **Freshness is one value for a render, not a dimension on every signal** (§102.1). A page can ask
  "is any of this a guess" and not "is *this list* a guess". The stronger reading of §3.7 would need
  the pending events' reach through the dataflow to be tracked per operator, which is the same
  machinery §5.3's engine uses for change and is not wired to this.
- **`beck test` cannot render a pending page.** The harness is the server's fold, so every page it
  renders is `Confirmed` — which is correct rather than broken, and it means a program's "saving…"
  branch is gated in Rust (§102.6) and not in Beck. Making it expressible means the test surface
  saying "proposed but not confirmed", which is a fourth clause shape and was not taken under this
  report's time.
- **No codegen**, unchanged: [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md) and
  [`08`](08-roadmap.md) §8.5.5's Lane E. This is now the whole of what Phase 3's Mode B bullet still
  owes — and [`93`](93-the-native-backends-report.md), which landed alongside this work and gave both native
  backends the *algebraic* half of a heap, says plainly that it does not bring Mode B codegen
  closer: a page is `Html` and `Str` all the way down, and that heap holds neither yet.
- **One component per program**, unchanged ([`94`](94-mode-b-report.md) §94.8), and with it lazy
  routes ([`100`](100-client-polish-report.md) §100.7). A program has one `page`, so §5.1's "a page
  mixes modes freely; the boundary is per component subtree" is unbuilt *in the language* rather
  than in Mode B.
- **No `wasm-opt`**, unchanged, so the kernel's compressed number is still a ceiling. The new CI
  gate is on the **component**, which is what §5.1's sentence budgets; the kernel keeps
  `mode_b.rs`'s 8 MB ceiling, which is deliberately not a budget.
- **The count is of commands in flight, not of guesses that survived.** A command whose events
  `Client::state` now skips — because it no longer validates against the state that arrived — is
  still counted. That is the reading a person watching a spinner wants, and it means `Pending(n)`
  and the page's own content can disagree about *n* for the moment before a refusal arrives.

## 102.8 What this corrects, elsewhere

- [`03`](03-type-and-effect-system.md) §3.7's freshness sentence is built, and §3.7's staging list
  item 3 — "except freshness-typed optimism, which needs Mode B and is Phase 3's" — is discharged,
  with §102.1's narrowing stated in both places.
- [`05`](05-tier-lowering.md) §5.1's "Size budgets enforced in CI" is true of the component bundle
  as of this change. It was the only clause of that bullet that named CI and had none.
- [`08`](08-roadmap.md) Phase 3's Mode B bullet has one item left — codegen — where it had three.
- [`94`](94-mode-b-report.md) §94.8's last bullet ("no `wasm-opt`, no size gate in CI") is half
  answered: the size gate exists and `wasm-opt` still does not run. Its §94.14 shortcut is
  unchanged in the common case and now asks a second question of the components that need one
  (§102.3).
- **The bundle format is 2** (§102.3), and `FORMAT` is checked on load, so a kernel and a compiler
  that disagree refuse each other rather than misreading a byte.
- `beck bundle` is a new command; the generated reference (`docs/reference/`) carries it, as it
  carries `B0517` and `B0518`.

## 102.9 What Phase 3 is still not

Unchanged: the per-component boundary in the language, and lazy routes behind it. Every other item
[`94`](94-mode-b-report.md) §94.11 listed has since had a report of its own — the playground
([`98`](98-playground-report.md)), identity's relying party
([`95`](95-oidc-relying-party-report.md)), presence ([`96`](96-presence-report.md)), the
supply-chain bullet's remaining pieces ([`99`](99-supply-chain-report.md)) and client polish
([`100`](100-client-polish-report.md)) — each of which says for itself what it did and did not
close.

The exit criterion is a claim about a person, and no outside developer has read the guide
[`86`](86-getting-started.md) publishes.
