# 31 — Generated documentation

**Built.** Doc comments in the language, `beck doc` for a module, a language reference derived from
the compiler's own tables, a drift gate that fails when the two disagree, and a published site.

This report is history, like the phase reports it sits beside: it says what was built, what was
measured, and what is claimed — and it names what it does not do first, not last.

---

## 31.1 What this closes

[`16-packages-and-ecosystem.md`](16-packages-and-ecosystem.md) §16.2 promises the docs.rs model:
"documentation generated from types and doc-comments for every published version, automatically",
scheduled to Phase 4/5 with the Mere. Half of it had existed since Phase 2 without being published
— [`20-phase-2-report.md`](20-phase-2-report.md)'s `Interface` is every published name's type,
effect row and placement, all of it *derived*. The other half — doc comments — did not exist at
all: `#` comments are discarded by the lexer, so a Beck program had no way to carry prose past
parsing, and `beck fmt` deleted every comment in a file it rewrote.

The argument for generating rather than writing this is sharper for Beck than for most languages.
Three of the five things a reference page says about a name are *inferred*:

| Part of a page | Where it comes from |
|---|---|
| Signature | Hindley–Milner inference |
| **Effects** | The inferred row (§3.2), closed at the module boundary (§3.6) |
| **Placement** | The cost-model solver (§3.4) — not an annotation |
| Types, fields, variants | The module's own declarations |
| Prose | The `##` doc comment, where somebody wrote one |

A hand-written page claiming `map_list` is pure would be wrong and would stay wrong. **A doc
comment can go stale; a signature cannot.**

## 31.2 Doc comments

`##` in the Python surface, `;;` in the S-expression one — one more marker than an ordinary
comment, as `///` is one more than `//`.

```beck
## An amount of money in minor units — pence, cents — never a float.
##
## A newtype rather than an alias, so it cannot be passed where a plain `Int` is expected.
type Money = newtype[Int]

## One line on an order: a product, bought some number of times.
model Line:
    ## What was bought. Opaque here; the catalogue owns the meaning.
    sku: Str
```

Three decisions are worth stating, because each had a plausible alternative.

**Collected from the source, not lexed.** The layout algorithm treats a comment-only line as having
no indentation at all — that is what lets a comment sit at column zero inside an indented block
without closing it. Lexing `##` as a token would put that rule at risk for every file in order to
serve a feature that only ever reads declarations. So a run of `##` lines is collected from the
source text and attached to nodes afterwards, by position: a run belongs to the declaration on the
first line beneath it, and a blank line ends a run — so a file header documents the file rather than
the first definition. The token stream, the layout algorithm and the parser are untouched.

**Attached to the outermost node.** `@on(client)` above a `def` is one declaration, so a doc comment
written above the decorator documents the whole of it.

**Metadata, not a form.** A doc comment lives in `Meta`, beside the span, which puts it outside
`Node::structurally_eq`. Three consequences, and all three are tested: documenting a definition does
not change what the program means, does not invalidate a memo, and does not move
`Interface::digest`. **Documenting a function is not an API change**, and a downstream module does
not rebuild because somebody wrote a sentence.

`beck fmt` preserves `##` and still discards `#`. That distinction is the reason for the second
marker, and a test asserts both halves of it — including that a documented module round-trips
through *both* surfaces with every comment landing back on the node it started on.

## 31.3 `beck doc module`

One page per module: every published type with its fields or variants, and every published name
with its signature, its inferred effects and the tier the solver put it on. Markdown for review,
HTML for the site, JSON for a tool.

Measured on the two examples:

| Module | Types | Names | Documented |
|---|---|---|---|
| [`examples/todo.beck`](../compiler/examples/todo.beck) | 6 | 13 | 0/19 |
| [`examples/documented.beck`](../compiler/examples/documented.beck) | 3 | 6 | 9/9 |

The sketch scores zero and the page is still worth reading, which is the point: the signature,
effect and placement columns are there whether or not anyone wrote prose. `documented.beck` exists
to show the difference, and a test fails if a name in it loses its comment.

Coverage is **reported, never enforced**. A coverage gate that fails a build is how a codebase ends
up with `## the id` on a field called `id`.

## 31.4 The language reference

[`reference/`](reference/README.md) — five pages, 543 lines, all generated, all checked in.

| Page | Derived from | Size |
|---|---|---|
| [Error index](reference/errors.md) | the codes the compiler emits | 92 codes |
| [Command reference](reference/cli.md) | the `clap` command tree that parses the arguments | 23 commands |
| [Effects and tiers](reference/effects.md) | `Tier::discharges`, *evaluated* | 15 atoms × 4 tiers |
| [The prelude](reference/prelude.md) | `prelude::prims` and `prelude::types` | 50 names, 5 types |
| [Forms](reference/forms.md) | `sym::RESERVED_FORMS` | 15 forms |

