# 65 — Phase 3, part 34: `beck lsp`, and the line where re-checking the file stops working

**Built.** [`compiler/crates/beck-cli/src/lsp.rs`](../compiler/crates/beck-cli/src/lsp.rs) —
diagnostics as you type, hover, go-to-definition and document symbols, over the Language Server
Protocol, from the same front end `beck check` runs.

One of the nine Phase 3 bullets [`23`](23-incremental-views-report.md) §23.19 lists as untouched.
It is here now because [`64`](64-compile-speed-report.md) made the case for the design: §64.6
measured the front end's parse-expand-check prefix at 4.7 ms on the largest file in the tree, which
is what decides whether a language server needs incremental analysis or can re-check the buffer.

## 65.1 The claim it has to keep, and how that is tested

[`04`](04-compiler-architecture.md) §4.6:

> **One binary** serves `beck build`, `beck check`, `beck lsp` and `beck explain`; there is no
> separate language server implementation to drift.

The way that claim decays is not that somebody writes a second typechecker. It is smaller and much
likelier: the server formats its own message, or renders a signature its own way, or answers from a
copy of the file on disk. Each of those is a drift nobody notices until an editor and a CI run
disagree.

So the harness does not assert against strings written in the harness. It asserts against **the
library**:

| The server's answer | is compared to |
|---|---|
| a published diagnostic | `beck_core::compile_or_library_str`'s own `code`, `severity` and `message` |
| a hover | `iface::render_item` — the function `beck iface` writes `.becki` with |
| document symbols | `Interface::of(…).items`, the names the module publishes |

`render_item` was private and is now `pub` for exactly that reason: hover shows what `beck iface`
publishes because it *calls* what `beck iface` calls. A signature rendered a second way here would
be a second implementation of the one thing §4.6 names.

The transport is tested through a **subprocess**, because framing, flushing and not writing anything
else to stdout are properties of a process and a test calling `serve()` directly would assert none
of them. Nine tests in [`lsp.rs`](../compiler/crates/beck-cli/tests/lsp.rs).

## 65.2 What it does

`initialize`, `initialized`, `shutdown`, `exit`; `didOpen`, `didChange`, `didSave`, `didClose`;
`hover`, `definition`, `documentSymbol`. Sync is **Full** — the whole buffer arrives on every change
— which is what a server that re-checks the whole file wants anyway.

Three details are worth naming because each is a bug this does not have:

- **Positions are UTF-16.** The protocol's default, and `SourceMap::line_col` is the wrong unit
  twice over — it counts characters and is one-based. Getting this wrong is invisible until somebody
  puts an emoji in a string literal, which `beck-syntax`'s own security tests say they will, so the
  conversion is written out and unit-tested in both directions.
- **A hover finds the name the caret is *after*.** An editor sends the caret's position, and a caret
  at the end of `total` is one byte past the `l`. A server that only looked under the caret would
  answer nothing for the commonest way of asking.
- **A diagnostic carries its notes and its fix.** A `B0350` that says only "cannot find `foo`" is a
  worse diagnostic in an editor than in a terminal, because the editor drops everything the terminal
  printed underneath — including the suggested annotation §3.4 insists on. They travel in the
  message.

The protocol is spoken directly rather than through a framework;
[`adr/0016`](adr/0016-the-language-server-speaks-json-rpc-directly.md) records that decision, what
it refused, and the three things that would reverse it.

## 65.3 What it costs, measured end to end

Through the real server, over the real protocol: a `didChange` with the whole buffer, timed until
`publishDiagnostics` comes back. Twenty edits per file, release build, median and worst.

| | lines | median | worst |
|---|---|---|---|
| `corpus/01-counter.beck` | 59 | **0.84 ms** | 1.52 ms |
| `examples/todo.beck` | 178 | **1.96 ms** | 2.14 ms |
| `sicp/ch2.beck` | 442 | **4.63 ms** | 4.98 ms |
| `awfy/cd.beck` — the largest real file | 914 | **7.37 ms** | 9.49 ms |

That is the whole round trip, not a phase of it: framing, JSON, parse, expand, check, place, the
security pass, and the reply. Every Beck file that exists is answered in under 10 ms.

## 65.4 Where it stops working, stated as a number rather than as a worry

Re-checking the buffer is `O(file)`, so the budget is crossed at a file size rather than never. The
same measurement, on generated modules:

| lines | median | |
|---|---|---|
| 9,599 | 57 ms | |
| **12,899** | **88 ms** | **§4.6's 100 ms budget is met, and only just** |
| 19,199 | 152 ms | over |
| 38,399 | 343 ms | well over |

**The 100 ms target holds to about 13,000 lines in one module and not beyond.** §4.6's target is
worded for a *50 kLOC project*, and it also says how it is meant to be reached — "editing a function
body invalidates `typecheck_body` and `core` for that item and nothing upstream", which is the Salsa
query graph nothing has built. A 50 kLOC project of 13,000-line modules is answered inside the
budget today; a 50 kLOC *module* is not, and would not be by anything short of the incremental
front end §4.6 describes.

Two things make that a plan rather than a hole. §3.6's module firewall is what makes per-item
invalidation possible at all, and it is already load-bearing elsewhere. And
[`64`](64-compile-speed-report.md) §64.2 has just removed a quadratic from the same path — the
numbers above are the post-fix ones, and the pre-fix compiler crossed 100 ms at roughly 3,000 lines
rather than 13,000.

## 65.5 What is **not** built

| | Status |
|---|---|
| Completion | **not built.** The largest single thing a user would notice next |
| Incremental sync, and incremental analysis | **not built**, per §65.4. The first is easy and pointless without the second |
| Rename, references, code actions, formatting | **not built.** `beck fmt` exists and is not wired to `textDocument/formatting` |
| Semantic tokens, inlay hints | **not built.** Inlay hints are the interesting one for Beck specifically: an inferred *tier* and an inferred *effect row* are exactly what an inlay hint is for, and hover is the placeholder |
| Cross-module analysis | **not built.** A file is analysed alone. `beck_core::project` compiles a multi-module project and the server does not use it, so a name imported from a sibling is not resolved and `definition` will not leave the file |
| `beck explain` in the editor | **not built**, and it is the one that would be most Beck-specific — §4.7's placement explanation is a code lens waiting to happen |
| Anything about a workspace | **not built.** No `workspace/*`, no configuration, no file watching |

## 65.6 What this corrects

- **[`23`](23-incremental-views-report.md) §23.19's "no LSP" is no longer true.** That sentence
  named nine untouched bullets in one breath; `Result`/error rows went in
  [`27`](27-the-walls-come-down-report.md) and the standard library across
  [`46`](46-standard-library-report.md)–[`56`](56-decimal-report.md), and this is the third of them
  to go.
- **[`04`](04-compiler-architecture.md) §4.6's keystroke→diagnostics target has a measurement**, and
  §65.4 says where it holds. The claim "there is no separate language server implementation to
  drift" now has a harness that would fail if one appeared.
- **`iface::render_item` is public.** It was an implementation detail of `beck iface` and is now the
  published signature renderer, which is what §4.6 requires of it.
