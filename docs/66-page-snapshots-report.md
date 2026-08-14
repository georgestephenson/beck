# 66 — Phase 3, part 35: page snapshots, and `beck test --update`

**Built.** `expect page matches snapshot`, and the flag that records one. The last open question in
[`21`](21-tests-in-beck-and-proof.md) §21.2, and the last of the small items on
[`23`](23-incremental-views-report.md) §23.19's list.

[`22`](22-phase-3-report.md) §22.6 named it precisely and left it:

> **No `test --update`, and no page snapshots.** §21.2 lists golden assertions as an open question
> with a known answer (`insta`'s update flow, which the compiler's own suite already uses); a page
> assertion is `contains` and nothing else today.

## 66.1 What it is

```beck
test "the page a returning user sees":
    given [
        Added(id=Id("1"), text="milk"),
        Added(id=Id("2"), text="bread"),
        Toggled(id=Id("1")),
    ] by "ana"
    expect page(session("ana")) matches snapshot
```

That is checked in, in [`examples/todo.beck`](../compiler/examples/todo.beck), against
`examples/snapshots/the-page-a-returning-user-sees@ana.html`.

The difference from `contains` is the whole point. `expect page contains "milk"` asserts one string
somebody thought to name; this asserts **every attribute, every `data-b-*` binding and the order of
the list** — which is what the client actually interprets ([`05`](05-tier-lowering.md) §5.1). A
change to how a page is built shows up as a diff in a checked-in file rather than as nothing at all.

The name is optional and defaults to the test's; the actor is part of the key, because one test may
assert two people's pages and those are two snapshots. `a-snapshotted-page@ana.html` is what the two
produce together.

## 66.2 Three decisions, and each is the difference between an assertion and a habit

§21.2 stated the risk and the mitigation in one line — *"the risk is snapshot rot, and the
mitigation is the same one — review the diff"* — and taking that seriously rather than quoting it
fixes three things that a snapshot feature gets wrong by default.

**A missing snapshot is a failure, not a silent write.** The first run of a new assertion has to say
it recorded nothing, or a test that has never compared anything reads as a test that passes:

```console
test "the page a returning user sees" … FAILED
  no snapshot recorded at examples/snapshots/the-page-a-returning-user-sees@ana.html
    run `beck test --update` to write it, then review the file like any other diff
```

**Writing is only ever `--update`.** A snapshot that rewrites itself when it disagrees asserts
nothing at all — it is a log of what happened wearing a test's clothes. Nothing infers the flag.

**The diff is in the failure**, and getting this right took a second attempt. The first version
printed both sides elided from the start of the line, which for a rendered page — one very long line
— shows two identical prefixes and hides the difference. That is exactly the failure
[`04`](04-compiler-architecture.md) §4.5 is about: an error message is a product surface. The window
is centred on the first differing *character*:

```console
  the page `ana` sees does not match examples/snapshots/…@ana.html
    line 1, column 295:
      snapshot: …&quot;id&quot;:&quot;2&quot;}">BREAD</span><button data-b-click=…
      rendered: …&quot;id&quot;:&quot;2&quot;}">bread</span><button data-b-click=…
```

## 66.3 What it touches, which is five layers and no new dependency

`insta` is what §21.2 pointed at and what the compiler's own suite uses. It is not used here, and
the reason is not preference: `insta` snapshots a Rust value from a Rust test, and this snapshots a
**Beck** page from a **Beck** test, keyed by a Beck test's name, in a directory beside a `.beck`
file. The parts that would have been reused are `std::fs::read_to_string` and a diff.

| | |
|---|---|
| `beck-syntax` | one more shape of `expect`, and its printer case |
| `beck-core` | `Expectation::PageMatchesSnapshot`, and the checker clause |
| `beck-rt` | the comparison, the key, and the diff window |
| `beck-cli` | `--update`, and the generated reference entry |
| `examples/` | one assertion and one checked-in page |

The form carries **two slots always** — the name and the actor, with `none` where nothing was
written — rather than a list whose length varies with which optional part is present. A form like
that cannot be read back without guessing which one it was, and `beck fmt` reads every form back:
`a_snapshot_assertion_survives_being_printed_and_read_back` covers all four combinations.

One consequence is worth naming because it caught two harnesses the moment the sketch grew a
snapshot. `snapshots/` resolves against `Options::base_dir`, which `beck test` sets from the file's
own directory and `Options::default()` leaves **empty** — so an in-process harness running a
program read off disk resolves against its own working directory instead. That is right (a
snapshot belongs beside the program, not beside whatever invoked it) and it means a harness has to
say where the program lives. `examples_options()` is that, and the two tests that run the sketch's
own tests in-process now use it.

## 66.4 How it is tested

`tests_in_beck.rs::a_page_snapshot_is_recorded_only_when_asked_and_compared_every_other_time` walks
all four states **through the binary**, because `--update` is a flag and the property that matters
is that nothing writes without one. A test that called the runtime with `update_snapshots: true`
would assert the writing and not the policy.

1. Nothing recorded → fails, says `--update`, and **has not written a file**.
2. `--update` → writes it, and the file is the page.
3. Run again → passes, against the file rather than against anything in memory.
4. Change the file → fails, names the column, and shows both sides *at the difference*.

## 66.5 What is **not** built

| | Status |
|---|---|
| Snapshots of anything but a page | **not built.** `state`, `events` and the `Repr` of a value are all snapshottable in principle and none is. The page is the one §21.2 asked for and the one where `contains` was visibly not enough |
| An interactive review flow | **not built.** `cargo insta review` steps through pending snapshots; here the review is `git diff`, which is §21.2's stated mitigation and needs no tool |
| Pruning an orphaned snapshot | **not built.** Delete a test and its snapshot file stays. `insta --unreferenced` is the shape; nothing here notices |
| A structural diff | **not built.** The comparison is textual, so a reordered attribute is a difference. That is arguably right for a page whose bytes are what the client parses, and it is a decision that has not been forced yet |
| `--update` on anything else | **not built.** The flag writes page snapshots and nothing else |

## 66.6 What this corrects

- **[`21`](21-tests-in-beck-and-proof.md) §21.2's last open question is closed.** Its table row —
  "Golden/snapshot assertions … **Still open — not built**" — is answered, with the update flow it
  named and not the library it named.
- **[`22`](22-phase-3-report.md) §22.6's item is built**, and with it
  [`23`](23-incremental-views-report.md), [`23`](23-incremental-views-report.md) and
  [`23`](23-incremental-views-report.md)'s repeated "no `test --update`".
- **The sketch asserts its whole page.** `examples/todo.beck` had five page assertions and all of
  them were `contains`; there is now one that would notice a change to any part of it.

## 66.7 What Phase 3 is still not

The exit criterion is not met. This closes a sub-item of the test-construct bullet rather than a
bullet, so the count is unchanged from [`65`](65-lsp-report.md) §65.7: **six of the fourteen remain
untouched** — Mode B, client polish, structured concurrency, the SQLite substrate, the playground
and supply-chain tooling — with no LLVM backend, identity holding its seam and not its relying
party, and the incremental-views bullet still without read models, pgwire or fusion.
[`23`](23-incremental-views-report.md) §23.19 still names them one at a time.