The effects page is the one worth pointing at. It is **not** a transcription of §3.3's table — it
is `Tier::discharges` called at every (tier, atom) pair, which is the same predicate the placement
solver evaluates when it decides where a definition runs. If that rule changes, the page changes,
and the drift gate puts it in the diff.

The prelude page reads `map_list` as

```text
(list[a], (a) -> b ! {e}) -> list[b] ! {e}
```

which is §3.2's promised signature, printed by the compiler rather than quoted from the document.
The scheme's internal variable numbers are rewritten as letters for the page; nothing else is.

The error index is where the honesty line sits. Every diagnostic site in the compiler already
carries a label, usually a note and often a fix-it — prose the project wrote next to the condition
that raises it. The index's explanations are that prose, condensed; they are not a second,
independent account of what the compiler does, because a second account is a thing that goes out of
date. `beck explain error B0341` prints one at the terminal, in the shape `rustc --explain` uses.

## 31.5 The gates

Five properties, all deterministic — no cluster, no container, no network — in
`compiler/crates/beck-cli/tests/docs.rs`:

1. **The error index is complete in both directions.** Every `"Bnnnn"` literal in the workspace's
   non-test source is in the index, and every index entry is a code something raises. A new
   diagnostic without an entry fails `cargo test`; so does an entry whose code was deleted.
2. **The checked-in reference is what the compiler generates.** `beck doc reference --check`
   regenerates all six files in memory and compares.
3. **`beck doc` runs over the corpus** — 25 programs × 3 formats, which is the only way to know the
   generator does not panic on a shape no unit test covers.
4. **Every link in the generated site resolves.** This found a real defect: a module page is
   written one directory below the reference pages, so the shell's header link back to the index is
   not the same href from both, and half the site's header was a 404.
5. **A doc comment survives `beck fmt`** — and an ordinary comment still does not.

These are `cargo test` gates and not only workflow steps, for the reason
[`20-phase-2-report.md`](20-phase-2-report.md) §20.4 item 8 gives: the Phase 1 CI workflow had never
run, and nothing said so. A test cannot be silently skipped.

`.github/workflows/docs.yml` grew from one job to three: the original markdown link gate, a
`generated` job running the drift gate and a warning-free `cargo doc`, and a `publish` job that
builds the site from `main`. Every step of all three was run by hand before being trusted, which is
the rule §20.4 item 8 left behind.

## 31.6 Rustdoc

`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` **failed to build** before this change
— six of the nine crates could not be documented at all — and is clean across all nine after it.
Fixing it turned up 55 problems, of two kinds:

- **46 shortcut links with no target** — `[`docs/19-phase-1-report.md`]` rather than the
  `[`docs/19-phase-1-report.md`](…)` the rest of the codebase writes. Rustdoc reads a bracketed code
  span with no target as an intra-doc link and cannot resolve it. They were never links; they are
  code spans now.
- **Nine real ones**: a link to a `#[cfg(test)]` item that no documentation build contains; three
  links into private items from public documentation; one unresolvable `[`20`]`; three ambiguities
  where a name is both a module and a function (`crate::telemetry`, `beck_rt::telemetry`) or both a
  function and a primitive type (`slice`); and one pair of `<dir>` / `<file>` placeholders inside a
  doc comment that rustdoc read as unclosed HTML tags.

One limitation stated plainly: the `[`docs/03-…`](../../../../docs/03-…)` links throughout the
crates resolve when reading the *source*, which is what they were written for, and do not resolve
from the rustdoc HTML, whose directory depth is different. Rustdoc does not check them and neither
does the link gate, which reads markdown. Nothing here made that better or worse.

## 31.7 What this does not do

- **`missing_docs` is not enabled.** 863 public items across the nine crates carry no `///`. CI
  denies warnings, so the lint would fail the build until every one had a sentence — and most of
  those sentences would restate the name. The number is here so the decision stays visible;
  [`adr/0007`](adr/0007-generated-reference-documentation.md) records why, and a crate-by-crate
  opt-in is the path if it is taken.
- **No cross-module linking.** A type from an imported module renders as its name, not as a link.
  The Mere (§16.2) is where a link between published versions belongs, and the Mere is not built.
- **Markdown in a doc comment is not parsed.** The HTML renderer escapes it and preserves paragraph
  breaks. A doc comment reads as prose, not as formatted prose.
- **`beck doc` documents one module.** A project of several modules is documented a module at a
  time; there is no index across them and no search.
- **The prose in the error index can still drift**, and only the prose. The *set* of codes cannot —
  that is what the gate covers. An entry whose explanation stopped matching its diagnostic's note
  is a thing a reader would have to notice.
- **Nothing is versioned.** The site is built from `main`. §16.2's "for every published version"
  needs the Mere and a package registry, neither of which exists.
