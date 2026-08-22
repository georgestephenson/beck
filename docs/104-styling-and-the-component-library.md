# 104 — Styling, and the components everybody rebuilds

> **Design, with measurements. Nothing here is built.** Every number below was produced by a command
> this document quotes, against the tree at the time of writing. The styling half is **decided** —
> [`10`](10-decisions.md) D29 — and the work is scheduled in [`08`](08-roadmap.md) §8.5.4; the
> component half is measured and open. §104.8's Wall 1 was left unsettled here and was settled
> afterwards by [`10`](10-decisions.md) D30, which adopts §104.8's five homes and builds the fourth.
> Three of
> the four things this audit found wrong are **defects** and are registered in
> [`DEFECTS.md`](../DEFECTS.md) with the gate each fix owes; the fourth — focus cannot be placed by a
> view — is an *absence* rather than a defect, so it is a scheduled item and not a register entry.

Two questions, and they turn out to be one:

1. **Styling.** CSS is absorbed — [`00`](00-original-idea.md) makes the browser's three languages an
   *instruction set* rather than an authoring format, and `(my-javascript (my-css (my-html)))` is
   only literal if the middle term has somewhere to live. It does not. §104.1 says what is actually
   there, and it is eight rules hard-coded in Rust.
2. **Components.** Date pickers, data tables, charts, carousels, modals. Every web project rebuilds
   them, which is the exact thing this language exists to stop
   ([`01`](01-vision-and-premise.md) §1.1). Is there a Beck answer, and does it need language work?

They are one question because both are answered by the same sentence: **Beck has a compiler and the
CSS ecosystem does not.** Every awkward part of Tailwind — the content scanner, the safelist, the
`@apply` that its own documentation advises against — is a workaround for not being able to see the
program. Every awkward part of a React component library — the render props, the ref forwarding, the
controlled/uncontrolled duality — is a workaround for not being able to see the state. Beck can see
both. §104.4 and §104.10 are what that buys, and §104.8 is the price of admission, which is three
walls that are load-bearing rather than incidental.

## 104.1 What exists today

**The stylesheet used to be eight rules, hard-coded in Rust.** `beck-rt/src/css.rs` held `STYLES`,
a `&'static [Rule]` containing the todo sketch's own CSS transcribed by hand; `http.rs` served it at
`/beck.css` and the page shell linked it. A user's program could not contribute a rule, override one,
or remove one. **That file is gone** (§104.4): `/beck.css` now carries the sheet the compiler derives
from the classes the running program's pages can carry, and `beck build` writes the same sheet to
disk. The rest of this section is the position that made the item worth doing and is otherwise
unchanged.

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
has, which events the client listens for, or that `class` is special. §104.8 is what that costs.

**A component is a `def` returning `Html`, and `component` is not a keyword.** [`11`](11-language-tour.md)
§11.6's `component TodoList(sess)` does not parse. [`94`](94-the-client-report.md) §94.15 states the
consequence from the other end: a program has one component, because it has one `page`.

So the position is not "styling is basic". It is that **nothing in the language is about styling at
all**, and the one artefact that is, is a Rust constant.

## 104.2 Tailwind in a Beck page, measured

The measurement is Tailwind CSS 4.3.3 under Node 22.22.2. The page was
[`examples/todo.beck`](../compiler/examples/todo.beck) with its `class=` values replaced by
utilities and `done_class` widened to a full row style — an edit at the time, and the sketch's own
markup since §104.4a landed:

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

## 104.3 What a scanner cannot see, measured

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
kit of §104.7 — `card`, `stack` and `action` in `kit.beck`, imported by `app.beck`. Point Tailwind
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

## 104.4 The design system is Tailwind's, the extraction is the compiler's

Split Tailwind in two and take one half. This is [`10`](10-decisions.md) **D29**, and the rest of
this section is its argument.

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
   whole difference between Tailwind and a language that absorbed it. **Built** — `B0222`, and
   §104.4b is what it took.
3. **The editor is free.** [`65`](65-the-editor-report.md) has completion, hover and rename served
   from `beck_core`. A checked class attribute makes `class="fl‸"` complete to `flex` and hover
   print the declarations — that is the Tailwind IntelliSense extension, without an extension,
   because the answers come from the compiler. **Built**, both of those; go-to-definition on a
   token waits on item 5's theme being a thing a program can land on.
4. **A class the compiler cannot enumerate is refused**, not silently dropped.
   `"text-" + kind + "-700"` is exactly as invisible to Beck as it is to Tailwind, and the
   difference is that Beck can *say so*. The shape a program should write instead is the shape
   Beck programs already write — `row_class` above is two constant alternatives behind an `if`,
   which constant-folds to a set of two. **And the shared half does not have to be repeated in
   both of them**: `class=["flex", "gap-2", row_class(r)]` is a *list*, the enumeration follows it
   through the join and the call and the branch, and the page is maintained by delta exactly as
   much as the version that writes both alternatives out —
   [`examples/todo.beck`](../compiler/examples/todo.beck) is written that way and
   `incremental.rs::a_join_of_a_fixed_list_is_pointwise_and_a_join_over_a_collection_is_not` holds
   both halves of it. That was not true when this section was written: the incrementality analysis
   blocked on the *name* `str_join` rather than on what it was applied to, so the list shape
   reported as a recompute while compiling to a byte-identical plan. An escape hatch
   (`@style(dynamic)`, one attribute, on the pattern of `@a11y(exempt, reason=…)`) covers the
   genuine cases and puts them in the audit trail rather than in a safelist.

