- **2026-08-20 — Nine CI gates could not fail, and the sketch's restyle is what found them.**
  CI went red on the styling change: the workflow asserted `grep -q '<footer>0 remaining</footer>'`
  and the footer now carries a class. Two lines to fix — matched as `'>0 remaining</footer>'`, since
  the property is that the *text* was server-rendered, not what the tag wears. What the fix turned up
  is the entry.
  **`! cmd` does not fail a step.** `bash -e` "shall not exit" when the command that failed "is part
  of a `!` expression" — POSIX's own words — so a negation followed by any further line is a comment
  with a process behind it. `compiler.yml` had **ten** such assertions and **nine were dead**: that a
  deliberately-false Beck test fails the build, that a `match` covering one list shape of two is
  refused, that a rigid `T` is not silently generalised, that a breaking wire change needs
  `--breaking`, that a Mode A page does not load the Mode B kernel, that the derived grant carries no
  `DELETE`, that stripping `@on` leaves none, that a deep recursion does not overflow the host stack,
  and that the sheet has no rule for a class the page cannot carry. The tenth was live only because
  it was the last line of its step.
  All ten are now `if cmd; then echo 'why'; exit 1; fi`, which aborts wherever it sits and says what
  broke. **All nine were asserting things that are true** — verified one at a time against the
  current tree — so nothing had been hiding behind them.
  **The file already knew.** The deep-recursion step's own comment reads "an exit status of 134 or
  139 … is exactly the thing a `! cmd` gate would have accepted, so the status is checked rather than
  only the failure": somebody hit the trap, understood it exactly, fixed the instance in front of
  them, and left the nine others. That is
  [`docs/82`](../docs/82-the-edge-report.md) §82.10's pattern, now recorded there with this as its
  largest instance.
  `workflows.rs::no_workflow_asserts_with_a_negation_that_cannot_fail` forbids a `run:` line
  beginning with `!` in any workflow, with **no exemption for the last-line case** — an exemption
  that depends on position is lost the moment somebody appends a line, which is how nine of these
  happened.
  **And a second thing the restyle broke silently.** The wire-compat step's "a body edit is not a
  wire change" ran `sed 's/"done" if t.done else ""/…/'` — `done_class`'s *old* body. The `sed`
  matched nothing, so both `check`s were about the same file and the claim was vacuous. It now edits
  a quoted text literal (`"todos"` → `"to-dos"`; unquoted ` remaining` also matches
  `def remaining(…)`, and renaming a definition genuinely *is* an interface change) and `diff`s to
  prove the edit landed.
  New in the serving step: the page's stylesheet is fetched over HTTP and checked to carry a rule for
  a class the page serves, the token that rule reads, and nothing for a class it cannot carry —
  `beck build` writing the file was already gated, this process answering for it was not.
