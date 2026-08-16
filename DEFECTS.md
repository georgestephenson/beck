# Defects

**What is wrong right now.** [`CHANGELOG.md`](CHANGELOG.md) is what has been fixed; this is what has
not. An entry is **deleted by the change that fixes it**, in the same commit, and the CHANGELOG
bullet for that change is where it goes on record. So this file is always the current list and never
a history — git holds the history, as it does for everything else in this repository
([`AGENTS.md`](AGENTS.md)).

**What belongs here: something that behaves wrongly.** Silent, misleading, or contrary to what a
document says. **What does not: something that is merely absent.** A feature nobody has built is a
line in [`docs/08`](docs/08-roadmap.md) §8.5, which is the only place that holds an order; putting
absences here would turn the register into a second roadmap that disagrees with the first.

**Every entry names the gate a fix owes.** This project has repeatedly shipped fixes behind gates
that could not have failed ([`docs/82`](docs/82-the-edge-report.md) §82.10), and the cure is to write
down *what would have to go red* while the defect is still in front of you. A fix that lands without
its gate has not been fixed; it has been made invisible.

**Ids are slugs, not numbers, and are never reused.** Entries are deleted, so a number would imply a
sequence that does not survive.

This register was opened alongside [`docs/104`](docs/104-styling-and-the-component-library.md) and is
**seeded rather than complete**: it holds what that audit found plus one older defect already
recorded in a report. Anything you find that meets the admission rule above belongs here, whether or
not you are the one to fix it.

---

## `ui-vocabulary` — `ui:` checks no attribute or event name

**What is wrong.** The `ui:` macro turns any `name=value` into an attribute and any `on_x=` into
`data-b-x`, with no vocabulary of its own. It does not know which attributes HTML has, which events
the client listens for, or that `class` is special. A misspelling is not a compile error, not a lint,
and not visible in a snapshot review: it is a page that quietly does less than it says.

**Measured.** `span(on_keydown=…, on_mouseenter=…)` gives `ok: 9 definitions, 4 signals` from
`beck check`, passes `beck test`, and ships `data-b-keydown` and `data-b-mouseenter` to the browser,
where `beck-rt/client/beck-patch.js` listens for five events and neither is one of them. The page
snapshot — the gate added precisely because it "asserts every attribute"
([`docs/22`](docs/22-phase-3-report.md) §22.10) — records both dead attributes as the expected
output. The same hole passes `cls="done"`, which is the spelling
[`docs/01`](docs/01-vision-and-premise.md) §1.3's sketch uses, and the browser ignores it.

**The gate a fix owes.** A refused-program test per case — an unknown event name, an unknown
attribute name, `cls` specifically — and a test that the escape hatch for a genuine custom attribute
still compiles. Check that each would have gone red *before* the fix.

**Fix it as a table rather than as expander code.** `ui:` is a compiler-provided special case
standing in for a user-written macro, and typed macros retire it
([`docs/08`](docs/08-roadmap.md) §8.5.4, [`docs/10`](docs/10-decisions.md) D22) — so a vocabulary
buried in today's expander would be written twice, and the second copy is the one that would drift.

**Where it is argued.** [`docs/104`](docs/104-styling-and-the-component-library.md) §104.8, Wall 2.
Scheduled in [`docs/08`](docs/08-roadmap.md) §8.5.4 as a **G** item, because
[`docs/12`](docs/12-standards-and-conformance.md) §12.4's three accessibility checks are already
scheduled over the same tree and are dishonest until this exists.

---

## `svg-namespace` — a patched-in SVG subtree is built in the wrong namespace

**What is wrong.** `beck-rt/client/beck-patch.js:10` builds patched-in subtrees with
`document.createElement(tag)` and no namespace. Server-side rendering goes through the browser's
HTML parser and is therefore correct; the patch path is not. So an SVG chart paints on first load
and **does not render after the first patch that changes it** — which is every time the data it is a
function of changes.

**Measured.** In Chromium 141.0.7390.37, on the tree shape the patch carries:

| path | namespace | `instanceof SVGElement` | rendered width of a `rect` |
|---|---|---|---|
| server-side render | `…/2000/svg` | true | **50** |
| a patch (`createElement`) | `…/1999/xhtml` | false | **0** |

**The gate a fix owes.** A browser test (`beck-cli/tests/browser.rs`) that patches an `svg` into a
page and asserts the `rect`'s bounding box is non-zero. Asserting the namespace alone is weaker and
would pass on a fix that got the namespace right and the inheritance wrong; assert the box.

**Where it is argued.** [`docs/104`](docs/104-styling-and-the-component-library.md) §104.9. It blocks
charts outright, which is why it is first in [`docs/08`](docs/08-roadmap.md) §8.5.4's styling and
components cluster.

---

## `non-durable-fold` — a decided construct is unbuilt, and the failure is silent

**What is wrong.** [`docs/10`](docs/10-decisions.md) D1 provides for non-durable folds — "high-churn
ephemera get non-durable folds — same semantics, no log persistence" — and
[`docs/15`](docs/15-scale-and-distribution.md) assigns hot ephemeral state to them. A `fold` that is
not wrapped in `durable` does not make a signal graph, does not get a page, and does not run. The
compiler does not say the construct is unimplemented; it reports the program as something else
entirely.

**Measured.** A program whose only state is `ui_state: Signal[State] = fold(apply_event, …, events)`
gives `ok: 4 definitions — a library: no durable state, so there is nothing to run`. The consequence
is that interface state — a modal's open flag, a date picker's visible month — has nowhere to live
but the durable log, so opening a dropdown is a permanent, replicated, replayed event.

