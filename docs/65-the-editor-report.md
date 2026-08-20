# 65 — The editor

**Built, and [`08`](08-roadmap.md)'s LSP row has no unbuilt entry.** `beck lsp` speaks the Language
Server Protocol directly: diagnostics as you type, hover with **inferred placement**,
go-to-definition, document symbols, completion, semantic tokens, references, document highlight,
rename and inlay hints. Every answer lives in `beck_core::editor` rather than in the server, because
[`98`](98-playground-report.md)'s browser tab asks the same questions and **a second implementation
is a second thing to be wrong**; `beck lsp` translates JSON-RPC and nothing else.

Two things are worth the chapter.

**§65.4 is where re-checking the buffer stops working, stated as a number rather than as a worry.**
The 100 ms budget holds to about 13,000 lines in one module and not beyond, which is a fact about a
design decision rather than a defect — and it is the measurement that decided the design in the
first place.

**§65.6 is rename, and none of the work was the editing.** A rename is an assertion that a set of
byte ranges is *every* place a name is written, and this compiler had four separate reasons why the
obvious ways to compute that set are wrong. The rule that survived all four needs **two accounts of
a name to agree**, and its refusals are the feature: 366 of the corpus's 376 names rename, ten
decline, and the tenth declines because the edit was made and the file no longer type-checked.

---

## 65.1 The claim it has to keep, and how that is tested

[`04`](04-compiler-architecture.md) §4.6:

> **One binary** serves `beck build`, `beck check`, `beck lsp` and `beck explain`; there is no
> separate language server implementation to drift.

**The way that claim decays is not that somebody writes a second typechecker.** It is smaller and
much likelier: the server formats its own message, or renders a signature its own way, or answers
from a copy of the file on disk. Each of those is a drift nobody notices until an editor and a CI
run disagree.

So the harness does not assert against strings written in the harness. It asserts against **the
library**:

| The server's answer | is compared to |
|---|---|
| a published diagnostic | the compiler's own `code`, `severity` and `message` |
| a hover | `iface::render_item` — the function `beck iface` writes `.becki` with |
| document symbols | the names the module publishes |
| a semantic token, a completion | `beck_core::editor`'s, over the same source the playground asks about ([`98`](98-playground-report.md) §98.4) |

`render_item` was private and is now public for exactly that reason: **hover shows what `beck iface`
publishes because it *calls* what `beck iface` calls.** A signature rendered a second way here would
be a second implementation of the one thing §4.6 names.

The transport is tested through a **subprocess**, because framing, flushing and not writing anything
else to stdout are properties of a process and a test calling the server directly would assert none
of them. The protocol is spoken directly rather than through a framework;
[`adr/0016`](adr/0016-the-language-server-speaks-json-rpc-directly.md) records that decision, what it
refused, and the three things that would reverse it.

## 65.2 What it does, and three bugs it does not have

`initialize`, `initialized`, `shutdown`, `exit`; `didOpen`, `didChange`, `didSave`, `didClose`;
`hover`, `definition`, `documentSymbol`, `completion`, `semanticTokens/full`, `references`,
`documentHighlight`, `prepareRename`, `rename`, `inlayHint`. Sync is **Full** — the whole buffer
arrives on every change — which is what a server that re-checks the whole file wants anyway.

- **Positions are UTF-16.** The protocol's default, and the compiler's own line/column is the wrong
  unit twice over: it counts characters and is one-based. **Getting this wrong is invisible until
  somebody puts an emoji in a string literal**, which `beck-syntax`'s own security tests say they
  will, so the conversion is written out and unit-tested in both directions.
- **A hover finds the name the caret is *after*.** An editor sends the caret's position, and a caret
  at the end of `total` is one byte past the `l`. A server that only looked under the caret would
  answer nothing for the commonest way of asking.
- **A diagnostic carries its notes and its fix.** A "cannot find `foo`" that says only that is a
  worse diagnostic in an editor than in a terminal, because the editor drops everything the terminal
  printed underneath — including the suggested annotation §3.4 insists on. They travel in the
  message.

## 65.3 What it costs, measured end to end

Through the real server, over the real protocol: a `didChange` with the whole buffer, timed until
the diagnostics come back. Twenty edits per file, release build.

| | lines | median | worst |
|---|---|---|---|
| `corpus/01-counter.beck` | 59 | **0.84 ms** | 1.52 ms |
| `examples/todo.beck` | 178 | **1.96 ms** | 2.14 ms |
| `sicp/ch2.beck` | 442 | **4.63 ms** | 4.98 ms |
| `awfy/cd.beck` — the largest real file | 914 | **7.37 ms** | 9.49 ms |

