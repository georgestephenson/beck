# 102 — Styling, and the components everybody rebuilds

> **Design, with measurements. Nothing here is built.** Every number below was produced by a command
> this document quotes, against the tree at the time of writing. The recommendations are
> recommendations; what is established is the *state*, which was worse than any document said.

Two questions, and they turn out to be one:

1. **Styling.** CSS is absorbed — [`00`](00-original-idea.md) makes the browser's three languages an
   *instruction set* rather than an authoring format, and `(my-javascript (my-css (my-html)))` is
   only literal if the middle term has somewhere to live. It does not. §102.1 says what is actually
   there, and it is eight rules hard-coded in Rust.
2. **Components.** Date pickers, data tables, charts, carousels, modals. Every web project rebuilds
   them, which is the exact thing this language exists to stop
   ([`01`](01-vision-and-premise.md) §1.1). Is there a Beck answer, and does it need language work?

They are one question because both are answered by the same sentence: **Beck has a compiler and the
CSS ecosystem does not.** Every awkward part of Tailwind — the content scanner, the safelist, the
`@apply` that its own documentation advises against — is a workaround for not being able to see the
program. Every awkward part of a React component library — the render props, the ref forwarding, the
controlled/uncontrolled duality — is a workaround for not being able to see the state. Beck can see
both. §102.4 and §102.10 are what that buys, and §102.8 is the price of admission, which is three
walls that are load-bearing rather than incidental.

## 102.1 What exists today

**The stylesheet is eight rules, hard-coded in Rust.**
[`beck-rt/src/css.rs`](../compiler/crates/beck-rt/src/css.rs) holds `STYLES`, a `&'static [Rule]`
containing the todo sketch's own CSS transcribed by hand; `http.rs` serves it at `/beck.css` and the
page shell links it. A user's program cannot contribute a rule, override one, or remove one. The
file's own doc comment says "there is no runtime CSS story in Beck and there should not be one",
which is right about *runtime* and has been read as covering the whole subject.

**`css:` does not exist** — no parser, no macro, no test. It was written as present tense in
[`11`](11-language-tour.md) §11.6, and twice in [`12`](12-standards-and-conformance.md) §12.4, the
sharper of the two being a chartered "colour-contrast over statically-known `css:` values" — a check
scheduled over a construct with no syntax. Both documents are corrected in place by this change.
[`01`](01-vision-and-premise.md) §1.3 keeps its `styles = css:` and is right to: that section is the
original sketch translated construct-for-construct, and the sketch is where the requirement comes
from.

**`class=` is a `Str`, and no attribute name is checked.** The `ui:` macro turns any `name=value`
into an attribute, converting `_` to `-` (so `aria_label` becomes `aria-label`, which is right) and
`on_x=` into `data-b-x`. It has no vocabulary of its own: it does not know which attributes HTML
has, which events the client listens for, or that `class` is special. §102.8 is what that costs.

**A component is a `def` returning `Html`, and `component` is not a keyword.** [`11`](11-language-tour.md)
§11.6's `component TodoList(sess)` does not parse. [`94`](94-the-client-report.md) §94.15 states the
consequence from the other end: a program has one component, because it has one `page`.

So the position is not "styling is basic". It is that **nothing in the language is about styling at
all**, and the one artefact that is, is a Rust constant.

## 102.2 Tailwind in a Beck page, measured

The measurement is Tailwind CSS 4.3.3 under Node 22.22.2. The page is
[`examples/todo.beck`](../compiler/examples/todo.beck) with its `class=` values replaced by utilities
and `done_class` widened to a full row style:

```python
def render(todos: list[Todo], left: Int) -> Html:
    return ui:
        main(class="mx-auto max-w-prose p-8 font-sans text-slate-900 dark:text-slate-100"):
            h1(class="text-4xl font-thin tracking-tight text-rose-300"): "todos"
            ...
                    li(key=t.id, class=row_class(t)):

def row_class(t: Todo) -> Str:
    base = "flex items-baseline gap-2"
    return base + " line-through opacity-50" if t.done else base
```

