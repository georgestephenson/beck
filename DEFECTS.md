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
