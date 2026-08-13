# 110 — The editor edits

**Built.** `beck lsp` answers five more requests, and two of them change the file: **references**,
**document highlight**, **prepare-rename**, **rename**, and **inlay hints**.
[`65`](65-lsp-report.md) §65.5 listed rename among the things it had not built and called inlay
hints "the interesting one for Beck specifically: an inferred *tier* and an inferred *effect row*
are exactly what an inlay hint is for, and hover is the placeholder". Both of those are here, and
[`08`](08-roadmap.md) §8.5.2's list of what the LSP is for — "completion, hover with *inferred
placement*, go-to-def, rename, inline diagnostics" — now has no unbuilt entry.

Every answer is in [`beck_core::editor`](../compiler/crates/beck-core/src/editor.rs) rather than in
the server, for [`04`](04-compiler-architecture.md) §4.6's reason and
[`103`](103-playground-phase-3-report.md)'s: a browser tab asks the same questions, and a second
implementation is a second thing to be wrong. `beck lsp` translates JSON-RPC and nothing else.

The feature that took the work is **rename**, and none of it was the editing. A rename is an
assertion that a set of byte ranges is *every* place a name is written — and this compiler had four
separate reasons why the obvious ways to compute that set are wrong. §110.1 is the rule that
survived all four; §110.2 is what it declines and why declining is the point; §110.4 is what
happened when it was run over every name in [`corpus/`](../compiler/corpus/), which is where two of
those reasons came from and where the safety net fired.

**316 of the corpus's 325 names rename; nine decline.** Eight decline because the name is also a
model field, and the ninth because the edit was made and the file no longer type-checked — which is
the last refusal of §110.2 doing the job the other eight never get to.

---

## 110.1 A name has two accounts, and an edit needs both to agree

There are two ways to ask where a name is written, and each is complete in the way the other is not.

The **lexical** account is [`editor::tokens`](../compiler/crates/beck-core/src/editor.rs) — every
identifier-shaped run in the text that reads as that word. It saw the whole file, so it misses
nothing; it resolved nothing, so a parameter that shares the name is in it, and so is the `page` in
`expect page contains "1"`, which is the test grammar's word rather than a reference to anything.

The **semantic** account is the checked program — a `CoreKind::Global` node per reference, so a
shadowing local is *not* in it and an imported name is. It knows what each reference means and it is
not complete: a reference the checker rewrote has a span that is no longer an identifier.

Neither is a set of ranges you may edit. What is safe is the pair **agreeing**:

> every semantic reference begins on a lexical identifier that reads the name, and the only lexical
> identifier left over is the declaration's own.

A file where that holds has no shadow, no unspanned mention and no rewritten reference. Where it
does not hold, [`Editor::occurrences`](../compiler/crates/beck-core/src/editor.rs) answers `None`
and both callers decline. The edits are then the **lexical** ranges, never the semantic spans, for a
reason worth stating because it is the first thing that went wrong here: a `Global` node that is
*called* carries the span of the **call**, so `double(x)` is one node spanning the parentheses and
their contents. Editing that span would have replaced a call with a name. The semantic account says
a reference begins at an offset; which token that is, is the lexical account's answer.

Three things had to be true before this rule could see the file it was reading:

- **A test block's clauses are a grammar, and its words are identifiers to the lexer.** `expect page
  contains "1"`, `when session("ana") sends Add(…)`, `expect state == fold_of […]`. Nothing inside a
  clause is edited on a lexical match and nothing inside one refuses the rename either; a clause's
  actual *expressions* are in the semantic account like any others and are edited from there.
- **`page` is a keyword.** It is in [`lexer::KEYWORDS`](../compiler/crates/beck-syntax/src/lexer.rs) because
  [`21`](21-tests-in-beck-and-proof.md) §21.2's `expect page …` reads it as syntax — and it is also
  the name of a signal in nearly every program in the corpus. Reading only "name" tokens meant the
  most common name in the language had *no* occurrences at all and every question about it was
  declined. The lexical account therefore counts a keyword run as a written name; what separates the
  two uses is not the token, it is whether it sits inside a clause.
- **`expect place(page) == client` names a definition and kept no position for it.**
  §21.2's static assertions are answered from the placement table without running anything, so the
  name in one is a reference the checker resolves and keeps no `Core` node for. It has a span now
  ([`testing::Expectation::Place`](../compiler/crates/beck-core/src/testing.rs)), which is three
  lines and the difference between renaming a signal and declining to.

## 110.2 The refusals are the feature, and the last one is a compile

A rename that quietly changes what a program means is worse than a rename that does not happen, so
this declines for seven stated reasons rather than guessing at any of them: the file does not
compile, there is no name under the caret, the name is declared in another module, the new name is
not one the lexer would read, the new name is already written in this file, the two accounts
disagree — and two that are only known *after* the edit is made.

Three of those are worth their own paragraph.

**What a name is, is the lexer's answer and not a rule written here.** A new name is lexed, and it
is a name if that produces exactly one identifier token reading exactly those characters. So the
Unicode profile [`44`](44-wave-0-report.md) §44.5 adopted — confusables, bidirectional controls, the
UTS #39 identifier set — governs what an editor will rename *to*, without this module knowing that
any of it exists.

**The new name may not appear anywhere in the file, in any form.** `occurrences` would catch a
collision with another top-level name and could not catch one with a local, because a `let` keeps no
name past the checker. The question asked instead is the lexical one, which needs no resolution to
answer: if the word is written in this file at all, the rename declines.

**And then the edit is made and the result is analysed.** This is the check that turns the reasoning
above into a fact about the text: the proposed file is put through the same front end, and a rename
whose result does not compile is not offered. It costs a second compile of one file — §110.5 —
on a keystroke nobody presses twice a minute.

Compiling is not sufficient on its own, which is the second post-hoc refusal. A module with no merge
point is a **library** rather than an error ([`27`](27-the-walls-come-down-report.md) §27.2 is why,
and it is right), so a rename that cost a program its page or its fold would pass a
"does it still compile" check while quietly demoting an application to a module that no longer runs.
The application-or-library verdict is compared across the edit, and a rename that changes it is
refused.

## 110.3 The two inferred halves of a signature, shown where they would be written

A Beck signature has exactly two parts nobody writes: **where it runs**, which §3.4 makes a solved
constraint rather than an annotation, and **what it performs**, which §3.6 infers and only a module
boundary demands. Those are the hints, and there are no others.

| | label | where | when |
|---|---|---|---|
| tier | `@on(server)` | the start of the declaration | the source did not write one, and the tier is not `any` |
| row | ` uses net.out(x)` | the colon that ends the signature | the source did not declare a row, and the row is not empty |

Every label is what an author could paste in **at the offset it carries** — which is asserted rather
than claimed: the harness inserts each hint at its own position and re-analyses the file. That is
what makes a hint worth showing rather than decorative, and it forced two decisions.

The row is rendered by [`iface::render_uses`](../compiler/crates/beck-core/src/iface.rs), extracted
from `render_item` so that a hint and a published signature cannot spell a row differently. §4.6
forbids a second renderer and this is where one would have appeared, reading plausibly and
suggesting a clause that does not parse.

The colon is found by scanning the declaration's tokens for the first one at **bracket depth zero**.
A signature contains colons of its own, one per parameter, so the first colon is `x: Int`'s and an
offset there would write the clause into the middle of a parameter list. The first attempt looked
*backwards* from the body's span instead and was wrong for a duller reason: a body's first
expression starts after the `return` that introduces it, so there is no whitespace-only gap to
recognise the signature's colon by.

**`@on(any)` is never hinted.** §3.3 calls that tier *unplaced* — pure code, compiled to every tier
that needs it — so it is the absence of a placement rather than one, and a library whose every
helper carried `@on(any)` would be a file of hints saying nothing.

**A definition that already carries `@on(...)` is not hinted about it**, and making that true
required a fact the compiler had and threw away. `Def::tier_is_annotated` does not mean "somebody
wrote this": [`project::link`](../compiler/crates/beck-core/src/project.rs) sets it on **every**
definition it links, because an imported placement is part of a published signature and the root's
solve must not move it. In a linked program — which is what an editor holds — the flag is true of
everything. The checker's own answer is now kept beside it as `Def::tier_is_written`, set once and
never overwritten. Until it existed, every definition in every file was hinted with the annotation
it already had.

## 110.4 What the corpus said

[`corpus/`](../compiler/corpus/) is 34 programs written to test placement inference, with folds,
signals, views, tests and static expectations in them, and none of them written with an editor in
mind. Every name in every one of them was renamed, the result re-analysed, and its published
interface compared to the original's with that one name substituted — because a rename that dropped
an occurrence would still compile whenever the name it left behind resolved to something else.

| | |
|---|---|
| names renamed, verified compiling **and** publishing the same interface | **316** |
| declined because the two accounts disagree | **8** |
| declined because the edit was made and would not compile | **1** |

The eight are one shape: **a signal whose name is also a model field**. `corpus/10-cart.beck`
declares `carts: Map[Str, Map[Str, Int]]` inside `model State` and a signal `carts` outside it, and
`s.carts` is a field access rather than a reference to the signal. The lexical account sees the
word, the semantic account has no reference there, and the rename declines rather than renaming a
field somebody did not ask about.

The ninth is `corpus/21-two-folds.beck`, and it is the one this suite most wants to see. `tally` is
one of two folds, so its name is also **the field it occupies in the fused accumulator**, and the
test asserts `expect state.tally.joins == 2`. The rename was accounted for, made, and the edited
file did not type-check — `no field or function 'tally' for '$State'`. The verification step of
§110.2 caught it, which is the difference between a refusal and a corrupted file. It is asserted as
a refusal that *must keep happening*: a corpus that stopped triggering it would be a corpus that
stopped testing the net.

## 110.5 What it costs

`cargo test --release --test measure_compile -- --nocapture`, in-process, median of five, after one
untimed analysis — a cold first call parses the standard library too, and reporting that as the cost
of an editor's answer would overstate it by an order of magnitude:

| | lines | analyse | hints | rename |
|---|---|---|---|---|
| `corpus/01-counter.beck` | 59 | 1.02 ms | **0.056 ms** (4 hints) | **1.12 ms** |
| `awfy/cd.beck` — the largest real file | 914 | 16.84 ms | **2.10 ms** (no hints) | **19.03 ms** |

Two sizes rather than one, because one measurement cannot tell a linear cost from a quadratic one.
Neither answer is on the keystroke path: hints are asked for once per viewport and a rename once per
rename.

**A rename is about one more analysis, not two.** The file being edited is already analysed — that
is what an `Editor` is — so what a rename adds is the verification pass of §110.2: the proposed
text, put through the front end once. The promise that a rename which would not compile is not
offered costs exactly that, and it is the one number here worth knowing, because it is the price of
the whole argument.

`cd.beck` produces **no hints**, which is not a failure to measure: it is a benchmark of pure
arithmetic, so every definition is unplaced and performs nothing, and §110.3's rule is that neither
of those is worth a label. What its 2.10 ms measures is the token scan finding that out.

**Hinting was quadratic when it was first written**, and the shape gate is in
`compile_speed.rs::the_front_end_cost_per_declaration_does_not_grow_with_a_module`. Finding the
colon that ends a signature meant filtering the file's whole token stream, once per definition —
`definitions × tokens`, which reads as a small constant at every size anybody checks by hand. It is
[`64`](64-compile-speed-report.md) §64.2's defect in a different file: a per-item pass that walks the
module. The tokens are in source order, so the fix is a binary search to the declaration's first
token and a scan that stops at the colon, and the gate measures **×1.42 per definition across
sixteen times as many** where the bound is 4.0.

That gate is a function called by the existing test rather than a test of its own, and writing it as
a test first demonstrated why: two timing tests in one binary run concurrently, and the axis next to
it went from ×2.73 to ×4.18 and failed a gate it has nothing to do with. The module's own docs had
already recorded that trap, from the first draft of the test it happened to.

## 110.6 What this corrects

- **[`65`](65-lsp-report.md) §65.5's "rename, references … not built" and "semantic tokens, inlay
  hints: not built" are both out of date.** Semantic tokens arrived with
  [`103`](103-playground-phase-3-report.md); references, highlight, rename and inlay hints are this
  report. Formatting and code actions from that row are still not built — §110.7.
- **[`08`](08-roadmap.md) §8.5.5's Lane C row is empty.** Its items were the recursion bound
  ([`44`](44-wave-0-report.md)), the two syntax decisions ([`10`](10-decisions.md) D21/D22), Unicode
  and UTS #39 ([`44`](44-wave-0-report.md) §44.5), `test --update`
  ([`66`](66-page-snapshots-report.md)), fuzzing ([`85`](85-what-the-generator-found-report.md)) and
  the LSP. The lane has nothing left in it.
- **`Def::tier_is_annotated` did not mean what its name says**, and one of its two meanings had no
  field. §110.3 has the split; nothing about placement changed, because the solver reads the
  original flag and `link` still sets it.
- **`Expectation::Place` kept no span for the name it names**, so nothing downstream could point at
  it. It has one.

## 110.7 What is **not** built

| | |
|---|---|
| Renaming a name that is also a field | **not built**, and refused rather than attempted — §110.4's eight. The field and the signal are different things with one spelling, and telling them apart at a use is a question about types rather than about names |
| Renaming across modules | **not built.** An editor here analyses one file, so a name *declared* elsewhere is refused by name (`Refusal::Imported`) and a name *used* elsewhere is not seen. Cross-module analysis is [`65`](65-lsp-report.md) §65.5's row and is still that row |
| Formatting | **not built, and now with a reason rather than a gap.** `beck fmt` exists and wiring it to `textDocument/formatting` would be four lines — but the lexer *skips* ordinary comments, so a formatted file loses every `#` line in it ([`syntax::print`](../compiler/crates/beck-syntax/src/print.rs) says so). A formatter an editor runs on save must not delete what somebody wrote; the missing piece is comment-preserving printing, not the wiring |
| Code actions | **not built**, and the first one is obvious now: a hint from §110.3 is an edit somebody would accept, and `textDocument/codeAction` is how it would be offered |
| The playground | **not wired.** These answers are in `beck_core::editor` where [`103`](103-playground-phase-3-report.md)'s module can reach them, and it does not ask for them yet |
| Incremental analysis | **not built**, per [`65`](65-lsp-report.md) §65.4. A rename now costs two whole-file analyses rather than one, which moves that ceiling nearer without changing where it is |