**The gate a fix owes.** Two, and the second is the one that will be forgotten: a program with a
non-durable fold runs and its page reflects it, **and** the fold's state does not appear in the log
after a restart. A fix that only satisfies the first has built a durable fold with a different
spelling.

**Where it is argued.** [`docs/104`](docs/104-styling-and-the-component-library.md) §104.8, Wall 1.
The *language* question of where interface state should live is a decision rather than a defect and
is scheduled separately in [`docs/08`](docs/08-roadmap.md) §8.5.4; this entry is only that a decided
construct silently is not there.

---

## `fmt-comments` — `beck fmt` deletes `#` comments

**What is wrong.** The lexer skips ordinary comments, so a formatted file loses every `#` line in it.
A doc comment (`##`) survives and an ordinary one does not, which is a distinction the generated-docs
feature wants and a formatter must not act on. This is why the language server deliberately does not
offer `textDocument/formatting` ([`docs/02`](docs/02-syntax.md) §2.2,
[`docs/65`](docs/65-the-editor-report.md)): a formatter an editor runs on save must not delete what
somebody wrote.

**Measured.** Recorded in [`docs/65`](docs/65-the-editor-report.md)'s table of what the editor does
not do. Not re-measured for this register.

**The gate a fix owes.** A round-trip test that a file with comments in every position the grammar
allows — before a definition, at the end of a line, inside a block, between arms — is byte-identical
after `beck fmt`, and the LSP formatting request enabled in the same change so the fix has a caller.

**Where it is argued.** [`docs/65`](docs/65-the-editor-report.md), and it is Lane C's in
[`docs/08`](docs/08-roadmap.md) §8.5.5.

---

## `libm-determinism` — `sin` and `cos` are the host's, so a fold replays differently on another machine

**What is wrong.** [`docs/10`](docs/10-decisions.md) D3 rests the whole data tier on the log folding
to one state, and [`docs/03`](docs/03-type-and-effect-system.md) §3.7 enforces the purity that is
supposed to make it true. Purity is not enough for transcendentals: all three backends call out to
whatever libm the host supplies, and **IEEE 754 does not require `sin` or `cos` to be correctly
rounded**. Implementations differ in the last ulp between libms and between versions of one libm, so
two machines can fold one log to two different states. `sqrt` is not affected — IEEE 754 does require
it correctly rounded.

The replay-determinism harness cannot see this, and that is the part worth stating: it replays a log
**on one machine**, where the answer is stable. The three-way differential has the same blind spot —
the evaluator, LLVM and Cranelift agree because all three resolve to the same host libm, not because
they compute the same function.

**Measured.** Read from the tree, not benchmarked. The evaluator calls Rust's `f64::sin`
([`interp.rs`](compiler/crates/beck-eval/src/interp.rs), `Prim::Sin => Value::float(f.sin())`); the
LLVM emitter emits `llvm.sin.f64` ([`emit.rs`](compiler/crates/beck-llvm/src/emit.rs)); the Cranelift
emitter calls the extern symbol `sin`
([`emit.rs`](compiler/crates/beck-clif/src/emit.rs)). Three paths, one host libm, no vendored
implementation anywhere in the workspace.

**The gate a fix owes.** The existing three-way differential run against **two different libms** —
a CI matrix row rather than a new harness — over a program whose result depends on `sin` and `cos`.
That is the gate that would have caught this, and a fix gated only by the current single-host
differential would be [`docs/82`](docs/82-the-edge-report.md) §82.10's pattern again: a gate that
cannot fail. A vendored correctly-rounded implementation behind the three `Prim`s is the fix that
makes it pass.

**Where it is argued.** [`docs/08`](docs/08-roadmap.md) §8.5.2 as retrofit item F9 and §8.5.4 as its
first item. It was recorded as "owed rather than pending" for three phases with no position, which is
the failure §8.5 opens by describing.

---

## `cost-report-undercount` — `beck explain cost` prints an `O(n)` operator and excludes it from its own tally

**What is wrong.** `cost_report`'s summary counts operators whose cost string contains
`n entries copied`, and the capture line — the one that says a per-element function captured a plan
node — is emitted *after* the count. So a program whose loop body captured the accumulator is told
"1 of 29 operators cost `O(n)` per event" when two do. The headline number is wrong on exactly the
programs where the cost matters most, and it is wrong in the reassuring direction.

The capture line also names the node it captured rather than what *moves* that node, so a captured
`const` (never moves), a captured `session` (per route change) and a captured **state** (every event)
are printed identically. Telling them apart means tracing inputs transitively back to the
accumulator, which the line does not do and a reader has no reason to do by hand — and one of the
three real cases in the corpus is two hops from `#0`.

**Measured.** `beck explain cost corpus/27-review.beck` reports `1 of 29 operators`, quoted in full
in [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.3, which also sweeps the corpus for
the pattern: **18 capture sites in 10 programs**, of which 3 capture the state.

**The gate a fix owes.** A test that the tally equals the number of `O(n)`-per-event operators the
report itself prints, on a program that captures the accumulator — so the count and the body cannot
disagree again — plus a classification test that a captured `const`, a captured `session` and a
captured state print differently, since the fix is worthless if the reader still cannot tell which
one they have.

**Where it is argued.** [`docs/99`](docs/99-the-data-tier-means-of-combination.md) §99.3, and it is
item 2 of §99.9's order of work — deliberately before any operator, because it is the instrument
every later item is read through.