and the whole Tailwind configuration is two lines of CSS:

```css
@import "tailwindcss" source(none);
@source "./todo_tw.beck";
```

```
$ npx tailwindcss -i styled.css -o styled.out.css --minify
Done in 63ms
```

| | |
|---|---|
| Configuration needed to teach Tailwind about `.beck` | **none** — the extractor is language-agnostic |
| Language changes needed | **none** — `class=` was already a string |
| Build | **63 ms**, one command |
| Sheet | **7,047 bytes** minified: 4,165 of reset and theme, **2,882** for 35 utilities |
| Utilities found | **35 of 35**, including `dark:`, `hover:` and `focus:` variants |
| Found inside a Beck string concatenation (`base + " line-through opacity-50"`) | **yes** |

**So the answer to "how simply can Tailwind be used in a Beck website" is: completely simply, today,
with nothing built and nothing decided.** That is a real answer and it should be the starting point
of any plan. It is also the whole of the good news.

## 102.3 What a scanner cannot see, measured

Tailwind does not read the program. It reads the *bytes of the files*, extracts anything that could
be a class name, and keeps the ones its compiler recognises. Three consequences, each measured.

**It emits CSS for programs that have no user interface.** Pointed at the 71 `.beck` files in
[`lib/`](../compiler/lib/), [`corpus/`](../compiler/corpus/), [`sicp/`](../compiler/sicp/),
[`awfy/`](../compiler/awfy/) and [`clbg/`](../compiler/clbg/) — one of which has a `class=` at all,
and its two values are `"mine"` and `"theirs"` — it produces **15 utility rules, 3,838 bytes over
baseline**:

```
absolute block contents filter fixed grow hidden invisible rounded shadow static table transform truncate visible
```

**Every one of them comes from English prose**, not from code: "below absolute zero never reaches
the log", "a test block's row must be empty", "does not grow with the number of operations", "stated
rather than hidden", "a definition may shadow a primitive", "none of them is a table", "it does not
truncate". A comment about a *language feature* becomes a CSS rule about a *box*, because the two
vocabularies share a dictionary and a regex cannot tell them apart. Those 15 spurious rules cost
*more bytes than the 35 real ones* (3,838 against 2,882), because `filter`, `transform` and `shadow`
each drag in `@property` declarations and custom-property defaults — so a CSS budget can be blown by
writing a paragraph.

**A misspelling is silent, and so is a computed class.** This file:

```python
def badge(kind: Str) -> Html:
    return ui:
        span(class="rounded-ful bg-emerald-500 " + tone(kind)): kind

def tone(kind: Str) -> Str:
    return "text-" + kind + "-700"
```

produces one utility of the three it names — `bg-emerald-500` — and exits **0**. `rounded-ful` is a
typo for `rounded-full` and Tailwind has no opinion about it; `"text-" + kind + "-700"` is invisible
to a regex. The standard remedy is a **safelist**: a hand-maintained list of class names the scanner
could not find, which is a build configuration file that must be kept in sync with the program by a
human. `tailwindcss canonicalize` does not help — it sorts a class list and passes
`not-a-real-utility` straight through.

**And it breaks at the library boundary, which is the one that matters here.** Take the component
kit of §102.7 — `card`, `stack` and `action` in `kit.beck`, imported by `app.beck`. Point Tailwind
at the application:

| Source given to the scanner | Utilities emitted |
|---|---|
| `app.beck` | **1** (`text-slate-600` — the only class written in the application) |
| `app.beck` + `kit.beck` | 12 |

Eleven of twelve utilities silently missing, exit 0. This is the well-known failure that forces
every JavaScript component library to put an `@source` line in its installation instructions — and a
Beck package is a **tarn**, a content-addressed OCI artefact ([`16`](16-packages-and-ecosystem.md)
§16.2), so there is no source directory for the user to point at at all. **A styling story that
depends on scanning source text cannot survive a package manager**, and a component ecosystem is
the entire subject of the second half of this document.

