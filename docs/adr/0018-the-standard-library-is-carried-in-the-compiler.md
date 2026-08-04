# ADR 0018 — The standard library's Beck half is carried in the compiler

**Status:** accepted
**Date:** 2026-08-04
**Context:** [`68`](../68-clbg-report.md) §68.4, [`69`](../69-standard-library-imports-report.md),
[`10`](../10-decisions.md) D23, [`16`](../16-packages-and-ecosystem.md) §16.7

## The decision

Every file in `compiler/lib/` is compiled into the `beck` binary (`beck_core::stdlib`), and
`beck_core::project::check_project` resolves an `import` against the caller's loader **first** and
that table **second**.

So `import bignum` works from any directory, and a `bignum.beck` beside the program wins.

## The problem it solves

[`68`](../68-clbg-report.md) §68.4: `import` resolved against the root module's own directory and
against nothing else, so the Beck half of the standard library was reachable from `lib/` and from
nowhere. Three phases of work had not noticed, because nothing had reached across a directory.

## The alternatives, and why not

**A search path discovered at run time** — an exe-relative `../lib`, a `BECK_PATH`, a `beck.toml`
entry. This is what most compilers do and it is what a *package manager* will need, but every
version of it makes the library a thing that can be missing, stale or half-installed: the compiler
would be built against one `lib/` and run against another, and the failure would show up as a type
error inside somebody else's library. The primitive half of the standard library
([`crate::prelude`](../../compiler/crates/beck-core/src/prelude.rs)) has always been in the binary,
and the Beck half is written against it version for version. Splitting the two across a filesystem
boundary buys a flexibility nothing has asked for and pays for it in a class of bug that cannot
happen today.

**A `build.rs` that walks `lib/`** — the table would then be complete by construction rather than by
a test. Refused because a file appearing in the standard library is an API addition: it adds a name
every program in the language can import, and that should be written down in a reviewable list
rather than picked up from a directory listing. `beck-cli/tests/stdlib.rs` fails if the directory
and the list disagree, so completeness is still gated; what is not automatic is *silence*.

**A prefix or namespace** — `import std/bignum`, `import @beck/std`. This is where
[`16`](../16-packages-and-ecosystem.md) §16.7 ends up, and it is a package-system decision rather
than a compiler one: it needs a namespace syntax, a resolution rule for third-party names, and a
story for what `beck add` writes. Taking it now would settle the package system's vocabulary in a
change about imports. `import bignum` is what `lib/decimal.beck` already writes, and it is what a
namespaced form would have to keep working anyway.

## Consequences

- **The library is versioned with the compiler.** Changing `lib/money.beck` means rebuilding `beck`
  for anything outside `lib/` to see it. Inside `lib/`, the caller's directory wins, so a library
  file importing a sibling gets the file beside it and development is unaffected.
- **The binary carries the sources**, about 200 KB of text. It is compiled per import rather than
  cached, so a program that imports `decimal` typechecks `decimal` and `bignum` on every build.
  Nothing measures that yet; [`69`](../69-standard-library-imports-report.md) §69.4 has the figure
  it costs today.
- **A library module's `test` blocks do not become the importing program's.** They are checked and
  then dropped at the link, the way no test runner runs its dependencies' tests.
  `beck-cli/tests/stdlib.rs` is where they run.
- **The flat namespace now spans the library.** Two library modules that define one name cannot be
  imported by one program (`B0601`), which was true before and unreachable before. Two such pairs
  existed on the day this was taken; `stdlib.rs::the_whole_library_links_into_one_program` is the
  gate that found them and keeps them found.
- **When the package system lands**, this table is what `@beck/std` resolves to, and this record is
  what has to be revisited rather than quietly extended.