**And the oracle is Tailwind itself, not a table somebody typed in.** Tailwind's compiler is a total
function from a candidate name to a rule or to nothing, which is precisely the predicate Beck needs:

| candidate | Tailwind emits | so Beck should |
|---|---|---|
| `flex`, `line-through`, `rounded-full` | a rule | accept |
| `p-2.5`, `p-[13px]`, `supports-[display:grid]:grid`, `dark:md:flex` | a rule | accept |
| `rounded-ful`, `bg-emerald-550`, `text-4xxl` | nothing | **refuse, with a suggestion** |

**Asked, and one row of that table was wrong in a way worth keeping.** Tailwind 4's spacing scale is
*multiplicative* — `calc(var(--spacing) * n)` — so `p-2.75` and `gap-13.5` are rules, and a table of
steps would have refused both. This section names `p-[13px]` as the arbitrary case and does not say
that the **numeric** scale is open as well; a person writing the table down would have got it wrong,
and asking the oracle is what caught it. Beck's spacing families take a number rather than a member
of a set for that reason.

**And the first attempt at the oracle was wrong, which is the other half of why this pattern
exists.** `compiler/style/generate.sh` originally asked `grep -F ".rounded-ful"` of Tailwind's
output and got a hit — from `.rounded-full`. So a misspelling came back as a utility, and a gate
built on it would have been green about the exact name §104.4 chose to illustrate the point. The
generator compares *selectors* now, as a set. A generator that reads its oracle wrongly is worse than
no oracle, because the answer looks authoritative.

That is [`clbg/`](../compiler/clbg/README.md)'s pattern — rebuild the asserted constants from
somebody else's published artefact so a wrong constant fails even against a matching wrong
expectation — applied to a utility table. **Built**, and the shape it took is worth two corrections
to this paragraph.

**And the oracle had to be asked a bigger question than "does this name exist".** A table that is a
*predicate* cannot emit a sheet: `is_utility("p-4")` says nothing about `padding`. So
`beck_core::style::rule` turns a name into declarations, `is_utility` is defined as "there is a rule
for it" — one table, so a predicate and a generator cannot disagree about a page — and
`generate.sh` records the at-rules, the selector and the declarations Tailwind emits for every
candidate. The gate compares them **byte for byte**: **3,474 of the 3,625 names Tailwind emits a
rule for are rendered identically here**, 35 names it refuses are refused, and the 151 that remain
are the families this table has not taken.

**Asking for the rule rather than the name caught four things a person writing it down would not
have.** Three are in the spacing scale, which is not the arithmetic it looks like: `1` is
`var(--spacing)` rather than `calc(var(--spacing) * 1)`, `0` is `0px` rather than `0`, and
`space-x-0` drops the reverse-margin `calc` entirely. The fourth is the escaping: `2xl:flex` is
`.\32 xl\:flex` — CSS's hex escape for a leading digit, terminated by a *space* that is part of the
escape — which the previous reader of the oracle stopped at, so every `2xl:` rule had been silently
absent from its answer.

**And seventeen unsound acceptances were sitting behind a list nobody had widened.** The table
accepted `size-screen`, `max-w-auto` and fifteen `-auto` paddings; Tailwind emits nothing for any of
them, so each would have gone into a stylesheet as a rule the browser finds nothing behind. They
survived every green run because `candidates.txt` had never been asked about them —
[`82`](82-the-edge-report.md) §82.10's pattern exactly. The cure is that the table now **enumerates
itself**: `beck_core::style::enumerate` lists the closed part of it, and
`style.rs::every_name_the_table_accepts_was_asked_about` fails when a name it accepts is not in the
list the oracle was run over.

The gate does **not** run Tailwind: it reads a committed answer. `compiler/style/candidates.txt` is
the list of names, `compiler/style/generate.sh` asks Tailwind about every one of them, and
`compiler/style/expected/tailwind-4.3.3.txt` is what it said. A gate that installed a package from a
registry would fail when somebody else's server did, which is not a property a compiler's test suite
should have — so the script is run by a person when the pinned version moves, exactly as `clbg/`
holds the Game's published output rather than re-running it.

And the gate has **three** buckets rather than two. *Unsound* — Beck accepts a name Tailwind refuses,
or renders it differently — is a page the browser reads differently from every other page on the web,
and is asserted at zero. *Wrongly refused* is the same error read the other way, also zero. The third
is a **gap**: a name Tailwind accepts that Beck's table does not know, which is not a failure because
the table is a documented subset. It is counted and printed — **3,474 of 3,625 today** — so that the
subset cannot quietly become the claim, and the candidate list is deliberately wider than the table
so the number means something.