That is the whole round trip, not a phase of it: framing, JSON, parse, expand, check, place, the
security pass, and the reply. **Every Beck file that exists is answered in under 10 ms.**

## 65.4 Where it stops working, stated as a number rather than as a worry

Re-checking the buffer is `O(file)`, so **the budget is crossed at a file size rather than never**:

| lines | median | |
|---|---|---|
| 9,599 | 57 ms | |
| **12,899** | **88 ms** | **§4.6's 100 ms budget is met, and only just** |
| 19,199 | 152 ms | over |
| 38,399 | 343 ms | well over |

**The 100 ms target holds to about 13,000 lines in one module and not beyond.** §4.6's target is
worded for a *50 kLOC project*, and it also says how it is meant to be reached — "editing a function
body invalidates `typecheck_body` and `core` for that item and nothing upstream", which is the Salsa
query graph nothing has built. **A 50 kLOC project of 13,000-line modules is answered inside the
budget today; a 50 kLOC *module* is not**, and would not be by anything short of the incremental
front end §4.6 describes.

Two things make that a plan rather than a hole. §3.6's module firewall is what makes per-item
invalidation possible at all, and it is already load-bearing elsewhere. And
[`64`](64-compile-speed-report.md) §64.2 removed a quadratic from the same path — the numbers above
are the post-fix ones, and **the pre-fix compiler crossed 100 ms at roughly 3,000 lines rather than
13,000.**

This is also why the server exists in the shape it does. [`64`](64-compile-speed-report.md) §64.6
measured the front end's parse-expand-check prefix at 4.7 ms on the largest file in the tree, **which
is what decides whether a language server needs incremental analysis or can re-check the buffer.**

## 65.5 The two inferred halves of a signature, shown where they would be written

A Beck signature has exactly two parts nobody writes: **where it runs**, which §3.4 makes a solved
constraint rather than an annotation, and **what it performs**, which §3.6 infers and only a module
boundary demands. Those are the inlay hints, and there are no others.

| | label | where | when |
|---|---|---|---|
| tier | `@on(server)` | the start of the declaration | the source did not write one, and the tier is not `any` |
| row | ` uses net.out(x)` | the colon that ends the signature | the source did not declare a row, and the row is not empty |

**Every label is what an author could paste in at the offset it carries** — which is asserted rather
than claimed: the harness inserts each hint at its own position and re-analyses the file. That is
what makes a hint worth showing rather than decorative, and it forced two decisions.

The row is rendered by the same function that renders a published signature, extracted from
`render_item` so that **a hint and a `.becki` cannot spell a row differently**. §4.6 forbids a second
renderer and this is where one would have appeared, reading plausibly and suggesting a clause that
does not parse.

The colon is found by scanning the declaration's tokens for the first one at **bracket depth zero**.
A signature contains colons of its own, one per parameter, so the first colon is `x: Int`'s and an
offset there would write the clause into the middle of a parameter list. The first attempt looked
*backwards* from the body's span instead and was wrong for a duller reason: **a body's first
expression starts after the `return` that introduces it**, so there is no whitespace-only gap to
recognise the signature's colon by.

**`@on(any)` is never hinted.** §3.3 calls that tier *unplaced* — pure code, compiled to every tier
that needs it — so it is the absence of a placement rather than one, and a library whose every helper
carried `@on(any)` would be a file of hints saying nothing.

**And a definition that already carries `@on(...)` is not hinted about it**, which required a fact
the compiler had and threw away. `Def::tier_is_annotated` does not mean "somebody wrote this":
`project::link` sets it on **every** definition it links, because an imported placement is part of a
published signature and the root's solve must not move it. In a linked program — which is what an
editor holds — **the flag is true of everything**. The checker's own answer is kept beside it now,
set once and never overwritten. Until it existed, every definition in every file was hinted with the
annotation it already had.

## 65.6 A name has two accounts, and an edit needs both to agree

There are two ways to ask where a name is written, **and each is complete in the way the other is
not.**

The **lexical** account is every identifier-shaped run in the text that reads as that word. It saw
the whole file, so it misses nothing; it resolved nothing, so a parameter that shares the name is in
it, and so is the `page` in `expect page contains "1"`, which is the test grammar's word rather than
a reference to anything.

The **semantic** account is the checked program — one node per reference, so a shadowing local is
*not* in it and an imported name is. It knows what each reference means and **it is not complete**: a
reference the checker rewrote has a span that is no longer an identifier.

