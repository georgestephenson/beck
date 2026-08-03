# 0012 — The front end counts its own recursion, against one ceiling and one declared stack

**Context.** The front end recurses over structure a user chooses: nested brackets and indentation
in the parser, nested lists in the S-expression reader, nested arguments in the macro expander,
nested expressions and types in the checker. Nothing counted any of it.
[`docs/42`](../42-security-assurance.md) §42.2 measured what that cost: an ~7.6 KB file of nested
parentheses aborted `beck check` in a debug build — `fatal runtime error: stack overflow`, no span,
nothing catchable — and the same file compiled fine in release, where the threshold was more than
an order of magnitude further out. The debug threshold also *moved* between commits, because it is
a function of whatever frames the checker happened to have that week.

[`0007`](0007-evaluator-stack-is-declared-not-discovered.md) had already argued the general case
for the evaluator and chosen a fixed count over a stack-headroom budget, on determinism: a headroom
budget "would let the same program over the same log succeed in a release build and refuse in a
debug one". The front end had exactly the behaviour that ADR rejected, in a worse form — abort in
one profile, accept in the other — and it was the *same 64 MiB thread*, sized for 4,000 evaluator
frames and exhausted by ~3,600 parser frames.

**Decision.** One count, `beck_diag::depth::MAX_NESTING`, shared by every front-end pass that
recurses over user-controlled structure, and one declared stack, `beck_diag::depth::STACK_BYTES`,
that the count is held to by measurement.

- The counter lives in `beck-diag` because three crates share it, and a ceiling with three
  definitions is three ceilings.
- The bound is applied **at each recursion site**, not at one grammar production. That is the
  Scriban lesson (GHSA-p6q4-fgr8-vx4p): a depth limit added at one production was bypassed through
  a different one. So the parser counts at `primary` and `block`, the reader at `list`, the
  expander at its argument walk, and the checker at `expr` and `ty_from_node`. Passes downstream of
  the checker walk the `Core` it built and are bounded by construction.
- Exceeding it is a diagnostic with a span — `B0121` reading, `B0213` expanding, `B0390`
  checking — reported once rather than once per level.
- `MAX_NESTING` is 256. The measured cost is ~18 KiB per level in the parser and ~4 KiB in the
  checker, unoptimised, so the ceiling costs under 5 MiB — inside the stack an ordinary main thread
  has, and a fraction of the 64 MiB declared.

**Alternatives.** A headroom budget was rejected for 0007's reason. A *per-pass* ceiling was
rejected because the number a user would have to reason about would then be four numbers, and
because the passes see the same tree. Raising the evaluator's declaration to cover both consumers
by addition was rejected because they are sequential: a compilation has finished reading before it
begins running, so the stack has to hold the larger of the two, not the sum.

**Consequences.** The macro expander's single `MAX_DEPTH` had to be split in two. It counted
structural descent and re-expansion on one counter, so a 65-level-deep expression *containing no
macros at all* reported that "macro expansion did not terminate". Expansion depth stays at 64 and
keeps `B0201`; structural depth joins the shared ceiling. That defect was invisible while the
parser accepted arbitrary depth, because nothing reached the expander with a deep tree and a
reader who saw the message had usually written a runaway macro.

An embedder that drives the front end from its own thread has to supply the stack, exactly as
`docs/31` §31.7 records for the evaluator; `beck_diag::depth::on_the_front_end_stack` is how, and
`beck-cli` needs neither call because its dispatch already runs inside the evaluator's.

Nothing here bounds *breadth*. A file with a million top-level items is still a file with a million
top-level items, and the memory it costs is linear and not a stack.

Measurements and the gate: [`docs/44`](../44-wave-0-report.md) §44.2.