## 104.4a The sheet, and what is in it

`beck build` writes `styles.css` and a running program serves the same bytes at `/beck.css`, derived
at startup from the program it is executing rather than read from disk — `/beck-bundle.bpk`'s rule,
for its reason. Four things are in it, in this order:

1. **A preflight**, nine rules, and each is there because a browser default fights a utility rather
   than because it is a taste: `p-0` cannot win against a `ul`'s padding, `flex` cannot lay out an
   `li` carrying a marker, and a `button` renders in the browser's font whatever `font-sans` says.
   It is **Beck's own and not Tailwind's**, which is four times the size and is part of the delivery
   mechanism this section refuses — an opinionated global sheet that arrives with the tool.
2. **The theme tokens the rules read, and only those.** A page using one colour defines one colour:
   the sketch's sheet defines six of the theme's 293. The values are Tailwind's, transcribed from
   its own output, because a ramp is not derivable from anything —
   `oklch(50.8% 0.118 165.612)` is a decade of somebody's taste written down. Item 5 is what makes
   this a Beck value a program can change.
3. **`@property` for the internals the rules read**, with the fallback Tailwind ships for browsers
   that do not register custom properties. Its condition is captured from the oracle rather than
   transcribed: a browser-detection expression is the kind of string nobody can check by reading it.
4. **One rule per class the program's pages can carry**, in name order — which puts `p-4` before
   `px-2` because `-` sorts before a letter, so a shorthand loses to the longhand after it.

**The sketch is the proof and it is 2.3 KB.** `examples/todo.beck` carries seventeen classes, every
one of them a utility, and the sheet defines seventeen rules and six tokens. Its `done` class — the
one name in it that was the program's own, and the reason `css.rs` existed — is `line-through` now,
which is the same look with nothing hard-coded.

**A class that is the program's own gets nothing**, and that is not an oversight: the compiler has
no rule for a name it did not define, and Beck still has no way for a program to write one (`css:`
has no parser, §104.1). `beck explain style` says which of a page's classes are which, so the
absence is visible rather than silent.

**The off switch is `AppConfig::styles`**, which is §104.4's `styles = none`:
[`08`](08-roadmap.md) §8.3 item 8 asks that a choice the compiler makes unbidden be switchable and
that the switched-off path be *run*, so `style.rs::the_stylesheet_has_an_off_switch_and_both_settings_run`
starts the runtime twice and reads what `/beck.css` would serve.

## 104.4b A misspelling, and the editor

`B0222` is §104.4's second consequence: a class that is not a utility **and is within one edit of
one** — two, from eight characters up — is a warning with a did-you-mean in it.

**It is a warning rather than a refusal, and that is the whole design.** `B0217` and `B0218` one
file over are errors because their vocabularies are *closed*: every attribute must be an HTML
attribute and every event must be one the client listens for, so an unknown one is wrong. A class is
not. `done`, `here`, `mine`, `theirs`, `column`, `columns`, `card` and `row-open` are names this
tree's own programs write, and a compiler that refused them would be refusing the escape hatch it
has not built yet — Beck still has no way to write a rule of your own (§104.1). So the strongest
thing that can honestly be said is "this is one slip from something that would have had a rule".

**The threshold has a margin, and the gate asserts the margin rather than the outcome.** Every
misspelling in `compiler/style/candidates.txt` is one edit from a real utility — `rounded-ful`,
`bg-emerald-550`, `text-4xxl`, `flexx`, `font-mediumm`, `justify-arround` — and `items-centre` is
two. Every class this tree's own programs write is **three or more** edits from anything in the
table: `card` to `grid`, `here` to `h-px`, `mine` to `inline`. So the rule sits one edit clear of
the population it must not touch, and `style.rs::a_misspelled_utility_is_a_diagnostic` asserts *that
distance* rather than asserting that nothing was said — the second passes on any threshold that
happens to be below it, and the first goes red when a family added to the table lands near a name
somebody chose, which is before their build starts warning about it rather than after.

A tie is broken towards the candidate of the same **length**, because a substitution is a likelier
slip than a deletion: `bg-emerald-550` is one edit from `bg-emerald-50` and from `bg-emerald-500`,
and only the second is a shade somebody meant.

### The editor, from the same table

`class="fl‸"` completes to `flex` and hovering `gap-2` prints `.gap-2 { gap: calc(var(--spacing) *
2); }` — the rule the sheet will actually carry, because it comes from the function that emits it.
The completion offers the closed part of the table plus the scale Tailwind's documentation lists,
which is what somebody typing `gap-` is looking for; the scale itself is open and `rule` accepts any
number whether or not it was offered.

**What holds it is a negative assertion, and getting that assertion right took two attempts.** A
class is a token inside a string, so the only thing between four thousand utility names and every
completion in the file is the test for where the caret is. The first version of that gate put a
caret in a string reading `"hello"` and asserted no completions came back — which passed for the
wrong reason, because no utility begins with `hel`. Pointed instead at a `placeholder="gap in the
diary"`, whose prefix matches seventy utilities, it went red immediately: `class=` was to the left
on the same line and the caret was inside *a* string, which was all the context test had been
checking. It now requires the text between `class=` and the caret's own string to be the value
itself — nothing, or a list's opening and the strings already in it — which is what tells
`class=["flex", "ga‸"]` from `class="flex", placeholder="ga‸"`.