The diagnosis is not that Tailwind is badly built. It is that a scanner is what you write when you
cannot resolve an import, and Beck resolves imports.

## 102.4 The proposal: the design system is Tailwind's, the extraction is the compiler's

Split Tailwind in two and take one half.

**Take the design system.** The scale of spacing, the colour ramps, the type scale, the variant
grammar (`hover:`, `dark:`, `md:`, `supports-[…]:`), and above all **the names**. These are a decade
of taste, they are MIT-licensed, and — the part no other option has — every web developer alive
already knows them, as does every model they will ask for help. A Beck-invented vocabulary would be
strictly worse at the only thing a vocabulary does.

**Refuse the delivery mechanism.** No scanner, no safelist, no `@source`. The compiler already knows
every string that can reach a `class=` attribute, across imported modules, because it resolved them.
Concretely, four things fall out and none of them is a language feature:

1. **Exact extraction.** `beck build` walks the typed tree, collects the class strings that reach a
   `class=`, and emits the sheet. No false positives (a `def truncate` is a definition, not a
   token), no false negatives across a module boundary, and no configuration.
2. **A misspelling is a diagnostic.** `rounded-ful` gets a `B0…` with a did-you-mean, because
   Levenshtein over a known table is what the compiler already does for field names. This is the
   whole difference between Tailwind and a language that absorbed it.
3. **The editor is free.** [`65`](65-the-editor-report.md) has completion, hover and rename served
   from `beck_core`. A checked class attribute makes `class="fl‸"` complete to `flex`, hover print
   the declarations, and go-to-definition on a *token* land on the theme — that is the Tailwind
   IntelliSense extension, without an extension, because the answers come from the compiler.
4. **A class the compiler cannot enumerate is refused**, not silently dropped.
   `"text-" + kind + "-700"` is exactly as invisible to Beck as it is to Tailwind, and the
   difference is that Beck can *say so*. The shape a program should write instead is the shape
   Beck programs already write — `row_class` above is two constant alternatives behind an `if`,
   which constant-folds to a set of two. An escape hatch (`@style(dynamic)`, one attribute, on the
   pattern of `@a11y(exempt, reason=…)`) covers the genuine cases and puts them in the audit trail
   rather than in a safelist.

**And the oracle is Tailwind itself, not a table somebody typed in.** Tailwind's compiler is a total
function from a candidate name to a rule or to nothing, which is precisely the predicate Beck needs:

| candidate | Tailwind emits | so Beck should |
|---|---|---|
| `flex`, `line-through`, `rounded-full` | a rule | accept |
| `p-2.5`, `p-[13px]`, `supports-[display:grid]:grid`, `dark:md:flex` | a rule | accept |
| `rounded-ful`, `bg-emerald-550`, `text-4xxl` | nothing | **refuse, with a suggestion** |

That is [`clbg/`](../compiler/clbg/README.md)'s pattern — rebuild the asserted constants from
somebody else's published artefact so a wrong constant fails even against a matching wrong
expectation — applied to a utility table. The gate is: for every name Beck accepts, Tailwind emits a
rule; for every name Beck refuses, Tailwind emits nothing. It runs where a Node is available and
skips loudly where one is not, like every other environment-dependent suite this repository has.

## 102.5 Why Tailwind cannot be a dependency, and should still be the default look

The npm package is not the design system. It is:

| | |
|---|---|
| Packages installed | **24** |
| Bytes on disk | **20 MB** |
| Prebuilt native binaries fetched at install time | **3** (`@tailwindcss/oxide`, `lightningcss`, `@parcel/watcher`) |
| Runtime required | Node |

Set that beside what Beck ships: one static binary, `git clone && beck up`, and an absence checklist
that names "no Dockerfile" ([`01`](01-vision-and-premise.md) §1.3) in the same breath as no routes
and no SQL. Set it beside [`16`](16-packages-and-ecosystem.md) §16.4, which calls npm's install-time
code execution "its worst security legacy" and makes its absence a design property. And beside
[`92`](92-supply-chain-and-release-report.md), which spends a chapter on provenance for artefacts
this project builds itself.

