- **2026-09-07 — A stub stands in for a definition, so its row is that definition's — and it can
  fail.**
  Two open items, one missing rule. A `stub` clause replaces the definitions that *perform* an
  atom, so what those definitions' rows hold is charged to them — which the signature already
  declares — and not to the `test` block that named the clause. Read forwards that makes a
  `raises(E)` an answer a stub may give (`stub net.out(h): raise Declined`), closing
  [`08`](../docs/08-roadmap.md) §8.5.4's **a stub that fails**: the branch every program with an
  outbound call writes — the peer is down, refuse the command — was the one branch its tests could
  not reach. Read backwards it says those definitions do not run, so the atom the clause names —
  exactly, since every definition performing it is replaced — and the failures those definitions
  carry stop being the block's, and an *expectation* may call one. That closes
  `DEFECTS.md::a-stub-in-a-library-test-is-accepted-and-can-never-fire`: a library has no `when` to
  reach a stub through, so the clause was accepted, reported `called 0×` and passed.
  What did not move is that a stub may not **perform** — a body reaching an effect is `B0700` as
  before, because a stub that performed the effect it stands in for would have stood in for
  nothing.
  Two things the item did not predict. A raise had to survive
  [`backend`](../compiler/crates/beck-core/src/backend.rs)'s seam: `try:` catches by *type name*,
  so `ExecError` now carries the raised value and `Interceptor::intercept` answers with a `Result`
  — a failure flattened to a message travels straight past the handler the program wrote. And the
  bound needed `B0708`, because a stub raising what the signature does not declare would unwind
  through callers type-checked against a row that says they cannot fail.
  Gated in `beck-cli/tests/tests_in_beck.rs` in four directions — the stub fires and the refusal
  arm is reached, the same call with no clause is still `B0700`, one peer stubbed does not excuse
  the other — including a peer a performer merely *inherits*, which is the case a whole-row
  discharge would let through — and a raise the signature does not declare is refused. The first
  goes red if the seam flattens the raise; the middle two go red if the discharge is written more
  broadly than the atom the clause names. [`86`](../docs/86-getting-started.md) §86.8 is where the
  missing arm was found and now demonstrates it, run by `beck-cli/tests/getting_started.rs`.