## 104.5 Why Tailwind cannot be a dependency, and should still be the default look

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
the product.** [`10`](10-decisions.md) **D29** settles the distinction that follows:

- **On by default, and it is a default rather than a requirement.** The examples, the tutorial,
  `beck init` and the playground ship *styled*, using Tailwind's names, with the sheet emitted by
  `beck build` from the compiler's own table. A user who types `beck new` gets something that looks
  deliberate, with no Node anywhere.
- **`styles = none` turns all of it off.** No sheet emitted, no class checking, `class=` an
  unexamined `Str` again, and the program free to link its own stylesheet or carry a foreign design
  system. [`08`](08-roadmap.md) §8.3's rule is what makes this load-bearing rather than polite: a
  choice the system makes for you owes an off switch **proved rather than promised**, so the
  switched-off program is compiled and run by the same gate as the switched-on one. The switch is
  owed to two people in particular — the team arriving with a design system they are not going to
  abandon, and whoever finds a defect in the emitter at four in the morning.
- **Off by default as a dependency, and never unavailable.** Tailwind-the-npm-package stays a
  supported, documented, one-command upgrade for anyone who wants the complete utility surface or
  arbitrary plugins — and the same exact extraction feeds it, since `beck build` can emit the class
  list as an artefact the scanner reads instead of guessing at source. That is strictly better input
  than Tailwind gets from any other language, which is a pleasant way for the two positions to
  agree.