**A Beck application whose styling requires `npm install` has lost the argument that the tarball is
the product.** So the recommendation is a distinction rather than a yes or no:

- **On by default in appearance.** The examples, the tutorial, `beck init` and the playground ship
  *styled*, using Tailwind's names, with the sheet emitted by `beck build` from the compiler's own
  table. A user who types `beck new` gets something that looks deliberate, with no Node anywhere.
- **Off by default as a dependency.** Tailwind-the-npm-package stays a supported, documented,
  one-command upgrade for anyone who wants the complete utility surface, arbitrary plugins, or an
  existing design system — and the same exact extraction feeds it, since `beck build` can emit the
  class list as an artefact the scanner reads instead of guessing at source.

The cost of that position is honest and should be stated: the compiler carries a **subset** table,
it will lag upstream, and the gap has to be visible (`beck explain style` printing "this name is
Tailwind's and not in Beck's table" rather than a bare refusal). The two rungs below are how the
subset stops being a permanent apology.

**And the port is smaller than it sounds.** Of the three native binaries above, two are Rust already:
Tailwind's *scanner* (Oxide) and `lightningcss`, which is on crates.io — at `1.0.0-alpha.72`, which
[`07`](07-dependencies.md)'s standard would want an argument for. Tailwind's *utility compiler* is
the JavaScript half (`dist/*.mjs`). Beck does not need the scanner — that is the half §102.4
replaces — and it does not need a CSS parser to *emit* CSS, so the only thing that has to cross is
the utility table, which is data. Generated, and held against upstream by the gate above, that is a
wave's maintenance rather than a fork.

## 102.6 Syntactic sugar, mostly refused

The question was whether syntax can make styling joyful. The honest answer is that **the joy is in
the diagnostics and the editor, not in the notation**, and most sugar proposed here would cost more
than it returns. Ranked, with verdicts:

| Idea | Verdict |
|---|---|
| **Leave `class="…"` a string literal** | **Yes.** It is already the notation the world's documentation, examples and training data use. Checking it changes everything; respelling it changes nothing and breaks the transfer |
| **`@apply`, as an ordinary function** | **Yes, and it is free.** `def card_box() -> Class: return cls("rounded-lg border p-4 shadow-sm")`. Tailwind's own documentation warns against `@apply` because it is a text splice with no semantics; in Beck it is a function — typed, documented, renameable, go-to-definable, and dead-code-eliminated. **The standard objection to the feature evaporates when the language has functions**, which is as compact a statement of this project's thesis as styling is going to produce |
| **Conditional utilities as library functions** — `when(t.done, "line-through opacity-50")` | **Yes**, and as a library, not syntax. It wants `class=` to accept a list as well as a string, which is one line in the `ui:` macro |
| **The theme as a Beck value** — tokens as a record, generating both the `@theme` block and the accepted table | **Yes.** Renaming a brand colour becomes a rename, checked. This is the piece that makes a *design system* rather than a stylesheet |
| **Variants as values** — `hover(bg_rose_500)` instead of `"hover:bg-rose-500"` | **No.** Loses the transfer, gains nothing a checked string does not already have |
| **A `css:` macro** for what utilities cannot say — keyframes, `@container`, complex selectors | **Later, and cheaply.** It is a typed literal macro ([`02`](02-syntax.md) §2.5), so it waits on the macro interpreter, which is [`08`](08-roadmap.md) §8.5.4's **first** item. Nothing here justifies unblocking it early, and §102.4 works without it |
| **A `component` keyword** | **No.** §102.7 measures what functions already do, and the answer is everything |

## 102.7 A component library needs no language feature — measured

This kit compiles today, on the tree as it stands:

```python
# kit.beck
def card(title: Str, body: Html) -> Html:
    return ui:
        section(class="rounded-lg border p-4"):
            h2: title
            body

## A button carrying the application's own command.
def action[C](label: Str, cmd: C) -> Html:
    return ui:
        button(class="rounded bg-slate-900 px-3 py-1 text-white", on_click=cmd): label

def stack(children: list[Html]) -> Html:
    return ui:
        div(class="flex flex-col gap-2"):
            for c in children:
                c
```

```
$ beck check app.beck
ok: 7 definitions, 4 signals, wire id 97af0f6d2f658494
```

and `beck test` renders it to

```html
<section class="rounded-lg border p-4"><h2>count</h2><div class="flex flex-col gap-2">
<p class="text-slate-600">0</p>
<button class="rounded bg-slate-900 px-3 py-1 text-white" data-b-click="{&quot;c&quot;:&quot;Bump&quot;}">bump</button>
</div></section>
```

Four things are established by that, and each was a question before it was run:

1. **Components compose as functions**, across a module boundary, with children passed as `Html`
   values — no slots, no children props, no keyword.
2. **A component is generic over the application's command.** `action[C]` is a library button that
   carries `Bump`, a constructor of a union the library has never heard of, and it serialises
   correctly to `data-b-click`. That is the single hardest requirement on a third-party UI
   component, and type parameters on every declaration ([`27`](27-the-walls-come-down-report.md))
   already discharged it.
3. **The markup is markup**, not escaped text — the defect [`94`](94-the-client-report.md) §94.13
   found in exactly this shape is fixed and stays fixed.
4. **SVG works, with its attribute names intact.** `svg(viewBox="0 0 100 20", aria_label="count")`
   emits `viewBox` verbatim (camelCase preserved) and `aria-label` hyphenated, and children nest.

Point 4 answers an open question. [`09`](09-risks-and-open-questions.md) §9.6 item 8 asks whether a
visualization vocabulary is a library, an addition to the `ui:` core, or a typed literal macro. It
is **a library**: a chart is a pure function from data to `Html`, the axis arithmetic is Beck, and
`ui:` needs nothing added. That is the cheapest of the three answers and it was the one nobody had
checked — and §102.9 is the defect that makes it not quite true yet.

## 102.8 The three walls

### Wall 1 — interface state has no home, so opening a dropdown is a log entry

A modal's open flag, an accordion's expanded section, a combobox's highlighted option, a table's
sort column, a carousel's index. Where does that state live?

Every application in [`corpus/`](../compiler/corpus/) and [`examples/`](../compiler/examples/) holds
its state in `durable(fold(…))` — 36 of 37 files, the exception being `documented.beck`, which is a
library. [`10`](10-decisions.md) D1 and [`15`](15-scale-and-distribution.md)
provide for the alternative in so many words ("high-churn ephemera get non-durable folds — same
semantics, no log persistence"), so the obvious move is a fold that is not `durable`. Measured:

```
$ beck check modal.beck        # ui_state: Signal[State] = fold(apply_event, State(open=False), events)
ok: 4 definitions — a library: no durable state, so there is nothing to run
```

The program is not an application. A non-durable fold does not make a signal graph, does not get a
page, does not run. D1's escape hatch is **decided and unbuilt**. And placing the durable one where
the state belongs is refused, correctly:

```
error[B0401]: `ui_state` is placed on `client`, which cannot discharge `durable`
   = note: `durable` is the log: placing it on the client would ship the database to the browser
```

So today the only expressible answer is that **opening a modal is a `Command`, validated by
`validate`, recorded as an `Event`, and folded into the durable log forever** — replicated to the
data tier, replayed on every genesis replay, and included in the state digest. A date picker paging
to next year is twelve permanent log entries. This is not a performance nit; it is the semantics
being wrong about what happened. Nobody *decided* it, which is why it is a wall and not a trade-off.

Three candidate answers, in increasing order of ambition, and this document does not pick:

- **A client-placed non-durable fold** over the client's own commands. Cheapest, and it needs a
  client-local stream, which does not exist: the only stream is `merge_clients()`, and it is
  server-placed by §3.5.
- **The URL.** State that deserves a bookmark should be in `session.path` — [`94`](94-the-client-report.md)
  §94.3 already makes a route a field of `Session` and not a router. This is the *right* answer for
  a table's sort and filter and the wrong one for a tooltip.
- **The platform.** §102.9: for a large fraction of these components the browser will hold the state
  if asked in markup, and then it is nobody's state at all.

### Wall 2 — the event vocabulary is five, and an unknown one is silent

`beck-patch.js` listens for exactly five things: `click`, `keydown` filtered to Enter, `submit`,
`input` and `change`. The W3C's ARIA Authoring Practices keyboard tables — the specification any
serious component is held to — want arrows, `Home`, `End`, `Escape`, `Space`, `PageUp`/`PageDown`
and typeahead, plus `focus` and `blur`.

That gap is expected for a young client. What is not expected is that **the compiler does not know
about it**:

```
$ beck check keys.beck          # span(on_keydown=Toggle(id=t.id), on_mouseenter=Toggle(id=t.id))
ok: 9 definitions, 4 signals, wire id a833a9a1988d5462
$ beck test keys.beck
8 passed, 1 failed, 0 skipped   # the one failure is a missing snapshot, which --update writes
```

and the page snapshot the harness then records as *correct* contains

```html
<span data-b-keydown="{&quot;c&quot;:&quot;Toggle&quot;…}" data-b-mouseenter="{…}">bread</span>
```

Two dead attributes, shipped to the browser on every render, wired to nothing. `beck check` is
happy, `beck test` is happy, and the snapshot — the gate [`22`](22-phase-3-report.md) §22.10 added
precisely because "`contains` asserts one string somebody thought to name and a snapshot asserts
every attribute" — has pinned the defect as the expected output.

The same hole swallows attribute names, and the spelling it swallows is one a reader will arrive
with: [`01`](01-vision-and-premise.md) §1.3 writes `cls=`, faithfully, because the original sketch
did — that section is the seed translated construct-for-construct and is right to keep it. What
happens to somebody who copies it is measured:

```html
<li cls="done" data-b-k="1">
```

`cls` is not an HTML attribute. The page loses its styling, the browser ignores it, and every gate
stays green. **The `ui:` macro has no vocabulary**, so a typo in a handler name or an attribute name
is not a compile error, not a lint, and not visible in a snapshot review — it is a page that quietly
does less than it says.

This is [`82`](82-the-edge-report.md) §82.10's pattern again — a gate that cannot fail, written by
people who knew what they meant. The fix is small and owed — a known set of events (which
the client's own listener table already is) and a known set of attributes, with a diagnostic and an
escape hatch for genuine custom attributes.

### Wall 3 — focus is not a function of state

Every APG pattern that is worth having moves focus: a dialog focuses its first control and restores
on close, a menu focuses the item the arrow key selected, a combobox keeps focus in the input while
`aria-activedescendant` moves. Beck has `autofocus="on"`, a static attribute, and the caret
restoration [`94`](94-the-client-report.md) §94.8 built, which *preserves* focus across a patch and
cannot *place* it.

The imperative answer — a `focus()` effect — is the wrong one for this language, and the right shape
is already implied by everything else here: **focus is a function of state**, an attribute the view
writes and the client reconciles the way it reconciles every other attribute, refusing to move focus
into an element that did not exist before the patch. That is one client-side rule and one attribute,
and it keeps the page a pure function of state, which is the property the whole design is for.

## 102.9 What the platform already owns

The libraries a JavaScript component kit is made of exist because the browser could not do these
things when they were written. Measured against Chromium 141.0.7390.37 — the browser
`beck-cli/tests/browser.rs` already drives:

| Capability | Present | What it replaces |
|---|---|---|
| `<dialog>.showModal()`, `inert` | yes | focus trap, scroll lock, backdrop, Escape |
| `popover` attribute | yes | light dismiss, top layer, the popover half of a menu |
| `anchor-name` / `position-anchor` / `position-area` | yes | **Floating UI** — tooltip and menu positioning, in CSS |
| `::scroll-marker`, `::scroll-button`, scroll-snap | yes | a carousel's dots, arrows and paging |
| `<details name>` | yes | an exclusive accordion |
| `<input type=date>` | yes | the common date picker outright |
| `command` / `commandfor` invokers | yes | **the button that opens the dialog, with no handler and no state** |
| `content-visibility`, container queries, `:has()`, view transitions | yes | virtualisation's cheap half, responsive components, transitions |
| `interestfor` (hover/focus invokers) | **no** | hover-triggered tooltips still need script |

The command-invoker row deserves its own sentence, because it dissolves Wall 1 for the most-cited
case. `<button command="show-modal" commandfor="dlg">` opens a dialog **with no `data-b-click`, no
`Command`, no `Event` and no log entry**, because the state was never the application's. The general
rule that falls out is on this project's own thesis: *interface state the document can own
declaratively should not become application state* — and deciding which tier owns a piece of state
is what Beck does. [`03`](03-type-and-effect-system.md)'s placement lattice has `client`, `server`,
`data` and `any`; what the components want is a fifth position for **state the browser holds**,
which is not a tier the compiler can put code on but is a place a fact can live. That is an idea to
argue in [`100`](100-placement-at-runtime.md)'s terms rather than a proposal this document is
entitled to make.

**The caveat is the measurement's, and it is large.** This is one browser at one version, not
Baseline. Anchor positioning and the scroll-marker pseudo-elements are the two rows most likely to
be absent elsewhere, and [`94`](94-the-client-report.md) §94.15 already lists "a browser other than
Chromium" as not built. A component library that leans on the platform needs a support matrix and a
degradation story per row, and neither exists.

**And one defect blocks the chart of §102.7 outright.** `beck-patch.js:10` builds patched-in
subtrees with `document.createElement(tag)` — no namespace. Measured in the same Chromium, on the
same tree the patch carries:

| path | namespace | `instanceof SVGElement` | rendered width of a `rect` |
|---|---|---|---|
| server-side render (the HTML parser) | `…/2000/svg` | true | **50** |
| a patch (`createElement`) | `…/1999/xhtml` | false | **0** |

So an SVG chart paints on first load and **vanishes the first time it changes** — which is every
time the data it is a function of changes, which is the only reason to have drawn it. The fix is
`createElementNS` with a namespace inherited from the ancestor, and the gate is a browser test that
asserts a patched `rect` has a non-zero box. It is not fixed here, because a client change wants its
own change and its own browser gate; it is item 1 of §102.11 for the same reason.

## 102.10 Which components Beck should own, and why some of them it should own better

The user-facing list, with what each actually needs:

| Component | Where the state is | Verdict |
|---|---|---|
| **Data table** — sort, filter, paginate, group | the query | **The flagship.** Sorting and filtering a table is a *view*, and keeping views incremental is what [`23`](23-incremental-views-report.md) built. Everyone else re-sorts an array in the browser on every keystroke; Beck maintains a dataflow and sends a patch. This is the component where the language wins on the merits rather than on ergonomics |
| **Charts** | pure function of data | **Library, no walls** — proved in §102.7, blocked only by the namespace defect. Answers [`09`](09-risks-and-open-questions.md) §9.6 item 8 |
| **Modal / dialog** | the platform | Markup. `<dialog>` + invokers, no application state (§102.9) |
| **Accordion / disclosure / tabs** | `<details name>`, or the URL | Markup, plus Wall 2's keyboard vocabulary for the ARIA-conformant tab list |
| **Carousel** | CSS scroll position | Markup and CSS, with the support caveat |
| **Date picker** | `<input type=date>` first | The platform control for the common case; a custom one is the hardest thing on this list and needs all three walls down |
| **Combobox / autocomplete / menu** | Wall 1 and Wall 3 | **Blocked.** Highlighted option is interface state, and the pattern is defined by where focus is |
| **Toast, tooltip, popover** | the platform, except hover triggers | Markup, minus `interestfor` |

The shape of the answer: **the library is markup and utilities, not a port of Radix.** Porting a
JavaScript component library would be porting its workarounds — for state it could not place, for
positioning the platform could not do, and for a compiler it did not have. What is worth absorbing
from that ecosystem is its *specification work*: the ARIA Authoring Practices keyboard tables and
the accessibility invariants, which are somebody else's artefact and therefore exactly the kind of
oracle this repository prefers ([`93`](93-the-native-backends-report.md) §93.12,
[`clbg/`](../compiler/clbg/README.md)). A component ships with the APG's keyboard table as its test,
or it is not a component.

That also gives [`12`](12-standards-and-conformance.md) §12.4 its missing artefact. The three
accessibility checks already scheduled there — alt text, accessible name, input label — are the same
mechanism Wall 2 needs, over the same typed tree, and would be built once.

## 102.11 What to build, in order

Nothing below is scheduled; [`08`](08-roadmap.md) §8.5 is the only place in `docs/` that holds an
order, and these are candidates for it. Cheapest first, and the first three are each smaller than
the paragraph describing them.

1. **`createElementNS` in the patch applier**, with a browser gate asserting a patched `rect` has a
   non-zero box (§102.9). Without it there are no charts, and charts are the highest-value thing on
   this list that needs nothing else.
2. **A vocabulary for `ui:`** — known events, known attributes, a diagnostic with a suggestion, an
   escape hatch (§102.8, Wall 2). This is the gate that would have caught `cls=` in the canonical
   sketch, and it carries §12.4's first three accessibility checks along with it.
3. **`class=` accepts a list**, and `Class` becomes a type (§102.4, §102.6). One macro change, and
   it is the prerequisite for everything in the styling half.
4. **The utility table and the sheet emitter** — exact extraction over the typed tree,
   `beck build` writing `/beck.css`, and the differential gate against Tailwind (§102.4). This is
   what retires `beck-rt/src/css.rs`.
5. **The theme as a Beck value**, and `beck new` producing something styled (§102.5).
6. **A decision on Wall 1** — client-local ephemeral state — which is a language decision and wants
   a D-number, not an implementation (§102.8).
7. **Focus as an attribute** (§102.8, Wall 3), after 6.
8. **The kit**: table, chart, dialog, accordion, tabs, each with the APG's own keyboard table as
   its test (§102.10). It is a `lib/` directory or a tarn depending on how 3–5 land, and that choice
   should be made *after* them rather than in advance.

## 102.12 What this does not establish

- **Nothing is built.** Every measurement is of the tree as it stands or of third-party tools run
  against it. No compiler change, no client change, no library, no gate.
- **The Tailwind numbers are one page and 71 files at one version** (4.3.3). The false-positive
  count is a property of that corpus and that extractor and will differ for another; what the
  measurement establishes is the *shape* — a scanner reads bytes and a compiler reads a program —
  not the constant.
- **The platform table is one browser at one version.** It is evidence that the capabilities exist,
  not that they can be depended on. Nothing here has been checked against Firefox or Safari, and the
  support matrix a component library would need does not exist.
- **The SVG namespace defect is measured in isolation** — the mechanism `beck-patch.js` uses, in
  Chromium, against the same tree shape — and not observed end-to-end in a running Beck page. It is
  a one-line read of the client plus a browser measurement, which is strong evidence and not a
  reproduction.
- **Wall 1 has no recommendation.** Three candidate answers are stated and none is argued for,
  because the choice touches D1, §3.5's placement lattice and the merge point, and a document that
  measured the problem this week should not also settle it.
- **No claim is made about performance.** Not the sheet's effect on first paint, not the cost of
  exact extraction at build time, not what a maintained data-table view costs against re-sorting in
  the browser. §102.10 calls the data table a flagship on a *design* argument;
  [`25`](25-benchmarks-and-expressiveness.md) §25.9's rule is what would have to be satisfied before
  anybody says it is faster than anything.