Neither is a set of ranges you may edit. What is safe is the pair **agreeing**:

> every semantic reference begins on a lexical identifier that reads the name, and the only lexical
> identifier left over is the declaration's own.

A file where that holds has no shadow, no unspanned mention and no rewritten reference. Where it does
not hold, both callers decline. The edits are then the **lexical** ranges, never the semantic spans,
for a reason worth stating because it is the first thing that went wrong: **a node that is *called*
carries the span of the call**, so `double(x)` is one node spanning the parentheses and their
contents — editing that span would have replaced a call with a name. The semantic account says a
reference begins at an offset; **which token that is, is the lexical account's answer.**

Three things had to be true before the rule could see the file it was reading:

- **A test block's clauses are a grammar, and its words are identifiers to the lexer.** Nothing
  inside a clause is edited on a lexical match and nothing inside one refuses the rename either; a
  clause's actual *expressions* are in the semantic account like any others.
- **`page` is a keyword** — because §21.2's `expect page …` reads it as syntax — **and it is also the
  name of a signal in nearly every program in the corpus.** Reading only "name" tokens meant the most
  common name in the language had *no* occurrences at all and every question about it was declined.
  The lexical account counts a keyword run as a written name; what separates the two uses is not the
  token, it is whether it sits inside a clause.
- **`expect place(page) == client` names a definition and kept no position for it.** §21.2's static
  assertions are answered from the placement table without running anything, so the name in one is a
  reference the checker resolves and keeps no node for. It has a span now, which is three lines and
  the difference between renaming a signal and declining to.

### The refusals are the feature, and the last one is a compile

**A rename that quietly changes what a program means is worse than a rename that does not happen**,
so this declines for seven stated reasons rather than guessing at any of them: the file does not
compile, there is no name under the caret, the name is declared in another module, the new name is
not one the lexer would read, the new name is already written in this file, the two accounts
disagree — and two that are only known *after* the edit is made.

**What a name is, is the lexer's answer and not a rule written here.** A new name is lexed, and it is
a name if that produces exactly one identifier token reading exactly those characters. So the Unicode
profile [`44`](44-wave-0-report.md) §44.5 adopted — confusables, bidirectional controls, the UTS #39
identifier set — **governs what an editor will rename *to*, without this module knowing that any of
it exists.**

**The new name may not appear anywhere in the file, in any form.** The occurrence rule would catch a
collision with another top-level name and could not catch one with a local, because a `let` keeps no
name past the checker. The question asked instead is the lexical one, which needs no resolution to
answer.

**And then the edit is made and the result is analysed.** This is the check that turns the reasoning
above into a fact about the text: the proposed file goes through the same front end, and **a rename
whose result does not compile is not offered.** Compiling is not sufficient on its own, which is the
second post-hoc refusal: a module with no merge point is a **library** rather than an error, so a
rename that cost a program its page or its fold would pass a "does it still compile" check while
quietly demoting an application to a module that no longer runs. The application-or-library verdict
is compared across the edit.

### What the corpus said

Every name in every corpus program was renamed, the result re-analysed, and its published interface
compared to the original's with that one name substituted — **because a rename that dropped an
occurrence would still compile whenever the name it left behind resolved to something else.**

| | |
|---|---|
| names renamed, verified compiling **and** publishing the same interface | **366** |
| declined because the two accounts disagree | **9** |
| declined because the edit was made and would not compile | **1** |

The nine are one shape: **a signal whose name is also a model field.** `s.carts` is a field access
rather than a reference to the signal, so the lexical account sees the word, the semantic account has
no reference there, and the rename declines rather than renaming a field somebody did not ask about.

**The ninth is the one this suite most wants to see.** `tally` is one of two folds, so its name is
also the field it occupies in the *fused* accumulator ([`23`](23-incremental-views-report.md) §23.3),
and a test asserts `expect state.tally.joins == 2`. The rename was accounted for, made, and the
edited file did not type-check. The verification step caught it, **which is the difference between a
refusal and a corrupted file** — and it is asserted as a refusal that *must keep happening*, because
a corpus that stopped triggering it would be a corpus that stopped testing the net.

## 65.7 What rename and hints cost

Median of five, after one untimed analysis — **a cold first call parses the standard library too, and
reporting that as the cost of an editor's answer would overstate it by an order of magnitude:**