The cost of that position is honest and should be stated: the compiler carries a **subset** table,
it will lag upstream, and the gap has to be visible (`beck explain style` printing "this name is
Tailwind's and not in Beck's table" rather than a bare refusal). The paragraph below is how the
subset stops being a permanent apology.

**And the port is smaller than it sounds.** Of the three native binaries above, two are Rust already:
Tailwind's *scanner* (Oxide) and `lightningcss`, which is on crates.io — at `1.0.0-alpha.72`, which
[`07`](07-dependencies.md)'s standard would want an argument for. Tailwind's *utility compiler* is
the JavaScript half (`dist/*.mjs`). Beck does not need the scanner — that is the half §104.4
replaces — and it does not need a CSS parser to *emit* CSS, so the only thing that has to cross is
the utility table, which is data. Generated, and held against upstream by the gate above, that is a
wave's maintenance rather than a fork.

## 104.6 Syntactic sugar, mostly refused

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
| **A `css:` macro** for what utilities cannot say — keyframes, `@container`, complex selectors | **Later, and now genuinely cheap.** It is a typed literal macro ([`02`](02-syntax.md) §2.5), which used to mean it waited on the macro interpreter; the interpreter is **built** ([`102`](102-the-macro-interpreter-report.md)) and [`08`](08-roadmap.md) §8.5.4 lists typed literal macros among what it unblocked and *free of Lane A*. So the cost argument for deferring it is gone and only the need argument is left, which still holds: nothing in the styling half wants it, and §104.4 works without it |
| **A `component` keyword** | **No.** §104.7 measures what functions already do, and the answer is everything |

## 104.7 A component library needs no language feature — measured

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
checked — and §104.9 is the defect that makes it not quite true yet.

## 104.8 The three walls

### Wall 1 — interface state had no home, so opening a dropdown was a log entry — **DOWN** ([`10`](10-decisions.md) D30)

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
page, does not run — and `B0519` now says so directly rather than reporting the program as a
library. And placing the durable one where the state belongs is refused, correctly:

```
error[B0401]: `ui_state` is placed on `client`, which cannot discharge `durable`
   = note: `durable` is the log: placing it on the client would ship the database to the browser
```

So until D30 the only expressible answer was that **opening a modal is a `Command`, validated by
`validate`, recorded as an `Event`, and folded into the durable log forever** — replicated to the
data tier, replayed on every genesis replay, and included in the state digest. A date picker paging
to next year was twelve permanent log entries. That is not a performance nit; it is the semantics
being wrong about what happened. Nobody *decided* it, which is what made it a wall and not a
trade-off.

**It is decided now.** The survey below produced the five homes, and
[`10`](10-decisions.md) D30 adopts them with the fourth built as `gestures(step, init)`. The rest of
this section is the reasoning that got there, kept because the *order* of the homes is the part a
reader needs and the survey is what justifies it.

#### What other systems do, and what they agree on

Surveyed August 2026, because the question is old enough everywhere else to have an answer:

| System | The homes it offers | What it says |
|---|---|---|
| **Redux / React** | store, or local component state | [organizing state](https://redux.js.org/faq/organizing-state): "The classic example is tracking an `isDropdownOpen` flag. In most situations, the rest of the app doesn't care about this, so in most cases it should stay in component state" |
| **Remix** | the URL, then cookies, then client state | [state management](https://v2.remix.run/docs/discussion/state-management): "read and set the state in the URL directly with boring old HTML forms" |
| **SwiftUI** | `@State`, `@SceneStorage`, `@AppStorage` | three lifetimes, chosen **at the declaration** — transient, scene-restored, app-persisted |
| **Akka** | `Behaviors`, or `EventSourcedBehavior` | the same fold either way; journalling is opt-in per behaviour |
| **Phoenix LiveView** | per-connection `assigns` | the closest architecture to Beck's, and its known weakness is the gap itself: LiveView cannot tell which assigns are transient and which are permanent |

Two things they agree on, and neither is one of the three candidates below.

**The lifetime is a declaration, not an inference.** Every system that has more than one home makes
the author name which one, at the point of declaration. Beck already does this for the one
distinction it has — `durable(fold(…))` against `fold(…)` — which is why D1's construct is the right
shape and not merely an available one.

**The assignment is by audience, not by mechanism.** "Does anybody else need to see this, and should
it be here tomorrow" decides the home; the storage follows. That is what makes Redux's rule about
`isDropdownOpen` and Remix's rule about the URL the *same* rule stated twice.

#### The five homes, and the rule this document recommends

Not three candidates but five homes with an order of preference, which is the form the survey says
the answer takes. **This is [`10`](10-decisions.md) D30**, which adopts the order and builds the
fourth home; it needed a D-number rather than a recommendation because a reader of a Beck program
can observe which home a piece of state is in.

1. **The platform, first.** §104.9: `<dialog>`, `popover`, `<details name>` and command invokers hold
   a large fraction of this state if asked in markup, and then it is nobody's state at all — no
   command, no event, no fold, nothing to place. A modal's open flag is this row.
2. **The URL**, for anything that deserves a bookmark — a table's sort, a filter, a selected tab.
   [`94`](94-the-client-report.md) §94.3 already makes a route a field of `Session` rather than a
   router, so this home exists today and is unused for anything but routing.
3. **Awareness**, for ephemera a *second person* must see: a cursor, a selection, a typing
   indicator. This is the home a search for counter-examples turned up, and it is not a fold —
   [Yjs](https://docs.yjs.dev/getting-started/adding-awareness), the canonical collaborative stack,
   keeps it in a protocol of its own precisely because "awareness information isn't stored in the
   Yjs document, as it doesn't need to be persisted across sessions", and its shape is a
   `Map<client, state>` that is broadcast and deleted on disconnect. It was nine-tenths built
   before anything was written for it, because `presence()` is that map with no payload, and
   **`awareness(f)` now exists** for the half of it a session can answer — see below for what that
   half is and what the other half still waits on. It is what D1's "cursors" always wanted.
4. **A client-placed non-durable fold**, for what is left after those three: ephemera that *nobody
   else* sees — a combobox's highlighted option, a tooltip's target. **`gestures(step, init)` now
   exists** for this. It was thought to need a client-local *stream*, and that was the thing
   blocking it: the only stream is `merge_clients()` and it is server-placed by §3.5. The
   construct takes no stream at all — a gesture stream has exactly one consumer by construction, so
   naming it buys a declaration and no expressiveness, and `awareness(f)` had already set that
   precedent. `compiler/examples/interface.beck` is the program and
   `beck-cli/tests/gestures.rs` is the gate.
5. **The durable log**, only for what a second person *and* a later day should see. Which is where
   all of it goes today.

The order is the decision. Its value is that the first three are **free or built** — one is markup,
one is a field that already exists, and the third is `awareness(f)` — so the expensive home is
needed for far less than it looks.

**The client-local fold was the only home that had to be built, and nothing found needs a
server-side one.** A
search for counter-examples returned exactly one server-side ephemeral need — awareness, above — and
its shape is a keyed map of each client's latest value, not an accumulation over occurrences. The
other candidates were already answered: rate counters are §82.5's deliberately *sharded* table,
sessions and presence are connections, and a cache "does not exist as a concept" because the
incrementally-maintained views are the cache ([`15`](15-scale-and-distribution.md)). So D1's
sentence named the right problem and the wrong mechanism, and D30's correction is that
**ephemerality comes from the stream and the audience, never from the absence of a `durable`
wrapper** — which is why `B0519` remains an error for a bare `fold` over the log's stream, and why
the new construct is a primitive rather than a permission.

#### What awareness is, and the half of it that is built

**The construct**, which exists. Awareness is a signal operation and not a command: nothing about a
cursor is proposed, validated or recorded.

```python
reading: Signal[Map[Str, Str]] = awareness(whereabouts)
```

`f` produces this client's contribution and the roster is everybody's latest, keyed by actor.
`compiler/corpus/33-awareness.beck` is the program, `beck-cli/tests/awareness.rs` is the gate, and
`beck_rt::awareness` is the registry that applies `f` to each connection's `Session` and publishes
what comes back.

`f` is a function of a `Session` rather than a signal, and that signature is the whole design:
**the subscribers are the runtime's fact and not the graph's**, so a program cannot name another
connection's session — it can only say what it would like to know about one. Nothing in the signal
graph could have expressed the roster; the runtime holds it, one row per socket, and hands it to the
view as a source beside `presence`.

It inherits `presence()`'s three rules unchanged, which is why it was built there:

- it is a **non-log input to a view**, so everything below it runs per subscriber — the shared
  dataflow is versioned by the log's `seq` and this moves when the log does not;
- it is **capacity-bounded** (§82.5's table-bounds-the-attacker finding applies verbatim, since the
  key is again a name the client chooses) — *and* bounded a second time, per contribution, which
  presence needs no equivalent of: a roster of counts costs its capacity, and a roster of values
  costs its capacity times whatever the program's `f` returns;
- it **may not reach the chokepoint** (`B0520`) — an event whose existence depended on where
  somebody's cursor was would not survive a replay — and it **may not render in the browser**
  (`B0521`), because a client handed the accumulator holds nothing this could be rendered from.

**And it splits in two, which was the finding and still is.** What `f` may read decides how much of
it can be built:

- **`f : Session -> T` is built, and needed no wire change at all.** The server already holds every
  subscriber's `Session` — the route arrives on `hello` and on every `Nav` — so it computes each
  client's contribution itself and never asks. *Who is looking at what* is the most-wanted awareness
  feature after presence itself, and it cost a source, a role and an aggregation.
- **`f` over a client-local value — a cursor, a selection — is not**, and not for a protocol reason:
  the client has nothing to derive one *from*. `beck-patch.js` listens for five events and
  `mousemove` is not among them (§104.8's Wall 2), so there is no client-local value in the language
  to publish. Arbitrary awareness therefore has the **same prerequisite as the client-local fold**,
  and the two are one piece of work rather than two.

So the remaining order is: the client-local value next, which lets `awareness` take a client-placed
signal and gives the full [Yjs](https://docs.yjs.dev/getting-started/adding-awareness) shape; and
the client-local fold falls out of the same work.

#### What the client-local fold costs to build, which is why this needs a D-number

D1's non-durable fold is not blocked on plumbing.
`DEFECTS.md::non-durable-fold` has the finding: an accumulator outside the log is **not a function
of the log**, `beck-cli/tests/replay.rs` asserts `digest(replayed) == digest(live)`, and
[`10`](10-decisions.md) D3 rests on that digest. So the construct needs an answer to *what the state
digest covers* before it needs any code.

And the volume half of D1's own motivation is untouched by it: §3.7 logs **every validated event**,
so a cursor that moves a hundred times a second writes a hundred log entries whether or not the
accumulator they feed is durable. An un-journalled accumulator is not an un-journalled stream, and
D1's examples — presence, cursors — want the second.

**Scoped after awareness shipped, and it splits the same way**, which is the finding rather than a
coincidence: both are a client-held value that a *rendered page* has to see, so both are decided by
where the page renders.

- **What is needed is a stream, and the type is what would route it.** Today the only stream is
  `merge_clients()` and §3.5 places it on the server, so every `on_click` in the language becomes a
  proposal. The shape that fits is a **second, client-placed source over a second union** — a `Ui`
  where `Command` is the one the chokepoint sees — so a variant's *type* decides whether an
  interaction becomes a log entry or stays in the tab. Nothing about it is an annotation, and
  `merge_clients()` stays the sole chokepoint because a `Ui` value is not a `Command` and can never
  reach `validate`.
- **In Mode B it needs no wire at all.** The browser already holds the accumulator, runs the fold and
  renders the page (`beck-wasm`'s kernel), so a second accumulator folded from a client-local stream
  and passed to the view is entirely inside the tab. Nothing is logged, nothing is replayed, nothing
  enters the digest, and `DEFECTS.md::non-durable-fold`'s question — *what does the state digest
  cover* — is not asked, because this accumulator is not a projection of the log in the first place.
  `B0519` would narrow from "a non-durable fold is not built" to "a non-durable fold whose stream is
  not client-local".
- **In Mode A it cannot work without one, and choosing that wire is the D-number.** A Mode A page is
  rendered where the state is, so a value the *browser* holds reaches it only by being sent — and
  once sent it is a per-connection accumulator the **server** folds. That is a real option and not a
  defeat: it is presence's shape rather than the log's, so it is bounded per connection, dropped on
  disconnect, and outside the digest exactly as the roster is. But it is a second kind of state in
  the runtime and a second thing an operator has to reason about, and it is the sentence D1 does not
  contain.

So the decision this needs is narrow and nameable: **does a client-local fold exist only where the
client renders, or does Mode A get a per-connection accumulator on the server to make it work
there too?** The first is buildable now and refuses `@render(server)` with a diagnostic; the second
is the more useful feature and the larger claim.

### Wall 2 — the event vocabulary is five, and an unknown one is silent

`beck-patch.js` listens for exactly five things: `click`, `keydown` filtered to Enter, `submit`,
`input` and `change`. The W3C's ARIA Authoring Practices keyboard tables — the specification any
serious component is held to — want arrows, `Home`, `End`, `Escape`, `Space`, `PageUp`/`PageDown`
and typeahead, plus `focus` and `blur`.

That gap is expected for a young client. What was not expected is that **the compiler did not know
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

Two dead attributes, shipped to the browser on every render, wired to nothing. `beck check` was
happy, `beck test` was happy, and the snapshot — the gate [`22`](22-phase-3-report.md) §22.10 added
precisely because "`contains` asserts one string somebody thought to name and a snapshot asserts
every attribute" — had pinned the defect as the expected output.

The same hole swallows attribute names, and the spelling it swallows is one a reader will arrive
with: [`01`](01-vision-and-premise.md) §1.3 writes `cls=`, faithfully, because the original sketch
did — that section is the seed translated construct-for-construct and is right to keep it. What
happens to somebody who copies it is measured:

```html
<li cls="done" data-b-k="1">
```

`cls` is not an HTML attribute. The page lost its styling, the browser ignored it, and every gate
stayed green. **The `ui:` macro had no vocabulary**, so a typo in a handler name or an attribute
name was not a compile error, not a lint, and not visible in a snapshot review — it was a page that
quietly did less than it said.

That was [`82`](82-the-edge-report.md) §82.10's pattern again — a gate that cannot fail, written by
people who knew what they meant.

**Fixed.** `beck_macro::vocabulary` is the table: five events, the HTML and SVG attribute names, and
the elements §12.4's checks will read. `B0217` refuses an event the client does not listen for and
`B0218` an attribute HTML does not have, with **`data_…` and `aria_…` admitted by prefix** — the
escape hatch for an attribute that is genuinely yours is HTML's own, so there was none to invent.

Three things about the fix are worth more than the table.

- **It is a table in a crate, not a check in the expander.** `ui:` is a compiler-provided special
  case standing in for a user-written macro, and typed macros retire it
  ([`10`](10-decisions.md) D22); a vocabulary buried in today's expander would be written twice, and
  the second copy is the one that drifts. §12.4's three accessibility checks — since built — read this module rather
  than a list of their own, which is what makes them a day's work rather than a table's.
- **The events are asserted to be the client's, not declared to be.** They are written in different
  languages in different crates, so `client.rs::the_event_vocabulary_is_what_the_client_listens_for`
  reads `beck-patch.js`'s own `on(…, "data-b-…")` registrations and compares the two sets **in both
  directions**. An event the client drops is a handler that compiles and does nothing — this defect
  arriving from the other side.
- **The suggestion is a rule, not a list.** `ui:` turns `_` into `-`, so a program that writes
  `max_length=` reaches HTML as `max-length` and the attribute is `maxlength` — squashing the
  hyphens out and looking again catches every attribute of that shape at once. `cls` is the one that
  needs an alias, because it is *one* edit from `cols` and two from `class`, so a distance search
  confidently says the wrong thing to exactly the reader §1.3 sent.

What is **not** refused is an unknown *element*, and that is a limit of today's surface rather than
a judgement: inside a `ui:` block a lowercase call whose arguments are all keyword arguments is
indistinguishable from an element, so refusing one would refuse a user's own helper. It is where
typed macros make a difference rather than a table.

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

## 104.9 What the platform already owns

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

**One defect blocked the chart of §104.7 outright, and is now fixed.** `beck-patch.js` built
patched-in subtrees with `document.createElement(tag)` — no namespace. Measured in the same
Chromium, on the same tree the patch carries:

| path | namespace | `instanceof SVGElement` | rendered width of a `rect` |
|---|---|---|---|
| server-side render (the HTML parser) | `…/2000/svg` | true | **50** |
| a patch (`createElement`) | `…/1999/xhtml` | false | **0** |

So an SVG chart painted on first load and **vanished the first time it changed** — which is every
time the data it is a function of changes, which is the only reason to have drawn it. The client
now builds with `createElementNS`, taking the namespace from the tag when the tag opens one
(`svg`) and from **the destination** otherwise, with `foreignObject` handing it back to HTML.

The second half of that sentence is the whole of the fix's difficulty, and the gate is built around
it: `browser.rs::a_patched_in_chart_is_still_a_chart` drives two patches — one that replaces a
paragraph with the whole `svg`, and one that adds a bar to a chart already in the document — and
asserts the **laid-out width** of every `rect` rather than its namespace. A fix that reads the tag
and ignores the destination passes the first and measures `30,0` on the second. `examples/chart.beck`
is what it runs, and it is the first program in the tree whose page is an SVG.

## 104.10 Which components Beck should own, and why some of them it should own better

The user-facing list, with what each actually needs:

| Component | Where the state is | Verdict |
|---|---|---|
| **Data table** — sort, filter, paginate, group | the query | **The flagship.** Sorting and filtering a table is a *view*, and keeping views incremental is what [`23`](23-incremental-views-report.md) built. Everyone else re-sorts an array in the browser on every keystroke; Beck maintains a dataflow and sends a patch. This is the component where the language wins on the merits rather than on ergonomics |
| **Charts** | pure function of data | **Library, no walls** — proved in §104.7, blocked only by the namespace defect. Answers [`09`](09-risks-and-open-questions.md) §9.6 item 8 |
| **Modal / dialog** | the platform | Markup. `<dialog>` + invokers, no application state (§104.9) |
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
accessibility checks scheduled there — alt text, accessible name, input label — are the same
mechanism Wall 2 needs, over the same typed tree, and **are built** (`B0219`–`B0221`); the kit's
components inherit them rather than restating them, and what the kit adds is the APG's *keyboard*
table, which is a runtime property and not a shape.

## 104.11 What to build, in order

**These eight are scheduled**, in [`08`](08-roadmap.md) §8.5.4, which is the only place in `docs/`
that holds an order — with a class and a lane each, and each named there rather than gestured at.
The list is not repeated here, because two documents holding one order is how an order stops being
followed. What this section owes it is the *reason for the order*, which is not visible from a
schedule:

- **The first two were defects, not features** — the SVG namespace and the `ui:` vocabulary. They
  came first because each made something already claimed untrue rather than something desired
  absent, and because neither was expensive. **Both are done**: §104.9 has the namespace fix and its
  gate, and Wall 2 above has the vocabulary's. Neither cost more than a day, which is the argument
  for the ordering rather than a remark about it.
- **The vocabulary was a `G`** in §8.5.4's classification: §12.4's three accessibility checks are
  scheduled over the same typed tree, and a tree that swallowed `on_keydown` and `cls=` in silence
  could not honestly carry an accessibility claim. **Done, and so are the three it gated**: they
  took a `NAMING` table beside `ELEMENTS` and a day, which is the ordering paying for itself twice —
  and on their first run they refused every example program in this tree with a text input, each of
  which had labelled it with a placeholder and nothing else.
- **`class=` as a list is the `F`.** Everything in the styling half is behind it, and so is the
  editor affordance that costs nothing once it exists. **The surface and the analysis are built**:
  a list where HTML defines a space-separated value is joined in the `ui:` lowering, so every
  backend agrees by construction and nothing at the seam had to learn about it, and
  [`style.rs`](../compiler/crates/beck-core/src/style.rs) enumerates every class that can reach a
  `class=` — through a call and through both arms of an `if`, which is the shape every dynamic class
  in this tree is already written in. `beck explain style` prints the set and, beside it, the sites
  where a class is *built* rather than named, with the reason. The type is still owed and has moved:
  a `Class` has something to be checked against now that the table exists, so it lands with the
  emitter rather than in front of it.
- **Item 4 is built, both halves.** The table is held against Tailwind's own compiler as its oracle
  (§104.4), and the emitter with it: `beck build` writes `styles.css`, `/beck.css` serves the same
  sheet from the running program, `AppConfig::styles` is the `styles = none` switch and both of its
  settings are run by a gate. `beck-rt/src/css.rs` is deleted. What the second half changed about
  the first is the finding: a table that only said *which names are utilities* could not be the
  thing a sheet is emitted from, so `is_utility` is now defined as "there is a rule for it" and the
  oracle records what Tailwind **emits** rather than whether it emits. Asking that question found
  seventeen unsound acceptances the previous gate could not see. Its last two consequences landed
  after it: **`B0222`** warns about a class one edit from a utility, and the **editor** completes
  and explains one from the same table (§104.4b). The `Class` **type** is not built and is no
  longer owed — what it would have checked is what `B0222` checks.
- **The last three are behind a decision, not behind effort.** Where interface state lives (Wall 1)
  determines what a combobox, a menu and a custom date picker even look like, so building the kit
  before that decision would be building the part of it that does not depend on the answer and
  discovering which part that was afterwards.

Two things are deliberately *not* on the list. **Whether the kit is a `lib/` directory or a tarn**
is a decision to take after items 3–5, because the argument for either is mostly about how the
utility table ships. And **`css:`** — the macro for keyframes, `@container` and complex selectors —
which is off the list for a *changed* reason and worth saying so. It used to be behind the macro
interpreter, and the argument was that unblocking the largest fan-out item in the plan for the
smallest of its successors was the wrong trade. The interpreter has since been **built**
([`102`](102-the-macro-interpreter-report.md)), and §8.5.4 lists typed literal macros among what it
freed and outside Lane A — so that argument is spent. What is left is the weaker and sufficient one:
nothing in items 1–8 needs `css:`, so it is a follow-on rather than a prerequisite, and it should be
written when a program wants a keyframe rather than because it is now affordable.

## 104.12 What this does not establish

- **This document established nothing built**, and four of its eight items have since been. Every
  measurement in it is of the tree as it stands or of third-party tools run against it; §104.11
  records which items have landed and [`08`](08-roadmap.md) §8.5.4 holds the order.
- **`Class` is not a type.** §104.7's `def card_box() -> Class` is what the component half assumes
  and it does not compile: a class is a `Str` today. What that type was scheduled *for* — checking a
  name against the table — is built as a check instead (§104.4b), so nothing is waiting on it; what
  it would still buy is a signature that says what a function returns, which is a readability
  argument rather than a correctness one.
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
  the browser. §104.10 calls the data table a flagship on a *design* argument;
  [`25`](25-benchmarks-and-expressiveness.md) §25.9's rule is what would have to be satisfied before
  anybody says it is faster than anything.