| | lines | analyse | hints | rename |
|---|---|---|---|---|
| `corpus/01-counter.beck` | 59 | 1.02 ms | **0.056 ms** (4 hints) | **1.12 ms** |
| `awfy/cd.beck` — the largest real file | 914 | 16.84 ms | **2.10 ms** (no hints) | **19.03 ms** |

Two sizes rather than one, because one measurement cannot tell a linear cost from a quadratic one.
Neither answer is on the keystroke path: hints are asked for once per viewport and a rename once per
rename.

**A rename is about one more analysis, not two.** The file being edited is already analysed, so what
a rename adds is the verification pass: the proposed text, put through the front end once. **The
promise that a rename which would not compile is not offered costs exactly that**, and it is the one
number here worth knowing, because it is the price of the whole argument.

`cd.beck` produces **no hints**, which is not a failure to measure: it is a benchmark of pure
arithmetic, so every definition is unplaced and performs nothing, and §65.5's rule is that neither of
those is worth a label. What its 2.10 ms measures is the token scan finding that out.

**Hinting was quadratic when it was first written.** Finding the colon that ends a signature meant
filtering the file's whole token stream, once per definition — `definitions × tokens`, **which reads
as a small constant at every size anybody checks by hand.** It is [`64`](64-compile-speed-report.md)
§64.2's defect in a different file: a per-item pass that walks the module. The tokens are in source
order, so the fix is a binary search to the declaration's first token and a scan that stops at the
colon, and the shape gate measures **×1.42 per definition across sixteen times as many** where the
bound is 4.0.

That gate is a function called by an existing test rather than a test of its own, **and writing it as
a test first demonstrated why**: two timing tests in one binary run concurrently, and the axis next to
it went from ×2.73 to ×4.18 and failed a gate it has nothing to do with. The module's own
documentation had already recorded that trap, from the first draft of the test it happened to.

## 65.8 What is not built

| | Status |
|---|---|
| **Incremental sync, and incremental analysis** | **Not built**, per §65.4. The first is easy and pointless without the second, and a rename now costs two whole-file analyses rather than one — which moves that ceiling nearer without changing where it is |
| **Cross-module analysis** | **Not built.** A file is analysed alone, so a name imported from a sibling resolves against the standard library ([`46`](46-standard-library-report.md) §46.12) and against nothing else, `definition` will not leave the file, a name *declared* elsewhere is refused by name, and a name *used* elsewhere is not seen. There is no directory: a language server resolving a relative path off a URI is a decision nobody has taken |
| **Formatting** | **Built**, and the wiring was the small half. The lexer skipped ordinary comments, so a formatted file lost every `#` line in it — **a formatter an editor runs on save must not delete what somebody wrote**, which is why this was withheld rather than missing. Comments are now collected by position in the pass that already collected documentation (`beck_syntax::doc`) and printed back in the three positions they hold, and `roundtrip.rs` asserts over the tree that **1,850 of them** survive a format. The request answers with one edit covering the document, an empty list when there is nothing to do, and `null` for a file that does not parse |
| **Code actions** | **Not built**, and the first one is obvious now: a hint from §65.5 is an edit somebody would accept, and `textDocument/codeAction` is how it would be offered |
| Renaming a name that is also a field | **Refused rather than attempted** — §65.6's eight. The field and the signal are different things with one spelling, and telling them apart at a use is a question about types rather than about names |
| `beck explain` in the editor | **Not built**, and it is the one that would be most Beck-specific — §4.7's placement explanation is a code lens waiting to happen |
| Anything about a workspace | **Not built.** No `workspace/*`, no configuration, no file watching |

### What this corrects, elsewhere

- **[`04`](04-compiler-architecture.md) §4.6's keystroke→diagnostics target has a measurement**, and
  §65.4 says where it holds. The claim "there is no separate language server implementation to
  drift" now has a harness that would fail if one appeared — and since
  [`98`](98-playground-report.md) §98.4 the answers themselves live in `beck_core::editor`, so the
  claim covers a browser tab as well.
- **`iface::render_item` and `render_uses` are public.** Both were implementation details of `beck
  iface` and are now the published signature renderers, which is what §4.6 requires of them.
- **`Def::tier_is_annotated` did not mean what its name says**, and one of its two meanings had no
  field (§65.5). Nothing about placement changed, because the solver reads the original flag.
- **`Expectation::Place` kept no span for the name it names** (§65.6), so nothing downstream could
  point at it. It has one.
- **[`08`](08-roadmap.md) §8.5.5's Lane C is empty.** Its items were the recursion bound, the two
  syntax decisions, Unicode and UTS #39, `test --update`, fuzzing and the LSP.
